use crate::config::AppConfig;
use crate::message_store::MessageStore;
use crate::sse::ConnectionManager;

#[derive(Clone)]
pub struct GatewayState {
    pub connection_manager: ConnectionManager,
    pub message_store: MessageStore,
}

impl GatewayState {
    pub fn new(config: &AppConfig) -> Self {
        let connection_manager = ConnectionManager::new(config.server.instance_id.clone());
        let message_store = MessageStore::new();
        Self { connection_manager, message_store }
    }
}
