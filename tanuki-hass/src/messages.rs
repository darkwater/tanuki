use core::fmt::Display;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tanuki_common::capabilities::light::ColorMode;

use crate::entity::ServiceCallTarget;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[expect(clippy::enum_variant_names)]
pub enum AuthServerMessage {
    AuthRequired {
        ha_version: String,
    },
    AuthOk {
        #[expect(dead_code)]
        ha_version: String,
    },
    AuthInvalid {
        message: String,
    },
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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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

impl Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct Event {
    // pub data: serde_json::Value,
    // pub event_type: String,
    pub time_fired: DateTime<Utc>,
    pub origin: String,
    pub context: serde_json::Value,
    #[serde(flatten)]
    pub data: EventData,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(tag = "event_type", content = "data", rename_all = "snake_case")]
pub enum EventData {
    StateChanged(Box<StateChangeEvent>),
    ZhaEvent(Box<ZhaEvent>),
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct ZhaEvent {
    pub device_id: String,
    pub device_ieee: String,
    pub unique_id: String,
    pub command: String,
    pub args: serde_json::Value,
    pub params: serde_json::Value,
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
        target: ServiceCallTarget,
        // return_response: bool,
    },
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct StateChangeEvent {
    pub entity_id: String,
    pub new_state: SensorState,
    pub old_state: SensorState,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct StateEvent {
    pub entity_id: String,
    #[serde(flatten)]
    pub state: SensorState,
}

#[derive(Debug, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct StateAttributes {
    // sensor
    pub unit_of_measurement: String,

    // light
    pub brightness: Option<u8>,
    pub color_mode: Option<ColorMode>,
    pub rgbww_color: Option<[f32; 5]>,
    pub rgbw_color: Option<[f32; 4]>,
    pub rgb_color: Option<[f32; 3]>,
    pub hs_color: Option<[f32; 2]>,
    pub xy_color: Option<[f32; 2]>,
    pub color_temp: Option<u16>,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct SensorState {
    pub attributes: StateAttributes,
    pub last_changed: DateTime<Utc>,
    pub last_updated: DateTime<Utc>,
    pub last_reported: DateTime<Utc>,
    pub state: String,
}
