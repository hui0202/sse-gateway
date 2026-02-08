use tokio::sync::mpsc;
use uuid::Uuid;

use super::SseEvent;

pub type ConnectionId = String;

#[derive(Debug)]
pub struct SseConnection {
    pub id: ConnectionId,
    pub channel_id: String,
    pub sender: mpsc::Sender<SseEvent>,
    pub metadata: ConnectionMetadata,
}

#[derive(Debug, Clone)]
pub struct ConnectionMetadata {
    pub connected_at: chrono::DateTime<chrono::Utc>,
    pub instance_id: String,
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
}

impl SseConnection {
    pub fn new(
        channel_id: String,
        instance_id: String,
        client_ip: Option<String>,
        user_agent: Option<String>,
    ) -> (Self, mpsc::Receiver<SseEvent>) {
        let (sender, receiver) = mpsc::channel(100);
        
        let connection = Self {
            id: Uuid::new_v4().to_string(),
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

    pub async fn send(&self, event: SseEvent) -> Result<(), mpsc::error::SendError<SseEvent>> {
        self.sender.send(event).await
    }

    pub fn is_active(&self) -> bool {
        !self.sender.is_closed()
    }
}
