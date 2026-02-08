//! Example: Using ChannelSource for programmatic message sending
//!
//! Run with: cargo run --example channel_source
//!
//! This example shows how to send messages to the gateway from your own code.

use sse_gateway::{
    source::ChannelSource, Gateway, IncomingMessage, MemoryStorage,
};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    // Create a channel source
    let (source, sender) = ChannelSource::new();

    // Spawn a task to send messages
    let sender_task = tokio::spawn(async move {
        let mut count = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            count += 1;

            // Send to specific channel
            let _ = sender
                .send(
                    IncomingMessage::new(
                        "update",
                        serde_json::json!({"count": count, "channel": "channel-a"}).to_string(),
                    )
                    .with_channel("channel-a"),
                )
                .await;

            // Broadcast to all
            let _ = sender
                .send(IncomingMessage::broadcast(
                    "broadcast",
                    serde_json::json!({"count": count, "msg": "Hello everyone!"}).to_string(),
                ))
                .await;

            println!("Sent message #{}", count);
        }
    });

    println!("Starting SSE Gateway with ChannelSource");
    println!("Open http://localhost:8080/dashboard");
    println!("Connect to 'channel-a' to see targeted messages");
    println!("All connections will receive broadcast messages");

    // Run gateway (this will block)
    let result = Gateway::builder()
        .port(8080)
        .source(source)
        .storage(MemoryStorage::default())
        .build()?
        .run()
        .await;

    sender_task.abort();
    result
}
