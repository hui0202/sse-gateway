use redis::aio::ConnectionManager;
use redis::streams::{StreamRangeReply, StreamId};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::sse::{SseEvent, EventData};

const MAX_MESSAGES_PER_CHANNEL: usize = 100;

#[derive(Clone)]
pub struct MessageStore {
    redis: Arc<RwLock<Option<ConnectionManager>>>,
}

impl MessageStore {
    pub fn new() -> Self {
        Self {
            redis: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn connect(&self, redis_url: &str) -> anyhow::Result<()> {
        let client = redis::Client::open(redis_url)?;
        let manager = ConnectionManager::new(client).await?;
        
        let mut conn = self.redis.write().await;
        *conn = Some(manager);
        
        info!("MessageStore connected to Redis");
        Ok(())
    }

    pub async fn is_connected(&self) -> bool {
        self.redis.read().await.is_some()
    }

    fn stream_key(channel_id: &str) -> String {
        format!("sse:stream:{}", channel_id)
    }

    pub async fn store(&self, channel_id: &str, event: &SseEvent) -> Option<String> {
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
            .arg("~")  // 近似修剪，性能更好
            .arg(MAX_MESSAGES_PER_CHANNEL)
            .arg("*")  // 自动生成 stream_id
            .arg("event_type")
            .arg(&event.event_type)
            .arg("data")
            .arg(&data_str);
        
        if let Some(ref id) = event.id {
            cmd.arg("id").arg(id);
        }

        let result: Result<String, redis::RedisError> = cmd.query_async(&mut conn).await;

        match result {
            Ok(stream_id) => {
                info!(channel_id, stream_id = %stream_id, "Message stored in Redis Stream");
                Some(stream_id)
            }
            Err(e) => {
                warn!(error = %e, channel_id, "Failed to store message in Redis Stream");
                None
            }
        }
    }

    pub async fn get_messages_after(&self, channel_id: &str, after_id: Option<&str>) -> Vec<SseEvent> {
        let after_id = match after_id {
            Some(id) => id,
            None => return vec![],
        };

        let conn = self.redis.read().await;
        let Some(ref manager) = *conn else {
            return vec![];
        };

        let mut conn = manager.clone();
        let key = Self::stream_key(channel_id);

        // XRANGE key (after_id +
        // "(" 表示不包含 after_id 本身（exclusive）
        let start = format!("({}", after_id);
        
        let result: Result<StreamRangeReply, redis::RedisError> = redis::cmd("XRANGE")
            .arg(&key)
            .arg(&start)
            .arg("+")
            .arg("COUNT")
            .arg(MAX_MESSAGES_PER_CHANNEL)
            .query_async(&mut conn)
            .await;

        match result {
            Ok(reply) => {
                let events = Self::parse_stream_entries(reply.ids);
                info!(
                    channel_id,
                    after_id,
                    count = events.len(),
                    "Retrieved messages from Redis Stream"
                );
                events
            }
            Err(e) => {
                warn!(error = %e, channel_id, after_id, "Failed to get messages from Redis Stream");
                vec![]
            }
        }
    }

    fn parse_stream_entries(entries: Vec<StreamId>) -> Vec<SseEvent> {
        entries
            .into_iter()
            .filter_map(|entry| {
                let stream_id = entry.id;
                let map = entry.map;
                
                let event_type = map.get("event_type")
                    .and_then(|v| match v {
                        redis::Value::BulkString(bytes) => String::from_utf8(bytes.clone()).ok(),
                        redis::Value::SimpleString(s) => Some(s.clone()),
                        _ => None,
                    })?;
                
                let data = map.get("data")
                    .and_then(|v| match v {
                        redis::Value::BulkString(bytes) => String::from_utf8(bytes.clone()).ok(),
                        redis::Value::SimpleString(s) => Some(s.clone()),
                        _ => None,
                    })?;
                
                let id = map.get("id")
                    .and_then(|v| match v {
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

impl Default for MessageStore {
    fn default() -> Self {
        Self::new()
    }
}
