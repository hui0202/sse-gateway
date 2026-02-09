//! Example: HTTP Webhook as message source
//!
//! Run with: cargo run --example webhook_source
//!
//! This example shows how to receive messages via HTTP POST and forward them to SSE clients.
//!
//! Send messages with:
//! curl -X POST http://localhost:9000/webhook \
//!   -H "Content-Type: application/json" \
//!   -d '{"channel_id": "test", "event_type": "notification", "data": {"msg": "Hello!"}}'

use async_trait::async_trait;
use axum::{extract::State, routing::post, Json, Router};
use sse_gateway::{
    CancellationToken, ConnectionManager, Gateway, IncomingMessage, MemoryStorage, MessageHandler,
    MessageSource,
};
use tokio::sync::mpsc;

/// Webhook source that receives messages via HTTP
struct WebhookSource {
    port: u16,
    receiver: tokio::sync::Mutex<Option<mpsc::Receiver<IncomingMessage>>>,
    sender: mpsc::Sender<IncomingMessage>,
}

impl WebhookSource {
    fn new(port: u16) -> Self {
        let (sender, receiver) = mpsc::channel(1000);
        Self {
            port,
            receiver: tokio::sync::Mutex::new(Some(receiver)),
            sender,
        }
    }

    fn sender(&self) -> mpsc::Sender<IncomingMessage> {
        self.sender.clone()
    }
}

#[async_trait]
impl MessageSource for WebhookSource {
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

        // Start webhook HTTP server
        let sender = self.sender.clone();
        let webhook_router = Router::new()
            .route("/webhook", post(handle_webhook))
            .with_state(sender);

        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], self.port));
        let listener = tokio::net::TcpListener::bind(addr).await?;

        tracing::info!(port = self.port, "Webhook server listening");

        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            axum::serve(listener, webhook_router)
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
        "Webhook"
    }
}

#[derive(serde::Deserialize)]
struct WebhookPayload {
    channel_id: Option<String>,
    event_type: String,
    data: serde_json::Value,
}

async fn handle_webhook(
    State(sender): State<mpsc::Sender<IncomingMessage>>,
    Json(payload): Json<WebhookPayload>,
) -> &'static str {
    let mut msg = IncomingMessage::new(&payload.event_type, payload.data.to_string());
    if let Some(channel_id) = payload.channel_id {
        msg = msg.with_channel(channel_id);
    }

    if sender.send(msg).await.is_ok() {
        "OK"
    } else {
        "ERROR"
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let source = WebhookSource::new(9000);

    println!("Starting SSE Gateway with Webhook source");
    println!();
    println!("SSE endpoint: http://localhost:8080/sse/connect?channel_id=test");
    println!("Dashboard: http://localhost:8080/dashboard");
    println!();
    println!("Send messages with:");
    println!(r#"  curl -X POST http://localhost:9000/webhook \"#);
    println!(r#"    -H "Content-Type: application/json" \"#);
    println!(r#"    -d '{{"channel_id": "test", "event_type": "notification", "data": {{"msg": "Hello!"}}}}'"#);

    Gateway::builder()
        .port(8080)
        .source(source)
        .storage(MemoryStorage::default())
        .build()?
        .run()
        .await
}
