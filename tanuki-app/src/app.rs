use std::{
    collections::hash_map::Entry,
    sync::{Arc, mpsc::Receiver},
    time::Instant,
};

use egui::{
    Align, CentralPanel, Layout, ScrollArea, SidePanel, TextWrapMode,
    ahash::{HashMap, HashMapExt as _},
    vec2,
};
use tanuki::{
    PublishEvent, TanukiConnection,
    capabilities::{User, on_off::OnOff},
};
use tanuki_common::{
    EntityId, Topic,
    capabilities::{
        buttons::ButtonEvent, light::LightState, on_off::OnOffCommand, sensor::SensorValue,
    },
};

pub struct TanukiApp {
    rx: Receiver<PublishEvent>,
    tanuki: Arc<TanukiConnection>,
    tokio_rt: tokio::runtime::Handle,
    entities: HashMap<EntityId, TanukiEntity>,
    selected_entity: Option<EntityId>,
    selected_capability: Option<String>,
}

pub struct TanukiEntity {
    pub id: EntityId,
    pub name: Option<String>,
    pub capabilities: HashMap<String, TanukiCapability>,
}

impl TanukiEntity {
    pub fn capability_mut(&mut self, name: &str) -> Option<&mut TanukiCapability> {
        match self.capabilities.entry(name.to_string()) {
            Entry::Occupied(entry) => Some(entry.into_mut()),
            Entry::Vacant(entry) => {
                if let Some(cap) = TanukiCapability::new_from_name(name) {
                    Some(entry.insert(cap))
                } else {
                    None
                }
            }
        }
    }
}

pub enum TanukiCapability {
    TanukiSensor(TanukiSensorState),
    TanukiOnOff(TanukiOnOffState),
    TanukiLight(TanukiLightState),
    TanukiButtons(TanukiButtonsState),
}

impl TanukiCapability {
    pub fn new_from_name(name: &str) -> Option<Self> {
        match name {
            "tanuki.sensor" => Some(TanukiCapability::TanukiSensor(Default::default())),
            "tanuki.on_off" => Some(TanukiCapability::TanukiOnOff(Default::default())),
            "tanuki.light" => Some(TanukiCapability::TanukiLight(Default::default())),
            "tanuki.buttons" => Some(TanukiCapability::TanukiButtons(Default::default())),
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct TanukiSensorState {
    pub sensors: HashMap<EntityId, SensorHistory>,
}

#[derive(Default)]
pub struct SensorHistory {
    pub unit: String,
    pub timeline: Timeline<SensorValue>,
}

#[derive(Default)]
pub struct TanukiOnOffState {
    pub on: Timeline<bool>,
}

#[derive(Default)]
pub struct TanukiLightState {
    pub state: Option<LightState>,
}

#[derive(Default)]
pub struct TanukiButtonsState {
    pub buttons: HashMap<String, Timeline<ButtonEvent>>,
}

pub struct Timeline<T> {
    pub readings: Vec<(Instant, T)>,
}

impl<T> Default for Timeline<T> {
    fn default() -> Self {
        Self { readings: Vec::new() }
    }
}

impl<T> Timeline<T> {
    pub fn update(&mut self, payload: T) {
        self.readings.push((Instant::now(), payload));
    }

    pub fn update_with_timestamp(&mut self, timestamp: Instant, payload: T) {
        self.readings.push((timestamp, payload));
    }
}

impl TanukiApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<PublishEvent>();

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();

        let tokio_rt = rt.handle().clone();

        let (tanuki_tx, tanuki_rx) = std::sync::mpsc::sync_channel(1);

        let ctx = cc.egui_ctx.clone();
        std::thread::spawn(move || {
            rt.block_on(async {
                let tanuki = tanuki::TanukiConnection::connect("tanuki-app", "192.168.0.106:1883")
                    .await
                    .unwrap();

                tanuki_tx.send(tanuki.clone()).unwrap();

                tanuki.raw_subscribe("tanuki/#").await.unwrap();

                loop {
                    match tanuki.recv().await {
                        Ok(packet) => {
                            log::debug!("Received packet: {packet:#?}");
                            tx.send(packet).unwrap();
                            ctx.request_repaint();
                        }
                        Err(e) => {
                            log::error!("Error receiving packet: {e}");
                        }
                    }
                }
            });
        });

        let tanuki = tanuki_rx.recv().unwrap();

        cc.egui_ctx.all_styles_mut(|s| {
            s.interaction.selectable_labels = false;

            s.spacing.button_padding = vec2(8., 6.);
        });

        Self {
            rx,
            tanuki,
            tokio_rt,
            entities: HashMap::new(),
            selected_entity: None,
            selected_capability: None,
        }
    }

    pub fn entity_mut(&mut self, id: EntityId) -> &mut TanukiEntity {
        self.entities
            .entry(id.clone())
            .or_insert_with(|| TanukiEntity {
                id,
                name: None,
                capabilities: HashMap::new(),
            })
    }
}

impl eframe::App for TanukiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(packet) = self.rx.try_recv() {
            match packet.topic {
                Topic::EntityMeta { entity, key } if key == "name" => {
                    if let Some(name) = packet.payload.as_str() {
                        self.entity_mut(entity).name = Some(name.to_owned());
                    }
                }
                Topic::CapabilityMeta { entity, capability, key } if key == "version" => {
                    log::info!("New capability: {entity} / {capability}");
                    if let Some(cap) = TanukiCapability::new_from_name(&capability) {
                        log::info!("Created capability instance for {capability}");

                        let entity_entry = self.entity_mut(entity);

                        entity_entry
                            .capabilities
                            .insert(capability.to_string(), cap);
                    } else {
                        log::warn!("Unknown capability name: {capability}");
                    }
                }
                _ => {}
            }
        }

        SidePanel::left("entities")
            .resizable(false)
            .show(ctx, |ui| {
                ui.style_mut().wrap_mode = Some(TextWrapMode::Extend);
                ScrollArea::vertical().show(ui, |ui| {
                    ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
                        for (entity_id, entity) in &self.entities {
                            ui.selectable_value(
                                &mut self.selected_entity,
                                Some(entity_id.clone()),
                                entity.name.as_deref().unwrap_or(entity_id.as_str()),
                            );
                        }
                    });
                });
            });

        if let Some(selected_entity_id) = &self.selected_entity {
            let entity = self.entities.get(selected_entity_id).unwrap();

            SidePanel::left("capabilities")
                .resizable(false)
                .show(ctx, |ui| {
                    ui.style_mut().wrap_mode = Some(TextWrapMode::Extend);

                    ScrollArea::vertical().show(ui, |ui| {
                        ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
                            for (cap_name, _capability) in &entity.capabilities {
                                ui.selectable_value(
                                    &mut self.selected_capability,
                                    Some(cap_name.clone()),
                                    cap_name,
                                );
                            }
                        });
                    });
                });

            if let Some(selected_capability_name) = &self.selected_capability
                && let Some(capability) = entity.capabilities.get(selected_capability_name)
            {
                CentralPanel::default().show(ctx, |ui| match capability {
                    TanukiCapability::TanukiSensor(state) => {
                        ui.heading("todo");
                    }
                    TanukiCapability::TanukiOnOff(state) => {
                        ui.heading("todo");

                        if ui.button("Toggle").clicked() {
                            let tanuki = self.tanuki.clone();
                            let entity = selected_entity_id.clone();
                            self.tokio_rt.spawn(async move {
                                let entity = tanuki.entity(entity).await.unwrap();
                                let cap = entity.capability::<OnOff<User>>().await.unwrap();
                                cap.command(OnOffCommand::Toggle).await.unwrap();
                            });
                        }
                    }
                    TanukiCapability::TanukiLight(state) => {
                        ui.heading("todo");
                    }
                    TanukiCapability::TanukiButtons(state) => {
                        ui.heading("todo");
                    }
                });
            }
        }
    }
}
