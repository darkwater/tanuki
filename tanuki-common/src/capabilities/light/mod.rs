use alloc::{vec, vec::Vec};

use serde::{Deserialize, Serialize};

use crate::{Property, property};

pub trait LightProperty: Property {}

#[property(LightProperty, State, key = "state")]
pub struct LightState {
    /// Should also be provided by tanuki.on_off
    pub on: bool,
    /// Brightness level (0.0-1.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brightness: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
#[serde(deny_unknown_fields)]
pub enum Color {
    /// Red, green, blue, cool white, warm white, each 0-255
    Rgbww { r: u8, g: u8, b: u8, cw: u8, ww: u8 },
    /// Red, green, blue, white, each 0-255
    Rgbw { r: u8, g: u8, b: u8, w: u8 },
    /// Red, green, blue, each 0-255
    Rgb { r: u8, g: u8, b: u8 },
    /// Hue (0-360), saturation (0-100)
    Hs { h: f32, s: f32 },
    /// CIE 1931 color space x,y coordinates (0.0-1.0)
    Xy { x: f32, y: f32 },
}

impl Color {
    /// Convert to Home Assistant color representation
    pub fn to_hass(&self) -> Vec<f32> {
        match *self {
            Color::Rgbww { r, g, b, cw, ww } => {
                vec![r as f32, g as f32, b as f32, cw as f32, ww as f32]
            }
            Color::Rgbw { r, g, b, w } => vec![r as f32, g as f32, b as f32, w as f32],
            Color::Rgb { r, g, b } => vec![r as f32, g as f32, b as f32],
            Color::Hs { h, s } => vec![h, s],
            Color::Xy { x, y } => vec![x, y],
        }
    }

    pub fn hass_service_data_key(&self) -> &'static str {
        match *self {
            Color::Rgbww { .. } => "rgbww_color",
            Color::Rgbw { .. } => "rgbw_color",
            Color::Rgb { .. } => "rgb_color",
            Color::Hs { .. } => "hs_color",
            Color::Xy { .. } => "xy_color",
        }
    }

    pub fn from_slice(mode: ColorMode, data: &[f32]) -> Option<Self> {
        match (mode, data) {
            (ColorMode::Rgbww, &[r, g, b, cw, ww]) => Some(Color::Rgbww {
                r: r as u8,
                g: g as u8,
                b: b as u8,
                cw: cw as u8,
                ww: ww as u8,
            }),
            (ColorMode::Rgbww, _) => None,
            (ColorMode::Rgbw, &[r, g, b, w]) => Some(Color::Rgbw {
                r: r as u8,
                g: g as u8,
                b: b as u8,
                w: w as u8,
            }),
            (ColorMode::Rgbw, _) => None,
            (ColorMode::Rgb, &[r, g, b]) => Some(Color::Rgb { r: r as u8, g: g as u8, b: b as u8 }),
            (ColorMode::Rgb, _) => None,
            (ColorMode::Hs, &[h, s]) => Some(Color::Hs { h, s }),
            (ColorMode::Hs, _) => None,
            (ColorMode::Xy, &[x, y]) => Some(Color::Xy { x, y }),
            (ColorMode::Xy, _) => None,
            (ColorMode::ColorTemp, _) => None,
            (ColorMode::Brightness, _) => None,
            (ColorMode::OnOff, _) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorMode {
    Rgbww,
    Rgbw,
    Rgb,
    Hs,
    Xy,
    ColorTemp,
    Brightness,
    #[serde(alias = "onoff")]
    OnOff,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_colors() {
        assert_eq!(
            serde_json::from_value::<Color>(
                serde_json::json!({ "r": 255, "g": 0, "b": 128, "cw": 32, "ww": 16 })
            )
            .unwrap(),
            Color::Rgbww { r: 255, g: 0, b: 128, cw: 32, ww: 16 }
        );

        assert_eq!(
            serde_json::from_value::<Color>(
                serde_json::json!({ "r": 255, "g": 0, "b": 128, "w": 64 })
            )
            .unwrap(),
            Color::Rgbw { r: 255, g: 0, b: 128, w: 64 }
        );

        assert_eq!(
            serde_json::from_value::<Color>(serde_json::json!({ "r": 255, "g": 0, "b": 128 }))
                .unwrap(),
            Color::Rgb { r: 255, g: 0, b: 128 }
        );

        assert_eq!(
            serde_json::from_value::<Color>(serde_json::json!({ "h": 180.0, "s": 0.5 })).unwrap(),
            Color::Hs { h: 180.0, s: 0.5 }
        );

        assert_eq!(
            serde_json::from_value::<Color>(serde_json::json!({ "x": 0.3, "y": 0.6 })).unwrap(),
            Color::Xy { x: 0.3, y: 0.6 }
        );
    }
}
