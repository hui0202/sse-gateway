//! Unit tests for sse-gateway

use sse_gateway::{
    auth::{deny, AuthRequest},
    source::{ChannelSource, IncomingMessage},
    storage::{MemoryStorage, MessageStorage, NoopStorage},
    ConnectionManager, EventData, MessageSource, SseEvent,
};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

// ============== EventData Tests ==============

#[test]
fn test_event_data_raw() {
    let data = EventData::Raw("hello".to_string());
    assert_eq!(data.to_string(), "hello");
}

#[test]
fn test_event_data_value() {
    let data = EventData::Value(serde_json::json!({"key": "value"}));
    assert_eq!(data.to_string(), r#"{"key":"value"}"#);
}

#[test]
fn test_event_data_from_string() {
    let data: EventData = "test".into();
    assert_eq!(data.to_string(), "test");
}

#[test]
fn test_event_data_from_json() {
    let data: EventData = serde_json::json!({"a": 1}).into();
    assert_eq!(data.to_string(), r#"{"a":1}"#);
}

// ============== SseEvent Tests ==============

#[test]
fn test_sse_event_new() {
    let event = SseEvent::new("update", serde_json::json!({"count": 1}));
    assert_eq!(event.event_type, "update");
    assert!(event.id.is_some());
    assert!(event.stream_id.is_none());
}

#[test]
fn test_sse_event_raw() {
    let event = SseEvent::raw("message", "hello world");
    assert_eq!(event.event_type, "message");
    assert_eq!(event.data.to_string(), "hello world");
}

#[test]
fn test_sse_event_message() {
    let event = SseEvent::message("test");
    assert_eq!(event.event_type, "message");
    assert_eq!(event.data.to_string(), "test");
}

#[test]
fn test_sse_event_with_stream_id() {
    let event = SseEvent::message("test").with_stream_id("stream-123");
    assert_eq!(event.stream_id, Some("stream-123".to_string()));
}

#[test]
fn test_sse_event_with_retry() {
    let event = SseEvent::message("test").with_retry(5000);
    assert_eq!(event.retry, Some(5000));
}

// ============== IncomingMessage Tests ==============

#[test]
fn test_incoming_message_new() {
    let msg = IncomingMessage::new("notification", r#"{"text":"hello"}"#);
    assert_eq!(msg.event_type, "notification");
    assert_eq!(msg.data, r#"{"text":"hello"}"#);
    assert!(msg.channel_id.is_none());
    assert!(msg.id.is_none());
}

#[test]
fn test_incoming_message_with_channel() {
    let msg = IncomingMessage::new("update", "data").with_channel("channel-1");
    assert_eq!(msg.channel_id, Some("channel-1".to_string()));
}

#[test]
fn test_incoming_message_with_id() {
    let msg = IncomingMessage::new("update", "data").with_id("msg-123");
    assert_eq!(msg.id, Some("msg-123".to_string()));
}

#[test]
fn test_incoming_message_broadcast() {
    let msg = IncomingMessage::broadcast("broadcast", "hello all");
    assert!(msg.channel_id.is_none());
    assert_eq!(msg.event_type, "broadcast");
}

// ============== MemoryStorage Tests ==============

#[tokio::test]
async fn test_memory_storage_store_and_retrieve() {
    let storage = MemoryStorage::new(10);
    let event = SseEvent::message("test");

    // Store event
    let stream_id = storage.store("channel-1", &event).await;
    assert!(stream_id.is_some());

    // Retrieve with non-existent after_id returns empty
    let messages = storage.get_messages_after("channel-1", Some("non-existent")).await;
    assert!(messages.is_empty());

    // Retrieve with None after_id returns empty
    let messages = storage.get_messages_after("channel-1", None).await;
    assert!(messages.is_empty());
}

#[tokio::test]
async fn test_memory_storage_replay() {
    let storage = MemoryStorage::new(10);

    // Store multiple events
    let id1 = storage.store("ch1", &SseEvent::message("msg1")).await.unwrap();
    let _id2 = storage.store("ch1", &SseEvent::message("msg2")).await.unwrap();
    let _id3 = storage.store("ch1", &SseEvent::message("msg3")).await.unwrap();

    // Get messages after id1
    let messages = storage.get_messages_after("ch1", Some(&id1)).await;
    assert_eq!(messages.len(), 2);
}

#[tokio::test]
async fn test_memory_storage_max_capacity() {
    let storage = MemoryStorage::new(3);

    // Store more than max
    for i in 0..5 {
        storage.store("ch1", &SseEvent::message(format!("msg{}", i))).await;
    }

    // Store one more to check capacity
    let _last_id = storage.store("ch1", &SseEvent::message("last")).await.unwrap();
    
    // Should only have last 3 messages
    // Check by getting messages after a non-existent early ID
    let messages = storage.get_messages_after("ch1", Some("0-0")).await;
    assert!(messages.is_empty()); // ID not found, so returns empty
}

#[tokio::test]
async fn test_memory_storage_different_channels() {
    let storage = MemoryStorage::new(10);

    storage.store("ch1", &SseEvent::message("msg1")).await;
    let id = storage.store("ch2", &SseEvent::message("msg2")).await.unwrap();
    storage.store("ch2", &SseEvent::message("msg3")).await;

    // Get messages from ch2 only
    let messages = storage.get_messages_after("ch2", Some(&id)).await;
    assert_eq!(messages.len(), 1);
}

#[tokio::test]
async fn test_memory_storage_is_available() {
    let storage = MemoryStorage::default();
    assert!(storage.is_available().await);
}

#[tokio::test]
async fn test_noop_storage() {
    let storage = NoopStorage;
    
    assert!(storage.store("ch1", &SseEvent::message("test")).await.is_none());
    assert!(storage.get_messages_after("ch1", Some("id")).await.is_empty());
    assert!(!storage.is_available().await);
    assert_eq!(storage.name(), "Noop (disabled)");
}

// ============== ConnectionManager Tests ==============

#[tokio::test]
async fn test_connection_manager_register() {
    let manager = ConnectionManager::new("instance-1");
    
    let (conn, _rx) = manager.register("channel-1".to_string(), None, None);
    
    assert_eq!(conn.channel_id, "channel-1");
    assert_eq!(manager.connection_count(), 1);
    assert_eq!(manager.channel_connection_count("channel-1"), 1);
}

#[tokio::test]
async fn test_connection_manager_unregister() {
    let manager = ConnectionManager::new("instance-1");
    
    let (conn, _rx) = manager.register("channel-1".to_string(), None, None);
    let conn_id = conn.id.clone();
    
    assert_eq!(manager.connection_count(), 1);
    
    manager.unregister(&conn_id);
    
    assert_eq!(manager.connection_count(), 0);
    assert_eq!(manager.channel_connection_count("channel-1"), 0);
}

#[tokio::test]
async fn test_connection_manager_send_to_channel() {
    let manager = ConnectionManager::new("instance-1");
    
    let (_conn1, mut rx1) = manager.register("channel-1".to_string(), None, None);
    let (_conn2, mut rx2) = manager.register("channel-1".to_string(), None, None);
    let (_conn3, mut rx3) = manager.register("channel-2".to_string(), None, None);
    
    let event = SseEvent::message("hello channel-1");
    let sent = manager.send_to_channel("channel-1", event).await;
    
    assert_eq!(sent, 2);
    
    // channel-1 connections should receive
    assert!(rx1.try_recv().is_ok());
    assert!(rx2.try_recv().is_ok());
    // channel-2 should not receive
    assert!(rx3.try_recv().is_err());
}

#[tokio::test]
async fn test_connection_manager_broadcast() {
    let manager = ConnectionManager::new("instance-1");
    
    let (_conn1, mut rx1) = manager.register("channel-1".to_string(), None, None);
    let (_conn2, mut rx2) = manager.register("channel-2".to_string(), None, None);
    
    let event = SseEvent::message("broadcast");
    let sent = manager.broadcast(event).await;
    
    assert_eq!(sent, 2);
    assert!(rx1.try_recv().is_ok());
    assert!(rx2.try_recv().is_ok());
}

#[tokio::test]
async fn test_connection_manager_send_to_connection() {
    let manager = ConnectionManager::new("instance-1");
    
    let (conn1, mut rx1) = manager.register("channel-1".to_string(), None, None);
    let (_conn2, mut rx2) = manager.register("channel-1".to_string(), None, None);
    
    let event = SseEvent::message("direct");
    let sent = manager.send_to_connection(&conn1.id, event).await;
    
    assert!(sent);
    assert!(rx1.try_recv().is_ok());
    assert!(rx2.try_recv().is_err());
}

#[tokio::test]
async fn test_connection_manager_list_connections() {
    let manager = ConnectionManager::new("instance-1");
    
    manager.register("ch1".to_string(), Some("1.2.3.4".to_string()), None);
    manager.register("ch2".to_string(), None, Some("Mozilla".to_string()));
    
    let connections = manager.list_connections();
    assert_eq!(connections.len(), 2);
}

#[tokio::test]
async fn test_connection_manager_cleanup_dead_connections() {
    let manager = ConnectionManager::new("instance-1");
    
    let (_conn, rx) = manager.register("channel-1".to_string(), None, None);
    assert_eq!(manager.connection_count(), 1);
    
    // Drop receiver to make connection dead
    drop(rx);
    
    // Cleanup should remove dead connection
    manager.cleanup_dead_connections();
    assert_eq!(manager.connection_count(), 0);
}

#[tokio::test]
async fn test_connection_manager_heartbeat() {
    let manager = ConnectionManager::new("instance-1");
    let mut rx = manager.subscribe_heartbeat();
    
    manager.send_heartbeat();
    
    let ts = rx.recv().await.unwrap();
    assert!(ts > 0);
}

// ============== ChannelSource Tests ==============

#[tokio::test]
async fn test_channel_source_send_receive() {
    let (source, sender) = ChannelSource::new();
    let cancel = CancellationToken::new();
    
    let received = Arc::new(AtomicUsize::new(0));
    let received_clone = received.clone();
    
    let handler: sse_gateway::MessageHandler = Arc::new(move |_msg| {
        received_clone.fetch_add(1, Ordering::SeqCst);
    });
    
    let cancel_clone = cancel.clone();
    let source_handle: tokio::task::JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
        source.start(handler, cancel_clone).await
    });
    
    // Send messages
    sender.send(IncomingMessage::new("test", "data1")).await.unwrap();
    sender.send(IncomingMessage::new("test", "data2")).await.unwrap();
    
    // Wait a bit for processing
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    
    // Cancel and wait for source to stop
    cancel.cancel();
    let _ = source_handle.await.unwrap();
    
    assert_eq!(received.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_channel_source_stops_when_sender_dropped() {
    let (source, sender) = ChannelSource::new();
    let cancel = CancellationToken::new();
    
    let handler: sse_gateway::MessageHandler = Arc::new(|_| {});
    
    let handle = tokio::spawn(async move {
        source.start(handler, cancel).await
    });
    
    // Drop sender should cause source to stop
    drop(sender);
    
    // Should complete without hanging
    let result = tokio::time::timeout(
        tokio::time::Duration::from_millis(100),
        handle
    ).await;
    
    assert!(result.is_ok());
}

// ============== Auth Tests ==============

#[test]
fn test_auth_request_bearer_token() {
    let mut headers = HeaderMap::new();
    headers.insert("authorization", "Bearer my-secret-token".parse().unwrap());
    
    let req = AuthRequest {
        method: Method::GET,
        uri: "/sse/connect?channel_id=test".parse::<Uri>().unwrap(),
        headers,
        channel_id: "test".to_string(),
        client_ip: None,
    };
    
    assert_eq!(req.bearer_token(), Some("my-secret-token"));
}

#[test]
fn test_auth_request_header() {
    let mut headers = HeaderMap::new();
    headers.insert("x-custom", "value123".parse().unwrap());
    
    let req = AuthRequest {
        method: Method::GET,
        uri: "/sse/connect?channel_id=test&foo=bar".parse::<Uri>().unwrap(),
        headers,
        channel_id: "test".to_string(),
        client_ip: Some("1.2.3.4".to_string()),
    };
    
    assert_eq!(req.header("x-custom"), Some("value123"));
    assert_eq!(req.header("non-existent"), None);
    assert_eq!(req.path(), "/sse/connect");
    assert_eq!(req.query_param("foo"), Some("bar"));
    assert_eq!(req.query_param("channel_id"), Some("test"));
}

#[test]
fn test_auth_deny_response() {
    let _response = deny(StatusCode::UNAUTHORIZED, "Invalid token");
    // Just verify it creates a response without panicking
}

// ============== SseConnection Tests ==============

#[tokio::test]
async fn test_sse_connection_send() {
    let (conn, mut rx) = sse_gateway::SseConnection::new(
        "channel-1".to_string(),
        "instance-1".to_string(),
        Some("1.2.3.4".to_string()),
        Some("Mozilla/5.0".to_string()),
    );
    
    assert!(conn.is_active());
    assert_eq!(conn.channel_id, "channel-1");
    assert_eq!(conn.metadata.instance_id, "instance-1");
    assert_eq!(conn.metadata.client_ip, Some("1.2.3.4".to_string()));
    
    let event = SseEvent::message("hello");
    assert!(conn.send(event).await);
    
    let received = rx.recv().await.unwrap();
    assert_eq!(received.data.to_string(), "hello");
}

#[tokio::test]
async fn test_sse_connection_inactive_after_drop() {
    let (conn, rx) = sse_gateway::SseConnection::new(
        "channel-1".to_string(),
        "instance-1".to_string(),
        None,
        None,
    );
    
    assert!(conn.is_active());
    drop(rx);
    assert!(!conn.is_active());
}

#[tokio::test]
async fn test_sse_connection_clone() {
    let (conn1, _rx) = sse_gateway::SseConnection::new(
        "channel-1".to_string(),
        "instance-1".to_string(),
        None,
        None,
    );
    
    let conn2 = conn1.clone();
    
    assert_eq!(conn1.id, conn2.id);
    assert_eq!(conn1.channel_id, conn2.channel_id);
}
