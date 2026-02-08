mod config;
mod gateway;
mod message_store;
mod pubsub;
mod sse;
mod sse_handler;
mod test_handler;

pub use message_store::MessageStore;

use axum::{routing::{get, post}, Router};
use std::net::SocketAddr;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::AppConfig;
use crate::gateway::GatewayState;
use crate::pubsub::PubSubSubscriber;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let test_mode = std::env::var("TEST_MODE").map(|v| v == "1" || v == "true").unwrap_or(false);
    
    let config = if test_mode {
        tracing::info!("🧪 Running in TEST MODE - Pub/Sub disabled");
        AppConfig::test_config()
    } else {
        AppConfig::load()?
    };
    
    tracing::info!(
        instance_id = %config.server.instance_id,
        project = %config.pubsub.project_id,
        subscription = %config.pubsub.subscription_id,
        redis = ?config.redis.url,
        test_mode = test_mode,
        "Gateway starting"
    );

    let state = GatewayState::new(&config);
    let cancel = CancellationToken::new();

    if let Some(ref redis_url) = config.redis.url {
        match state.message_store.connect(redis_url).await {
            Ok(_) => tracing::info!("Redis connected - message replay enabled"),
            Err(e) => tracing::warn!(error = %e, "Failed to connect Redis - message replay disabled"),
        }
    } else {
        tracing::info!("Redis not configured - message replay disabled");
    }

    if !test_mode {
        let subscriber = PubSubSubscriber::new(
            state.connection_manager.clone(),
            state.message_store.clone(),
            &config.pubsub.project_id,
            &config.pubsub.subscription_id,
        );
        let cancel_clone = cancel.clone();
        
        tokio::spawn(async move {
            if let Err(e) = subscriber.start(cancel_clone).await {
                tracing::error!(error = %e, "Pub/Sub subscriber fatal error");
            }
        });
    }

    let cleanup_manager = state.connection_manager.clone();
    let cancel_cleanup = cancel.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                _ = cancel_cleanup.cancelled() => {
                    tracing::info!("Cleanup task cancelled");
                    break;
                }
                _ = interval.tick() => {
                    let before = cleanup_manager.connection_count();
                    cleanup_manager.cleanup_dead_connections();
                    let after = cleanup_manager.connection_count();
                    
                    tracing::info!(
                        active_connections = after,
                        cleaned = before.saturating_sub(after),
                        "Connection status"
                    );
                }
            }
        }
    });

    let heartbeat_manager = state.connection_manager.clone();
    let cancel_heartbeat = cancel.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                _ = cancel_heartbeat.cancelled() => {
                    tracing::info!("Heartbeat task cancelled");
                    break;
                }
                _ = interval.tick() => {
                    heartbeat_manager.send_heartbeat();
                }
            }
        }
    });

    let enable_dashboard = std::env::var("ENABLE_DASHBOARD")
        .map(|v| v != "0" && v != "false")
        .unwrap_or(true);

    let mut app = Router::new()
        .route("/health", get(|| async { "OK" }))
        .route("/ready", get(|| async { "READY" }))
        .route("/sse/connect", get(sse_handler::sse_connect));
    
    if enable_dashboard {
        tracing::info!("Dashboard enabled at /dashboard and /test");
        app = app
            .route("/dashboard", get(test_handler::dashboard_page))
            .route("/api/stats", get(test_handler::get_stats))
            .route("/api/send", post(test_handler::send_test_message))
            .route("/test", get(test_handler::dashboard_page))
            .route("/test/send", post(test_handler::send_test_message))
            .route("/test/stats", get(test_handler::get_stats));
    }
    
    let app = app
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let port = config.server.port;
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("Listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    
    let cancel_for_shutdown = cancel.clone();
    let shutdown_signal = async move {
        let ctrl_c = async {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to install Ctrl+C handler");
        };
        
        #[cfg(unix)]
        let terminate = async {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Failed to install SIGTERM handler")
                .recv()
                .await;
        };
        
        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();
        
        tokio::select! {
            _ = ctrl_c => tracing::info!("Received Ctrl+C, initiating graceful shutdown"),
            _ = terminate => tracing::info!("Received SIGTERM, initiating graceful shutdown"),
        }
        
        cancel_for_shutdown.cancel();
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal)
        .await?;

    tracing::info!("Server stopped, waiting for background tasks...");
    
    tokio::time::sleep(Duration::from_secs(2)).await;
    
    tracing::info!("Gateway shutdown complete");
    Ok(())
}

fn init_tracing() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gateway=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer().json())
        .init();
}
