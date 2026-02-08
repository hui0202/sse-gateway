//! Simple example: SSE Gateway with memory storage
//!
//! Run with: cargo run --example simple
//!
//! Then open http://localhost:8080/dashboard in your browser

use sse_gateway::{Gateway, MemoryStorage, NoopSource};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    Gateway::builder()
        .port(8080)
        .source(NoopSource)
        .storage(MemoryStorage::default())
        .dashboard(true)
        .build()?
        .run()
        .await
}
