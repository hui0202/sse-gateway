# sse-gateway

A lightweight, pluggable SSE (Server-Sent Events) gateway library for Rust.

See the [main README](../../README.md) for full documentation.

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
