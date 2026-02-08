use redis::{aio::ConnectionManager, AsyncCommands};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::sse::SseEvent;

/// 每个 channel 最多缓存的消息数
const MAX_MESSAGES_PER_CHANNEL: isize = 100;
/// 消息过期时间（秒）
const MESSAGE_TTL_SECONDS: i64 = 3600; // 1 小时

/// Redis 消息存储，支持多实例共享
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

    /// 连接 Redis
    pub async fn connect(&self, redis_url: &str) -> anyhow::Result<()> {
        let client = redis::Client::open(redis_url)?;
        let manager = ConnectionManager::new(client).await?;
        
        let mut conn = self.redis.write().await;
        *conn = Some(manager);
        
        info!("MessageStore connected to Redis");
        Ok(())
    }

    /// 检查是否已连接
    pub async fn is_connected(&self) -> bool {
        self.redis.read().await.is_some()
    }

    fn channel_key(channel_id: &str) -> String {
        format!("sse:messages:{}", channel_id)
    }

    /// 存储消息
    pub async fn store(&self, channel_id: &str, event: &SseEvent) {
        let conn = self.redis.read().await;
        let Some(ref manager) = *conn else {
            return; // Redis 未连接，静默跳过
        };

        let mut conn = manager.clone();
        let key = Self::channel_key(channel_id);

        // 序列化消息
        let message = match serde_json::to_string(event) {
            Ok(m) => m,
            Err(e) => {
                error!(error = %e, "Failed to serialize message");
                return;
            }
        };

        // 使用 pipeline 执行：LPUSH + LTRIM + EXPIRE
        let result: Result<(), redis::RedisError> = redis::pipe()
            .lpush(&key, &message)
            .ltrim(&key, 0, MAX_MESSAGES_PER_CHANNEL - 1)
            .expire(&key, MESSAGE_TTL_SECONDS)
            .query_async(&mut conn)
            .await;

        if let Err(e) = result {
            warn!(error = %e, channel_id, "Failed to store message in Redis");
        }
    }

    /// 获取指定 message_id 之后的所有消息
    pub async fn get_messages_after(&self, channel_id: &str, after_id: Option<&str>) -> Vec<SseEvent> {
        let after_id = match after_id {
            Some(id) => id,
            None => return vec![], // 新连接，不需要历史消息
        };

        let conn = self.redis.read().await;
        let Some(ref manager) = *conn else {
            return vec![]; // Redis 未连接
        };

        let mut conn = manager.clone();
        let key = Self::channel_key(channel_id);

        // 获取所有缓存的消息（按时间倒序存储，所以需要反转）
        let messages: Vec<String> = match conn.lrange(&key, 0, MAX_MESSAGES_PER_CHANNEL - 1).await {
            Ok(m) => m,
            Err(e) => {
                warn!(error = %e, channel_id, "Failed to get messages from Redis");
                return vec![];
            }
        };

        // 反转顺序（因为 LPUSH 是头部插入）
        let messages: Vec<SseEvent> = messages
            .into_iter()
            .rev()
            .filter_map(|m| serde_json::from_str(&m).ok())
            .collect();

        // 找到 after_id 之后的消息
        let mut found = false;
        let mut result = Vec::new();

        for msg in &messages {
            if found {
                result.push(msg.clone());
            } else if msg.id.as_deref() == Some(after_id) {
                found = true;
            }
        }

        // 如果没找到 after_id（可能消息已过期），返回所有缓存的消息
        if !found {
            warn!(channel_id, after_id, "Last-Event-ID not found, returning all cached messages");
            return messages;
        }

        result
    }
}

impl Default for MessageStore {
    fn default() -> Self {
        Self::new()
    }
}
