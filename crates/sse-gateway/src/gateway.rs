//! Gateway builder and runner

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::{routing::get, Router};
use tokio_util::sync::CancellationToken;
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};

// Error types now use anyhow for better ergonomics
use crate::{auth::AuthFn, handler};
use crate::manager::ConnectionManager;
use crate::source::{ConnectionInfo, IncomingMessage, MessageSource, NoopSource};
use crate::storage::{MemoryStorage, MessageStorage, NoopStorage};
use crate::event::SseEvent;

/// Connection lifecycle callback type
pub type LifecycleCallback = Arc<dyn Fn(&ConnectionInfo) + Send + Sync>;

/// Gateway configuration and runner
pub struct Gateway<Source: MessageSource, Storage: MessageStorage> {
    port: u16,
    source: Source,
    storage: Storage,
    connection_manager: ConnectionManager,
    enable_dashboard: bool,
    heartbeat_interval: Duration,
    cleanup_interval: Duration,
    auth: Option<AuthFn>,
}

impl<Source: MessageSource, Storage: MessageStorage> Gateway<Source, Storage> {
    /// Run the gateway server
    pub async fn run(self) -> anyhow::Result<()> {
        let cancel = CancellationToken::new();

        tracing::info!(
            port = self.port,
            source = self.source.name(),
            storage = self.storage.name(),
            "Starting SSE Gateway"
        );

        // Wrap source in Arc for sharing
        let source = Arc::new(self.source);

        // Create lifecycle callbacks that delegate to source
        let source_for_connect = source.clone();
        let on_connect: LifecycleCallback = Arc::new(move |info| {
            source_for_connect.on_connect(info);
        });

        let source_for_disconnect = source.clone();
        let on_disconnect: LifecycleCallback = Arc::new(move |info| {
            source_for_disconnect.on_disconnect(info);
        });

        // Create shared state
        let state = handler::GatewayState {
            connection_manager: self.connection_manager.clone(),
            storage: self.storage.clone(),
            auth: self.auth.clone(),
            on_connect: Some(on_connect),
            on_disconnect: Some(on_disconnect),
        };

        // Start message source
        let dispatcher = Dispatcher::new(self.connection_manager.clone(), self.storage.clone());
        let handler = dispatcher.to_handler();
        let source_cancel = cancel.clone();
        let source_name = source.name();
        let source_connection_manager = self.connection_manager.clone();

        tokio::spawn(async move {
            if let Err(e) = source.start(handler, source_connection_manager, source_cancel).await {
                tracing::error!(error = %e, source = source_name, "Message source error");
            }
        });

        // Start cleanup task
        let cleanup_manager = self.connection_manager.clone();
        let cleanup_cancel = cancel.clone();
        let cleanup_interval = self.cleanup_interval;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(cleanup_interval);
            loop {
                tokio::select! {
                    _ = cleanup_cancel.cancelled() => break,
                    _ = interval.tick() => {
                        let before = cleanup_manager.connection_count();
                        cleanup_manager.cleanup_dead_connections();
                        let after = cleanup_manager.connection_count();
                        tracing::debug!(
                            connections = after,
                            cleaned = before.saturating_sub(after),
                            "Connection cleanup"
                        );
                    }
                }
            }
        });

        // Start heartbeat task
        let heartbeat_manager = self.connection_manager.clone();
        let heartbeat_cancel = cancel.clone();
        let heartbeat_interval = self.heartbeat_interval;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(heartbeat_interval);
            loop {
                tokio::select! {
                    _ = heartbeat_cancel.cancelled() => break,
                    _ = interval.tick() => {
                        heartbeat_manager.send_heartbeat();
                    }
                }
            }
        });

        // Build router
        let mut app = Router::new()
            .route("/health", get(|| async { "OK" }))
            .route("/ready", get(|| async { "READY" }))
            .route("/sse/connect", get(handler::sse_connect::<Storage>));

        if self.enable_dashboard {
            tracing::info!("Dashboard enabled at /dashboard");
            app = app
                .route("/dashboard", get(handler::dashboard_page))
                .route("/api/stats", get(handler::get_stats::<Storage>))
                .route("/api/send", axum::routing::post(handler::send_message::<Storage>));
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

        let addr = SocketAddr::from(([0, 0, 0, 0], self.port));
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
                _ = ctrl_c => tracing::info!("Received Ctrl+C"),
                _ = terminate => tracing::info!("Received SIGTERM"),
            }

            cancel_for_shutdown.cancel();
        };

        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal)
            .await?;

        tracing::info!("Gateway shutdown complete");
        Ok(())
    }
}

/// Builder for Gateway
pub struct GatewayBuilder<Source = NoopSource, Storage = NoopStorage> {
    port: u16,
    source: Option<Source>,
    storage: Option<Storage>,
    instance_id: Option<String>,
    enable_dashboard: bool,
    heartbeat_interval: Duration,
    cleanup_interval: Duration,
    auth: Option<AuthFn>,
}

impl Default for GatewayBuilder {
    fn default() -> Self {
        Self {
            port: 8080,
            source: None,
            storage: None,
            instance_id: None,
            enable_dashboard: true,
            heartbeat_interval: Duration::from_secs(30),
            cleanup_interval: Duration::from_secs(30),
            auth: None,
        }
    }
}

impl Gateway<NoopSource, MemoryStorage> {
    /// Create a new gateway builder
    pub fn builder() -> GatewayBuilder {
        GatewayBuilder::default()
    }
}

impl<Source, Storage> GatewayBuilder<Source, Storage> {
    /// Set the server port
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Set the message source
    pub fn source<S: MessageSource>(self, source: S) -> GatewayBuilder<S, Storage> {
        GatewayBuilder {
            port: self.port,
            source: Some(source),
            storage: self.storage,
            instance_id: self.instance_id,
            enable_dashboard: self.enable_dashboard,
            heartbeat_interval: self.heartbeat_interval,
            cleanup_interval: self.cleanup_interval,
            auth: self.auth,
        }
    }

    /// Set the message storage
    pub fn storage<S: MessageStorage>(self, storage: S) -> GatewayBuilder<Source, S> {
        GatewayBuilder {
            port: self.port,
            source: self.source,
            storage: Some(storage),
            instance_id: self.instance_id,
            enable_dashboard: self.enable_dashboard,
            heartbeat_interval: self.heartbeat_interval,
            cleanup_interval: self.cleanup_interval,
            auth: self.auth,
        }
    }

    /// Set the authentication callback
    ///
    /// The callback receives an `AuthRequest` containing headers, channel_id, and client_ip.
    /// Return `None` to allow the connection, or `Some(Response)` to deny with a custom response.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use sse_gateway::{Gateway, MemoryStorage, NoopSource};
    /// use sse_gateway::auth::{AuthRequest, deny};
    /// use axum::http::StatusCode;
    ///
    /// Gateway::builder()
    ///     .port(8080)
    ///     .source(NoopSource)
    ///     .storage(MemoryStorage::default())
    ///     .auth(|req: AuthRequest| async move {
    ///         match req.bearer_token() {
    ///             Some(token) if token == "secret" => None, // Allow
    ///             Some(_) => Some(deny(StatusCode::UNAUTHORIZED, "Invalid token")),
    ///             None => Some(deny(StatusCode::UNAUTHORIZED, "Token required")),
    ///         }
    ///     })
    ///     .build()?
    ///     .run()
    ///     .await
    /// ```
    pub fn auth<F, Fut>(mut self, auth_fn: F) -> Self
    where
        F: Fn(crate::auth::AuthRequest) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = crate::auth::AuthResponse> + Send + 'static,
    {
        self.auth = Some(crate::auth::auth_fn(auth_fn));
        self
    }

    /// Set the instance ID
    pub fn instance_id(mut self, id: impl Into<String>) -> Self {
        self.instance_id = Some(id.into());
        self
    }

    /// Enable or disable the dashboard
    pub fn dashboard(mut self, enable: bool) -> Self {
        self.enable_dashboard = enable;
        self
    }

    /// Set the heartbeat interval
    pub fn heartbeat_interval(mut self, interval: Duration) -> Self {
        self.heartbeat_interval = interval;
        self
    }

    /// Set the cleanup interval
    pub fn cleanup_interval(mut self, interval: Duration) -> Self {
        self.cleanup_interval = interval;
        self
    }
}

impl<Source: MessageSource, Storage: MessageStorage> GatewayBuilder<Source, Storage> {
    /// Build the gateway
    pub fn build(self) -> anyhow::Result<Gateway<Source, Storage>> {
        let source = self.source.ok_or_else(|| anyhow::anyhow!("Source is required"))?;
        let storage = self.storage.ok_or_else(|| anyhow::anyhow!("Storage is required"))?;
        let instance_id = self.instance_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        if self.auth.is_some() {
            tracing::info!("Authentication enabled for SSE connections");
        }

        Ok(Gateway {
            port: self.port,
            source,
            storage,
            connection_manager: ConnectionManager::new(instance_id),
            enable_dashboard: self.enable_dashboard,
            heartbeat_interval: self.heartbeat_interval,
            cleanup_interval: self.cleanup_interval,
            auth: self.auth,
        })
    }
}

/// Internal dispatcher for routing messages
struct Dispatcher<S: MessageStorage> {
    connection_manager: ConnectionManager,
    storage: S,
}

impl<S: MessageStorage> Dispatcher<S> {
    fn new(connection_manager: ConnectionManager, storage: S) -> Self {
        Self {
            connection_manager,
            storage,
        }
    }

    async fn handle(&self, msg: IncomingMessage) {
        let mut event = SseEvent::raw(&msg.event_type, msg.data.clone());
        if let Some(id) = msg.id {
            event.id = Some(id);
        }

        let sent = match &msg.channel_id {
            Some(channel_id) => {
                // Generate ID first
                let stream_id = self.storage.generate_id();
                if !stream_id.is_empty() {
                    event.stream_id = Some(stream_id.clone());
                }

                // Send to clients immediately
                let sent = self.connection_manager.send_to_channel(channel_id, event.clone()).await;

                // Store in background (fire-and-forget, don't block sending)
                let storage = self.storage.clone();
                let channel_id = channel_id.clone();
                tokio::spawn(async move {
                    storage.store(&channel_id, &stream_id, &event).await;
                });

                sent
            }
            None => self.connection_manager.broadcast(event).await,
        };

        tracing::debug!(
            channel_id = ?msg.channel_id,
            event_type = %msg.event_type,
            sent_count = sent,
            "Message dispatched"
        );
    }

    fn to_handler(self) -> Arc<dyn Fn(IncomingMessage) + Send + Sync>
    where
        S: 'static,
    {
        let dispatcher = Arc::new(self);
        Arc::new(move |msg| {
            let dispatcher = dispatcher.clone();
            tokio::spawn(async move {
                dispatcher.handle(msg).await;
            });
        })
    }
}
