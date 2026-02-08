use sse_gateway::{Gateway, MemoryStorage};
use sse_gateway_gcp::GcpPubSubSource;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sse_gateway=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    let project = std::env::var("GCP_PROJECT")
        .map_err(|_| anyhow::anyhow!("GCP_PROJECT environment variable required"))?;

    let subscription = std::env::var("PUBSUB_SUBSCRIPTION")
        .map_err(|_| anyhow::anyhow!("PUBSUB_SUBSCRIPTION environment variable required"))?;

    tracing::info!(
        port,
        project = %project,
        subscription = %subscription,
        "Starting SSE Gateway (GCP Pub/Sub)"
    );

    Gateway::builder()
        .port(port)
        .source(GcpPubSubSource::new(&project, &subscription))
        .storage(MemoryStorage::default())
        .build()?
        .run()
        .await
}
