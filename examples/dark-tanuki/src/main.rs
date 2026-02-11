#![feature(deref_patterns)]
#![expect(incomplete_features, reason = "deref_patterns")]

pub mod handler;

use std::sync::Arc;

use tanuki::{
    TanukiConnection,
    capabilities::{buttons::ButtonEvent, sensor::SensorEvent},
};
use tanuki_common::capabilities::buttons::ButtonAction;

use self::handler::Handler;

#[tokio::main]
async fn main() {
    tanuki::log::init();

    tokio::spawn(async move {
        tanuki_bthome::bridge("192.168.0.106:1883", [
            ("ATC_164B6D", "balcony_door.temperature", "ATC Balcony"),
            ("ATC_C11CAF", "front_door.temperature", "ATC Door Ceiling"),
        ])
        .await
        .unwrap();
    });

    {
        use tanuki_hass::entity::*;

        let mut mappings = vec![
            MappedEntity {
                tanuki_id: "tapo_tv".into(),
                from_hass: vec![
                    EntityDataMapping::State {
                        from_id: "sensor.tv_voltage".into(),
                        map_to: CapMapping::sensor("voltage"),
                    },
                    EntityDataMapping::State {
                        from_id: "sensor.tv_current".into(),
                        map_to: CapMapping::sensor("current"),
                    },
                    EntityDataMapping::State {
                        from_id: "sensor.tv_current_consumption".into(),
                        map_to: CapMapping::sensor("current_consumption"),
                    },
                ],
                to_hass: vec![],
            },
            MappedEntity {
                tanuki_id: "vindstyrka".into(),
                from_hass: vec![
                    EntityDataMapping::State {
                        from_id: "sensor.vindstyrka_temperature".into(),
                        map_to: CapMapping::sensor("temperature"),
                    },
                    EntityDataMapping::State {
                        from_id: "sensor.vindstyrka_humidity".into(),
                        map_to: CapMapping::sensor("humidity"),
                    },
                    EntityDataMapping::State {
                        from_id: "sensor.vindstyrka_pm2_5".into(),
                        map_to: CapMapping::sensor("pm2_5"),
                    },
                ],
                to_hass: vec![],
            },
            MappedEntity {
                tanuki_id: "motion_room".into(),
                from_hass: vec![
                    EntityDataMapping::State {
                        from_id: "binary_sensor.motion_sensor_motion".into(),
                        map_to: CapMapping::binary_sensor("motion"),
                    },
                    EntityDataMapping::State {
                        from_id: "sensor.motion_sensor_battery".into(),
                        map_to: CapMapping::sensor("battery"),
                    },
                ],
                to_hass: vec![],
            },
            MappedEntity {
                tanuki_id: "motion_kitchen".into(),
                from_hass: vec![
                    EntityDataMapping::State {
                        from_id: "binary_sensor.myggspray_wrlss_mtn_sensor_occupancy".into(),
                        map_to: CapMapping::binary_sensor("motion"),
                    },
                    EntityDataMapping::State {
                        from_id: "sensor.myggspray_wrlss_mtn_sensor_battery".into(),
                        map_to: CapMapping::sensor("battery"),
                    },
                ],
                to_hass: vec![],
            },
            MappedEntity {
                tanuki_id: "balcony_door.open".into(),
                from_hass: vec![
                    EntityDataMapping::State {
                        from_id: "binary_sensor.myggbett_door_window_sensor_door".into(),
                        map_to: CapMapping::binary_sensor("open"),
                    },
                    EntityDataMapping::State {
                        from_id: "sensor.myggbett_door_window_sensor_battery".into(),
                        map_to: CapMapping::sensor("battery"),
                    },
                ],
                to_hass: vec![],
            },
            MappedEntity {
                tanuki_id: "rodret_remote_1".into(),
                from_hass: vec![EntityDataMapping::ZhaCommands {
                    device_ieee: "88:0f:62:ff:fe:4f:86:e1".to_owned(),
                    translations: vec![
                        ZhaEventTranslation {
                            command: "on".to_owned(),
                            params: serde_json::json!({}),
                            map_to: CapEventMapping::Button {
                                button: "on".to_owned(),
                                action: ButtonAction::Pressed,
                            },
                        },
                        ZhaEventTranslation {
                            command: "move_with_on_off".to_owned(),
                            params: serde_json::json!({
                                "move_mode": 0,
                            }),
                            map_to: CapEventMapping::Button {
                                button: "on".to_owned(),
                                action: ButtonAction::LongPressed,
                            },
                        },
                        ZhaEventTranslation {
                            command: "off".to_owned(),
                            params: serde_json::json!({}),
                            map_to: CapEventMapping::Button {
                                button: "off".to_owned(),
                                action: ButtonAction::Pressed,
                            },
                        },
                        ZhaEventTranslation {
                            command: "move".to_owned(),
                            params: serde_json::json!({
                                "move_mode": 1,
                            }),
                            map_to: CapEventMapping::Button {
                                button: "off".to_owned(),
                                action: ButtonAction::LongPressed,
                            },
                        },
                    ],
                }],
                to_hass: vec![],
            },
            MappedEntity {
                tanuki_id: "symfonisk_remote_1".into(),
                from_hass: vec![EntityDataMapping::ZhaCommands {
                    device_ieee: "94:de:b8:ff:fe:53:fd:97".to_owned(),
                    translations: vec![
                        ZhaEventTranslation {
                            command: "toggle".to_owned(),
                            params: serde_json::json!({}),
                            map_to: CapEventMapping::Button {
                                button: "play_pause".to_owned(),
                                action: ButtonAction::Pressed,
                            },
                        },
                        ZhaEventTranslation {
                            command: "move_with_on_off".to_owned(),
                            params: serde_json::json!({
                                "move_mode": 0,
                            }),
                            map_to: CapEventMapping::Button {
                                button: "volume_up".to_owned(),
                                action: ButtonAction::Pressed,
                            },
                        },
                        ZhaEventTranslation {
                            command: "move_with_on_off".to_owned(),
                            params: serde_json::json!({
                                "move_mode": 1,
                            }),
                            map_to: CapEventMapping::Button {
                                button: "volume_down".to_owned(),
                                action: ButtonAction::Pressed,
                            },
                        },
                        ZhaEventTranslation {
                            command: "step".to_owned(),
                            params: serde_json::json!({
                                "step_mode": 0,
                            }),
                            map_to: CapEventMapping::Button {
                                button: "next".to_owned(),
                                action: ButtonAction::Pressed,
                            },
                        },
                        ZhaEventTranslation {
                            command: "step".to_owned(),
                            params: serde_json::json!({
                                "step_mode": 1,
                            }),
                            map_to: CapEventMapping::Button {
                                button: "previous".to_owned(),
                                action: ButtonAction::Pressed,
                            },
                        },
                        ZhaEventTranslation {
                            command: "shortcut_v1_events".to_owned(),
                            params: serde_json::json!({
                                "shortcut_button": 1,
                                "shortcut_event": 1,
                            }),
                            map_to: CapEventMapping::Button {
                                button: "shortcut_one".to_owned(),
                                action: ButtonAction::Pressed,
                            },
                        },
                        ZhaEventTranslation {
                            command: "shortcut_v1_events".to_owned(),
                            params: serde_json::json!({
                                "shortcut_button": 2,
                                "shortcut_event": 1,
                            }),
                            map_to: CapEventMapping::Button {
                                button: "shortcut_two".to_owned(),
                                action: ButtonAction::Pressed,
                            },
                        },
                        ZhaEventTranslation {
                            command: "shortcut_v1_events".to_owned(),
                            params: serde_json::json!({
                                "shortcut_button": 1,
                                "shortcut_event": 3,
                            }),
                            map_to: CapEventMapping::Button {
                                button: "shortcut_one".to_owned(),
                                action: ButtonAction::LongPressed,
                            },
                        },
                        ZhaEventTranslation {
                            command: "shortcut_v1_events".to_owned(),
                            params: serde_json::json!({
                                "shortcut_button": 2,
                                "shortcut_event": 3,
                            }),
                            map_to: CapEventMapping::Button {
                                button: "shortcut_two".to_owned(),
                                action: ButtonAction::LongPressed,
                            },
                        },
                    ],
                }],
                to_hass: vec![],
            },
        ];

        const HASS_LIGHTS: [(&str, &str); 8] = [
            ("north_lamp", "light.north_light"),
            ("south_lamp", "light.kajplats_e27_cws_globe_1055lm"),
            ("cabinet_strip", "light.cabinet_strip_light"),
            ("couch_strip", "light.couch_strip"),
            ("bed_strip", "light.bed_strip_light"),
            ("cabinet_lamp", "light.cabinet_lamp_light"),
            ("cabinet_extra_lamp", "light.ikea_of_sweden_tradfri_driver_30w_light"),
            ("kitchen_lamp", "light.kitchen_light"),
        ];

        for (tanuki, hass) in HASS_LIGHTS {
            mappings.push(MappedEntity {
                tanuki_id: tanuki.into(),
                from_hass: vec![EntityDataMapping::State {
                    from_id: hass.into(),
                    map_to: CapMapping::Light,
                }],
                to_hass: vec![
                    EntityServiceMapping {
                        hass_id: hass.to_string(),
                        service: ServiceMapping::OnOff { domain: "light" },
                    },
                    EntityServiceMapping {
                        hass_id: hass.to_string(),
                        service: ServiceMapping::Light,
                    },
                ],
            });
        }

        tokio::spawn(async move {
            tanuki_hass::bridge(
                "192.168.0.106:1883",
                std::env::var("HASS_HOST").unwrap().as_str(),
                std::env::var("HASS_TOKEN").unwrap().as_str(),
                mappings,
            )
            .await
            .unwrap();
        });

        tokio::spawn(async move {
            let tanuki: Arc<TanukiConnection> =
                TanukiConnection::connect("tanuki", "192.168.0.106:1883")
                    .await
                    .unwrap();

            let mut handler = Handler::new(tanuki.clone());

            loop {
                let msg = tanuki.recv().await.unwrap();
                msg.try_handle::<SensorEvent>(&mut handler);
                msg.try_handle::<ButtonEvent>(&mut handler);
            }
        });
    }

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}
