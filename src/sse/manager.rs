use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::{info, warn};

use super::{ConnectionId, SseConnection, SseEvent};

const HEARTBEAT_CHANNEL_CAPACITY: usize = 16;

#[derive(Clone)]
pub struct ConnectionManager {
    connections: Arc<DashMap<ConnectionId, Arc<SseConnection>>>,
    channel_connections: Arc<DashMap<String, Vec<ConnectionId>>>,
    instance_id: String,
    heartbeat_sender: broadcast::Sender<i64>,
}

impl ConnectionManager {
    pub fn new(instance_id: String) -> Self {
        let (heartbeat_sender, _) = broadcast::channel(HEARTBEAT_CHANNEL_CAPACITY);
        
        Self {
            connections: Arc::new(DashMap::new()),
            channel_connections: Arc::new(DashMap::new()),
            instance_id,
            heartbeat_sender,
        }
    }
    
    pub fn subscribe_heartbeat(&self) -> broadcast::Receiver<i64> {
        self.heartbeat_sender.subscribe()
    }
    
    pub fn send_heartbeat(&self) {
        let ts = chrono::Utc::now().timestamp();
        let _ = self.heartbeat_sender.send(ts);
    }

    pub fn register(
        &self,
        channel_id: String,
        client_ip: Option<String>,
        user_agent: Option<String>,
    ) -> (Arc<SseConnection>, mpsc::Receiver<SseEvent>) {
        let (connection, receiver) = SseConnection::new(
            channel_id.clone(),
            self.instance_id.clone(),
            client_ip,
            user_agent,
        );
        
        let connection_id = connection.id.clone();
        let connection = Arc::new(connection);
        
        self.connections.insert(connection_id.clone(), connection.clone());
        
        self.channel_connections
            .entry(channel_id.clone())
            .or_default()
            .push(connection_id.clone());
        
        info!(
            connection_id = %connection_id,
            channel_id = %channel_id,
            total_connections = self.connections.len(),
            "SSE connection registered"
        );
        
        (connection, receiver)
    }

    pub fn unregister(&self, connection_id: &str) {
        if let Some((_, connection)) = self.connections.remove(connection_id) {
            let channel_id = connection.channel_id.clone();
            
            // Remove from channel index and clean up empty entries
            let mut should_remove = false;
            if let Some(mut channel_conns) = self.channel_connections.get_mut(&channel_id) {
                channel_conns.retain(|id| id != connection_id);
                should_remove = channel_conns.is_empty();
            }
            if should_remove {
                self.channel_connections.remove(&channel_id);
            }
            
            info!(
                connection_id = %connection_id,
                channel_id = %channel_id,
                remaining_connections = self.connections.len(),
                "SSE connection unregistered and cleaned up"
            );
        } else {
            warn!(connection_id = %connection_id, "Attempted to unregister unknown connection");
        }
    }

    pub async fn send_to_connection(&self, connection_id: &str, event: SseEvent) -> bool {
        if let Some(connection) = self.connections.get(connection_id) {
            match connection.send(event).await {
                Ok(_) => true,
                Err(_) => {
                    warn!(connection_id = %connection_id, "Failed to send event, connection may be closed");
                    self.unregister(connection_id);
                    false
                }
            }
        } else {
            false
        }
    }

    pub async fn send_to_channel(&self, channel_id: &str, event: SseEvent) -> usize {
        let connection_ids: Vec<ConnectionId> = self
            .channel_connections
            .get(channel_id)
            .map(|ids| ids.clone())
            .unwrap_or_default();
        
        if connection_ids.is_empty() {
            return 0;
        }

        let futures: Vec<_> = connection_ids
            .into_iter()
            .map(|conn_id| {
                let event = event.clone();
                let this = self.clone();
                async move { this.send_to_connection(&conn_id, event).await }
            })
            .collect();

        let results = futures::future::join_all(futures).await;
        let sent_count = results.into_iter().filter(|&sent| sent).count();
        
        info!(channel_id = %channel_id, sent_count = sent_count, "Sent event to channel");
        sent_count
    }

    pub async fn broadcast(&self, event: SseEvent) -> usize {
        let connection_ids: Vec<ConnectionId> = self
            .connections
            .iter()
            .map(|entry| entry.key().clone())
            .collect();
        
        if connection_ids.is_empty() {
            return 0;
        }

        let futures: Vec<_> = connection_ids
            .into_iter()
            .map(|conn_id| {
                let event = event.clone();
                let this = self.clone();
                async move { this.send_to_connection(&conn_id, event).await }
            })
            .collect();

        let results = futures::future::join_all(futures).await;
        let sent_count = results.into_iter().filter(|&sent| sent).count();
        
        info!(sent_count = sent_count, "Broadcast event to all connections");
        sent_count
    }

    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }


    pub fn cleanup_dead_connections(&self) {
        let dead_connections: Vec<ConnectionId> = self
            .connections
            .iter()
            .filter(|entry| !entry.value().is_active())
            .map(|entry| entry.key().clone())
            .collect();
        
        for conn_id in dead_connections {
            self.unregister(&conn_id);
        }
    }

    pub fn list_connections(&self) -> Vec<Arc<SseConnection>> {
        self.connections
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }
}
