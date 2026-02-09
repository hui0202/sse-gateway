//! Redis Pub/Sub message source

use async_trait::async_trait;
use sse_gateway::{IncomingMessage, MessageHandler, MessageSource};
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn, debug};

pub struct RedisPubSubSource {
    redis_url: String,
    patterns: Vec<String>,
}

impl RedisPubSubSource {
    /// Create a new Redis Pub/Sub source
    pub fn new(redis_url: impl Into<String>, patterns: Vec<String>) -> Self {
        Self {
            redis_url: redis_url.into(),
            patterns,
        }
    }

    /// Create with default pattern "*"
    pub fn with_defaults(redis_url: impl Into<String>) -> Self {
        Self::new(redis_url, vec!["*".to_string()])
    }
}

#[async_trait]
impl MessageSource for RedisPubSubSource {
    async fn start(&self, handler: MessageHandler, cancel: CancellationToken) -> anyhow::Result<()> {
        info!(url = %self.redis_url, patterns = ?self.patterns, "Starting Redis Pub/Sub");

        let client = redis::Client::open(self.redis_url.as_str())?;
        let mut pubsub = client.get_async_pubsub().await?;

        for pattern in &self.patterns {
            pubsub.psubscribe(pattern).await?;
            info!(pattern = %pattern, "Subscribed");
        }

        let mut stream = pubsub.into_on_message();

        loop {
            tokio::select! {
                biased;  // 优先检查 cancel
                
                _ = cancel.cancelled() => break,
                
                msg = stream.next() => {
                    match msg {
                        Some(msg) => {
                            let channel = msg.get_channel_name().to_string();
                            if let Ok(payload) = msg.get_payload::<String>() {
                                debug!(channel = %channel, "Received message");
                                
                                let incoming = IncomingMessage {
                                    channel_id: Some(channel),
                                    event_type: "message".to_string(),
                                    data: payload,
                                    id: None,
                                };
                                
                                handler(incoming);
                            }
                        }
                        None => {
                            warn!("Redis stream ended");
                            break;
                        }
                    }
                }
            }
        }

        info!("Redis Pub/Sub stopped");
        Ok(())
    }

    fn name(&self) -> &'static str {
        "Redis Pub/Sub"
    }
}
