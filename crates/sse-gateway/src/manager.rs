//! Connection Manager for handling SSE connections

use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::info;

use crate::connection::SseConnection;
use crate::event::SseEvent;

/// Manages all SSE connections
#[derive(Clone)]
pub struct ConnectionManager {
    /// All active connections: connection_id -> connection
    connections: Arc<DashMap<String, SseConnection>>,
    /// Index: channel_id -> [connection_ids]
    channel_index: Arc<DashMap<String, Vec<String>>>,
    /// Heartbeat broadcaster
    heartbeat_tx: broadcast::Sender<i64>,
    /// Gateway instance ID
    instance_id: String,
}

impl ConnectionManager {
    /// Create a new connection manager
    pub fn new(instance_id: impl Into<String>) -> Self {
        let (heartbeat_tx, _) = broadcast::channel(16);
        Self {
            connections: Arc::new(DashMap::new()),
            channel_index: Arc::new(DashMap::new()),
            heartbeat_tx,
            instance_id: instance_id.into(),
        }
    }

    /// Register a new connection
    pub fn register(
        &self,
        channel_id: String,
        client_ip: Option<String>,
        user_agent: Option<String>,
    ) -> (SseConnection, mpsc::Receiver<SseEvent>) {
        let (connection, receiver) =
            SseConnection::new(channel_id.clone(), self.instance_id.clone(), client_ip, user_agent);

        let connection_id = connection.id.clone();

        // Store connection
        self.connections.insert(connection_id.clone(), connection.clone());

        // Update channel index
        self.channel_index
            .entry(channel_id)
            .or_default()
            .push(connection_id);

        (connection, receiver)
    }

    /// Unregister a connection
    pub fn unregister(&self, connection_id: &str) {
        if let Some((_, connection)) = self.connections.remove(connection_id) {
            // Remove from channel index
            if let Some(mut ids) = self.channel_index.get_mut(&connection.channel_id) {
                ids.retain(|id| id != connection_id);
            }
            info!(connection_id, channel_id = %connection.channel_id, "Connection unregistered");
        }
    }

    /// Send event to a specific channel
    pub async fn send_to_channel(&self, channel_id: &str, event: SseEvent) -> usize {
        let connection_ids = self
            .channel_index
            .get(channel_id)
            .map(|ids| ids.clone())
            .unwrap_or_default();

        let mut sent = 0;
        for conn_id in connection_ids {
            if let Some(conn) = self.connections.get(&conn_id) {
                if conn.send(event.clone()).await {
                    sent += 1;
                }
            }
        }
        sent
    }

    /// Send event to a specific connection
    pub async fn send_to_connection(&self, connection_id: &str, event: SseEvent) -> bool {
        if let Some(conn) = self.connections.get(connection_id) {
            conn.send(event).await
        } else {
            false
        }
    }

    /// Broadcast event to all connections
    pub async fn broadcast(&self, event: SseEvent) -> usize {
        let mut sent = 0;
        for entry in self.connections.iter() {
            if entry.send(event.clone()).await {
                sent += 1;
            }
        }
        sent
    }

    /// Send heartbeat to all connections
    pub fn send_heartbeat(&self) {
        let ts = chrono::Utc::now().timestamp_millis();
        let _ = self.heartbeat_tx.send(ts);
    }

    /// Subscribe to heartbeat events
    pub fn subscribe_heartbeat(&self) -> broadcast::Receiver<i64> {
        self.heartbeat_tx.subscribe()
    }

    /// Get total connection count
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    /// Get connections for a specific channel
    pub fn channel_connection_count(&self, channel_id: &str) -> usize {
        self.channel_index
            .get(channel_id)
            .map(|ids| ids.len())
            .unwrap_or(0)
    }

    /// List all connections
    pub fn list_connections(&self) -> Vec<SseConnection> {
        self.connections.iter().map(|e| e.value().clone()).collect()
    }

    /// Clean up dead connections
    pub fn cleanup_dead_connections(&self) {
        let dead_ids: Vec<String> = self
            .connections
            .iter()
            .filter(|e| !e.value().is_active())
            .map(|e| e.key().clone())
            .collect();

        for id in dead_ids {
            self.unregister(&id);
        }
    }

    /// Get the instance ID
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }
}
