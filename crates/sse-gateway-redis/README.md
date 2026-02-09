# sse-gateway-redis

Redis adapters for SSE Gateway - Pub/Sub message source and Streams storage for message replay.

## Features

- **RedisPubSubSource**: Receive messages from Redis Pub/Sub with pattern subscription
- **RedisStorage**: Store messages in Redis Streams with batching for high throughput
- Automatic message cleanup with TTL and MAXLEN
- High-performance batch writes

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
sse-gateway = "0.1"
sse-gateway-redis = "0.1"
```

## Components

### RedisPubSubSource

Receives messages from Redis Pub/Sub channels using pattern subscription.

```rust
use sse_gateway::Gateway;
use sse_gateway_redis::RedisPubSubSource;

let gateway = Gateway::builder()
    .source(RedisPubSubSource::with_defaults("redis://localhost:6379"))
    .storage(sse_gateway::MemoryStorage::default())
    .build()?;
```

#### Default Behavior

By default, `with_defaults()` subscribes to pattern `*` (all channels).

The Redis channel name is used directly as the SSE `channel_id`. For example:
- Publishing to Redis channel `user123` → delivered to SSE channel `user123`
- Publishing to Redis channel `notifications` → delivered to SSE channel `notifications`

#### Custom Patterns

```rust
// Subscribe to specific patterns
let source = RedisPubSubSource::new(
    "redis://localhost:6379",
    vec![
        "sse:*".to_string(),         // Match channels starting with "sse:"
        "notifications:*".to_string(), // Match channels starting with "notifications:"
    ]
);
```

### RedisStorage

Stores messages in Redis Streams for replay when clients reconnect with `Last-Event-ID`.

```rust
use sse_gateway::Gateway;
use sse_gateway_redis::{RedisPubSubSource, RedisStorage};

let storage = RedisStorage::new();
storage.connect("redis://localhost:6379").await?;

let gateway = Gateway::builder()
    .source(RedisPubSubSource::with_defaults("redis://localhost:6379"))
    .storage(storage)
    .build()?;
```

#### Configuration

```rust
// Default: 100 messages per channel, 1 hour TTL
let storage = RedisStorage::new();

// Custom max messages per channel (default TTL: 1 hour)
let storage = RedisStorage::with_max_messages(500);

// Custom max messages and TTL
let storage = RedisStorage::with_options(
    500,   // max messages per channel
    7200,  // TTL in seconds (2 hours)
);
```

#### Storage Keys

Messages are stored in Redis Streams with keys: `sse:stream:{channel_id}`

#### Batching

RedisStorage uses internal batching to improve throughput:
- Batch size: 100 messages
- Flush interval: 10ms
- Non-blocking writes (fire-and-forget)

## Usage Examples

### Full Example with Both Components

```rust
use sse_gateway::Gateway;
use sse_gateway_redis::{RedisPubSubSource, RedisStorage};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let redis_url = "redis://localhost:6379";

    // Setup storage for message replay
    let storage = RedisStorage::new();
    storage.connect(redis_url).await?;

    // Build gateway
    Gateway::builder()
        .port(8080)
        .source(RedisPubSubSource::with_defaults(redis_url))
        .storage(storage)
        .build()?
        .run()
        .await
}
```

## Publishing Messages

### redis-cli

```bash
# Send to specific channel (channel name = SSE channel_id)
redis-cli PUBLISH user123 '{"text": "Hello!"}'

# Send to another channel
redis-cli PUBLISH notifications 'New notification'

# Plain text message
redis-cli PUBLISH user123 "Simple text message"
```

### Python

```python
import redis
import json

r = redis.Redis()

# Send to channel "user123"
r.publish('user123', json.dumps({
    'text': 'Hello!'
}))

# Send to channel "notifications"
r.publish('notifications', 'New notification!')
```

### Node.js

```javascript
const Redis = require('ioredis');
const redis = new Redis();

// Send to channel "user123"
await redis.publish('user123', JSON.stringify({
  text: 'Hello!'
}));

// Send to channel "notifications"
await redis.publish('notifications', 'New notification!');
```

## Message Replay

When a client reconnects with `Last-Event-ID`, the gateway automatically replays missed messages from Redis Streams:

```javascript
// Browser automatically sends Last-Event-ID on reconnection
const sse = new EventSource('/sse/connect?channel_id=user123');

sse.onmessage = (e) => {
  console.log('ID:', e.lastEventId);
  console.log('Data:', e.data);
};
```

## Architecture

```
┌─────────────┐     ┌─────────────────────────────────────────────┐
│   Client    │────▶│              SSE Gateway                    │
│ (Browser)   │◀────│                                             │
└─────────────┘     │  ┌─────────────────┐  ┌──────────────────┐  │
                    │  │ RedisPubSubSource│  │   RedisStorage   │  │
                    │  │  (subscribe *)   │  │  (Redis Streams) │  │
                    │  └────────┬─────────┘  └────────┬─────────┘  │
                    └───────────┼─────────────────────┼────────────┘
                                │                     │
                    ┌───────────▼─────────────────────▼───────────┐
                    │                  Redis                       │
                    │   Pub/Sub channels    │    Streams          │
                    │   (real-time push)    │    (persistence)    │
                    └─────────────────────────────────────────────┘
```

## License

MIT OR Apache-2.0
