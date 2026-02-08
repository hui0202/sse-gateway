# SSE Gateway

基于 Rust + Axum 的 SSE 实时消息推送网关，通过 Google Cloud Pub/Sub 接收消息并推送给客户端。

## 架构

```
┌──────────┐      SSE       ┌─────────┐   StreamingPull   ┌─────────┐
│ Frontend │ ◄───────────── │ Gateway │ ◄──────────────── │ Pub/Sub │
└──────────┘                └─────────┘                   └─────────┘
                                                               ▲
                                                               │ Publish
                                                          ┌─────────┐
                                                          │ Backend │
                                                          └─────────┘
```

## 快速开始

### 1. 客户端连接

```javascript
const sse = new EventSource(
  'https://your-service.run.app/sse/connect?channel_id=my-channel'
);

sse.addEventListener('notification', (e) => {
  console.log('收到消息:', JSON.parse(e.data));
});
```

### 2. 后端推送消息

```python
from google.cloud import pubsub_v1
import json

publisher = pubsub_v1.PublisherClient()
topic = publisher.topic_path("your-project", "gateway-subscription")

publisher.publish(
    topic,
    data=json.dumps({"msg": "Hello!"}).encode(),
    channel_id="my-channel",      # 目标频道
    event_type="notification"     # 事件类型
)
```

## API 文档

详见 [API.md](./API.md)

## 配置

```yaml
server:
  port: 8080

pubsub:
  project_id: "your-project"
  subscription_id: "gateway-subscription"
```

## 本地开发

```bash
# 测试模式（不需要真实 Pub/Sub）
TEST_MODE=1 cargo run

# 打开 http://localhost:8080/test 进行测试
```

## 部署

```bash
# 使用 Cloud Build 构建并部署
gcloud builds submit --tag gcr.io/PROJECT_ID/gateway-sse .

gcloud run deploy gateway-sse \
  --image gcr.io/PROJECT_ID/gateway-sse \
  --region asia-east1 \
  --allow-unauthenticated \
  --min-instances 0 \
  --timeout 3600s \
  --set-env-vars "GCP_PROJECT=PROJECT_ID,PUBSUB_SUBSCRIPTION=gateway-subscription-sub"
```

## 测试

使用 `test.html` 进行可视化测试。

## 特性

- ✅ 基于 channel_id 的消息路由
- ✅ 自动心跳保活（30秒）
- ✅ 连接断开自动清理
- ✅ 支持广播和定向推送
- ✅ Cloud Run 自动扩缩容
