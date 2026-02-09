use sse_gateway::Gateway;
use sse_gateway_redis::{RedisPubSubSource, RedisStorage};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install rustls crypto provider (required for TLS)
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

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

    run_with_redis(port).await
}


async fn run_with_redis(port: u16) -> anyhow::Result<()> {
    let redis_url = std::env::var("REDIS_URL")
        .unwrap_or_else(|_| "redis://localhost:6379".to_string());

    let storage = RedisStorage::new();
    storage.connect(&redis_url).await?;

    tracing::info!(
        port,
        redis_url = %redis_url,
        "Starting SSE Gateway (Redis)"
    );

    Gateway::builder()
        .port(port)
        .dashboard(true)
        .source(RedisPubSubSource::with_defaults(&redis_url))
        .storage(storage)
        .build()?
        .run()
        .await
}
