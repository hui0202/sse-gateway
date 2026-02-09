use sse_gateway::Gateway;
use sse_gateway_gcp::GcpPubSubSource;
use sse_gateway_redis::RedisStorage;
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

    let redis_url = std::env::var("REDIS_URL")
        .unwrap_or_else(|_| "redis://localhost:6379".to_string());

    // Initialize Redis storage
    let storage = RedisStorage::new();
    storage.connect(&redis_url).await?;

    tracing::info!(
        port,
        project = %project,
        subscription = %subscription,
        redis_url = %redis_url,
        "Starting SSE Gateway (GCP Pub/Sub + Redis)"
    );

    Gateway::builder()
        .port(port)
        .source(GcpPubSubSource::new(&project, &subscription))
        .storage(storage)
        .build()?
        .run()
        .await
}
