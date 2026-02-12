# SSE Gateway API

## Features

- Channel-based message routing via `channel_id`
- **Message Replay**: Automatically resend missed messages on reconnection (requires Redis)
- **Service Discovery**: Gateway instances register to Redis with heartbeat
- **Channel Routing**: Channel → Instance mapping for direct push
- Multi-instance deployment support
- Real-time Server-Sent Events (SSE)

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                            Redis                                     │
│  ┌─────────────────────┐  ┌─────────────────────────────────────┐  │
│  │ gateway:instances   │  │ channel:{id}:instance               │  │
│  │ gateway:instance:{} │  │ (channel → instance mapping)        │  │
│  └─────────────────────┘  └─────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────┘
         ▲                              ▲
         │ heartbeat                    │ register/unregister
         │                              │
┌────────┴──────────────────────────────┴─────────────────────────────┐
│                       Gateway Instance                               │
│  ┌──────────────────────┐     ┌──────────────────────────────────┐ │
│  │  SSE Server (:8080)  │     │  Push API Server (:9000)         │ │
│  │  /sse/connect        │     │  POST /push, /store              │ │
│  │  /dashboard          │     │  GET  /channel/{id}, /instances  │ │
│  └──────────────────────┘     └──────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Endpoints

### SSE Server (Default Port: 8080)

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/sse/connect?channel_id={id}` | GET | Connect to SSE stream for a specific channel |
| `/health` | GET | Health check endpoint |
| `/dashboard` | GET | Web dashboard (if enabled) |

### Push API Server (Default Port: 9000)

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/push` | POST | Push message to channel (real-time + store) |
| `/store` | POST | Store message for offline user only |
| `/channel/{id}` | GET | Query channel status and routing info |
| `/instances` | GET | List all gateway instances |
| `/channels` | GET | List all channel → instance mappings |

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

## Backend: Send Messages (Push API)

Messages are sent via HTTP POST to the Push API server.

### POST /push - Push Message

Push a message to connected clients. The message is sent in real-time AND stored for replay.

**Request:**

```json
{
  "channel_id": "my-channel",
  "event_type": "notification",
  "data": {"msg": "hello", "timestamp": 1234567890}
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `channel_id` | string | No | Target channel. If omitted, message is broadcast to all |
| `event_type` | string | Yes | Event type for frontend `addEventListener()` |
| `data` | any | Yes | Message payload (JSON) |

**Response:**

```json
{
  "success": true,
  "online": true,
  "stream_id": "1234567890-0"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `success` | boolean | Whether the message was queued |
| `online` | boolean | Whether the channel has active connections |
| `stream_id` | string | Message ID for replay tracking |

**Examples:**

```bash
# Send to specific channel
curl -X POST http://localhost:9000/push \
  -H "Content-Type: application/json" \
  -d '{"channel_id":"user-123","event_type":"notification","data":{"msg":"hello"}}'

# Broadcast to all (no channel_id)
curl -X POST http://localhost:9000/push \
  -H "Content-Type: application/json" \
  -d '{"event_type":"announcement","data":{"msg":"broadcast"}}'
```

### POST /store - Store for Offline User

Store a message for later replay. Does NOT push to connected clients.

**Request:**

```json
{
  "channel_id": "my-channel",
  "event_type": "notification",
  "data": {"msg": "stored message"}
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `channel_id` | string | Yes | Target channel (required) |
| `event_type` | string | Yes | Event type |
| `data` | any | Yes | Message payload |

**Response:**

```json
{
  "success": true,
  "stream_id": "1234567890-0"
}
```

**Example:**

```bash
curl -X POST http://localhost:9000/store \
  -H "Content-Type: application/json" \
  -d '{"channel_id":"user-123","event_type":"notification","data":{"msg":"offline msg"}}'
```

### GET /channel/{id} - Query Channel Status

Query the status of a channel, including which gateway instance handles it.

**Response:**

```json
{
  "channel_id": "my-channel",
  "online": true,
  "instance_id": "gateway-abc123",
  "instance_address": "10.0.0.5:9000"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `channel_id` | string | The queried channel ID |
| `online` | boolean | Whether the channel has an active connection |
| `instance_id` | string | Gateway instance ID handling this channel |
| `instance_address` | string | Address to push messages directly |

**Example:**

```bash
curl http://localhost:9000/channel/user-123
```

### GET /instances - List Gateway Instances

List all registered gateway instances (for monitoring/debugging).

**Response:**

```json
{
  "instances": [
    {
      "id": "gateway-abc123",
      "address": "10.0.0.5:9000",
      "last_seen": 1234567890
    }
  ],
  "count": 1
}
```

**Example:**

```bash
curl http://localhost:9000/instances
```

### GET /channels - List Channel Mappings

List all channel → instance mappings (for monitoring/debugging).

**Response:**

```json
{
  "user-123": "gateway-abc123",
  "user-456": "gateway-def456"
}
```

**Example:**

```bash
curl http://localhost:9000/channels
```

---

## SDK Examples

### Python

```python
import requests

PUSH_URL = "http://localhost:9000"

# Push message (real-time + store)
response = requests.post(f"{PUSH_URL}/push", json={
    "channel_id": "my-channel",
    "event_type": "notification",
    "data": {"msg": "hello"}
})
result = response.json()
print(f"Success: {result['success']}, Online: {result['online']}")

# Store for offline user
requests.post(f"{PUSH_URL}/store", json={
    "channel_id": "my-channel",
    "event_type": "notification",
    "data": {"msg": "offline message"}
})

# Query channel status
status = requests.get(f"{PUSH_URL}/channel/my-channel").json()
if status["online"]:
    print(f"Channel is online at {status['instance_address']}")
```

### Node.js

```javascript
const PUSH_URL = 'http://localhost:9000';

// Push message
const response = await fetch(`${PUSH_URL}/push`, {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    channel_id: 'my-channel',
    event_type: 'notification',
    data: { msg: 'hello' }
  })
});
const result = await response.json();
console.log(`Success: ${result.success}, Online: ${result.online}`);

// Query channel status
const status = await fetch(`${PUSH_URL}/channel/my-channel`).then(r => r.json());
if (status.online) {
  console.log(`Channel is online at ${status.instance_address}`);
}
```

### Go

```go
import (
    "bytes"
    "encoding/json"
    "net/http"
)

type PushPayload struct {
    ChannelID string      `json:"channel_id,omitempty"`
    EventType string      `json:"event_type"`
    Data      interface{} `json:"data"`
}

type PushResponse struct {
    Success  bool   `json:"success"`
    Online   bool   `json:"online"`
    StreamID string `json:"stream_id"`
}

func pushMessage(channelID, eventType string, data interface{}) (*PushResponse, error) {
    payload := PushPayload{
        ChannelID: channelID,
        EventType: eventType,
        Data:      data,
    }
    body, _ := json.Marshal(payload)
    
    resp, err := http.Post("http://localhost:9000/push", "application/json", bytes.NewReader(body))
    if err != nil {
        return nil, err
    }
    defer resp.Body.Close()
    
    var result PushResponse
    json.NewDecoder(resp.Body).Decode(&result)
    return &result, nil
}
```

### Rust

```rust
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct PushPayload<T> {
    channel_id: Option<String>,
    event_type: String,
    data: T,
}

#[derive(Deserialize)]
struct PushResponse {
    success: bool,
    online: bool,
    stream_id: String,
}

async fn push_message<T: Serialize>(
    client: &Client,
    channel_id: Option<String>,
    event_type: &str,
    data: T,
) -> Result<PushResponse, reqwest::Error> {
    let payload = PushPayload {
        channel_id,
        event_type: event_type.to_string(),
        data,
    };
    
    client
        .post("http://localhost:9000/push")
        .json(&payload)
        .send()
        .await?
        .json()
        .await
}
```

### cURL

```bash
# Push to a specific channel
curl -X POST http://localhost:9000/push \
  -H "Content-Type: application/json" \
  -d '{"channel_id":"my-channel","event_type":"notification","data":{"msg":"hello"}}'

# Broadcast to all connections
curl -X POST http://localhost:9000/push \
  -H "Content-Type: application/json" \
  -d '{"event_type":"announcement","data":{"msg":"broadcast"}}'

# Store for offline user
curl -X POST http://localhost:9000/store \
  -H "Content-Type: application/json" \
  -d '{"channel_id":"my-channel","event_type":"notification","data":{"msg":"offline"}}'

# Query channel status
curl http://localhost:9000/channel/my-channel

# List all instances
curl http://localhost:9000/instances

# List all channel mappings
curl http://localhost:9000/channels
```

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
| `PORT` | SSE server port | `8080` |
| `PUSH_PORT` | Push API server port | `9000` |
| `REDIS_URL` | Redis connection URL | `redis://localhost:6379` |
| `INSTANCE_ID` | Unique instance identifier | `$HOSTNAME` or UUID |
| `GATEWAY_ADDR` | Instance address for service discovery | `localhost:$PUSH_PORT` |
| `CHANNEL_TTL` | Channel mapping TTL (seconds) | `60` |
| `ENABLE_DASHBOARD` | Enable web dashboard | `true` |
| `RUST_LOG` | Log level | `info` |

---

## Redis Keys

| Key Pattern | Type | Description |
|-------------|------|-------------|
| `gateway:instances` | SET | All active instance IDs |
| `gateway:instance:{id}` | HASH | Instance details (address, last_seen) |
| `channel:{channel_id}:instance` | STRING | Channel → Instance ID mapping |
