# sse-gateway-redis

Redis adapters for SSE Gateway - Pub/Sub message source and Streams storage for message replay.

## Features

- **RedisPubSubSource**: Receive messages from Redis Pub/Sub
- **RedisStorage**: Store messages in Redis Streams for replay on reconnection
- Pattern-based channel subscription
- Automatic message parsing (JSON and plain text)

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
sse-gateway = "0.1"
sse-gateway-redis = "0.1"
```

## Components

### RedisPubSubSource

Receives messages from Redis Pub/Sub channels.

```rust
use sse_gateway::Gateway;
use sse_gateway_redis::RedisPubSubSource;

let gateway = Gateway::builder()
    .source(RedisPubSubSource::with_defaults("redis://localhost:6379"))
    .build()?;
```

#### Channel Naming Convention

- `sse:{channel_id}` - Send to specific channel (e.g., `sse:user123`)
- `sse:broadcast` - Broadcast to all connections

#### Message Format

Messages can be plain text or JSON:

```json
{
  "event_type": "notification",
  "data": {"text": "Hello!"},
  "channel_id": "user123",
  "id": "msg-001"
}
```

If plain text, it's sent as a `message` event type.

### RedisStorage

Stores messages in Redis Streams for replay when clients reconnect.

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
// Custom max messages per channel (default: 100)
let storage = RedisStorage::with_max_messages(500);
```

#### Storage Keys

Messages are stored in Redis Streams with keys: `sse:stream:{channel_id}`

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
    let gateway = Gateway::builder()
        .source(RedisPubSubSource::with_defaults(redis_url))
        .storage(storage)
        .build()?;

    gateway.run().await
}
```

### Custom Patterns

```rust
// Subscribe to multiple patterns
let source = RedisPubSubSource::new(
    "redis://localhost:6379",
    vec![
        "sse:*".to_string(),        // All SSE channels
        "notifications:*".to_string(), // Custom pattern
    ]
);
```

## Publishing Messages

### redis-cli

```bash
# Send to specific channel
redis-cli PUBLISH sse:user123 '{"event_type":"notification","data":{"text":"Hello!"}}'

# Broadcast to all
redis-cli PUBLISH sse:broadcast '{"event_type":"announcement","data":"Server maintenance in 5 minutes"}'

# Plain text message
redis-cli PUBLISH sse:user123 "Simple text message"
```

### Python

```python
import redis
import json

r = redis.Redis()

# JSON message
r.publish('sse:user123', json.dumps({
    'event_type': 'notification',
    'data': {'text': 'Hello!'},
    'id': 'msg-001'
}))

# Broadcast
r.publish('sse:broadcast', json.dumps({
    'event_type': 'announcement',
    'data': 'Hello everyone!'
}))
```

### Node.js

```javascript
const Redis = require('ioredis');
const redis = new Redis();

// Send to channel
await redis.publish('sse:user123', JSON.stringify({
  event_type: 'notification',
  data: { text: 'Hello!' }
}));

// Broadcast
await redis.publish('sse:broadcast', JSON.stringify({
  event_type: 'announcement',
  data: 'Hello everyone!'
}));
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

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `REDIS_URL` | Redis connection URL | Required |

## License

MIT OR Apache-2.0
