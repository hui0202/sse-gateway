# sse-gateway

A lightweight, pluggable SSE (Server-Sent Events) gateway library for Rust.

## Official Adapters

| Crate | Description |
|-------|-------------|
| [`sse-gateway-redis`](https://crates.io/crates/sse-gateway-redis) | Redis Pub/Sub source + Redis Streams storage |
| [`sse-gateway-gcp`](https://crates.io/crates/sse-gateway-gcp) | Google Cloud Pub/Sub source |

## Features

- `server` (default): Include built-in Axum server and HTTP handlers

## Basic Usage

```rust
use sse_gateway::{Gateway, MemoryStorage, NoopSource};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Gateway::builder()
        .port(8080)
        .source(NoopSource)
        .storage(MemoryStorage::default())
        .build()?
        .run()
        .await
}
```

## Authentication

Add authentication by providing an auth callback. Return `None` to allow the connection, or `Some(Response)` to deny.

```rust
use axum::http::StatusCode;
use sse_gateway::{Gateway, MemoryStorage, NoopSource};
use sse_gateway::auth::{deny, AuthRequest};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Gateway::builder()
        .port(8080)
        .source(NoopSource)
        .storage(MemoryStorage::default())
        .auth(|req: AuthRequest| async move {
            match req.bearer_token() {
                Some("secret-token") => None,  // Allow
                Some(_) => Some(deny(StatusCode::UNAUTHORIZED, "Invalid token")),
                None => Some(deny(StatusCode::UNAUTHORIZED, "Token required")),
            }
        })
        .build()?
        .run()
        .await
}
```

### AuthRequest

The auth callback receives an `AuthRequest` with full request access:

**Fields:**
- `method`: HTTP method (`GET` for SSE)
- `uri`: Full request URI (path + query string)
- `headers`: All HTTP headers
- `channel_id`: The channel being requested
- `client_ip`: Client IP (from `X-Forwarded-For` or direct connection)

**Helper methods:**
- `req.bearer_token()` - Extract Bearer token from Authorization header
- `req.header("name")` - Get any header value
- `req.path()` - Get request path
- `req.query_string()` - Get raw query string
- `req.query_param("name")` - Get a query parameter value

### Response Helpers

```rust
use sse_gateway::auth::{deny, deny_json};
use axum::http::StatusCode;

// Plain text error
deny(StatusCode::FORBIDDEN, "Access denied")

// JSON error
deny_json(StatusCode::UNAUTHORIZED, serde_json::json!({"error": "Invalid token"}))
```

## Gateway Configuration

```rust
use std::time::Duration;
use sse_gateway::{Gateway, MemoryStorage, NoopSource};

Gateway::builder()
    .port(8080)                                    // Server port (default: 8080)
    .source(NoopSource)                            // Message source (required)
    .storage(MemoryStorage::default())             // Message storage (required)
    .instance_id("gateway-1")                      // Instance ID (default: random UUID)
    .dashboard(true)                               // Enable dashboard (default: true)
    .heartbeat_interval(Duration::from_secs(30))   // Heartbeat interval (default: 30s)
    .cleanup_interval(Duration::from_secs(30))     // Dead connection cleanup (default: 30s)
    .build()?
```

## Implementing Custom Sources

```rust
use sse_gateway::{MessageSource, MessageHandler, IncomingMessage, ConnectionManager, CancellationToken};
use async_trait::async_trait;

struct MySource;

#[async_trait]
impl MessageSource for MySource {
    async fn start(
        &self,
        handler: MessageHandler,
        connection_manager: ConnectionManager,
        cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                // msg = your_receiver.recv() => {
                //     handler(IncomingMessage::new("event_type", "data")
                //         .with_channel("channel_id"));
                // }
            }
        }
        Ok(())
    }

    fn name(&self) -> &'static str { "MySource" }
}
```

### IncomingMessage

```rust
use sse_gateway::IncomingMessage;

// Create a message for a specific channel
let msg = IncomingMessage::new("notification", r#"{"text": "Hello"}"#)
    .with_channel("user123")
    .with_id("msg-001");  // Optional business ID

// Create a broadcast message (sent to all connections)
let broadcast = IncomingMessage::broadcast("announcement", "Server maintenance");
```

## Implementing Custom Storage

```rust
use sse_gateway::{MessageStorage, SseEvent};
use async_trait::async_trait;

#[derive(Clone)]
struct MyStorage;

#[async_trait]
impl MessageStorage for MyStorage {
    fn generate_id(&self) -> String {
        // Generate a unique stream ID (used for message replay)
        uuid::Uuid::new_v4().to_string()
    }

    async fn store(&self, channel_id: &str, stream_id: &str, event: &SseEvent) {
        // Store the event with the given stream_id
    }

    async fn get_messages_after(&self, channel_id: &str, after_id: Option<&str>) -> Vec<SseEvent> {
        // Return messages after the given ID for replay
        vec![]
    }

    async fn is_available(&self) -> bool { true }
    fn name(&self) -> &'static str { "MyStorage" }
}
```

## Using with Redis

```toml
[dependencies]
sse-gateway = "0.1"
sse-gateway-redis = "0.1"
```

```rust
use sse_gateway::Gateway;
use sse_gateway_redis::{RedisPubSubSource, RedisStorage};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let storage = RedisStorage::new();
    storage.connect("redis://localhost:6379").await?;

    Gateway::builder()
        .port(8080)
        .source(RedisPubSubSource::with_defaults("redis://localhost:6379"))
        .storage(storage)
        .build()?
        .run()
        .await
}
```

## Using with Google Cloud Pub/Sub

```toml
[dependencies]
sse-gateway = "0.1"
sse-gateway-gcp = "0.1"
```

```rust
use sse_gateway::{Gateway, MemoryStorage};
use sse_gateway_gcp::GcpPubSubSource;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Gateway::builder()
        .port(8080)
        .source(GcpPubSubSource::new("my-project", "my-subscription"))
        .storage(MemoryStorage::default())
        .build()?
        .run()
        .await
}
```

## Built-in Sources

| Source | Description |
|--------|-------------|
| `NoopSource` | Does nothing, for testing or when messages are sent via dashboard |
| `ChannelSource` | Programmatic message sending via Tokio channel |

### ChannelSource Example

```rust
use sse_gateway::{Gateway, MemoryStorage, IncomingMessage};
use sse_gateway::source::ChannelSource;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (source, sender) = ChannelSource::new();

    // Send messages programmatically
    tokio::spawn(async move {
        sender.send(IncomingMessage::new("hello", "world")
            .with_channel("test")).await.ok();
    });

    Gateway::builder()
        .port(8080)
        .source(source)
        .storage(MemoryStorage::default())
        .build()?
        .run()
        .await
}
```

## Built-in Storage

| Storage | Description |
|---------|-------------|
| `MemoryStorage` | In-memory storage, suitable for development and single-instance |
| `NoopStorage` | Disabled storage, no message replay |

## Advanced: Direct Push with Redis Channel Registry

For low-latency scenarios, you can implement a Direct Push architecture that bypasses Pub/Sub and uses Redis for channel-to-gateway mapping. This is ideal for multi-instance deployments where you want to push messages directly to the gateway handling a specific channel.

### Architecture

```
┌─────────────┐     ┌─────────────────────────────────────────────┐
│   Client    │────▶│              SSE Gateway                    │
│ (Browser)   │◀────│  :8080 SSE endpoint                         │
└─────────────┘     │                                             │
                    │  on_connect: register channel→gateway       │
                    │  on_disconnect: unregister                  │
                    └─────────────────────────────────────────────┘
                                          │
                                          ▼
┌─────────────┐     ┌─────────────────────────────────────────────┐
│   Agent     │────▶│         Direct Push API (:9000)             │
│ (Backend)   │     │  POST /push    - send + store               │
└─────────────┘     │  POST /store   - store only (offline)       │
       │            │  GET  /channel/{id} - query status          │
       │            └─────────────────────────────────────────────┘
       │                              │
       │                              ▼
       │            ┌─────────────────────────────────────────────┐
       └───────────▶│                 Redis                        │
                    │  channel:{id}:gateway → gateway_addr (TTL)  │
                    │  sse:stream:{id} → message history          │
                    └─────────────────────────────────────────────┘
```

### Connection Lifecycle Callbacks

Use `on_connect` and `on_disconnect` to manage channel registry:

```rust
use async_trait::async_trait;
use sse_gateway::{
    CancellationToken, ConnectionInfo, ConnectionManager, 
    IncomingMessage, MessageHandler, MessageSource,
};

struct DirectPushSource {
    channel_registry: RedisChannelRegistry,
    gateway_addr: String,
    // ... other fields
}

#[async_trait]
impl MessageSource for DirectPushSource {
    async fn start(
        &self,
        handler: MessageHandler,
        connection_manager: ConnectionManager,
        cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        // Start HTTP server for direct push
        // Forward messages to handler
        Ok(())
    }

    fn name(&self) -> &'static str { "DirectPush" }

    /// Called when a new SSE connection is established
    fn on_connect(&self, info: &ConnectionInfo) {
        let registry = self.channel_registry.clone();
        let channel_id = info.channel_id.clone();
        let gateway_addr = self.gateway_addr.clone();
        
        // Register channel→gateway mapping in Redis (fire-and-forget)
        tokio::spawn(async move {
            registry.register(&channel_id, &gateway_addr).await;
        });
    }

    /// Called when an SSE connection is closed
    fn on_disconnect(&self, info: &ConnectionInfo) {
        let registry = self.channel_registry.clone();
        let channel_id = info.channel_id.clone();
        
        // Unregister from Redis (fire-and-forget)
        tokio::spawn(async move {
            registry.unregister(&channel_id).await;
        });
    }
}
```

### ConnectionInfo

The lifecycle callbacks receive a `ConnectionInfo` struct:

```rust
pub struct ConnectionInfo {
    pub channel_id: String,      // The channel ID
    pub connection_id: String,   // Unique connection ID
    pub instance_id: String,     // Gateway instance ID
}
```

### Push API Endpoints

Implement a separate HTTP server for direct push:

```rust
#[derive(serde::Deserialize)]
struct PushPayload {
    channel_id: Option<String>,  // None = broadcast
    event_type: String,
    data: serde_json::Value,
}

#[derive(serde::Serialize)]
struct PushResponse {
    success: bool,
    online: bool,       // Whether channel is connected to THIS gateway
    stream_id: String,
}

/// POST /push - Send message to connected clients + store to Redis
async fn handle_push(
    State(state): State<AppState>,
    Json(payload): Json<PushPayload>,
) -> Json<PushResponse> {
    let channel_id = payload.channel_id.clone().unwrap_or_default();
    let stream_id = state.storage.generate_id();
    
    // Check if channel has active connections on THIS gateway (local memory, no Redis)
    let online = state.connection_manager.channel_connection_count(&channel_id) > 0;
    
    // Send through gateway + store
    let msg = IncomingMessage::new(&payload.event_type, payload.data.to_string())
        .with_channel(&channel_id);
    let success = state.sender.send(msg).await.is_ok();
    
    Json(PushResponse { success, online, stream_id })
}

/// POST /store - Store only (for offline users)
async fn handle_store(
    State(state): State<AppState>,
    Json(payload): Json<StorePayload>,
) -> Json<StoreResponse> {
    let stream_id = state.storage.generate_id();
    let event = SseEvent::raw(&payload.event_type, payload.data.to_string());
    state.storage.store(&payload.channel_id, &stream_id, &event).await;
    
    Json(StoreResponse { success: true, stream_id })
}

/// GET /channel/{id} - Query channel status
async fn handle_channel_status(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
) -> Json<ChannelStatus> {
    let gateway = state.registry.get_gateway(&channel_id).await;
    Json(ChannelStatus {
        channel_id,
        online: gateway.is_some(),
        gateway,
    })
}
```

### Agent Workflow

```
1. Agent wants to send message to user "user123"
2. Agent queries Redis: GET channel:user123:gateway
3. If exists → POST to that gateway's /push endpoint
4. If not exists → User offline, POST to /store for later replay
5. Check response.online:
   - true: Message delivered in real-time
   - false: User may have disconnected, refresh cache and retry
```

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `PORT` | SSE gateway port | 8080 |
| `PUSH_PORT` | Direct push API port | 9000 |
| `REDIS_URL` | Redis connection URL | redis://localhost:6379 |
| `GATEWAY_ADDR` | This gateway's address (for registry) | localhost:8080 |
| `CHANNEL_TTL` | Channel mapping TTL in seconds | 60 |

### Benefits

1. **Low Latency**: Direct HTTP push bypasses Pub/Sub, reducing latency
2. **Efficient Routing**: Agent knows exactly which gateway to push to
3. **Offline Support**: Store messages for offline users, replay on reconnect
4. **Scalable**: Multiple gateway instances with Redis coordination

## HTTP Endpoints

| Endpoint | Description |
|----------|-------------|
| `GET /health` | Health check |
| `GET /ready` | Readiness check |
| `GET /sse/connect?channel_id=xxx` | SSE connection endpoint |
| `GET /dashboard` | Web dashboard (if enabled) |
| `GET /api/stats` | Connection statistics (if dashboard enabled) |
| `POST /api/send` | Send message via HTTP (if dashboard enabled) |

## License

MIT OR Apache-2.0
