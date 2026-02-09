//! SSE Gateway with Direct Push and Redis Channel Registry
//!
//! This implementation:
//! 1. Registers channel-to-gateway mappings in Redis when SSE connections are established
//! 2. Cleans up mappings when connections are closed
//! 3. Receives direct push via HTTP (bypassing pub/sub for lower latency)
//!
//! Architecture:
//!   Agent → HTTP POST /push → Gateway (direct) → SSE Client
//!
//! Redis keys:
//!   - channel:{channel_id}:gateway -> gateway address (with TTL)

use async_trait::async_trait;
use axum::{extract::State, routing::post, Json, Router};
use redis::aio::ConnectionManager;
use sse_gateway::{
    CancellationToken, ConnectionInfo, Gateway, IncomingMessage, MessageHandler, MessageSource,
    MessageStorage,
};
use sse_gateway_redis::RedisStorage;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Redis-backed channel registry
#[derive(Clone)]
struct RedisChannelRegistry {
    redis: Arc<RwLock<Option<ConnectionManager>>>,
    /// TTL for channel mappings in seconds
    ttl_seconds: u64,
}

impl RedisChannelRegistry {
    fn new(ttl_seconds: u64) -> Self {
        Self {
            redis: Arc::new(RwLock::new(None)),
            ttl_seconds,
        }
    }

    async fn connect(&self, redis_url: &str) -> anyhow::Result<()> {
        let client = redis::Client::open(redis_url)?;
        let manager = ConnectionManager::new(client).await?;
        *self.redis.write().await = Some(manager);
        tracing::info!("Redis channel registry connected");
        Ok(())
    }

    fn channel_key(channel_id: &str) -> String {
        format!("channel:{}:gateway", channel_id)
    }

    /// Register a channel -> gateway mapping with TTL
    async fn register(&self, channel_id: &str, gateway_addr: &str) {
        let manager = {
            let conn = self.redis.read().await;
            match &*conn {
                Some(m) => m.clone(),
                None => return,
            }
        };

        let mut conn = manager;
        let key = Self::channel_key(channel_id);

        let mut cmd = redis::cmd("SET");
        cmd.arg(&key)
            .arg(gateway_addr)
            .arg("EX")
            .arg(self.ttl_seconds);

        let fut = cmd.query_async::<()>(&mut conn);

        match tokio::time::timeout(std::time::Duration::from_millis(100), fut).await {
            Ok(Ok(_)) => {
                tracing::debug!(channel_id, gateway_addr, "Registered channel in Redis");
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, channel_id, "Failed to register channel");
            }
            Err(_) => {
                tracing::debug!(channel_id, "Register timeout, skipping");
            }
        }
    }

    /// Remove a channel -> gateway mapping
    async fn unregister(&self, channel_id: &str) {
        let manager = {
            let conn = self.redis.read().await;
            match &*conn {
                Some(m) => m.clone(),
                None => return,
            }
        };

        let mut conn = manager;
        let key = Self::channel_key(channel_id);

        let mut cmd = redis::cmd("DEL");
        cmd.arg(&key);

        let fut = cmd.query_async::<()>(&mut conn);

        match tokio::time::timeout(std::time::Duration::from_millis(100), fut).await {
            Ok(Ok(_)) => {
                tracing::debug!(channel_id, "Unregistered channel from Redis");
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, channel_id, "Failed to unregister channel");
            }
            Err(_) => {
                tracing::debug!(channel_id, "Unregister timeout, skipping");
            }
        }
    }

    /// Get all channel -> gateway mappings (for debugging)
    async fn get_all(&self) -> anyhow::Result<std::collections::HashMap<String, String>> {
        let conn = self.redis.read().await;
        let Some(ref manager) = *conn else {
            anyhow::bail!("Redis not connected");
        };

        let mut conn = manager.clone();

        // Get all channel keys
        let keys: Vec<String> = redis::cmd("KEYS")
            .arg("channel:*:gateway")
            .query_async(&mut conn)
            .await?;

        let mut result = std::collections::HashMap::new();
        for key in keys {
            if let Ok(value) = redis::cmd("GET")
                .arg(&key)
                .query_async::<String>(&mut conn)
                .await
            {
                // Extract channel_id from key "channel:{channel_id}:gateway"
                if let Some(channel_id) = key
                    .strip_prefix("channel:")
                    .and_then(|s| s.strip_suffix(":gateway"))
                {
                    result.insert(channel_id.to_string(), value);
                }
            }
        }

        Ok(result)
    }

    /// Get the gateway address for a channel
    async fn get_gateway(&self, channel_id: &str) -> Option<String> {
        let manager = {
            let conn = self.redis.read().await;
            match &*conn {
                Some(m) => m.clone(),
                None => return None,
            }
        };

        let mut conn = manager;
        let key = Self::channel_key(channel_id);

        let mut cmd = redis::cmd("GET");
        cmd.arg(&key);

        match tokio::time::timeout(
            std::time::Duration::from_millis(50),
            cmd.query_async::<String>(&mut conn),
        )
        .await
        {
            Ok(Ok(gateway)) => Some(gateway),
            _ => None,
        }
    }

    /// Check if a channel is connected to THIS gateway (local check)
    async fn is_local(&self, channel_id: &str) -> bool {
        if let Some(gateway) = self.get_gateway(channel_id).await {
            // Compare with our own gateway address
            // This is set during DirectPushSource creation
            gateway == self.get_local_addr()
        } else {
            false
        }
    }

    fn get_local_addr(&self) -> String {
        // This will be set from environment
        std::env::var("GATEWAY_ADDR").unwrap_or_else(|_| "localhost:8080".to_string())
    }
}

/// Direct push source with Redis-backed channel registry
struct DirectPushSource {
    push_port: u16,
    receiver: tokio::sync::Mutex<Option<mpsc::Receiver<IncomingMessage>>>,
    sender: mpsc::Sender<IncomingMessage>,
    /// Redis-backed channel registry
    channel_registry: RedisChannelRegistry,
    /// Redis storage for direct writes
    storage: RedisStorage,
    /// This gateway's address
    gateway_addr: String,
}

impl DirectPushSource {
    fn new(
        push_port: u16,
        gateway_addr: String,
        channel_registry: RedisChannelRegistry,
        storage: RedisStorage,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(1000);
        Self {
            push_port,
            receiver: tokio::sync::Mutex::new(Some(receiver)),
            sender,
            channel_registry,
            storage,
            gateway_addr,
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

        // Start HTTP server for direct push and store
        let sender = self.sender.clone();
        let registry = self.channel_registry.clone();
        let storage = self.storage.clone();
        let push_router = Router::new()
            .route("/push", post(handle_push))                          // Push (real-time + store)
            .route("/store", post(handle_store))                        // Direct store (offline)
            .route("/channel/{id}", axum::routing::get(handle_channel_status))  // Query channel status
            .route("/registry", axum::routing::get(get_registry))       // Debug: view all channels
            .with_state(AppState { sender, registry, storage, connection_manager });

        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], self.push_port));
        let listener = tokio::net::TcpListener::bind(addr).await?;

        tracing::info!(port = self.push_port, "Direct push server listening");

        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            axum::serve(listener, push_router)
                .with_graceful_shutdown(async move {
                    cancel_clone.cancelled().await;
                })
                .await
                .ok();
        });

        // Forward messages to handler
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
        "DirectPush+Redis"
    }

    /// Called when a new SSE connection is established
    fn on_connect(&self, info: &ConnectionInfo) {
        tracing::debug!(
            channel_id = %info.channel_id,
            connection_id = %info.connection_id,
            "SSE connected, registering channel"
        );

        let registry = self.channel_registry.clone();
        let channel_id = info.channel_id.clone();
        let gateway_addr = self.gateway_addr.clone();

        // Spawn async task to register in Redis (fire-and-forget)
        tokio::spawn(async move {
            registry.register(&channel_id, &gateway_addr).await;
        });
    }

    /// Called when an SSE connection is closed
    fn on_disconnect(&self, info: &ConnectionInfo) {
        tracing::debug!(
            channel_id = %info.channel_id,
            connection_id = %info.connection_id,
            "SSE disconnected, unregistering channel"
        );

        let registry = self.channel_registry.clone();
        let channel_id = info.channel_id.clone();

        // Spawn async task to unregister from Redis (fire-and-forget)
        tokio::spawn(async move {
            registry.unregister(&channel_id).await;
        });
    }
}

#[derive(Clone)]
struct AppState {
    sender: mpsc::Sender<IncomingMessage>,
    registry: RedisChannelRegistry,
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

#[derive(serde::Deserialize)]
struct SendPayload {
    channel_id: String,
    event_type: String,
    data: serde_json::Value,
}

#[derive(serde::Serialize)]
struct PushResponse {
    success: bool,
    /// Whether the channel has an active connection on THIS gateway
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
    gateway: Option<String>,
}

/// Direct push endpoint - sends to connected clients + stores to Redis
/// 
/// Returns:
/// - `online: true` if the channel has a connection on THIS gateway
/// - `online: false` if the channel is not connected to THIS gateway
///   (user may be offline or connected to another gateway)
/// 
/// Agent should cache the gateway address and check `online` field:
/// - If `online: false`, refresh cache from Redis and retry
async fn handle_push(
    State(state): State<AppState>,
    Json(payload): Json<PushPayload>,
) -> Json<PushResponse> {
    use sse_gateway::SseEvent;

    let channel_id = payload.channel_id.clone().unwrap_or_default();
    
    // Generate stream_id for storage
    let stream_id = state.storage.generate_id();
    
    // Check if channel has active connections on THIS gateway (local memory lookup, no Redis)
    let online = if !channel_id.is_empty() {
        state.connection_manager.channel_connection_count(&channel_id) > 0
    } else {
        false
    };

    // Create message and send through gateway
    let mut msg = IncomingMessage::new(&payload.event_type, payload.data.to_string());
    if let Some(cid) = payload.channel_id {
        msg = msg.with_channel(cid);
    }

    let success = state.sender.send(msg).await.is_ok();

    // Also store to Redis (already done by gateway, but we need the stream_id)
    if !channel_id.is_empty() {
        let event = SseEvent::raw(&payload.event_type, payload.data.to_string());
        state.storage.store(&channel_id, &stream_id, &event).await;
    }

    Json(PushResponse {
        success,
        online,
        stream_id,
    })
}

/// Store endpoint - directly writes to Redis without going through Gateway
/// Use this when clients are NOT connected (offline storage)
/// 
/// The stored message can be retrieved when client reconnects using Last-Event-ID
async fn handle_store(
    State(state): State<AppState>,
    Json(payload): Json<StorePayload>,
) -> Json<StoreResponse> {
    use sse_gateway::SseEvent;

    let stream_id = state.storage.generate_id();
    let event = SseEvent::raw(&payload.event_type, payload.data.to_string());

    // Store directly to Redis
    state.storage.store(&payload.channel_id, &stream_id, &event).await;

    Json(StoreResponse {
        success: true,
        stream_id,
    })
}

/// Query channel status - check if a channel is online and which gateway it's connected to
/// 
/// Agent can use this to:
/// 1. Check if user is online before starting generation
/// 2. Get the correct gateway address to push to
async fn handle_channel_status(
    State(state): State<AppState>,
    axum::extract::Path(channel_id): axum::extract::Path<String>,
) -> Json<ChannelStatus> {
    let gateway = state.registry.get_gateway(&channel_id).await;
    
    Json(ChannelStatus {
        channel_id,
        online: gateway.is_some(),
        gateway,
    })
}

/// Debug endpoint to view current channel registry from Redis
async fn get_registry(State(state): State<AppState>) -> Json<serde_json::Value> {
    match state.registry.get_all().await {
        Ok(registry) => Json(serde_json::json!(registry)),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install rustls crypto provider (required for TLS)
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Initialize tracing
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

    let gateway_addr = std::env::var("GATEWAY_ADDR")
        .unwrap_or_else(|_| format!("localhost:{}", gateway_port));

    let ttl_seconds: u64 = std::env::var("CHANNEL_TTL")
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or(60);

    // Initialize Redis channel registry
    let channel_registry = RedisChannelRegistry::new(ttl_seconds);
    channel_registry.connect(&redis_url).await?;

    // Initialize Redis storage for message replay
    let storage = RedisStorage::new();
    storage.connect(&redis_url).await?;

    // Create direct push source
    let source = DirectPushSource::new(
        push_port,
        gateway_addr.clone(),
        channel_registry,
        storage.clone(),
    );

    tracing::info!(
        gateway_port,
        push_port,
        redis_url = %redis_url,
        gateway_addr = %gateway_addr,
        ttl_seconds,
        "Starting SSE Gateway with Direct Push + Redis"
    );

    println!("===========================================");
    println!("  SSE Gateway with Direct Push + Redis");
    println!("===========================================");
    println!();
    println!(
        "SSE endpoint:     http://localhost:{}/sse/connect?channel_id=test",
        gateway_port
    );
    println!("Dashboard:        http://localhost:{}/dashboard", gateway_port);
    println!();
    println!("API Endpoints (port {}):", push_port);
    println!("  POST /push              Push message (real-time + store)");
    println!("  POST /store             Store only (offline)");
    println!("  GET  /channel/{{id}}      Query channel status");
    println!("  GET  /registry          View all channels");
    println!();
    println!("Redis URL:        {}", redis_url);
    println!("Gateway addr:     {}", gateway_addr);
    println!("Channel TTL:      {}s", ttl_seconds);
    println!();
    println!("Environment variables:");
    println!("  PORT={}          (SSE gateway port)", gateway_port);
    println!("  PUSH_PORT={}     (Direct push port)", push_port);
    println!("  REDIS_URL={}     (Redis connection)", redis_url);
    println!("  GATEWAY_ADDR={}  (This gateway's address)", gateway_addr);
    println!("  CHANNEL_TTL={}   (Channel mapping TTL in seconds)", ttl_seconds);
    println!();

    Gateway::builder()
        .port(gateway_port)
        .dashboard(true)
        .source(source)
        .storage(storage)
        .build()?
        .run()
        .await
}
