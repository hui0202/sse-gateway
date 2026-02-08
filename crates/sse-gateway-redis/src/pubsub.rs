//! Redis Pub/Sub message source

use async_trait::async_trait;
use sse_gateway::{IncomingMessage, MessageHandler, MessageSource};
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Redis Pub/Sub message source
///
/// # Message Format
///
/// Messages can be plain text or JSON:
///
/// ```json
/// {
///     "event_type": "notification",
///     "data": {"text": "Hello!"},
///     "channel_id": "user123",
///     "id": "msg-001"
/// }
/// ```
///
/// # Channel Naming
///
/// - `sse:{channel_id}` - Send to specific channel
/// - `sse:broadcast` - Broadcast to all connections
///
/// # Example
///
/// ```rust,ignore
/// use sse_gateway::Gateway;
/// use sse_gateway_redis::RedisPubSubSource;
///
/// Gateway::builder()
///     .source(RedisPubSubSource::new("redis://localhost:6379", vec!["sse:*".into()]))
///     .storage(sse_gateway::MemoryStorage::default())
///     .build()?
///     .run()
///     .await
/// ```
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

    /// Create with default pattern "sse:*"
    pub fn with_defaults(redis_url: impl Into<String>) -> Self {
        Self::new(redis_url, vec!["sse:*".to_string()])
    }

    fn parse_channel_id(channel_name: &str) -> Option<String> {
        if channel_name == "sse:broadcast" {
            return None;
        }
        channel_name.strip_prefix("sse:").map(|s| s.to_string())
    }

    fn parse_message(payload: &str) -> IncomingMessage {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(payload) {
            let event_type = json
                .get("event_type")
                .and_then(|v| v.as_str())
                .unwrap_or("message")
                .to_string();

            let data = json
                .get("data")
                .map(|v| {
                    if v.is_string() {
                        v.as_str().unwrap().to_string()
                    } else {
                        v.to_string()
                    }
                })
                .unwrap_or_else(|| payload.to_string());

            let id = json.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
            let channel_id = json
                .get("channel_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            IncomingMessage {
                channel_id,
                event_type,
                data,
                id,
            }
        } else {
            IncomingMessage {
                channel_id: None,
                event_type: "message".to_string(),
                data: payload.to_string(),
                id: None,
            }
        }
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
                _ = cancel.cancelled() => break,
                msg = stream.next() => {
                    match msg {
                        Some(msg) => {
                            let channel: String = msg.get_channel_name().to_string();
                            if let Ok(payload) = msg.get_payload::<String>() {
                                let mut incoming = Self::parse_message(&payload);
                                if incoming.channel_id.is_none() {
                                    incoming.channel_id = Self::parse_channel_id(&channel);
                                }
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
