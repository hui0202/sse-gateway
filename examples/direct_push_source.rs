//! Example: Direct Push Source with connection lifecycle hooks
//!
//! Run with: cargo run --example direct_push_source
//!
//! This example shows how to:
//! 1. Register channel-to-gateway mappings when SSE connections are established
//! 2. Clean up mappings when connections are closed
//! 3. Receive direct push via HTTP (bypassing pub/sub for lower latency)
//!
//! Architecture:
//!   Agent → HTTP POST /push → Gateway (direct) → SSE Client
//!
//! For production, store mappings in Redis and query from your Agent service.

use async_trait::async_trait;
use axum::{extract::State, routing::post, Json, Router};
use sse_gateway::{
    CancellationToken, ConnectionInfo, ConnectionManager, Gateway, IncomingMessage, MemoryStorage,
    MessageHandler, MessageSource,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// Channel registry type: channel_id -> gateway_addr
type ChannelRegistry = Arc<RwLock<HashMap<String, String>>>;

/// Direct push source with connection lifecycle hooks
struct DirectPushSource {
    port: u16,
    receiver: tokio::sync::Mutex<Option<mpsc::Receiver<IncomingMessage>>>,
    sender: mpsc::Sender<IncomingMessage>,
    /// Simulates Redis: channel_id -> gateway_addr
    channel_registry: ChannelRegistry,
    /// This gateway's address
    gateway_addr: String,
}

impl DirectPushSource {
    fn new(port: u16, gateway_addr: String) -> Self {
        let (sender, receiver) = mpsc::channel(1000);
        Self {
            port,
            receiver: tokio::sync::Mutex::new(Some(receiver)),
            sender,
            channel_registry: Arc::new(RwLock::new(HashMap::new())),
            gateway_addr,
        }
    }
}

#[async_trait]
impl MessageSource for DirectPushSource {
    async fn start(
        &self,
        handler: MessageHandler,
        _connection_manager: ConnectionManager,
        cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        let mut receiver = self
            .receiver
            .lock()
            .await
            .take()
            .ok_or_else(|| anyhow::anyhow!("Source already started"))?;

        // Start HTTP server for direct push
        let sender = self.sender.clone();
        let registry = self.channel_registry.clone();
        let push_router = Router::new()
            .route("/push", post(handle_push))
            .route("/registry", axum::routing::get(get_registry))
            .with_state(AppState { sender, registry });

        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], self.port));
        let listener = tokio::net::TcpListener::bind(addr).await?;

        tracing::info!(port = self.port, "Direct push server listening");

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
        "DirectPush"
    }

    /// Called when a new SSE connection is established
    fn on_connect(&self, info: &ConnectionInfo) {
        tracing::info!(
            channel_id = %info.channel_id,
            connection_id = %info.connection_id,
            instance_id = %info.instance_id,
            "Registering channel -> gateway mapping"
        );

        // In production: redis.set_ex(f"channel:{channel_id}:gateway", gateway_addr, 60)
        // Note: using try_write to avoid blocking in sync context
        if let Ok(mut registry) = self.channel_registry.try_write() {
            registry.insert(info.channel_id.clone(), self.gateway_addr.clone());
        }
    }

    /// Called when an SSE connection is closed
    fn on_disconnect(&self, info: &ConnectionInfo) {
        tracing::info!(
            channel_id = %info.channel_id,
            connection_id = %info.connection_id,
            "Cleaning up channel -> gateway mapping"
        );

        // In production: redis.del(f"channel:{channel_id}:gateway")
        if let Ok(mut registry) = self.channel_registry.try_write() {
            registry.remove(&info.channel_id);
        }
    }
}

#[derive(Clone)]
struct AppState {
    sender: mpsc::Sender<IncomingMessage>,
    registry: ChannelRegistry,
}

#[derive(serde::Deserialize)]
struct PushPayload {
    channel_id: Option<String>,
    event_type: String,
    data: serde_json::Value,
}

/// Direct push endpoint - Agent calls this directly
async fn handle_push(State(state): State<AppState>, Json(payload): Json<PushPayload>) -> &'static str {
    let mut msg = IncomingMessage::new(&payload.event_type, payload.data.to_string());
    if let Some(channel_id) = payload.channel_id {
        msg = msg.with_channel(channel_id);
    }

    if state.sender.send(msg).await.is_ok() {
        "OK"
    } else {
        "ERROR"
    }
}

/// Debug endpoint to view current channel registry
async fn get_registry(State(state): State<AppState>) -> Json<serde_json::Value> {
    let registry = state.registry.read().await;
    Json(serde_json::json!(*registry))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let gateway_port = 8080;
    let push_port = 9000;
    let gateway_addr = format!("localhost:{}", gateway_port);

    let source = DirectPushSource::new(push_port, gateway_addr);

    println!("===========================================");
    println!("  SSE Gateway with Direct Push Support");
    println!("===========================================");
    println!();
    println!("SSE endpoint:     http://localhost:{}/sse/connect?channel_id=test", gateway_port);
    println!("Dashboard:        http://localhost:{}/dashboard", gateway_port);
    println!();
    println!("Direct push:      http://localhost:{}/push", push_port);
    println!("View registry:    http://localhost:{}/registry", push_port);
    println!();
    println!("Usage:");
    println!("  1. Connect SSE client:");
    println!("     curl -N 'http://localhost:{}/sse/connect?channel_id=test'", gateway_port);
    println!();
    println!("  2. Check registry (channel should be registered):");
    println!("     curl http://localhost:{}/registry", push_port);
    println!();
    println!("  3. Send direct push:");
    println!(r#"     curl -X POST http://localhost:{}/push \"#, push_port);
    println!(r#"       -H "Content-Type: application/json" \"#);
    println!(r#"       -d '{{"channel_id": "test", "event_type": "message", "data": {{"msg": "Hello!"}}}}'"#);
    println!();
    println!("  4. Disconnect SSE and check registry again (should be empty)");
    println!();

    Gateway::builder()
        .port(gateway_port)
        .source(source)
        .storage(MemoryStorage::default())
        .build()?
        .run()
        .await
}
