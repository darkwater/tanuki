use std::sync::Arc;

use tanuki::{
    TanukiConnection,
    capabilities::{User, buttons::Buttons, on_off::OnOff},
};
use tanuki_common::capabilities::{buttons::ButtonEvent, on_off::OnOffCommand};

#[tokio::main]
async fn main() {
    tanuki::log::init();

    tokio::spawn(async move {
        tanuki_bthome::bridge("192.168.0.106:1883", [
            ("ATC_164B6D", "atc_balcony", "ATC Balcony"),
            ("ATC_2DB3D7", "atc_door_ceiling", "ATC Door Ceiling"),
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
                tanuki_id: "motion_sensor".into(),
                from_hass: vec![EntityDataMapping::State {
                    from_id: "binary_sensor.motion_sensor_motion".into(),
                    map_to: CapMapping::binary_sensor("motion"),
                }],
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
                                event: ButtonEvent::Pressed,
                            },
                        },
                        ZhaEventTranslation {
                            command: "move_with_on_off".to_owned(),
                            params: serde_json::json!({
                                "move_mode": 0,
                            }),
                            map_to: CapEventMapping::Button {
                                button: "on".to_owned(),
                                event: ButtonEvent::LongPressed,
                            },
                        },
                        ZhaEventTranslation {
                            command: "off".to_owned(),
                            params: serde_json::json!({}),
                            map_to: CapEventMapping::Button {
                                button: "off".to_owned(),
                                event: ButtonEvent::Pressed,
                            },
                        },
                        ZhaEventTranslation {
                            command: "move".to_owned(),
                            params: serde_json::json!({
                                "move_mode": 1,
                            }),
                            map_to: CapEventMapping::Button {
                                button: "off".to_owned(),
                                event: ButtonEvent::LongPressed,
                            },
                        },
                    ],
                }],
                to_hass: vec![],
            },
        ];

        const LIGHTS: [(&str, &str); 8] = [
            ("north_lamp", "light.north_light"),
            ("south_lamp", "light.south_light"),
            ("cabinet_strip", "light.cabinet_strip_light"),
            ("couch_strip", "light.couch_strip"),
            ("bed_strip", "light.bed_strip_light"),
            ("cabinet_lamp", "light.cabinet_lamp_light"),
            ("cabinet_extra_lamp", "light.ikea_of_sweden_tradfri_driver_30w_light"),
            ("kitchen_lamp", "light.kitchen_light"),
        ];

        for (tanuki, hass) in LIGHTS {
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

            let remote = tanuki
                .entity("rodret_remote_1")
                .await
                .unwrap()
                .capability::<Buttons<User>>()
                .await
                .unwrap();

            let set_lights = {
                let tanuki = tanuki.clone();
                move |cmd| {
                    let tanuki = tanuki.clone();
                    tokio::spawn(async move {
                        for (tanuki_id, _) in &LIGHTS[..6] {
                            tanuki
                                .entity(tanuki_id)
                                .await
                                .unwrap()
                                .capability::<OnOff<User>>()
                                .await
                                .unwrap()
                                .command(cmd)
                                .await
                                .unwrap();
                        }
                    });
                }
            };

            remote
                .listen(move |button, event| match dbg!((button, event)) {
                    ("on", ButtonEvent::Pressed) => {
                        set_lights(OnOffCommand::On);
                    }
                    ("off", ButtonEvent::Pressed) => {
                        set_lights(OnOffCommand::Off);
                    }
                    (button, event) => {
                        tracing::info!("Unhandled button event: {} {:?}", button, event);
                    }
                })
                .await
                .unwrap();

            #[allow(unreachable_code)] // unwrap will panic on error
            tanuki.handle().await.unwrap()
        });
    }

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}
