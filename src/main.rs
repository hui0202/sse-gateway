//! SSE Gateway with Service Discovery and Heartbeat
//!
//! Architecture:
//!   1. Gateway registers itself to Redis on startup (with heartbeat)
//!   2. Channel → Instance mapping stored in Redis
//!   3. Agent queries instance info, then pushes directly
//!
//! Redis keys:
//!   - gateway:instances (SET)           - All active instance IDs
//!   - gateway:instance:{id} (HASH)      - Instance details {address, last_seen}
//!   - channel:{channel_id}:instance     - Channel → Instance ID mapping

use async_trait::async_trait;
use axum::{extract::State, routing::post, Json, Router};
use redis::aio::ConnectionManager;
use sse_gateway::{
    CancellationToken, ConnectionInfo, Gateway, IncomingMessage, MessageHandler, MessageSource,
    MessageStorage,
};
use sse_gateway_redis::RedisStorage;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// ============================================================================
// Service Registry - Gateway instance registration with heartbeat
// ============================================================================

/// Service registry for gateway instances
#[derive(Clone)]
struct ServiceRegistry {
    redis: Arc<RwLock<Option<ConnectionManager>>>,
    instance_id: String,
    instance_addr: String,
    /// Heartbeat interval in seconds
    heartbeat_interval: u64,
    /// Instance TTL in seconds (should be > heartbeat_interval * 2)
    instance_ttl: u64,
}

impl ServiceRegistry {
    fn new(instance_id: String, instance_addr: String) -> Self {
        Self {
            redis: Arc::new(RwLock::new(None)),
            instance_id,
            instance_addr,
            heartbeat_interval: 10,
            instance_ttl: 30,
        }
    }

    async fn connect(&self, redis_url: &str) -> anyhow::Result<()> {
        let client = redis::Client::open(redis_url)?;
        let manager = ConnectionManager::new(client).await?;
        *self.redis.write().await = Some(manager);
        Ok(())
    }

    /// Register this instance and start heartbeat
    async fn register_and_start_heartbeat(&self, cancel: CancellationToken) {
        // Initial registration
        self.register_instance().await;

        // Start heartbeat loop
        let registry = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(registry.heartbeat_interval));
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        // Unregister on shutdown
                        registry.unregister_instance().await;
                        break;
                    }
                    _ = interval.tick() => {
                        registry.heartbeat().await;
                    }
                }
            }
        });
    }

    /// Register this gateway instance
    async fn register_instance(&self) {
        let Some(ref mut conn) = *self.redis.write().await else { return };

        let now = chrono::Utc::now().timestamp();

        // Use pipeline for atomic registration
        let mut pipe = redis::pipe();
        
        // Add to instances set
        pipe.cmd("SADD")
            .arg("gateway:instances")
            .arg(&self.instance_id)
            .ignore();

        // Set instance details with TTL
        let instance_key = format!("gateway:instance:{}", self.instance_id);
        pipe.cmd("HSET")
            .arg(&instance_key)
            .arg("address")
            .arg(&self.instance_addr)
            .arg("last_seen")
            .arg(now)
            .arg("registered_at")
            .arg(now)
            .ignore();

        pipe.cmd("EXPIRE")
            .arg(&instance_key)
            .arg(self.instance_ttl)
            .ignore();

        if let Err(e) = pipe.query_async::<()>(conn).await {
            tracing::warn!(error = %e, "Failed to register instance");
        } else {
            tracing::info!(
                instance_id = %self.instance_id,
                address = %self.instance_addr,
                "Instance registered"
            );
        }
    }

    /// Send heartbeat to keep instance alive
    async fn heartbeat(&self) {
        let Some(ref mut conn) = *self.redis.write().await else { return };

        let now = chrono::Utc::now().timestamp();
        let instance_key = format!("gateway:instance:{}", self.instance_id);

        let mut pipe = redis::pipe();
        
        // Update last_seen
        pipe.cmd("HSET")
            .arg(&instance_key)
            .arg("last_seen")
            .arg(now)
            .ignore();

        // Refresh TTL
        pipe.cmd("EXPIRE")
            .arg(&instance_key)
            .arg(self.instance_ttl)
            .ignore();

        if let Err(e) = pipe.query_async::<()>(conn).await {
            tracing::warn!(error = %e, "Heartbeat failed");
        } else {
            tracing::debug!(instance_id = %self.instance_id, "Heartbeat sent");
        }
    }

    /// Unregister this instance (on shutdown)
    async fn unregister_instance(&self) {
        let Some(ref mut conn) = *self.redis.write().await else { return };

        let instance_key = format!("gateway:instance:{}", self.instance_id);

        let mut pipe = redis::pipe();
        pipe.cmd("SREM")
            .arg("gateway:instances")
            .arg(&self.instance_id)
            .ignore();
        pipe.cmd("DEL")
            .arg(&instance_key)
            .ignore();

        if let Err(e) = pipe.query_async::<()>(conn).await {
            tracing::warn!(error = %e, "Failed to unregister instance");
        } else {
            tracing::info!(instance_id = %self.instance_id, "Instance unregistered");
        }
    }

    /// Get all active instances (for debugging/monitoring)
    async fn get_all_instances(&self) -> anyhow::Result<Vec<InstanceInfo>> {
        let Some(ref mut conn) = *self.redis.write().await else {
            anyhow::bail!("Redis not connected");
        };

        // Get all instance IDs
        let instance_ids: Vec<String> = redis::cmd("SMEMBERS")
            .arg("gateway:instances")
            .query_async(conn)
            .await?;

        let mut instances = Vec::new();
        for id in instance_ids {
            let key = format!("gateway:instance:{}", id);
            let info: Option<(String, i64)> = redis::cmd("HMGET")
                .arg(&key)
                .arg("address")
                .arg("last_seen")
                .query_async(conn)
                .await
                .ok();

            if let Some((address, last_seen)) = info {
                instances.push(InstanceInfo {
                    id: id.clone(),
                    address,
                    last_seen,
                });
            }
        }

        Ok(instances)
    }

    /// Get instance address by ID
    async fn get_instance_address(&self, instance_id: &str) -> Option<String> {
        let Some(ref mut conn) = *self.redis.write().await else { return None };

        let key = format!("gateway:instance:{}", instance_id);
        redis::cmd("HGET")
            .arg(&key)
            .arg("address")
            .query_async(conn)
            .await
            .ok()
    }
}

#[derive(serde::Serialize)]
struct InstanceInfo {
    id: String,
    address: String,
    last_seen: i64,
}

// ============================================================================
// Channel Registry - Channel → Instance mapping
// ============================================================================

/// Channel to instance mapping
#[derive(Clone)]
struct ChannelRegistry {
    redis: Arc<RwLock<Option<ConnectionManager>>>,
    instance_id: String,
    /// TTL for channel mappings
    channel_ttl: u64,
}

impl ChannelRegistry {
    fn new(instance_id: String, channel_ttl: u64) -> Self {
        Self {
            redis: Arc::new(RwLock::new(None)),
            instance_id,
            channel_ttl,
        }
    }

    async fn connect(&self, redis_url: &str) -> anyhow::Result<()> {
        let client = redis::Client::open(redis_url)?;
        let manager = ConnectionManager::new(client).await?;
        *self.redis.write().await = Some(manager);
        Ok(())
    }

    fn channel_key(channel_id: &str) -> String {
        format!("channel:{}:instance", channel_id)
    }

    /// Register channel → instance mapping
    async fn register_channel(&self, channel_id: &str) {
        let Some(ref mut conn) = *self.redis.write().await else { return };

        let key = Self::channel_key(channel_id);

        let result: Result<(), _> = redis::cmd("SET")
            .arg(&key)
            .arg(&self.instance_id)
            .arg("EX")
            .arg(self.channel_ttl)
            .query_async(conn)
            .await;

        match result {
            Ok(_) => tracing::debug!(channel_id, instance_id = %self.instance_id, "Channel registered"),
            Err(e) => tracing::warn!(error = %e, channel_id, "Failed to register channel"),
        }
    }

    /// Unregister channel
    async fn unregister_channel(&self, channel_id: &str) {
        let Some(ref mut conn) = *self.redis.write().await else { return };

        let key = Self::channel_key(channel_id);

        // Only delete if it's our instance (prevent race condition)
        let script = redis::Script::new(
            r#"
            if redis.call('GET', KEYS[1]) == ARGV[1] then
                return redis.call('DEL', KEYS[1])
            end
            return 0
            "#,
        );

        let _: Result<i32, _> = script
            .key(&key)
            .arg(&self.instance_id)
            .invoke_async(conn)
            .await;

        tracing::debug!(channel_id, "Channel unregistered");
    }

    /// Get instance ID for a channel
    async fn get_channel_instance(&self, channel_id: &str) -> Option<String> {
        let Some(ref mut conn) = *self.redis.write().await else { return None };

        let key = Self::channel_key(channel_id);
        redis::cmd("GET")
            .arg(&key)
            .query_async(conn)
            .await
            .ok()
    }

    /// Get all channel mappings (for debugging)
    async fn get_all_channels(&self) -> anyhow::Result<std::collections::HashMap<String, String>> {
        let Some(ref mut conn) = *self.redis.write().await else {
            anyhow::bail!("Redis not connected");
        };

        let keys: Vec<String> = redis::cmd("KEYS")
            .arg("channel:*:instance")
            .query_async(conn)
            .await?;

        let mut result = std::collections::HashMap::new();
        for key in keys {
            if let Ok(instance_id) = redis::cmd("GET")
                .arg(&key)
                .query_async::<String>(conn)
                .await
            {
                if let Some(channel_id) = key
                    .strip_prefix("channel:")
                    .and_then(|s| s.strip_suffix(":instance"))
                {
                    result.insert(channel_id.to_string(), instance_id);
                }
            }
        }

        Ok(result)
    }
}

// ============================================================================
// Direct Push Source
// ============================================================================

struct DirectPushSource {
    push_port: u16,
    receiver: tokio::sync::Mutex<Option<mpsc::Receiver<IncomingMessage>>>,
    sender: mpsc::Sender<IncomingMessage>,
    service_registry: ServiceRegistry,
    channel_registry: ChannelRegistry,
    storage: RedisStorage,
}

impl DirectPushSource {
    fn new(
        push_port: u16,
        service_registry: ServiceRegistry,
        channel_registry: ChannelRegistry,
        storage: RedisStorage,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(1000);
        Self {
            push_port,
            receiver: tokio::sync::Mutex::new(Some(receiver)),
            sender,
            service_registry,
            channel_registry,
            storage,
        }
    }
}

#[async_trait]
impl MessageSource for DirectPushSource {
    async fn start(
        &self,
        handler: MessageHandler,
        connection_manager: sse_gateway::ConnectionManager,
        cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        let mut receiver = self
            .receiver
            .lock()
            .await
            .take()
            .ok_or_else(|| anyhow::anyhow!("Source already started"))?;

        // Start heartbeat
        self.service_registry.register_and_start_heartbeat(cancel.clone()).await;

        // Start HTTP server
        let state = AppState {
            sender: self.sender.clone(),
            service_registry: self.service_registry.clone(),
            channel_registry: self.channel_registry.clone(),
            storage: self.storage.clone(),
            connection_manager,
        };

        let push_router = Router::new()
            .route("/push", post(handle_push))
            .route("/store", post(handle_store))
            .route("/channel/{id}", axum::routing::get(handle_channel_status))
            .route("/instances", axum::routing::get(get_instances))
            .route("/channels", axum::routing::get(get_channels))
            .with_state(state);

        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], self.push_port));
        let listener = tokio::net::TcpListener::bind(addr).await?;
        tracing::info!(port = self.push_port, "Direct push server listening");

        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            axum::serve(listener, push_router)
                .with_graceful_shutdown(async move { cancel_clone.cancelled().await })
                .await
                .ok();
        });

        // Forward messages
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                msg = receiver.recv() => {
                    match msg {
                        Some(msg) => handler(msg),
                        None => break,
                    }
                }
            }
        }

        Ok(())
    }

    fn name(&self) -> &'static str {
        "DirectPush+ServiceDiscovery"
    }

    fn on_connect(&self, info: &ConnectionInfo) {
        let registry = self.channel_registry.clone();
        let channel_id = info.channel_id.clone();

        tokio::spawn(async move {
            registry.register_channel(&channel_id).await;
        });
    }

    fn on_disconnect(&self, info: &ConnectionInfo) {
        let registry = self.channel_registry.clone();
        let channel_id = info.channel_id.clone();

        tokio::spawn(async move {
            registry.unregister_channel(&channel_id).await;
        });
    }
}

// ============================================================================
// HTTP Handlers
// ============================================================================

#[derive(Clone)]
struct AppState {
    sender: mpsc::Sender<IncomingMessage>,
    service_registry: ServiceRegistry,
    channel_registry: ChannelRegistry,
    storage: RedisStorage,
    connection_manager: sse_gateway::ConnectionManager,
}

#[derive(serde::Deserialize)]
struct PushPayload {
    channel_id: Option<String>,
    event_type: String,
    data: serde_json::Value,
}

#[derive(serde::Deserialize)]
struct StorePayload {
    channel_id: String,
    event_type: String,
    data: serde_json::Value,
}

#[derive(serde::Serialize)]
struct PushResponse {
    success: bool,
    online: bool,
    stream_id: String,
}

#[derive(serde::Serialize)]
struct StoreResponse {
    success: bool,
    stream_id: String,
}

#[derive(serde::Serialize)]
struct ChannelStatus {
    channel_id: String,
    online: bool,
    instance_id: Option<String>,
    instance_address: Option<String>,
}

/// Push message to channel
async fn handle_push(
    State(state): State<AppState>,
    Json(payload): Json<PushPayload>,
) -> Json<PushResponse> {
    use sse_gateway::SseEvent;

    let channel_id = payload.channel_id.clone().unwrap_or_default();
    let stream_id = state.storage.generate_id();

    let online = if !channel_id.is_empty() {
        state.connection_manager.channel_connection_count(&channel_id) > 0
    } else {
        false
    };

    let mut msg = IncomingMessage::new(&payload.event_type, payload.data.to_string());
    if let Some(cid) = payload.channel_id {
        msg = msg.with_channel(cid);
    }

    let success = state.sender.send(msg).await.is_ok();

    if !channel_id.is_empty() {
        let event = SseEvent::raw(&payload.event_type, payload.data.to_string());
        state.storage.store(&channel_id, &stream_id, &event).await;
    }

    Json(PushResponse { success, online, stream_id })
}

/// Store message for offline user
async fn handle_store(
    State(state): State<AppState>,
    Json(payload): Json<StorePayload>,
) -> Json<StoreResponse> {
    use sse_gateway::SseEvent;

    let stream_id = state.storage.generate_id();
    let event = SseEvent::raw(&payload.event_type, payload.data.to_string());
    state.storage.store(&payload.channel_id, &stream_id, &event).await;

    Json(StoreResponse { success: true, stream_id })
}

/// Query channel status with instance info
async fn handle_channel_status(
    State(state): State<AppState>,
    axum::extract::Path(channel_id): axum::extract::Path<String>,
) -> Json<ChannelStatus> {
    let instance_id = state.channel_registry.get_channel_instance(&channel_id).await;
    
    let instance_address = match &instance_id {
        Some(id) => state.service_registry.get_instance_address(id).await,
        None => None,
    };

    Json(ChannelStatus {
        channel_id,
        online: instance_id.is_some(),
        instance_id,
        instance_address,
    })
}

/// List all gateway instances
async fn get_instances(State(state): State<AppState>) -> Json<serde_json::Value> {
    match state.service_registry.get_all_instances().await {
        Ok(instances) => Json(serde_json::json!({
            "instances": instances,
            "count": instances.len()
        })),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

/// List all channel mappings
async fn get_channels(State(state): State<AppState>) -> Json<serde_json::Value> {
    match state.channel_registry.get_all_channels().await {
        Ok(channels) => Json(serde_json::json!(channels)),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sse_gateway=info,gateway=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let gateway_port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    let push_port: u16 = std::env::var("PUSH_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(9000);

    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());

    // Instance ID: use hostname or generate UUID
    let instance_id = std::env::var("INSTANCE_ID")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string());

    // Instance address: must be set by entrypoint or deployment
    let instance_addr = std::env::var("GATEWAY_ADDR")
        .unwrap_or_else(|_| format!("localhost:{}", push_port));

    let channel_ttl: u64 = std::env::var("CHANNEL_TTL")
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or(60);

    // Initialize registries
    let service_registry = ServiceRegistry::new(instance_id.clone(), instance_addr.clone());
    service_registry.connect(&redis_url).await?;

    let channel_registry = ChannelRegistry::new(instance_id.clone(), channel_ttl);
    channel_registry.connect(&redis_url).await?;

    let storage = RedisStorage::new();
    storage.connect(&redis_url).await?;

    let source = DirectPushSource::new(
        push_port,
        service_registry,
        channel_registry,
        storage.clone(),
    );

    tracing::info!(
        gateway_port,
        push_port,
        instance_id = %instance_id,
        instance_addr = %instance_addr,
        "Starting SSE Gateway with Service Discovery"
    );

    println!("==============================================");
    println!("  SSE Gateway with Service Discovery");
    println!("==============================================");
    println!();
    println!("Instance ID:      {}", instance_id);
    println!("Instance Addr:    {}", instance_addr);
    println!();
    println!("SSE endpoint:     http://localhost:{}/sse/connect?channel_id=test", gateway_port);
    println!("Dashboard:        http://localhost:{}/dashboard", gateway_port);
    println!();
    println!("Push API (port {}):", push_port);
    println!("  POST /push              Push message");
    println!("  POST /store             Store for offline user");
    println!("  GET  /channel/{{id}}      Query channel → instance");
    println!("  GET  /instances         List all gateway instances");
    println!("  GET  /channels          List all channel mappings");
    println!();

    Gateway::builder()
        .port(gateway_port)
        .instance_id(instance_id)
        .dashboard(true)
        .source(source)
        .storage(storage)
        .build()?
        .run()
        .await
}
