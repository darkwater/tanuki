use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthServerMessage {
    AuthRequired { ha_version: String },
    AuthOk { ha_version: String },
    AuthInvalid { message: String },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthClientMessage {
    Auth { access_token: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Packet<T> {
    pub(crate) id: PacketId,
    #[serde(flatten)]
    pub(crate) payload: T,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct PacketId(pub u32);

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Result {
        success: bool,
        #[serde(default)]
        result: serde_json::Value,
        #[serde(default)]
        error: Option<ServerError>,
    },
    Event {
        event: Event,
    },
}

#[derive(Debug, Deserialize)]
pub struct ServerError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct Event {
    pub data: serde_json::Value,
    pub event_type: String,
    pub time_fired: DateTime<Utc>,
    pub origin: String,
    pub context: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    GetStates,
    SubscribeEvents {
        #[serde(skip_serializing_if = "Option::is_none")]
        event_type: Option<String>,
    },
    CallService {
        domain: String,
        service: String,
        #[serde(skip_serializing_if = "Value::is_null")]
        service_data: serde_json::Value,
        #[serde(skip_serializing_if = "Value::is_null")]
        target: serde_json::Value,
        // return_response: bool,
    },
}
