# sse-gateway-gcp

Google Cloud Pub/Sub adapter for SSE Gateway.

## Features

- Receive messages from Google Cloud Pub/Sub subscriptions
- Automatic message acknowledgment
- Channel-based routing via message attributes
- Broadcast support (messages without `channel_id` attribute)

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
sse-gateway = "0.1"
sse-gateway-gcp = "0.1"
```

## Usage

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

## Message Attributes

When publishing messages to Pub/Sub, use these attributes to control routing:

| Attribute | Required | Description |
|-----------|----------|-------------|
| `channel_id` | No | Target SSE channel. If omitted, message is broadcast to all connections |
| `event_type` | No | SSE event type (default: `message`) |
| `id` | No | Business message ID for client-side deduplication |

## Publishing Messages

### gcloud CLI

```bash
# Send to specific channel
gcloud pubsub topics publish my-topic \
  --message='{"text": "Hello!"}' \
  --attribute=channel_id=user123,event_type=notification

# Broadcast to all connections (no channel_id)
gcloud pubsub topics publish my-topic \
  --message='{"text": "Announcement"}' \
  --attribute=event_type=announcement

# With custom message ID
gcloud pubsub topics publish my-topic \
  --message='{"text": "Important update"}' \
  --attribute=channel_id=user123,event_type=update,id=msg-001
```

### Python

```python
from google.cloud import pubsub_v1

publisher = pubsub_v1.PublisherClient()
topic = 'projects/my-project/topics/my-topic'

# Send to specific channel
publisher.publish(
    topic,
    b'{"text": "Hello!"}',
    channel_id='user123',
    event_type='notification'
)

# Broadcast to all connections
publisher.publish(
    topic,
    b'{"text": "Announcement"}',
    event_type='announcement'
)
```

### Node.js

```javascript
const { PubSub } = require('@google-cloud/pubsub');
const pubsub = new PubSub();

// Send to specific channel
await pubsub.topic('my-topic').publishMessage({
  data: Buffer.from(JSON.stringify({ text: 'Hello!' })),
  attributes: { channel_id: 'user123', event_type: 'notification' }
});

// Broadcast to all connections
await pubsub.topic('my-topic').publishMessage({
  data: Buffer.from(JSON.stringify({ text: 'Announcement' })),
  attributes: { event_type: 'announcement' }
});
```

### Go

```go
import (
    "cloud.google.com/go/pubsub"
    "context"
)

client, _ := pubsub.NewClient(ctx, "my-project")
topic := client.Topic("my-topic")

// Send to specific channel
result := topic.Publish(ctx, &pubsub.Message{
    Data: []byte(`{"text": "Hello!"}`),
    Attributes: map[string]string{
        "channel_id": "user123",
        "event_type": "notification",
    },
})
```

## Authentication

The adapter uses Google Cloud's default authentication chain. Ensure one of the following:

| Method | Description |
|--------|-------------|
| Service Account Key | Set `GOOGLE_APPLICATION_CREDENTIALS` to path of key file |
| Workload Identity | Running on GKE with Workload Identity configured |
| Default Service Account | Running on GCP (Cloud Run, GCE, GKE) with appropriate IAM roles |
| User Credentials | Run `gcloud auth application-default login` for local development |

### Required IAM Permissions

The service account needs the following permissions:
- `pubsub.subscriptions.consume` - to receive messages
- `pubsub.subscriptions.get` - to get subscription info

Or use the predefined role: `roles/pubsub.subscriber`

## Architecture

```
┌─────────────┐     ┌─────────────────────────────────────────────┐
│   Client    │────▶│              SSE Gateway                    │
│ (Browser)   │◀────│                                             │
└─────────────┘     │  ┌──────────────────────────────────────┐   │
                    │  │          GcpPubSubSource              │   │
                    │  │  (pulls from subscription)            │   │
                    │  └──────────────────┬───────────────────┘   │
                    └─────────────────────┼───────────────────────┘
                                          │
                    ┌─────────────────────▼───────────────────────┐
                    │           Google Cloud Pub/Sub               │
                    │                                              │
                    │   Topic ──▶ Subscription ──▶ Gateway        │
                    │                                              │
                    │   Publishers push messages to topic          │
                    └──────────────────────────────────────────────┘
```

## Full Example

```rust
use sse_gateway::{Gateway, MemoryStorage};
use sse_gateway::auth::{AuthRequest, deny};
use sse_gateway_gcp::GcpPubSubSource;
use axum::http::StatusCode;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    Gateway::builder()
        .port(8080)
        .source(GcpPubSubSource::new("my-project", "my-subscription"))
        .storage(MemoryStorage::default())
        .auth(|req: AuthRequest| async move {
            match req.bearer_token() {
                Some(token) if validate_token(token) => None,
                _ => Some(deny(StatusCode::UNAUTHORIZED, "Invalid token")),
            }
        })
        .build()?
        .run()
        .await
}

fn validate_token(token: &str) -> bool {
    // Your token validation logic
    token == "valid-token"
}
```

## Tips

1. **Subscription Mode**: The adapter uses streaming pull, which is efficient for high-throughput scenarios.

2. **Message Acknowledgment**: Messages are automatically acknowledged after being processed and sent to SSE clients.

3. **Error Handling**: Failed messages are not acknowledged and will be redelivered by Pub/Sub according to your subscription's retry policy.

4. **Scaling**: For high availability, deploy multiple gateway instances with the same subscription - Pub/Sub will distribute messages across instances.

## License

MIT OR Apache-2.0
