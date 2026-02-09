//! HTTP handlers for the SSE gateway

use axum::{
    extract::{OriginalUri, Query, State},
    http::{header, Method, StatusCode},
    response::{sse::Event, Html, IntoResponse, Json, Sse},
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use std::{
    convert::Infallible,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use crate::auth::{AuthFn, AuthRequest};
use crate::event::SseEvent;
use crate::gateway::LifecycleCallback;
use crate::manager::ConnectionManager;
use crate::source::ConnectionInfo;
use crate::storage::MessageStorage;

/// Shared state for handlers
#[derive(Clone)]
pub struct GatewayState<S: MessageStorage> {
    pub connection_manager: ConnectionManager,
    pub storage: S,
    pub auth: Option<AuthFn>,
    pub on_connect: Option<LifecycleCallback>,
    pub on_disconnect: Option<LifecycleCallback>,
}

#[derive(Debug, Deserialize)]
pub struct SseConnectParams {
    pub channel_id: String,
}

fn sse_event_to_axum(sse_event: SseEvent) -> Event {
    let data = sse_event.data.to_string();
    let event = Event::default().event(&sse_event.event_type).data(data);

    let event = if let Some(stream_id) = &sse_event.stream_id {
        event.id(stream_id.clone())
    } else if let Some(id) = &sse_event.id {
        event.id(id.clone())
    } else {
        event
    };

    if let Some(retry) = sse_event.retry {
        event.retry(Duration::from_millis(retry as u64))
    } else {
        event
    }
}

/// SSE connection endpoint
pub async fn sse_connect<S: MessageStorage>(
    State(state): State<GatewayState<S>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    Query(params): Query<SseConnectParams>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    let client_ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string());

    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let last_event_id = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Perform authentication if configured
    if let Some(auth_fn) = &state.auth {
        let auth_request = AuthRequest {
            method: method.clone(),
            uri: uri.clone(),
            headers: headers.clone(),
            channel_id: params.channel_id.clone(),
            client_ip: client_ip.clone(),
        };

        // If auth returns Some(response), deny the connection
        if let Some(response) = auth_fn(auth_request).await {
            tracing::warn!(
                channel_id = %params.channel_id,
                client_ip = ?client_ip,
                "SSE connection denied"
            );
            return response;
        }
    }

    tracing::info!(
        channel_id = %params.channel_id,
        client_ip = ?client_ip,
        last_event_id = ?last_event_id,
        "New SSE connection"
    );

    let (connection, receiver) = state.connection_manager.register(
        params.channel_id.clone(),
        client_ip,
        user_agent,
    );

    let connection_id = connection.id.clone();
    let instance_id = state.connection_manager.instance_id().to_string();
    let connection_manager = state.connection_manager.clone();

    // Call on_connect callback
    let conn_info = ConnectionInfo {
        channel_id: params.channel_id.clone(),
        connection_id: connection_id.clone(),
        instance_id: instance_id.clone(),
    };
    if let Some(ref on_connect) = state.on_connect {
        on_connect(&conn_info);
    }

    // Replay missed messages
    let replay_messages = state
        .storage
        .get_messages_after(&params.channel_id, last_event_id.as_deref())
        .await;

    if !replay_messages.is_empty() {
        tracing::info!(
            channel_id = %params.channel_id,
            count = replay_messages.len(),
            "Replaying messages"
        );
    }

    let replay_stream = futures::stream::iter(
        replay_messages
            .into_iter()
            .map(|event| Ok::<_, Infallible>(sse_event_to_axum(event))),
    );

    let event_stream = ReceiverStream::new(receiver)
        .map(|event| Ok::<_, Infallible>(sse_event_to_axum(event)));

    let heartbeat_stream = tokio_stream::wrappers::BroadcastStream::new(
        state.connection_manager.subscribe_heartbeat(),
    )
    .filter_map(|r| r.ok())
    .map(|ts| {
        Ok::<_, Infallible>(
            Event::default()
                .event("heartbeat")
                .data(serde_json::json!({"ts": ts}).to_string()),
        )
    });

    let realtime_stream = futures::stream::select(event_stream, heartbeat_stream);
    let merged_stream = replay_stream.chain(realtime_stream);

    let cleanup_id = connection_id.clone();
    let cleanup_channel = params.channel_id.clone();
    let cleanup_instance = instance_id.clone();
    let on_disconnect = state.on_disconnect.clone();
    let final_stream = CleanupStream {
        inner: Box::pin(merged_stream),
        connection_id: connection_id.clone(),
        cleanup: Some(Box::new(move || {
            tracing::info!(connection_id = %cleanup_id, channel_id = %cleanup_channel, "Connection closed");
            connection_manager.unregister(&cleanup_id);
            
            // Call on_disconnect callback
            if let Some(ref callback) = on_disconnect {
                let info = ConnectionInfo {
                    channel_id: cleanup_channel.clone(),
                    connection_id: cleanup_id.clone(),
                    instance_id: cleanup_instance,
                };
                callback(&info);
            }
        })),
    };

    Sse::new(final_stream)
        .keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(Duration::from_secs(10))
                .text("keep-alive"),
        )
        .into_response()
}

struct CleanupStream<S> {
    inner: Pin<Box<S>>,
    cleanup: Option<Box<dyn FnOnce() + Send>>,
    #[allow(dead_code)]
    connection_id: String,
}

impl<S> Drop for CleanupStream<S> {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            cleanup();
        }
    }
}

impl<S: Stream + Unpin> Stream for CleanupStream<S> {
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

// Stats endpoint
#[derive(Serialize)]
pub struct StatsResponse {
    pub total_connections: usize,
    pub connections: Vec<ConnectionStats>,
}

#[derive(Serialize)]
pub struct ConnectionStats {
    pub id: String,
    pub channel_id: String,
    pub connected_at: String,
    pub is_active: bool,
}

pub async fn get_stats<S: MessageStorage>(
    State(state): State<GatewayState<S>>,
) -> Json<StatsResponse> {
    let connections: Vec<ConnectionStats> = state
        .connection_manager
        .list_connections()
        .into_iter()
        .map(|c| ConnectionStats {
            id: c.id.clone(),
            channel_id: c.channel_id.clone(),
            connected_at: c.metadata.connected_at.to_rfc3339(),
            is_active: c.is_active(),
        })
        .collect();

    Json(StatsResponse {
        total_connections: connections.len(),
        connections,
    })
}

// Send message endpoint
#[derive(Deserialize)]
pub struct SendMessageRequest {
    pub channel_id: Option<String>,
    pub event_type: String,
    pub data: serde_json::Value,
}

#[derive(Serialize)]
pub struct SendMessageResponse {
    pub success: bool,
    pub sent_count: usize,
}

pub async fn send_message<S: MessageStorage>(
    State(state): State<GatewayState<S>>,
    Json(req): Json<SendMessageRequest>,
) -> impl IntoResponse {
    let mut event = SseEvent::new(&req.event_type, req.data);

    let sent_count = match &req.channel_id {
        Some(channel_id) if !channel_id.is_empty() => {
            // Generate ID first
            let stream_id = state.storage.generate_id();
            if !stream_id.is_empty() {
                event.stream_id = Some(stream_id.clone());
            }

            // Send to clients immediately
            let sent = state.connection_manager.send_to_channel(channel_id, event.clone()).await;

            // Store in background (fire-and-forget)
            let storage = state.storage.clone();
            let channel_id = channel_id.clone();
            tokio::spawn(async move {
                storage.store(&channel_id, &stream_id, &event).await;
            });

            sent
        }
        _ => state.connection_manager.broadcast(event).await,
    };

    (
        StatusCode::OK,
        Json(SendMessageResponse {
            success: sent_count > 0,
            sent_count,
        }),
    )
}

// Dashboard
pub async fn dashboard_page() -> Html<&'static str> {
    Html(include_str!("dashboard.html"))
}
