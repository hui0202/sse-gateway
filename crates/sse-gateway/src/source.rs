//! Message Source trait and implementations
//!
//! Implement `MessageSource` to receive messages from any backend.

use async_trait::async_trait;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Incoming message from a source
#[derive(Debug, Clone)]
pub struct IncomingMessage {
    /// Target channel ID. None means broadcast to all.
    pub channel_id: Option<String>,
    /// Event type (e.g., "message", "notification")
    pub event_type: String,
    /// Message data (usually JSON string)
    pub data: String,
    /// Optional business ID
    pub id: Option<String>,
}

impl IncomingMessage {
    /// Create a new incoming message
    pub fn new(event_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            channel_id: None,
            event_type: event_type.into(),
            data: data.into(),
            id: None,
        }
    }

    /// Set the target channel
    pub fn with_channel(mut self, channel_id: impl Into<String>) -> Self {
        self.channel_id = Some(channel_id.into());
        self
    }

    /// Set the message ID
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Create a broadcast message
    pub fn broadcast(event_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self::new(event_type, data)
    }
}

/// Message handler callback type
pub type MessageHandler = Arc<dyn Fn(IncomingMessage) + Send + Sync>;

/// Trait for message sources
///
/// Implement this trait to receive messages from any backend system.
///
/// # Example
///
/// ```rust,ignore
/// use sse_gateway::{MessageSource, MessageHandler, IncomingMessage};
/// use async_trait::async_trait;
/// use tokio_util::sync::CancellationToken;
///
/// struct MySource {
///     url: String,
/// }
///
/// #[async_trait]
/// impl MessageSource for MySource {
///     async fn start(&self, handler: MessageHandler, cancel: CancellationToken) -> anyhow::Result<()> {
///         loop {
///             tokio::select! {
///                 _ = cancel.cancelled() => break,
///                 msg = receive_message(&self.url) => {
///                     handler(IncomingMessage::new("message", msg));
///                 }
///             }
///         }
///         Ok(())
///     }
///
///     fn name(&self) -> &'static str { "MySource" }
/// }
/// ```
#[async_trait]
pub trait MessageSource: Send + Sync + 'static {
    /// Start receiving messages
    ///
    /// This method should run until the cancellation token is triggered.
    /// Call the handler for each received message.
    async fn start(&self, handler: MessageHandler, cancel: CancellationToken) -> anyhow::Result<()>;

    /// Return the source name (for logging)
    fn name(&self) -> &'static str;
}

/// A no-op source that does nothing (for testing)
pub struct NoopSource;

#[async_trait]
impl MessageSource for NoopSource {
    async fn start(&self, _handler: MessageHandler, cancel: CancellationToken) -> anyhow::Result<()> {
        tracing::info!("NoopSource started (no external messages will be received)");
        cancel.cancelled().await;
        Ok(())
    }

    fn name(&self) -> &'static str {
        "Noop"
    }
}

/// A channel-based source for programmatic message sending
///
/// Useful for testing or when you want to send messages from your own code.
pub struct ChannelSource {
    receiver: tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<IncomingMessage>>>,
}

impl ChannelSource {
    /// Create a new channel source
    pub fn new() -> (Self, tokio::sync::mpsc::Sender<IncomingMessage>) {
        let (tx, rx) = tokio::sync::mpsc::channel(1000);
        (
            Self {
                receiver: tokio::sync::Mutex::new(Some(rx)),
            },
            tx,
        )
    }
}

#[async_trait]
impl MessageSource for ChannelSource {
    async fn start(&self, handler: MessageHandler, cancel: CancellationToken) -> anyhow::Result<()> {
        let mut receiver = self.receiver.lock().await.take()
            .ok_or_else(|| anyhow::anyhow!("ChannelSource can only be started once"))?;

        tracing::info!("ChannelSource started");

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

        tracing::info!("ChannelSource stopped");
        Ok(())
    }

    fn name(&self) -> &'static str {
        "Channel"
    }
}
