use serde::{Deserialize, Serialize};

// Messages from frontend → backend (over WebSocket):
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")] // easier debugging
pub enum ClientMsg {
    /// User clicked "Send" — publish this payload to MQTT
    Publish { payload: String },

    /// User clicked "Ping Device" — send a request, expect a correlated reply
    PingDevice { correlation_id: String },
}

// Messages from backend → frontend (over WebSocket):
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMsg {
    /// A message arrived on the live MQTT subscription
    MqttUpdate { topic: String, payload: String },

    /// Response to a PingDevice request
    PingResponse {
        correlation_id: String,
        device_reply: String,
    },
}

// HTTP response for the "Fetch" button:
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastMessage {
    pub topic: String,
    pub payload: String,
    pub timestamp_ms: u64,
}
