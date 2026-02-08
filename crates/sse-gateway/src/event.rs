//! SSE Event types

use serde::{Deserialize, Serialize};

/// Event data can be raw string or structured JSON
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EventData {
    /// Raw string data
    Raw(String),
    /// Structured JSON data
    Value(serde_json::Value),
}

impl EventData {
    /// Convert to string representation
    pub fn to_string(&self) -> String {
        match self {
            EventData::Raw(s) => s.clone(),
            EventData::Value(v) => serde_json::to_string(v).unwrap_or_default(),
        }
    }
}

impl From<String> for EventData {
    fn from(s: String) -> Self {
        EventData::Raw(s)
    }
}

impl From<&str> for EventData {
    fn from(s: &str) -> Self {
        EventData::Raw(s.to_string())
    }
}

impl From<serde_json::Value> for EventData {
    fn from(v: serde_json::Value) -> Self {
        EventData::Value(v)
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

    /// Stream ID for reconnection replay (used as SSE's `id` field)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_id: Option<String>,

    /// Optional retry interval in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<u32>,
}

impl SseEvent {
    /// Create a new SSE event with JSON data
    pub fn new(event_type: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            event_type: event_type.into(),
            data: EventData::Value(data),
            id: Some(uuid::Uuid::new_v4().to_string()),
            stream_id: None,
            retry: None,
        }
    }

    /// Create a new SSE event with raw string data
    pub fn raw(event_type: impl Into<String>, raw_data: impl Into<String>) -> Self {
        Self {
            event_type: event_type.into(),
            data: EventData::Raw(raw_data.into()),
            id: Some(uuid::Uuid::new_v4().to_string()),
            stream_id: None,
            retry: None,
        }
    }

    /// Create a simple message event
    pub fn message(data: impl Into<String>) -> Self {
        Self::raw("message", data)
    }

    /// Set the stream ID for reconnection support
    pub fn with_stream_id(mut self, stream_id: impl Into<String>) -> Self {
        self.stream_id = Some(stream_id.into());
        self
    }

    /// Set the event ID
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the retry interval
    pub fn with_retry(mut self, retry_ms: u32) -> Self {
        self.retry = Some(retry_ms);
        self
    }
}
