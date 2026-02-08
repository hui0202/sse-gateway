//! Redis adapters for SSE Gateway
//!
//! This crate provides:
//! - `RedisPubSubSource`: Receive messages from Redis Pub/Sub
//! - `RedisStorage`: Store messages in Redis Streams for replay

mod pubsub;
mod storage;

pub use pubsub::RedisPubSubSource;
pub use storage::RedisStorage;
