use google_cloud_pubsub::client::{Client, ClientConfig};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::message_store::MessageStore;
use crate::sse::{ConnectionManager, SseEvent};

pub struct PubSubSubscriber {
    connection_manager: ConnectionManager,
    message_store: MessageStore,
    subscription_id: String,
}

impl PubSubSubscriber {
    pub fn new(
        connection_manager: ConnectionManager,
        message_store: MessageStore,
        _project_id: &str,
        subscription_id: &str,
    ) -> Self {
        Self {
            connection_manager,
            message_store,
            subscription_id: subscription_id.to_string(),
        }
    }

    pub async fn start(self, cancel: CancellationToken) -> anyhow::Result<()> {
        info!(subscription = %self.subscription_id, "Starting Pub/Sub subscriber");

        let config = ClientConfig::default().with_auth().await?;
        let client = Client::new(config).await?;

        let subscription = client.subscription(&self.subscription_id);

        info!("Pub/Sub connected, listening...");

        let connection_manager = self.connection_manager.clone();
        let message_store = self.message_store.clone();
        subscription
            .receive(
                move |message, _cancel| {
                    let cm = connection_manager.clone();
                    let ms = message_store.clone();
                    async move {
                        let msg = &message.message;
                        let channel_id = msg.attributes.get("channel_id").map(|s| s.as_str());
                        let event_type = msg.attributes.get("event_type").map(|s| s.as_str()).unwrap_or("message");

                        let data_str = String::from_utf8_lossy(&msg.data).to_string();
                        let mut event = SseEvent::raw(event_type, data_str);

                        let sent = match channel_id {
                            Some(id) => {
                                if let Some(stream_id) = ms.store(id, &event).await {
                                    event.stream_id = Some(stream_id);
                                }
                                cm.send_to_channel(id, event).await
                            }
                            None => cm.broadcast(event).await,
                        };

                        info!(channel_id = ?channel_id, event_type, sent, "Forwarded message");

                        if let Err(e) = message.ack().await {
                            error!(error = %e, "Failed to ack message");
                        }
                    }
                },
                cancel,
                None,
            )
            .await?;

        info!("Subscriber stopped");
        Ok(())
    }
}
