use axum::{
    extract::{Query, State},
    http::header,
    response::{sse::Event, Sse},
};
use futures::stream::Stream;
use serde::Deserialize;
use std::{convert::Infallible, pin::Pin, task::{Context, Poll}, time::Duration};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use crate::gateway::GatewayState;
use crate::sse::SseEvent;

#[derive(Debug, Deserialize)]
pub struct SseConnectParams {
    pub channel_id: String,
}

/// 将 SseEvent 转换为 axum SSE Event
fn sse_event_to_axum(sse_event: SseEvent) -> Event {
    let event = Event::default()
        .event(&sse_event.event_type)
        .data(sse_event.data.to_string());
    
    let event = if let Some(id) = &sse_event.id {
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

pub async fn sse_connect(
    State(state): State<GatewayState>,
    Query(params): Query<SseConnectParams>,
    headers: axum::http::HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let client_ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string());
    
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // 获取 Last-Event-ID 用于消息重放
    let last_event_id = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let is_reconnect = last_event_id.is_some();

    tracing::info!(
        channel_id = %params.channel_id,
        client_ip = ?client_ip,
        last_event_id = ?last_event_id,
        is_reconnect = is_reconnect,
        "New SSE connection"
    );

    let (connection, receiver) = state.connection_manager.register(
        params.channel_id.clone(),
        client_ip,
        user_agent,
    );

    let connection_id = connection.id.clone();
    let connection_manager = state.connection_manager.clone();

    tracing::info!(
        connection_id = %connection_id,
        channel_id = %params.channel_id,
        total_connections = state.connection_manager.connection_count(),
        "SSE connection established"
    );

    // 获取需要重放的消息
    let replay_messages = state.message_store.get_messages_after(
        &params.channel_id,
        last_event_id.as_deref(),
    ).await;

    let replay_count = replay_messages.len();
    if replay_count > 0 {
        tracing::info!(
            channel_id = %params.channel_id,
            replay_count = replay_count,
            "Replaying missed messages"
        );
    }

    // 创建重放流
    let replay_stream = futures::stream::iter(
        replay_messages.into_iter().map(|event| Ok::<_, Infallible>(sse_event_to_axum(event)))
    );

    // 实时消息流
    let stream = ReceiverStream::new(receiver);
    let event_stream = stream.map(move |sse_event: SseEvent| {
        Ok::<_, Infallible>(sse_event_to_axum(sse_event))
    });

    // 心跳流
    let heartbeat_stream = tokio_stream::wrappers::IntervalStream::new(
        tokio::time::interval(Duration::from_secs(30))
    ).map(|_| {
        Ok::<_, Infallible>(
            Event::default()
                .event("heartbeat")
                .data(serde_json::json!({"ts": chrono::Utc::now().timestamp()}).to_string())
        )
    });

    // 合并流：先重放历史消息，再接收实时消息和心跳
    let realtime_stream = futures::stream::select(event_stream, heartbeat_stream);
    let merged_stream = replay_stream.chain(realtime_stream);

    let cleanup_connection_id = connection_id.clone();
    let cleanup_channel_id = params.channel_id.clone();
    let stream_connection_id = connection_id.clone();
    let final_stream = CleanupStream {
        inner: Box::pin(merged_stream),
        connection_id: stream_connection_id,
        cleanup: Some(Box::new(move || {
            tracing::info!(
                connection_id = %cleanup_connection_id,
                channel_id = %cleanup_channel_id,
                "SSE connection disconnected, cleaning up"
            );
            connection_manager.unregister(&cleanup_connection_id);
        })),
    };

    Sse::new(final_stream)
        .keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(Duration::from_secs(10))  // 10秒检测一次断开
                .text("keep-alive")
        )
}

pub struct CleanupStream<S> {
    inner: Pin<Box<S>>,
    cleanup: Option<Box<dyn FnOnce() + Send>>,
    connection_id: String,
}

impl<S> Drop for CleanupStream<S> {
    fn drop(&mut self) {
        tracing::info!(
            connection_id = %self.connection_id,
            "CleanupStream dropped, executing cleanup"
        );
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
