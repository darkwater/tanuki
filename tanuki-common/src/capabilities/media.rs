use alloc::{string::String, vec::Vec};

use serde::{Deserialize, Serialize};

use crate::{Property, property};

pub trait MediaProperty: Property {}

#[property(MediaProperty, State, key = "capabilities")]
#[derive(Default)]
#[non_exhaustive]
pub struct MediaCapabilities {
    pub play: bool,
    pub pause: bool,
    pub stop: bool,
    pub next: bool,
    pub previous: bool,
    pub seek: bool,
    pub repeat: bool,
    pub shuffle: bool,
}

#[property(MediaProperty, State, key = "state")]
#[derive(Default)]
#[non_exhaustive]
pub struct MediaState {
    pub status: MediaStatus,
    pub duration_ms: Option<u64>,
    pub position_ms: Option<MediaPosition>,
    pub repeat: Repeat,
    pub shuffle: bool,
    pub info: MediaInfo,
    pub message: Option<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaStatus {
    Playing,
    Paused,
    Stopped,
    Buffering,
    Idle,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaPosition {
    pub position_ms: i64,
    pub timestamp_ms: i64,
    pub rate: f32,
}

impl MediaPosition {
    pub fn current_position(&self, current_timestamp_ms: i64) -> i64 {
        let elapsed_ms = ((current_timestamp_ms - self.timestamp_ms) as f32 * self.rate) as i64;
        self.position_ms + elapsed_ms
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Repeat {
    #[default]
    Off,
    One,
    All,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MediaInfo {
    pub title: Option<String>,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub genre: Option<String>,
    pub artwork_url: Option<String>,
    pub url: Option<String>,
    pub live: bool,
}

#[property(MediaProperty, Command, key = "command")]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum MediaCommand {
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    Seek { position_ms: u64 },
    SetRepeat { repeat: Repeat },
    SetShuffle { shuffle: bool },
}
