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

## Implementing Custom Sources

```rust
use sse_gateway::{MessageSource, MessageHandler, IncomingMessage, CancellationToken};
use async_trait::async_trait;

struct MySource;

#[async_trait]
impl MessageSource for MySource {
    async fn start(&self, handler: MessageHandler, cancel: CancellationToken) -> anyhow::Result<()> {
        // Your implementation
        Ok(())
    }

    fn name(&self) -> &'static str { "MySource" }
}
```

## Implementing Custom Storage

```rust
use sse_gateway::{MessageStorage, SseEvent};
use async_trait::async_trait;

#[derive(Clone)]
struct MyStorage;

#[async_trait]
impl MessageStorage for MyStorage {
    async fn store(&self, channel_id: &str, event: &SseEvent) -> Option<String> {
        // Store and return stream ID
        None
    }

    async fn get_messages_after(&self, channel_id: &str, after_id: Option<&str>) -> Vec<SseEvent> {
        // Return messages for replay
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
