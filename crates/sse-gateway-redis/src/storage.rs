//! Redis Streams message storage

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use redis::streams::{StreamId, StreamRangeReply};
use sse_gateway::{EventData, MessageStorage, SseEvent};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

const MAX_MESSAGES_PER_CHANNEL: usize = 100;

/// Redis Streams message storage
///
/// Stores messages in Redis Streams for replay on reconnection.
///
/// # Example
///
/// ```rust,ignore
/// use sse_gateway::Gateway;
/// use sse_gateway_redis::RedisStorage;
///
/// let storage = RedisStorage::new();
/// storage.connect("redis://localhost:6379").await?;
///
/// Gateway::builder()
///     .source(sse_gateway::NoopSource)
///     .storage(storage)
///     .build()?
///     .run()
///     .await
/// ```
#[derive(Clone)]
pub struct RedisStorage {
    redis: Arc<RwLock<Option<ConnectionManager>>>,
    max_per_channel: usize,
}

impl RedisStorage {
    /// Create a new Redis storage
    pub fn new() -> Self {
        Self {
            redis: Arc::new(RwLock::new(None)),
            max_per_channel: MAX_MESSAGES_PER_CHANNEL,
        }
    }

    /// Create with custom max messages per channel
    pub fn with_max_messages(max_per_channel: usize) -> Self {
        Self {
            redis: Arc::new(RwLock::new(None)),
            max_per_channel,
        }
    }

    /// Connect to Redis
    pub async fn connect(&self, redis_url: &str) -> anyhow::Result<()> {
        let client = redis::Client::open(redis_url)?;
        let manager = ConnectionManager::new(client).await?;
        *self.redis.write().await = Some(manager);
        info!("Redis storage connected");
        Ok(())
    }

    fn stream_key(channel_id: &str) -> String {
        format!("sse:stream:{}", channel_id)
    }

    /// Check if the ID is a valid Redis Stream ID format (timestamp-sequence)
    fn is_valid_stream_id(id: &str) -> bool {
        let parts: Vec<&str> = id.split('-').collect();
        if parts.len() != 2 {
            return false;
        }
        // Both parts should be numeric
        parts[0].parse::<u64>().is_ok() && parts[1].parse::<u64>().is_ok()
    }

    fn parse_stream_entries(entries: Vec<StreamId>) -> Vec<SseEvent> {
        entries
            .into_iter()
            .filter_map(|entry| {
                let stream_id = entry.id;
                let map = entry.map;

                let event_type = map.get("event_type").and_then(|v| match v {
                    redis::Value::BulkString(bytes) => String::from_utf8(bytes.clone()).ok(),
                    redis::Value::SimpleString(s) => Some(s.clone()),
                    _ => None,
                })?;

                let data = map.get("data").and_then(|v| match v {
                    redis::Value::BulkString(bytes) => String::from_utf8(bytes.clone()).ok(),
                    redis::Value::SimpleString(s) => Some(s.clone()),
                    _ => None,
                })?;

                let id = map.get("id").and_then(|v| match v {
                    redis::Value::BulkString(bytes) => String::from_utf8(bytes.clone()).ok(),
                    redis::Value::SimpleString(s) => Some(s.clone()),
                    _ => None,
                });

                Some(SseEvent {
                    event_type,
                    data: EventData::Raw(data),
                    id,
                    stream_id: Some(stream_id),
                    retry: None,
                })
            })
            .collect()
    }
}

impl Default for RedisStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MessageStorage for RedisStorage {
    async fn store(&self, channel_id: &str, event: &SseEvent) -> Option<String> {
        let conn = self.redis.read().await;
        let Some(ref manager) = *conn else {
            return None;
        };

        let mut conn = manager.clone();
        let key = Self::stream_key(channel_id);
        let data_str = event.data.to_string();

        let mut cmd = redis::cmd("XADD");
        cmd.arg(&key)
            .arg("MAXLEN")
            .arg("~")
            .arg(self.max_per_channel)
            .arg("*")
            .arg("event_type")
            .arg(&event.event_type)
            .arg("data")
            .arg(&data_str);

        if let Some(ref id) = event.id {
            cmd.arg("id").arg(id);
        }

        match cmd.query_async::<String>(&mut conn).await {
            Ok(stream_id) => Some(stream_id),
            Err(e) => {
                warn!(error = %e, "Failed to store message");
                None
            }
        }
    }

    async fn get_messages_after(&self, channel_id: &str, after_id: Option<&str>) -> Vec<SseEvent> {
        let after_id = match after_id {
            Some(id) => id,
            None => return vec![],
        };

        // Validate Redis Stream ID format: "timestamp-sequence" (e.g., "1234567890123-0")
        // If the ID is not in this format (e.g., UUID), we can't use it for XRANGE
        if !Self::is_valid_stream_id(after_id) {
            warn!(
                id = %after_id,
                "Invalid Redis Stream ID format, skipping replay"
            );
            return vec![];
        }

        let conn = self.redis.read().await;
        let Some(ref manager) = *conn else {
            return vec![];
        };

        let mut conn = manager.clone();
        let key = Self::stream_key(channel_id);
        let start = format!("({}", after_id);

        match redis::cmd("XRANGE")
            .arg(&key)
            .arg(&start)
            .arg("+")
            .arg("COUNT")
            .arg(self.max_per_channel)
            .query_async::<StreamRangeReply>(&mut conn)
            .await
        {
            Ok(reply) => Self::parse_stream_entries(reply.ids),
            Err(e) => {
                warn!(error = %e, "Failed to get messages");
                vec![]
            }
        }
    }

    async fn is_available(&self) -> bool {
        self.redis.read().await.is_some()
    }

    fn name(&self) -> &'static str {
        "Redis Streams"
    }
}
