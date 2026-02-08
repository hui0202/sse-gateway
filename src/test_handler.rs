use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Json},
};
use serde::{Deserialize, Serialize};

use crate::gateway::GatewayState;
use crate::sse::SseEvent;

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    /// channel_id to send to (if empty, broadcast to all)
    pub channel_id: Option<String>,
    /// Event type name
    pub event_type: String,
    /// Event data (JSON)
    pub data: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct SendMessageResponse {
    pub success: bool,
    pub sent_count: usize,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct StatsResponse {
    pub total_connections: usize,
    pub connections: Vec<ConnectionInfo>,
}

#[derive(Debug, Serialize)]
pub struct ConnectionInfo {
    pub id: String,
    pub channel_id: String,
    pub connected_at: String,
    pub is_active: bool,
}

/// POST /test/send - 模拟发送消息
pub async fn send_test_message(
    State(state): State<GatewayState>,
    Json(req): Json<SendMessageRequest>,
) -> impl IntoResponse {
    let event = SseEvent::new(&req.event_type, req.data);

    let sent_count = match &req.channel_id {
        Some(channel_id) if !channel_id.is_empty() => {
            // 存储消息用于重放
            state.message_store.store(channel_id, &event).await;
            state.connection_manager.send_to_channel(channel_id, event).await
        }
        _ => state.connection_manager.broadcast(event).await,
    };

    let response = SendMessageResponse {
        success: sent_count > 0,
        sent_count,
        message: format!("Message sent to {} connection(s)", sent_count),
    };

    (StatusCode::OK, Json(response))
}

/// GET /test/stats - 获取连接统计
pub async fn get_stats(State(state): State<GatewayState>) -> impl IntoResponse {
    let connections: Vec<ConnectionInfo> = state
        .connection_manager
        .list_connections()
        .into_iter()
        .map(|c| ConnectionInfo {
            id: c.id.clone(),
            channel_id: c.channel_id.clone(),
            connected_at: c.metadata.connected_at.to_rfc3339(),
            is_active: c.is_active(),
        })
        .collect();

    let response = StatsResponse {
        total_connections: connections.len(),
        connections,
    };

    Json(response)
}

/// GET /dashboard - Dashboard 页面（生产环境可用）
pub async fn dashboard_page() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

const DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html lang="zh">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>SSE Gateway Dashboard</title>
    <style>
        * { box-sizing: border-box; margin: 0; padding: 0; }
        body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; background: #0f172a; color: #e2e8f0; min-height: 100vh; }
        .header { background: linear-gradient(135deg, #1e293b 0%, #334155 100%); padding: 20px 30px; border-bottom: 1px solid #334155; }
        .header h1 { font-size: 24px; font-weight: 600; display: flex; align-items: center; gap: 10px; }
        .header .status-dot { width: 10px; height: 10px; background: #22c55e; border-radius: 50%; animation: pulse 2s infinite; }
        @keyframes pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.5; } }
        .container { max-width: 1400px; margin: 0 auto; padding: 20px; }
        
        /* 统计卡片 */
        .stats-row { display: grid; grid-template-columns: repeat(4, 1fr); gap: 15px; margin-bottom: 20px; }
        .stat-card { background: #1e293b; border-radius: 12px; padding: 20px; border: 1px solid #334155; }
        .stat-card .label { font-size: 12px; color: #94a3b8; text-transform: uppercase; letter-spacing: 0.5px; }
        .stat-card .value { font-size: 36px; font-weight: 700; color: #f8fafc; margin: 8px 0; }
        .stat-card .sub { font-size: 12px; color: #64748b; }
        .stat-card.highlight { background: linear-gradient(135deg, #3b82f6 0%, #1d4ed8 100%); border: none; }
        .stat-card.highlight .label { color: #bfdbfe; }
        .stat-card.highlight .sub { color: #93c5fd; }
        
        /* 主内容区 */
        .main-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 20px; }
        .card { background: #1e293b; border-radius: 12px; border: 1px solid #334155; overflow: hidden; }
        .card-header { padding: 15px 20px; border-bottom: 1px solid #334155; display: flex; justify-content: space-between; align-items: center; }
        .card-header h2 { font-size: 16px; font-weight: 600; }
        .card-body { padding: 20px; }
        
        /* 表单元素 */
        label { display: block; font-size: 12px; color: #94a3b8; margin-bottom: 6px; text-transform: uppercase; letter-spacing: 0.5px; }
        input, textarea, select { width: 100%; padding: 10px 12px; background: #0f172a; border: 1px solid #334155; border-radius: 6px; color: #e2e8f0; font-size: 14px; }
        input:focus, textarea:focus { outline: none; border-color: #3b82f6; }
        textarea { min-height: 80px; font-family: 'Monaco', 'Menlo', monospace; font-size: 13px; resize: vertical; }
        
        /* 按钮 */
        .btn { padding: 10px 20px; border-radius: 6px; font-size: 14px; font-weight: 500; cursor: pointer; border: none; transition: all 0.2s; }
        .btn-primary { background: #3b82f6; color: white; }
        .btn-primary:hover { background: #2563eb; }
        .btn-success { background: #22c55e; color: white; }
        .btn-success:hover { background: #16a34a; }
        .btn-danger { background: #ef4444; color: white; }
        .btn-danger:hover { background: #dc2626; }
        .btn-sm { padding: 6px 12px; font-size: 12px; }
        .btn-group { display: flex; gap: 10px; margin-top: 15px; }
        
        /* 状态 */
        .status-badge { display: inline-flex; align-items: center; gap: 6px; padding: 6px 12px; border-radius: 20px; font-size: 13px; font-weight: 500; }
        .status-badge.connected { background: rgba(34, 197, 94, 0.2); color: #22c55e; }
        .status-badge.disconnected { background: rgba(239, 68, 68, 0.2); color: #ef4444; }
        
        /* 事件日志 */
        .events-container { height: 300px; overflow-y: auto; background: #0f172a; border-radius: 8px; padding: 10px; font-family: 'Monaco', 'Menlo', monospace; font-size: 12px; }
        .event-item { padding: 8px 10px; border-radius: 4px; margin-bottom: 4px; border-left: 3px solid #3b82f6; background: rgba(59, 130, 246, 0.1); }
        .event-item.heartbeat { border-color: #64748b; background: rgba(100, 116, 139, 0.1); opacity: 0.6; }
        .event-item.system { border-color: #22c55e; background: rgba(34, 197, 94, 0.1); }
        .event-item.error { border-color: #ef4444; background: rgba(239, 68, 68, 0.1); }
        .event-time { color: #64748b; margin-right: 8px; }
        .event-type { color: #3b82f6; font-weight: 600; margin-right: 8px; }
        
        /* 连接列表 */
        .conn-list { max-height: 250px; overflow-y: auto; }
        .conn-item { background: #0f172a; border-radius: 8px; padding: 12px; margin-bottom: 8px; display: flex; justify-content: space-between; align-items: center; }
        .conn-item .info { flex: 1; }
        .conn-item .channel { font-weight: 600; color: #f8fafc; margin-bottom: 4px; }
        .conn-item .meta { font-size: 11px; color: #64748b; font-family: monospace; }
        .conn-item .badge { padding: 4px 8px; border-radius: 4px; font-size: 11px; background: #22c55e20; color: #22c55e; }
        
        /* 响应式 */
        @media (max-width: 1024px) {
            .stats-row { grid-template-columns: repeat(2, 1fr); }
            .main-grid { grid-template-columns: 1fr; }
        }
        @media (max-width: 640px) {
            .stats-row { grid-template-columns: 1fr; }
        }
        
        /* 滚动条 */
        ::-webkit-scrollbar { width: 6px; height: 6px; }
        ::-webkit-scrollbar-track { background: #1e293b; }
        ::-webkit-scrollbar-thumb { background: #475569; border-radius: 3px; }
        ::-webkit-scrollbar-thumb:hover { background: #64748b; }
        
        .form-row { margin-bottom: 15px; }
        .result-text { margin-top: 10px; padding: 10px; border-radius: 6px; font-size: 13px; }
        .result-text.success { background: rgba(34, 197, 94, 0.2); color: #22c55e; }
        .result-text.error { background: rgba(239, 68, 68, 0.2); color: #ef4444; }
        
        .auto-refresh { display: flex; align-items: center; gap: 8px; font-size: 12px; color: #64748b; }
        .auto-refresh input { width: auto; }
    </style>
</head>
<body>
    <div class="header">
        <h1><span class="status-dot"></span> SSE Gateway Dashboard</h1>
    </div>
    
    <div class="container">
        <!-- 统计卡片 -->
        <div class="stats-row">
            <div class="stat-card highlight">
                <div class="label">总连接数</div>
                <div class="value" id="totalConnections">0</div>
                <div class="sub">当前活跃的 SSE 连接</div>
            </div>
            <div class="stat-card">
                <div class="label">频道数</div>
                <div class="value" id="channelCount">0</div>
                <div class="sub">不同的 channel_id</div>
            </div>
            <div class="stat-card">
                <div class="label">活跃连接</div>
                <div class="value" id="activeConnections">0</div>
                <div class="sub">响应中的连接</div>
            </div>
            <div class="stat-card">
                <div class="label">上次更新</div>
                <div class="value" id="lastUpdate" style="font-size: 18px;">--:--:--</div>
                <div class="sub auto-refresh">
                    <input type="checkbox" id="autoRefresh" checked>
                    <label for="autoRefresh" style="margin: 0; text-transform: none;">自动刷新 (2s)</label>
                </div>
            </div>
        </div>
        
        <!-- 主内容 -->
        <div class="main-grid">
            <!-- 左侧：SSE 测试 -->
            <div>
                <div class="card">
                    <div class="card-header">
                        <h2>🔌 SSE 连接测试</h2>
                        <span id="connectionStatus" class="status-badge disconnected">未连接</span>
                    </div>
                    <div class="card-body">
                        <div class="form-row">
                            <label>Channel ID</label>
                            <input type="text" id="channelId" value="test-channel" placeholder="输入频道ID">
                        </div>
                        <div class="btn-group">
                            <button class="btn btn-success" onclick="connect()">连接</button>
                            <button class="btn btn-danger" onclick="disconnect()">断开</button>
                        </div>
                    </div>
                </div>
                
                <div class="card" style="margin-top: 20px;">
                    <div class="card-header">
                        <h2>📨 收到的事件</h2>
                        <button class="btn btn-sm btn-primary" onclick="clearEvents()">清空</button>
                    </div>
                    <div class="card-body">
                        <div class="events-container" id="events"></div>
                    </div>
                </div>
            </div>
            
            <!-- 右侧：发送 & 监控 -->
            <div>
                <div class="card">
                    <div class="card-header">
                        <h2>📤 发送测试消息</h2>
                    </div>
                    <div class="card-body">
                        <div class="form-row">
                            <label>目标 Channel ID（留空=广播）</label>
                            <input type="text" id="targetChannelId" placeholder="channel_id">
                        </div>
                        <div class="form-row">
                            <label>事件类型</label>
                            <input type="text" id="eventType" value="notification">
                        </div>
                        <div class="form-row">
                            <label>消息数据 (JSON)</label>
                            <textarea id="eventData">{"message": "Hello!", "ts": 1234567890}</textarea>
                        </div>
                        <button class="btn btn-primary" onclick="sendMessage()">发送消息</button>
                        <div id="sendResult"></div>
                    </div>
                </div>
                
                <div class="card" style="margin-top: 20px;">
                    <div class="card-header">
                        <h2>👥 连接列表</h2>
                        <button class="btn btn-sm btn-primary" onclick="refreshStats()">刷新</button>
                    </div>
                    <div class="card-body">
                        <div class="conn-list" id="connectionsList">
                            <div style="text-align: center; color: #64748b; padding: 20px;">暂无连接</div>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    </div>

    <script>
        let eventSource = null;
        let refreshInterval = null;
        
        // 获取 API 基础路径
        const apiBase = window.location.pathname.includes('/test') ? '/test' : '/api';
        
        function connect() {
            if (eventSource) eventSource.close();
            
            const channelId = document.getElementById('channelId').value;
            if (!channelId) { alert('请输入 Channel ID'); return; }
            
            eventSource = new EventSource(`/sse/connect?channel_id=${encodeURIComponent(channelId)}`);
            
            eventSource.onopen = () => {
                updateStatus(true, channelId);
                addEvent('system', `已连接到 channel: ${channelId}`);
                refreshStats();
            };
            
            eventSource.onerror = () => {
                if (eventSource.readyState === EventSource.CLOSED) {
                    updateStatus(false);
                    addEvent('error', '连接已断开');
                }
            };
            
            // 监听事件
            ['heartbeat', 'notification', 'message', 'update', 'chat'].forEach(type => {
                eventSource.addEventListener(type, (e) => {
                    addEvent(type, e.data, type === 'heartbeat');
                });
            });
            
            eventSource.onmessage = (e) => addEvent('message', e.data);
        }
        
        function disconnect() {
            if (eventSource) {
                eventSource.close();
                eventSource = null;
                updateStatus(false);
                addEvent('system', '已断开连接');
                setTimeout(refreshStats, 500);
            }
        }
        
        function updateStatus(connected, channelId = '') {
            const el = document.getElementById('connectionStatus');
            el.className = 'status-badge ' + (connected ? 'connected' : 'disconnected');
            el.textContent = connected ? `已连接 (${channelId})` : '未连接';
        }
        
        function addEvent(type, data, isHeartbeat = false) {
            const el = document.getElementById('events');
            const time = new Date().toLocaleTimeString();
            const div = document.createElement('div');
            let className = 'event-item';
            if (isHeartbeat) className += ' heartbeat';
            else if (type === 'system') className += ' system';
            else if (type === 'error') className += ' error';
            div.className = className;
            
            let displayData = data;
            try {
                const parsed = JSON.parse(data);
                displayData = JSON.stringify(parsed);
            } catch {}
            
            div.innerHTML = `<span class="event-time">${time}</span><span class="event-type">${type}</span>${displayData}`;
            el.appendChild(div);
            el.scrollTop = el.scrollHeight;
            
            // 限制事件数量
            while (el.children.length > 100) el.removeChild(el.firstChild);
        }
        
        function clearEvents() {
            document.getElementById('events').innerHTML = '';
        }
        
        async function sendMessage() {
            const channelId = document.getElementById('targetChannelId').value;
            const eventType = document.getElementById('eventType').value;
            let data;
            
            try {
                data = JSON.parse(document.getElementById('eventData').value);
            } catch {
                showResult('JSON 格式错误', false);
                return;
            }
            
            try {
                const res = await fetch(`${apiBase}/send`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ channel_id: channelId || null, event_type: eventType, data })
                });
                const result = await res.json();
                showResult(result.message, result.success || result.sent_count > 0);
            } catch (e) {
                showResult('发送失败: ' + e.message, false);
            }
        }
        
        function showResult(msg, success) {
            const el = document.getElementById('sendResult');
            el.className = 'result-text ' + (success ? 'success' : 'error');
            el.textContent = (success ? '✓ ' : '✗ ') + msg;
            setTimeout(() => el.textContent = '', 3000);
        }
        
        async function refreshStats() {
            try {
                const res = await fetch(`${apiBase}/stats`);
                const data = await res.json();
                
                const channels = new Set(data.connections.map(c => c.channel_id));
                const active = data.connections.filter(c => c.is_active).length;
                
                document.getElementById('totalConnections').textContent = data.total_connections;
                document.getElementById('channelCount').textContent = channels.size;
                document.getElementById('activeConnections').textContent = active;
                document.getElementById('lastUpdate').textContent = new Date().toLocaleTimeString();
                
                const listEl = document.getElementById('connectionsList');
                if (data.connections.length === 0) {
                    listEl.innerHTML = '<div style="text-align: center; color: #64748b; padding: 20px;">暂无连接</div>';
                } else {
                    listEl.innerHTML = data.connections.map(c => `
                        <div class="conn-item">
                            <div class="info">
                                <div class="channel">📢 ${c.channel_id}</div>
                                <div class="meta">${c.id.substring(0, 8)}... · ${new Date(c.connected_at).toLocaleTimeString()}</div>
                            </div>
                            <span class="badge">${c.is_active ? '活跃' : '空闲'}</span>
                        </div>
                    `).join('');
                }
            } catch (e) {
                console.error('Failed to fetch stats:', e);
            }
        }
        
        // 自动刷新
        function setupAutoRefresh() {
            if (refreshInterval) clearInterval(refreshInterval);
            if (document.getElementById('autoRefresh').checked) {
                refreshInterval = setInterval(refreshStats, 2000);
            }
        }
        
        document.getElementById('autoRefresh').addEventListener('change', setupAutoRefresh);
        
        // 初始化
        refreshStats();
        setupAutoRefresh();
    </script>
</body>
</html>"#;
