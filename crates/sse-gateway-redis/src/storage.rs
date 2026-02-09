//! Redis Streams message storage with batching support

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use redis::streams::{StreamId, StreamRangeReply};
use sse_gateway::{EventData, MessageStorage, SseEvent};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{info, warn};

const MAX_MESSAGES_PER_CHANNEL: usize = 100;
const BATCH_SIZE: usize = 100;
const BATCH_FLUSH_INTERVAL_MS: u64 = 10;
const DEFAULT_TTL_SECONDS: u64 = 3600; // 1 hour

/// Message to be stored
struct StoreRequest {
    channel_id: String,
    stream_id: String,
    event_type: String,
    data: String,
    id: Option<String>,
}

/// Redis Streams message storage with batching
///
/// Uses a background task to batch multiple XADD commands into a single pipeline,
/// reducing network round-trips and improving throughput.
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
    counter: Arc<AtomicU64>,
    /// TTL for stream keys in seconds
    ttl_seconds: u64,
    /// Channel for batching store requests
    store_tx: mpsc::Sender<StoreRequest>,
}

impl RedisStorage {
    /// Create a new Redis storage with batching
    pub fn new() -> Self {
        Self::with_options(MAX_MESSAGES_PER_CHANNEL, DEFAULT_TTL_SECONDS)
    }

    /// Create with custom max messages per channel
    pub fn with_max_messages(max_per_channel: usize) -> Self {
        Self::with_options(max_per_channel, DEFAULT_TTL_SECONDS)
    }

    /// Create with custom max messages and TTL
    pub fn with_options(max_per_channel: usize, ttl_seconds: u64) -> Self {
        let (store_tx, store_rx) = mpsc::channel(10000);
        let storage = Self {
            redis: Arc::new(RwLock::new(None)),
            max_per_channel,
            counter: Arc::new(AtomicU64::new(0)),
            ttl_seconds,
            store_tx,
        };

        storage.start_batch_processor(store_rx);
        storage
    }

    /// Start background task that batches and executes store requests
    fn start_batch_processor(&self, mut rx: mpsc::Receiver<StoreRequest>) {
        let redis = self.redis.clone();
        let max_per_channel = self.max_per_channel;
        let ttl_seconds = self.ttl_seconds;

        tokio::spawn(async move {
            let mut batch: Vec<StoreRequest> = Vec::with_capacity(BATCH_SIZE);
            let mut interval = tokio::time::interval(
                std::time::Duration::from_millis(BATCH_FLUSH_INTERVAL_MS)
            );

            loop {
                tokio::select! {
                    // Receive store requests
                    msg = rx.recv() => {
                        match msg {
                            Some(req) => {
                                batch.push(req);
                                // Flush if batch is full
                                if batch.len() >= BATCH_SIZE {
                                    Self::flush_batch(&redis, &mut batch, max_per_channel, ttl_seconds).await;
                                }
                            }
                            None => break, // Channel closed
                        }
                    }
                    // Periodic flush
                    _ = interval.tick() => {
                        if !batch.is_empty() {
                            Self::flush_batch(&redis, &mut batch, max_per_channel, ttl_seconds).await;
                        }
                    }
                }
            }
        });
    }

    /// Flush batch using Redis pipeline
    async fn flush_batch(
        redis: &Arc<RwLock<Option<ConnectionManager>>>,
        batch: &mut Vec<StoreRequest>,
        max_per_channel: usize,
        ttl_seconds: u64,
    ) {
        if batch.is_empty() {
            return;
        }

        let manager = {
            let conn = redis.read().await;
            match &*conn {
                Some(m) => m.clone(),
                None => {
                    batch.clear();
                    return;
                }
            }
        };

        let mut conn = manager;
        let mut pipe = redis::pipe();

        // Collect unique channel keys to set TTL
        let mut keys_to_expire: std::collections::HashSet<String> = std::collections::HashSet::new();

        for req in batch.iter() {
            let key = Self::stream_key(&req.channel_id);
            keys_to_expire.insert(key.clone());

            // XADD command
            pipe.cmd("XADD")
                .arg(&key)
                .arg("MAXLEN")
                .arg("~")
                .arg(max_per_channel)
                .arg(&req.stream_id)
                .arg("event_type")
                .arg(&req.event_type)
                .arg("data")
                .arg(&req.data);

            if let Some(ref id) = req.id {
                pipe.arg("id").arg(id);
            }

            pipe.ignore();
        }

        // Set TTL for all affected keys (refresh on each write)
        for key in keys_to_expire {
            pipe.cmd("EXPIRE")
                .arg(&key)
                .arg(ttl_seconds)
                .ignore();
        }

        let batch_size = batch.len();
        batch.clear();

        // Execute pipeline with timeout
        let fut = pipe.query_async::<()>(&mut conn);
        match tokio::time::timeout(std::time::Duration::from_millis(200), fut).await {
            Ok(Ok(_)) => {
                tracing::debug!(count = batch_size, "Batch stored to Redis");
            }
            Ok(Err(e)) => {
                warn!(error = %e, count = batch_size, "Failed to store batch");
            }
            Err(_) => {
                tracing::debug!(count = batch_size, "Batch store timeout");
            }
        }
    }

    /// Connect to Redis
    pub async fn connect(&self, redis_url: &str) -> anyhow::Result<()> {
        let client = redis::Client::open(redis_url)?;
        let manager = ConnectionManager::new(client).await?;
        *self.redis.write().await = Some(manager);
        info!("Redis storage connected (batching enabled)");
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
    fn generate_id(&self) -> String {
        let ts = chrono::Utc::now().timestamp_millis();
        let seq = self.counter.fetch_add(1, Ordering::SeqCst);
        format!("{}-{}", ts, seq)
    }

    async fn store(&self, channel_id: &str, stream_id: &str, event: &SseEvent) {
        // Send to batch processor (non-blocking)
        let req = StoreRequest {
            channel_id: channel_id.to_string(),
            stream_id: stream_id.to_string(),
            event_type: event.event_type.clone(),
            data: event.data.to_string(),
            id: event.id.clone(),
        };

        // try_send to avoid blocking, drop if channel is full
        if let Err(e) = self.store_tx.try_send(req) {
            tracing::debug!(error = %e, "Store channel full, dropping message");
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
