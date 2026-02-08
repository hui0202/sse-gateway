//! Message Storage trait and implementations
//!
//! Implement `MessageStorage` for message replay on reconnection.

use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::event::SseEvent;

/// Trait for message storage
///
/// Implement this trait to support message replay on client reconnection.
///
/// # Example
///
/// ```rust,ignore
/// use sse_gateway::{MessageStorage, SseEvent};
/// use async_trait::async_trait;
///
/// struct MyStorage {
///     db: Database,
/// }
///
/// #[async_trait]
/// impl MessageStorage for MyStorage {
///     async fn store(&self, channel_id: &str, event: &SseEvent) -> Option<String> {
///         let id = self.db.insert(channel_id, event).await?;
///         Some(id)
///     }
///
///     async fn get_messages_after(&self, channel_id: &str, after_id: Option<&str>) -> Vec<SseEvent> {
///         self.db.query_after(channel_id, after_id).await
///     }
///
///     async fn is_available(&self) -> bool { true }
///     fn name(&self) -> &'static str { "MyStorage" }
/// }
/// ```
#[async_trait]
pub trait MessageStorage: Send + Sync + Clone + 'static {
    /// Store a message and return the stream ID
    ///
    /// Return `None` if storage is disabled or fails.
    async fn store(&self, channel_id: &str, event: &SseEvent) -> Option<String>;

    /// Get messages after a specific ID (for replay)
    ///
    /// Used when a client reconnects with a `last-event-id` header.
    async fn get_messages_after(&self, channel_id: &str, after_id: Option<&str>) -> Vec<SseEvent>;

    /// Check if storage is available
    async fn is_available(&self) -> bool;

    /// Return the storage name (for logging)
    fn name(&self) -> &'static str;
}

/// In-memory message storage
///
/// Suitable for development and testing. Not suitable for multi-instance deployments.
#[derive(Clone)]
pub struct MemoryStorage {
    streams: Arc<DashMap<String, Vec<(String, SseEvent)>>>,
    counter: Arc<AtomicU64>,
    max_per_channel: usize,
}

impl MemoryStorage {
    /// Create a new memory storage with specified capacity per channel
    pub fn new(max_per_channel: usize) -> Self {
        Self {
            streams: Arc::new(DashMap::new()),
            counter: Arc::new(AtomicU64::new(0)),
            max_per_channel,
        }
    }

    fn generate_stream_id(&self) -> String {
        let ts = chrono::Utc::now().timestamp_millis();
        let seq = self.counter.fetch_add(1, Ordering::SeqCst);
        format!("{}-{}", ts, seq)
    }
}

impl Default for MemoryStorage {
    fn default() -> Self {
        Self::new(100)
    }
}

#[async_trait]
impl MessageStorage for MemoryStorage {
    async fn store(&self, channel_id: &str, event: &SseEvent) -> Option<String> {
        let stream_id = self.generate_stream_id();
        let max = self.max_per_channel;

        let mut stored_event = event.clone();
        stored_event.stream_id = Some(stream_id.clone());

        self.streams
            .entry(channel_id.to_string())
            .or_default()
            .push((stream_id.clone(), stored_event));

        // Trim old messages
        self.streams.alter(channel_id, |_, mut v| {
            if v.len() > max {
                v.drain(0..v.len() - max);
            }
            v
        });

        Some(stream_id)
    }

    async fn get_messages_after(&self, channel_id: &str, after_id: Option<&str>) -> Vec<SseEvent> {
        let after_id = match after_id {
            Some(id) => id,
            None => return vec![],
        };

        let Some(entries) = self.streams.get(channel_id) else {
            return vec![];
        };

        let mut found = false;
        entries
            .iter()
            .filter_map(|(id, event)| {
                if found {
                    return Some(event.clone());
                }
                if id == after_id {
                    found = true;
                }
                None
            })
            .collect()
    }

    async fn is_available(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "Memory"
    }
}

/// No-op storage (disabled)
#[derive(Clone, Default)]
pub struct NoopStorage;

#[async_trait]
impl MessageStorage for NoopStorage {
    async fn store(&self, _channel_id: &str, _event: &SseEvent) -> Option<String> {
        None
    }

    async fn get_messages_after(&self, _channel_id: &str, _after_id: Option<&str>) -> Vec<SseEvent> {
        vec![]
    }

    async fn is_available(&self) -> bool {
        false
    }

    fn name(&self) -> &'static str {
        "Noop (disabled)"
    }
}
