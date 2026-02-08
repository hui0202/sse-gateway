# SSE Gateway API

## Features

- Channel-based message routing via `channel_id`
- **Message Replay**: Automatically resend missed messages on reconnection (requires Redis)
- Multi-instance deployment support
- Real-time Server-Sent Events (SSE)

---

## Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/sse/connect?channel_id={id}` | GET | Connect to SSE stream for a specific channel |
| `/health` | GET | Health check endpoint |
| `/dashboard` | GET | Web dashboard (if enabled) |

---

## Frontend: Connect & Receive Messages

### Basic Usage

```javascript
const SERVICE_URL = 'https://your-service-url.run.app';

// Connect to a specific channel
const sse = new EventSource(`${SERVICE_URL}/sse/connect?channel_id=my-channel`);

// Receive messages
sse.addEventListener('notification', (e) => {
  const data = JSON.parse(e.data);
  console.log('Received:', data);
});

// Connection status
sse.onopen = () => console.log('Connected');
sse.onerror = () => console.log('Disconnected');

// Disconnect
sse.close();
```

### Complete Example

```html
<!DOCTYPE html>
<html>
<head>
  <title>SSE Client</title>
</head>
<body>
  <div id="status">Disconnected</div>
  <div id="messages"></div>

  <script>
    const SERVICE_URL = 'https://your-service-url.run.app';
    const channelId = 'my-channel';
    
    const statusEl = document.getElementById('status');
    const messagesEl = document.getElementById('messages');
    
    // Connect
    const sse = new EventSource(`${SERVICE_URL}/sse/connect?channel_id=${channelId}`);
    
    sse.onopen = () => {
      statusEl.textContent = 'Connected';
      statusEl.style.color = 'green';
    };
    
    sse.onerror = () => {
      statusEl.textContent = 'Disconnected';
      statusEl.style.color = 'red';
    };
    
    // Listen for notifications
    sse.addEventListener('notification', (e) => {
      const data = JSON.parse(e.data);
      const div = document.createElement('div');
      div.textContent = JSON.stringify(data);
      messagesEl.appendChild(div);
    });
    
    // Listen for all message types
    sse.onmessage = (e) => {
      console.log('Message:', e.data);
    };
  </script>
</body>
</html>
```

---

## Backend: Send Messages

Messages are sent via Google Cloud Pub/Sub. The gateway subscribes to the topic and broadcasts messages to connected clients.

### Python

```python
from google.cloud import pubsub_v1

publisher = pubsub_v1.PublisherClient()
topic = 'projects/YOUR_PROJECT_ID/topics/gateway-subscription'

# Send to a specific channel
publisher.publish(
    topic,
    b'{"msg": "hello"}',
    channel_id='my-channel',
    event_type='notification'
)

# Broadcast to all connections (no channel_id)
publisher.publish(
    topic,
    b'{"msg": "broadcast to all"}',
    event_type='announcement'
)
```

### Node.js

```javascript
const { PubSub } = require('@google-cloud/pubsub');
const pubsub = new PubSub({ projectId: 'YOUR_PROJECT_ID' });

// Send to a specific channel
await pubsub.topic('gateway-subscription').publishMessage({
  data: Buffer.from(JSON.stringify({ msg: 'hello' })),
  attributes: { channel_id: 'my-channel', event_type: 'notification' }
});

// Broadcast to all
await pubsub.topic('gateway-subscription').publishMessage({
  data: Buffer.from(JSON.stringify({ msg: 'broadcast' })),
  attributes: { event_type: 'announcement' }
});
```

### Go

```go
import "cloud.google.com/go/pubsub"

client, _ := pubsub.NewClient(ctx, "YOUR_PROJECT_ID")

// Send to a specific channel
client.Topic("gateway-subscription").Publish(ctx, &pubsub.Message{
    Data:       []byte(`{"msg": "hello"}`),
    Attributes: map[string]string{
        "channel_id": "my-channel",
        "event_type": "notification",
    },
})
```

### Rust

```rust
use google_cloud_pubsub::client::Client;

let client = Client::default().await?;
let topic = client.topic("gateway-subscription");

topic.publish(PubsubMessage {
    data: r#"{"msg": "hello"}"#.into(),
    attributes: [
        ("channel_id".into(), "my-channel".into()),
        ("event_type".into(), "notification".into()),
    ].into(),
    ..Default::default()
}).await?;
```

### gcloud CLI

```bash
# Send to a specific channel
gcloud pubsub topics publish gateway-subscription \
  --message='{"msg":"hello"}' \
  --attribute=channel_id=my-channel,event_type=notification

# Broadcast to all
gcloud pubsub topics publish gateway-subscription \
  --message='{"msg":"broadcast"}' \
  --attribute=event_type=announcement
```

---

## Message Attributes

| Attribute | Required | Description |
|-----------|----------|-------------|
| `channel_id` | No | Target channel. If omitted, message is broadcast to all connections |
| `event_type` | No | Event type for frontend `addEventListener()`. Default: `message` |

---

## Event Types

The frontend listens to specific event types using `addEventListener()`:

```javascript
// Custom event types
sse.addEventListener('notification', (e) => { /* notifications */ });
sse.addEventListener('message', (e) => { /* messages */ });
sse.addEventListener('update', (e) => { /* updates */ });
sse.addEventListener('alert', (e) => { /* alerts */ });

// System events
sse.addEventListener('heartbeat', (e) => { /* heartbeat every 30s */ });
```

---

## Message Replay (Reconnection)

When Redis is configured, the gateway supports automatic message replay on reconnection:

```javascript
// The browser automatically sends Last-Event-ID on reconnection
// The gateway will replay any missed messages

const sse = new EventSource('/sse/connect?channel_id=my-channel');

// Each message includes an ID for replay tracking
sse.onmessage = (e) => {
  console.log('Message ID:', e.lastEventId);
  console.log('Data:', e.data);
};
```

---

## Error Handling

```javascript
const sse = new EventSource('/sse/connect?channel_id=my-channel');

sse.onerror = (e) => {
  if (sse.readyState === EventSource.CONNECTING) {
    console.log('Reconnecting...');
  } else if (sse.readyState === EventSource.CLOSED) {
    console.log('Connection closed');
    // Implement custom reconnection logic if needed
  }
};
```

---

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `GCP_PROJECT` | Google Cloud Project ID | Required |
| `PUBSUB_SUBSCRIPTION` | Pub/Sub subscription name | Required |
| `REDIS_URL` | Redis URL for message replay | Optional |
| `ENABLE_DASHBOARD` | Enable web dashboard | `true` |
| `RUST_LOG` | Log level | `info` |
