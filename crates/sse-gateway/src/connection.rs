//! SSE Connection types

use tokio::sync::mpsc;
use crate::event::SseEvent;

/// Metadata about a connection
#[derive(Debug, Clone)]
pub struct ConnectionMetadata {
    /// When the connection was established
    pub connected_at: chrono::DateTime<chrono::Utc>,
    /// Gateway instance ID
    pub instance_id: String,
    /// Client IP address (if available)
    pub client_ip: Option<String>,
    /// User agent (if available)
    pub user_agent: Option<String>,
}

/// Represents an SSE connection
#[derive(Debug)]
pub struct SseConnection {
    /// Unique connection ID
    pub id: String,
    /// Channel ID this connection is subscribed to
    pub channel_id: String,
    /// Sender for pushing events to this connection
    pub sender: mpsc::Sender<SseEvent>,
    /// Connection metadata
    pub metadata: ConnectionMetadata,
}

impl SseConnection {
    /// Create a new SSE connection
    pub fn new(
        channel_id: String,
        instance_id: String,
        client_ip: Option<String>,
        user_agent: Option<String>,
    ) -> (Self, mpsc::Receiver<SseEvent>) {
        let (sender, receiver) = mpsc::channel(100);
        let connection = Self {
            id: uuid::Uuid::new_v4().to_string(),
            channel_id,
            sender,
            metadata: ConnectionMetadata {
                connected_at: chrono::Utc::now(),
                instance_id,
                client_ip,
                user_agent,
            },
        };
        (connection, receiver)
    }

    /// Check if the connection is still active
    pub fn is_active(&self) -> bool {
        !self.sender.is_closed()
    }

    /// Send an event to this connection
    pub async fn send(&self, event: SseEvent) -> bool {
        self.sender.send(event).await.is_ok()
    }
}

impl Clone for SseConnection {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            channel_id: self.channel_id.clone(),
            sender: self.sender.clone(),
            metadata: self.metadata.clone(),
        }
    }
}
