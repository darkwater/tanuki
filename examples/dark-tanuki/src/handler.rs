use std::sync::Arc;

use tanuki::{
    TanukiConnection,
    capabilities::{
        User,
        buttons::{ButtonEvent, ButtonName},
        on_off::OnOff,
        sensor::SensorEvent,
    },
    listener::EventHandler,
};
use tanuki_common::capabilities::{buttons::ButtonAction, on_off::OnOffCommand};

pub struct Handler {
    tanuki: Arc<TanukiConnection>,
}

impl Handler {
    pub fn new(tanuki: Arc<TanukiConnection>) -> Self {
        Self { tanuki }
    }

    fn set_lights(&self, command: OnOffCommand, extra: bool) {
        const LIGHTS: [&str; 8] = [
            "north_lamp",
            "south_lamp",
            "cabinet_strip",
            "couch_strip",
            "bed_strip",
            // extra
            "cabinet_lamp",
            "cabinet_extra_lamp",
            "kitchen_lamp",
        ];

        let extent = if extra { LIGHTS.len() } else { 5 };

        for tanuki_id in &LIGHTS[..extent] {
            self.tanuki.intent(async move |tanuki| {
                tanuki
                    .entity_cap::<OnOff<User>>(tanuki_id)
                    .command(command)
                    .await
            });
        }
    }
}

impl EventHandler<ButtonEvent> for Handler {
    fn handle(&mut self, event: ButtonEvent) {
        tracing::info!("Received button event: {event:?}");

        match event {
            ButtonEvent {
                entity: "rodret_remote_1",
                name: ButtonName::On,
                action: ButtonAction::Pressed,
            } => self.set_lights(OnOffCommand::On, false),

            ButtonEvent {
                entity: "rodret_remote_1",
                name: ButtonName::On,
                action: ButtonAction::LongPressed,
            } => self.set_lights(OnOffCommand::On, true),

            ButtonEvent {
                entity: "rodret_remote_1",
                name: ButtonName::Off,
                action: ButtonAction::Pressed,
            } => self.set_lights(OnOffCommand::Off, true),

            _ => {
                tracing::info!(
                    entity = %event.entity,
                    button = ?event.name,
                    action = ?event.action,
                    "Unhandled button event"
                );
            }
        }
    }
}

impl EventHandler<SensorEvent> for Handler {
    fn handle(&mut self, event: SensorEvent) {
        tracing::info!("Received sensor event: {event:?}");
    }
}
