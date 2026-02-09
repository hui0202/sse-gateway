//! Example: SSE Gateway with Authentication
//!
//! Run with: cargo run --example auth
//!
//! Test commands:
//! ```bash
//! # Without token (401)
//! curl -N "http://localhost:8080/sse/connect?channel_id=test"
//!
//! # With valid token
//! curl -N -H "Authorization: Bearer secret-token" \
//!   "http://localhost:8080/sse/connect?channel_id=test"
//! ```

use axum::http::StatusCode;
use sse_gateway::{
    auth::{deny, AuthRequest},
    source::ChannelSource,
    Gateway, IncomingMessage, MemoryStorage,
};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let (source, sender) = ChannelSource::new();

    // Send periodic messages
    tokio::spawn(async move {
        let mut count = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            count += 1;
            let _ = sender
                .send(
                    IncomingMessage::new(
                        "message",
                        serde_json::json!({"count": count}).to_string(),
                    )
                    .with_channel("test"),
                )
                .await;
            println!("Sent message #{}", count);
        }
    });

    println!("SSE Gateway with Auth: http://localhost:8080");
    println!();
    println!("Test:");
    println!("  curl -N 'http://localhost:8080/sse/connect?channel_id=test'  # 401");
    println!("  curl -N -H 'Authorization: Bearer secret-token' \\");
    println!("    'http://localhost:8080/sse/connect?channel_id=test'  # OK");

    Gateway::builder()
        .port(8080)
        .source(source)
        .storage(MemoryStorage::default())
        .auth(|req: AuthRequest| async move {
            // Return None to allow, Some(response) to deny
            match req.bearer_token() {
                Some("secret-token") => None,
                Some(_) => Some(deny(StatusCode::UNAUTHORIZED, "Invalid token")),
                None => Some(deny(StatusCode::UNAUTHORIZED, "Token required")),
            }
        })
        .build()?
        .run()
        .await
}
