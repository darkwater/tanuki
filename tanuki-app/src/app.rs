use std::{
    collections::hash_map::Entry,
    sync::{Arc, mpsc::Receiver},
    time::Instant,
};

use egui::{
    Align, Button, CentralPanel, Layout, Margin, ScrollArea, SidePanel, TextWrapMode,
    ahash::{HashMap, HashMapExt as _},
    vec2,
};
use tanuki::{
    PublishEvent, TanukiConnection,
    capabilities::{User, media::Media, on_off::OnOff},
};
use tanuki_common::{
    EntityId, Topic,
    capabilities::{
        buttons::ButtonEvent,
        light::LightState,
        media::{MediaCapabilities, MediaCommand, MediaState, MediaStatus},
        on_off::OnOffCommand,
        sensor::SensorValue,
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
    TanukiButtons(TanukiButtonsState),
    TanukiLight(TanukiLightState),
    TanukiMedia(TanukiMediaState),
    TanukiOnOff(TanukiOnOffState),
    TanukiSensor(TanukiSensorState),
}

impl TanukiCapability {
    pub fn new_from_name(name: &str) -> Option<Self> {
        match name {
            "tanuki.buttons" => Some(TanukiCapability::TanukiButtons(Default::default())),
            "tanuki.light" => Some(TanukiCapability::TanukiLight(Default::default())),
            "tanuki.media" => Some(TanukiCapability::TanukiMedia(Default::default())),
            "tanuki.on_off" => Some(TanukiCapability::TanukiOnOff(Default::default())),
            "tanuki.sensor" => Some(TanukiCapability::TanukiSensor(Default::default())),
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
pub struct TanukiMediaState {
    pub capabilities: MediaCapabilities,
    pub state: MediaState,
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
    pub fn last(&self) -> Option<&T> {
        self.readings.last().map(|(_, v)| v)
    }

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

            s.spacing.window_margin = Margin::symmetric(10, 8);
            s.spacing.item_spacing = vec2(8., 1.);
            s.spacing.button_padding = vec2(8., 6.);
            s.spacing.interact_size = vec2(40., 22.);
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

                        self.entity_mut(entity)
                            .capabilities
                            .insert(capability.to_string(), cap);
                    } else {
                        log::warn!("Unknown capability name: {capability}");
                    }
                }
                Topic::CapabilityData { entity, capability, rest }
                    if capability == "tanuki.media" && rest == "state" =>
                {
                    if let Some(TanukiCapability::TanukiMedia(state)) = self
                        .entity_mut(entity)
                        .capabilities
                        .get_mut(capability.as_str())
                        && let Ok(media_state) =
                            serde_json::from_value::<MediaState>(packet.payload)
                    {
                        state.state = media_state;
                    }
                }
                Topic::CapabilityData { entity, capability, rest }
                    if capability == "tanuki.media" && rest == "capabilities" =>
                {
                    if let Some(TanukiCapability::TanukiMedia(state)) = self
                        .entity_mut(entity)
                        .capabilities
                        .get_mut(capability.as_str())
                        && let Ok(media_caps) =
                            serde_json::from_value::<MediaCapabilities>(packet.payload)
                    {
                        state.capabilities = media_caps;
                    }
                }
                Topic::CapabilityData { entity, capability, rest }
                    if capability == "tanuki.on_off" && rest == "state" =>
                {
                    if let Some(TanukiCapability::TanukiOnOff(state)) = self
                        .entity_mut(entity)
                        .capabilities
                        .get_mut(capability.as_str())
                        && let Ok(on) = serde_json::from_value::<bool>(packet.payload)
                    {
                        state.on.update(on);
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
                    TanukiCapability::TanukiButtons(state) => {
                        ui.heading("todo");
                    }
                    TanukiCapability::TanukiLight(state) => {
                        ui.heading("todo");
                    }
                    TanukiCapability::TanukiMedia(state) => {
                        if let Some(title) = &state.state.info.title {
                            ui.heading(title);
                        }

                        if let Some(artist) = state.state.info.artists.first() {
                            ui.label(artist);
                        }

                        ui.add_space(4.);

                        match state.state.status {
                            MediaStatus::Playing => ui.label("Playing"),
                            MediaStatus::Paused => ui.label("Paused"),
                            MediaStatus::Stopped => ui.label("Stopped"),
                            MediaStatus::Buffering => ui.label("Buffering"),
                            MediaStatus::Idle => ui.label("Idle"),
                            MediaStatus::Unknown => ui.label("Unknown status"),
                        };

                        ui.add_space(8.);

                        ui.horizontal(|ui| {
                            for (cap, label, cmd) in [
                                (state.capabilities.play, "Play", MediaCommand::Play),
                                (state.capabilities.pause, "Pause", MediaCommand::Pause),
                                (state.capabilities.stop, "Stop", MediaCommand::Stop),
                                (state.capabilities.previous, "Previous", MediaCommand::Previous),
                                (state.capabilities.next, "Next", MediaCommand::Next),
                            ] {
                                if ui.add_enabled(cap, Button::new(label)).clicked() {
                                    let tanuki = self.tanuki.clone();
                                    let entity = selected_entity_id.clone();
                                    let cmd = cmd.clone();
                                    self.tokio_rt.spawn(async move {
                                        let entity = tanuki.entity(entity).await.unwrap();
                                        let cap = entity.capability::<Media<User>>().await.unwrap();
                                        cap.command(cmd).await.unwrap();
                                    });
                                }
                            }
                        });
                    }
                    TanukiCapability::TanukiLight(state) => {
                        ui.heading("todo");
                    }
                    TanukiCapability::TanukiOnOff(state) => {
                        if let Some(on) = state.on.last() {
                            ui.label(format!("State: {}", if *on { "On" } else { "Off" }));
                        }

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
                    TanukiCapability::TanukiSensor(state) => {
                        ui.heading("todo");
                    }
                });
            }
        }
    }
}
