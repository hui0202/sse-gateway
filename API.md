# SSE Gateway API

**服务地址**: `https://gateway-sse-912773111852.asia-east1.run.app`

## 特性

- 基于 channel_id 的消息路由
- **消息重放**：断线重连自动补发错过的消息（需配置 Redis）
- 多实例部署支持

---

## 前端：连接与接收消息

### JavaScript / TypeScript

```javascript
// 连接指定 channel
const sse = new EventSource('https://gateway-sse-912773111852.asia-east1.run.app/sse/connect?channel_id=my-channel');

// 收消息
sse.addEventListener('notification', (e) => {
  const data = JSON.parse(e.data);
  console.log('收到:', data);
});

// 连接状态
sse.onopen = () => console.log('已连接');
sse.onerror = () => console.log('连接断开');

// 断开连接
sse.close();
```

### React Hook

```typescript
function useSSE(channelId: string) {
  const [messages, setMessages] = useState<any[]>([]);

  useEffect(() => {
    const sse = new EventSource(
      `https://gateway-sse-912773111852.asia-east1.run.app/sse/connect?channel_id=${channelId}`
    );

    sse.addEventListener('notification', (e) => {
      setMessages(prev => [...prev, JSON.parse(e.data)]);
    });

    return () => sse.close();
  }, [channelId]);

  return messages;
}

// 使用
const messages = useSSE('my-channel');
```

---

## 后端：发送消息

### Python

```python
from google.cloud import pubsub_v1

publisher = pubsub_v1.PublisherClient()
topic = 'projects/hui-test-486414/topics/gateway-subscription'

# 发送到指定 channel
publisher.publish(topic, b'{"msg": "hello"}', channel_id='my-channel', event_type='notification')

# 广播（不带 channel_id）
publisher.publish(topic, b'{"msg": "broadcast"}', event_type='announcement')
```

### Node.js

```javascript
const { PubSub } = require('@google-cloud/pubsub');
const pubsub = new PubSub({ projectId: 'hui-test-486414' });

// 发送到指定 channel
await pubsub.topic('gateway-subscription').publishMessage({
  data: Buffer.from(JSON.stringify({ msg: 'hello' })),
  attributes: { channel_id: 'my-channel', event_type: 'notification' }
});
```

### Go

```go
client, _ := pubsub.NewClient(ctx, "hui-test-486414")

client.Topic("gateway-subscription").Publish(ctx, &pubsub.Message{
    Data:       []byte(`{"msg": "hello"}`),
    Attributes: map[string]string{"channel_id": "my-channel", "event_type": "notification"},
})
```

### gcloud CLI

```bash
gcloud pubsub topics publish gateway-subscription \
  --message='{"msg":"hello"}' \
  --attribute=channel_id=my-channel,event_type=notification
```

---

## 消息属性

| 属性 | 说明 |
|------|------|
| `channel_id` | 目标频道。不填 = 广播给所有连接 |
| `event_type` | 事件类型，前端用 `addEventListener(event_type, ...)` 监听 |

---

## 事件类型

前端监听对应的 `event_type`：

```javascript
sse.addEventListener('notification', (e) => { /* 通知 */ });
sse.addEventListener('message', (e) => { /* 消息 */ });
sse.addEventListener('update', (e) => { /* 更新 */ });
sse.addEventListener('heartbeat', (e) => { /* 心跳，每30秒 */ });
```
