//! Google Cloud Pub/Sub adapter for SSE Gateway
//!
//! # Example
//!
//! ```rust,ignore
//! use sse_gateway::Gateway;
//! use sse_gateway_gcp::GcpPubSubSource;
//!
//! Gateway::builder()
//!     .source(GcpPubSubSource::new("my-project", "my-subscription"))
//!     .storage(sse_gateway::MemoryStorage::default())
//!     .build()?
//!     .run()
//!     .await
//! ```

use async_trait::async_trait;
use google_cloud_pubsub::client::{Client, ClientConfig};
use sse_gateway::{ConnectionManager, IncomingMessage, MessageHandler, MessageSource};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

/// Google Cloud Pub/Sub message source
///
/// # Message Attributes
///
/// The source reads the following attributes from Pub/Sub messages:
/// - `channel_id`: Target channel (optional, omit for broadcast)
/// - `event_type`: Event type (defaults to "message")
/// - `id`: Business message ID (optional)
pub struct GcpPubSubSource {
    project_id: String,
    subscription_id: String,
}

impl GcpPubSubSource {
    /// Create a new GCP Pub/Sub source
    pub fn new(project_id: impl Into<String>, subscription_id: impl Into<String>) -> Self {
        Self {
            project_id: project_id.into(),
            subscription_id: subscription_id.into(),
        }
    }
}

#[async_trait]
impl MessageSource for GcpPubSubSource {
    async fn start(
        &self,
        handler: MessageHandler,
        _connection_manager: ConnectionManager,
        cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        info!(
            project = %self.project_id,
            subscription = %self.subscription_id,
            "Starting GCP Pub/Sub"
        );

        let config = ClientConfig::default().with_auth().await?;
        let client = Client::new(config).await?;
        let subscription = client.subscription(&self.subscription_id);

        info!("Connected to GCP Pub/Sub");

        subscription
            .receive(
                move |message, _cancel| {
                    let handler = handler.clone();
                    async move {
                        let msg = &message.message;

                        let channel_id = msg.attributes.get("channel_id").map(|s| s.to_string());
                        let event_type = msg
                            .attributes
                            .get("event_type")
                            .map(|s| s.as_str())
                            .unwrap_or("message")
                            .to_string();
                        let id = msg.attributes.get("id").map(|s| s.to_string());
                        let data = String::from_utf8_lossy(&msg.data).to_string();

                        handler(IncomingMessage {
                            channel_id,
                            event_type,
                            data,
                            id,
                        });

                        if let Err(e) = message.ack().await {
                            error!(error = %e, "Failed to ack message");
                        }
                    }
                },
                cancel,
                None,
            )
            .await?;

        info!("GCP Pub/Sub stopped");
        Ok(())
    }

    fn name(&self) -> &'static str {
        "GCP Pub/Sub"
    }
}
