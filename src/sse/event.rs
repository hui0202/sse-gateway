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
    
    /// Optional event ID for client-side tracking
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    
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
            retry: None,
        }
    }

    pub fn raw(event_type: impl Into<String>, raw_json: String) -> Self {
        Self {
            event_type: event_type.into(),
            data: EventData::Raw(raw_json),
            id: Some(uuid::Uuid::new_v4().to_string()),
            retry: None,
        }
    }

}
