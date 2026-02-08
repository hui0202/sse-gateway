//! Example: Custom message source
//!
//! This example shows how to implement your own message source.
//! Run with: cargo run --example custom_source
//!
//! Then open http://localhost:8080/dashboard and connect to channel "demo"

use async_trait::async_trait;
use sse_gateway::{
    CancellationToken, Gateway, IncomingMessage, MemoryStorage, MessageHandler, MessageSource,
};
use std::time::Duration;

/// A custom source that generates periodic messages
struct TimerSource {
    interval: Duration,
    channel_id: String,
}

impl TimerSource {
    fn new(channel_id: impl Into<String>, interval: Duration) -> Self {
        Self {
            interval,
            channel_id: channel_id.into(),
        }
    }
}

#[async_trait]
impl MessageSource for TimerSource {
    async fn start(&self, handler: MessageHandler, cancel: CancellationToken) -> anyhow::Result<()> {
        tracing::info!("TimerSource started");

        let mut interval = tokio::time::interval(self.interval);
        let mut count = 0u64;

        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = interval.tick() => {
                    count += 1;
                    let msg = IncomingMessage::new(
                        "tick",
                        serde_json::json!({
                            "count": count,
                            "timestamp": chrono::Utc::now().to_rfc3339()
                        }).to_string()
                    ).with_channel(&self.channel_id);

                    handler(msg);
                    tracing::info!(count, "Sent tick");
                }
            }
        }

        tracing::info!("TimerSource stopped");
        Ok(())
    }

    fn name(&self) -> &'static str {
        "Timer"
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    println!("Starting SSE Gateway with custom TimerSource");
    println!("Open http://localhost:8080/dashboard and connect to channel 'demo'");

    Gateway::builder()
        .port(8080)
        .source(TimerSource::new("demo", Duration::from_secs(2)))
        .storage(MemoryStorage::default())
        .build()?
        .run()
        .await
}
