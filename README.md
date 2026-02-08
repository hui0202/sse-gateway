# SSE Gateway

A lightweight, pluggable Server-Sent Events (SSE) gateway library for Rust.

## Features

- **Pluggable Message Sources**: Implement `MessageSource` to receive messages from any backend (Redis, Kafka, RabbitMQ, WebSocket, HTTP, etc.)
- **Pluggable Storage**: Implement `MessageStorage` for message replay on reconnection
- **Channel-based Routing**: Route messages to specific channels or broadcast to all connections
- **Built-in Server**: Optional Axum-based HTTP server with SSE endpoint
- **High Performance**: Designed for high-concurrency with minimal memory footprint

## Crates

| Crate | Description |
|-------|-------------|
| `sse-gateway` | Core library with traits and built-in implementations |
| `sse-gateway-redis` | Redis Pub/Sub source and Redis Streams storage |
| `sse-gateway-gcp` | Google Cloud Pub/Sub source |

## Quick Start

Add to your `Cargo.toml`:

```toml
[dependencies]
sse-gateway = "0.1"
tokio = { version = "1", features = ["full"] }
```

### Minimal Example

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

### Custom Message Source

```rust
use sse_gateway::{MessageSource, MessageHandler, IncomingMessage, CancellationToken};
use async_trait::async_trait;

struct MySource {
    // your fields
}

#[async_trait]
impl MessageSource for MySource {
    async fn start(&self, handler: MessageHandler, cancel: CancellationToken) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                // msg = your_receiver.recv() => {
                //     handler(IncomingMessage::new("event", msg));
                // }
            }
        }
        Ok(())
    }

    fn name(&self) -> &'static str { "MySource" }
}
```

### Using Redis

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

Send messages to Redis:

```bash
# Send to specific channel
redis-cli PUBLISH "sse:user123" '{"event_type":"notification","data":{"msg":"Hello!"}}'

# Broadcast to all
redis-cli PUBLISH "sse:broadcast" '{"event_type":"alert","data":"System update"}'
```

### Using Google Cloud Pub/Sub

```toml
[dependencies]
sse-gateway = "0.1"
sse-gateway-gcp = "0.1"
```

```rust
use sse_gateway::Gateway;
use sse_gateway_gcp::GcpPubSubSource;

Gateway::builder()
    .port(8080)
    .source(GcpPubSubSource::new("my-project", "my-subscription"))
    .storage(sse_gateway::MemoryStorage::default())
    .build()?
    .run()
    .await
```

## API Endpoints

| Endpoint | Description |
|----------|-------------|
| `GET /sse/connect?channel_id=xxx` | SSE connection endpoint |
| `GET /health` | Health check |
| `GET /ready` | Readiness check |
| `GET /dashboard` | Web dashboard (optional) |
| `GET /api/stats` | Connection statistics |
| `POST /api/send` | Send message (for testing) |

## Client Connection

```javascript
const es = new EventSource('/sse/connect?channel_id=my-channel');

es.addEventListener('message', (e) => {
    console.log('Received:', e.data);
});

es.addEventListener('notification', (e) => {
    console.log('Notification:', e.data);
});
```

## Message Format

### IncomingMessage

```rust
IncomingMessage {
    channel_id: Option<String>,  // None = broadcast
    event_type: String,          // SSE event type
    data: String,                // Message payload
    id: Option<String>,          // Business ID
}
```

### SseEvent

```rust
SseEvent {
    event_type: String,
    data: EventData,
    id: Option<String>,
    stream_id: Option<String>,   // For replay
    retry: Option<u32>,
}
```

## Examples

Run examples with:

```bash
# Simple gateway
cargo run --example simple

# Custom source (generates periodic messages)
cargo run --example custom_source

# Programmatic message sending
cargo run --example channel_source

# HTTP webhook as source
cargo run --example webhook_source
```

## 部署 (Cloud Run / GCE)

本项目包含一个使用 GCP Pub/Sub 的独立服务，可直接部署到 Cloud Run 或 GCE。

**部署脚本：**

```bash
# 部署到 Cloud Run
export PROJECT=your-gcp-project
./deploy-cloudrun.sh

# 部署到 GCE (托管实例组)
./deploy-gce-mig.sh
```

**手动构建 Docker 镜像：**

```bash
docker build -t gateway .
docker run -p 8080:8080 \
  -e GCP_PROJECT=your-project \
  -e PUBSUB_SUBSCRIPTION=your-subscription \
  gateway
```

**环境变量：**

| Variable | Description | Default |
|----------|-------------|---------|
| `PORT` | Server port | `8080` |
| `GCP_PROJECT` | GCP project ID | (required) |
| `PUBSUB_SUBSCRIPTION` | Pub/Sub subscription | (required) |

## License

MIT OR Apache-2.0
