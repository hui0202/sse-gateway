//! # SSE Gateway
//!
//! A lightweight, pluggable Server-Sent Events (SSE) gateway library for Rust.
//!
//! ## Features
//!
//! - **Pluggable Message Sources**: Implement `MessageSource` to receive messages from any backend
//! - **Pluggable Storage**: Implement `MessageStorage` for message replay on reconnection
//! - **Channel-based Routing**: Route messages to specific channels or broadcast to all
//! - **Built-in Server**: Optional Axum-based HTTP server with SSE endpoint
//! - **Memory Efficient**: Designed for high-concurrency with minimal memory footprint
//! - **Flexible Authentication**: Support for custom auth callbacks with channel-level permissions
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use sse_gateway::{Gateway, MemoryStorage, NoopSource};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     Gateway::builder()
//!         .port(8080)
//!         .source(NoopSource)
//!         .storage(MemoryStorage::default())
//!         .build()?
//!         .run()
//!         .await
//! }
//! ```
//!
//! ## With Authentication
//!
//! ```rust,ignore
//! use sse_gateway::{Gateway, MemoryStorage, NoopSource};
//! use sse_gateway::auth::{AuthRequest, deny};
//! use axum::http::StatusCode;
//!
//! Gateway::builder()
//!     .port(8080)
//!     .source(NoopSource)
//!     .storage(MemoryStorage::default())
//!     .auth(|req: AuthRequest| async move {
//!         // Return None to allow, Some(response) to deny
//!         match req.bearer_token() {
//!             Some(token) if token == "secret" => None,
//!             _ => Some(deny(StatusCode::UNAUTHORIZED, "Invalid token")),
//!         }
//!     })
//!     .build()?
//!     .run()
//!     .await
//! ```
//!
//! ## Custom Message Source
//!
//! ```rust,ignore
//! use sse_gateway::{MessageSource, MessageHandler, IncomingMessage};
//! use async_trait::async_trait;
//! use tokio_util::sync::CancellationToken;
//!
//! struct MySource;
//!
//! #[async_trait]
//! impl MessageSource for MySource {
//!     async fn start(&self, handler: MessageHandler, cancel: CancellationToken) -> anyhow::Result<()> {
//!         // Your message receiving logic here
//!         loop {
//!             tokio::select! {
//!                 _ = cancel.cancelled() => break,
//!                 // msg = your_receiver.recv() => { handler(msg.into()); }
//!             }
//!         }
//!         Ok(())
//!     }
//!     
//!     fn name(&self) -> &'static str { "MySource" }
//! }
//! ```

pub mod auth;
mod connection;
mod error;
mod event;
mod manager;
pub mod source;
pub mod storage;

#[cfg(feature = "server")]
mod gateway;
#[cfg(feature = "server")]
mod handler;

// Re-exports
pub use connection::{SseConnection, ConnectionMetadata};
pub use error::{Error, Result};
pub use event::{SseEvent, EventData};
pub use manager::ConnectionManager;
pub use source::{MessageSource, MessageHandler, IncomingMessage, NoopSource, ChannelSource};
pub use storage::{MessageStorage, MemoryStorage, NoopStorage};

#[cfg(feature = "server")]
pub use gateway::{Gateway, GatewayBuilder};

// Re-export commonly used types from dependencies
pub use async_trait::async_trait;
pub use tokio_util::sync::CancellationToken;
