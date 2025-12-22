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

    tokio::spawn(async move {
        use tanuki_hass::entity::*;

        let mut mappings = vec![
            MappedEntity {
                tanuki_id: "tapo_tv".into(),
                from_hass: vec![
                    EntityDataMapping {
                        from_id: "sensor.tv_voltage".into(),
                        map_to: CapMapping::sensor("voltage"),
                    },
                    EntityDataMapping {
                        from_id: "sensor.tv_current".into(),
                        map_to: CapMapping::sensor("current"),
                    },
                    EntityDataMapping {
                        from_id: "sensor.tv_current_consumption".into(),
                        map_to: CapMapping::sensor("current_consumption"),
                    },
                ],
                to_hass: vec![],
            },
            MappedEntity {
                tanuki_id: "vindstyrka".into(),
                from_hass: vec![
                    EntityDataMapping {
                        from_id: "sensor.vindstyrka_temperature".into(),
                        map_to: CapMapping::sensor("temperature"),
                    },
                    EntityDataMapping {
                        from_id: "sensor.vindstyrka_humidity".into(),
                        map_to: CapMapping::sensor("humidity"),
                    },
                    EntityDataMapping {
                        from_id: "sensor.vindstyrka_pm2_5".into(),
                        map_to: CapMapping::sensor("pm2_5"),
                    },
                ],
                to_hass: vec![],
            },
            MappedEntity {
                tanuki_id: "motion_sensor".into(),
                from_hass: vec![EntityDataMapping {
                    from_id: "binary_sensor.motion_sensor_motion".into(),
                    map_to: CapMapping::binary_sensor("motion"),
                }],
                to_hass: vec![],
            },
        ];

        for (tanuki, hass) in [
            ("north_lamp", "light.north_light"),
            ("south_lamp", "light.south_light"),
            ("kitchen_lamp", "light.kitchen_light"),
            ("cabinet_lamp", "light.cabinet_lamp_light"),
            ("cabinet_extra_lamp", "light.ikea_of_sweden_tradfri_driver_30w_light"),
            ("cabinet_strip", "light.cabinet_strip_light"),
            ("couch_strip", "light.couch_strip"),
            ("bed_strip", "light.bed_strip_light"),
        ] {
            mappings.push(MappedEntity {
                tanuki_id: tanuki.into(),
                from_hass: vec![EntityDataMapping {
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

        tanuki_hass::bridge(
            "192.168.0.106:1883",
            std::env::var("HASS_HOST").unwrap().as_str(),
            std::env::var("HASS_TOKEN").unwrap().as_str(),
            mappings,
        )
        .await
        .unwrap();
    });

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}
