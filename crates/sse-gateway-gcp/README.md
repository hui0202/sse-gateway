# sse-gateway-gcp

Google Cloud Pub/Sub adapter for SSE Gateway.

## Features

- Receive messages from Google Cloud Pub/Sub
- Automatic message acknowledgment
- Channel-based routing via message attributes
- Broadcast support (messages without `channel_id`)

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
sse-gateway = "0.1"
sse-gateway-gcp = "0.1"
```

## Usage

```rust
use sse_gateway::Gateway;
use sse_gateway_gcp::GcpPubSubSource;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let gateway = Gateway::builder()
        .source(GcpPubSubSource::new("my-project", "my-subscription"))
        .build()?;

    gateway.run().await
}
```

## Message Attributes

When publishing messages to Pub/Sub, use these attributes:

| Attribute | Required | Description |
|-----------|----------|-------------|
| `channel_id` | No | Target channel. If omitted, message is broadcast to all connections |
| `event_type` | No | SSE event type (default: `message`) |
| `id` | No | Business message ID for deduplication |

## Publishing Messages

### gcloud CLI

```bash
# Send to specific channel
gcloud pubsub topics publish my-topic \
  --message='{"text": "Hello!"}' \
  --attribute=channel_id=user123,event_type=notification

# Broadcast to all connections
gcloud pubsub topics publish my-topic \
  --message='{"text": "Announcement"}' \
  --attribute=event_type=announcement
```

### Python

```python
from google.cloud import pubsub_v1

publisher = pubsub_v1.PublisherClient()
topic = 'projects/my-project/topics/my-topic'

publisher.publish(
    topic,
    b'{"text": "Hello!"}',
    channel_id='user123',
    event_type='notification'
)
```

### Node.js

```javascript
const { PubSub } = require('@google-cloud/pubsub');
const pubsub = new PubSub();

await pubsub.topic('my-topic').publishMessage({
  data: Buffer.from(JSON.stringify({ text: 'Hello!' })),
  attributes: { channel_id: 'user123', event_type: 'notification' }
});
```

## Authentication

The adapter uses Google Cloud's default authentication. Ensure one of:

- `GOOGLE_APPLICATION_CREDENTIALS` environment variable is set
- Running on GCP (Cloud Run, GCE, GKE) with appropriate service account
- `gcloud auth application-default login` for local development

## License

MIT OR Apache-2.0
