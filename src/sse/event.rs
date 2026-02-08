use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EventData {
    Raw(String),
    Value(serde_json::Value),
}

impl EventData {
    pub fn to_string(&self) -> String {
        match self {
            EventData::Raw(s) => s.clone(),
            EventData::Value(v) => serde_json::to_string(v).unwrap_or_default(),
        }
    }
}

/// SSE Event to be sent to clients
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseEvent {
    /// Event type (e.g., "message", "notification", "update")
    #[serde(rename = "event")]
    pub event_type: String,
    
    /// Event data
    pub data: EventData,
    
    /// Business ID for client-side tracking (e.g., UUID)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    
    /// Redis Stream ID for reconnection replay
    /// This is used as SSE's `id` field for `last-event-id` header
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_id: Option<String>,
    
    /// Optional retry interval in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<u32>,
}

impl SseEvent {
    pub fn new(event_type: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            event_type: event_type.into(),
            data: EventData::Value(data),
            id: Some(uuid::Uuid::new_v4().to_string()),
            stream_id: None,
            retry: None,
        }
    }

    pub fn raw(event_type: impl Into<String>, raw_json: String) -> Self {
        Self {
            event_type: event_type.into(),
            data: EventData::Raw(raw_json),
            id: Some(uuid::Uuid::new_v4().to_string()),
            stream_id: None,
            retry: None,
        }
    }

    /// Set the stream_id (Redis Stream ID) for reconnection support
    pub fn with_stream_id(mut self, stream_id: String) -> Self {
        self.stream_id = Some(stream_id);
        self
    }
}
