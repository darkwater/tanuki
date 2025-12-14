use std::{
    collections::{HashMap, hash_map::Entry},
    sync::Arc,
};

use heck::ToSnakeCase;
use mqtt_endpoint_tokio::mqtt_ep::{
    self,
    packet::{
        Qos,
        v5_0::{self, Connack},
    },
    role,
    transport::{TcpTransport, connect_helper},
};
use tanuki::{Authority, TanukiConnection, capabilities::sensor::Sensor};
use tanuki_common::{capabilities::sensor::SensorPayload, meta};

mod bthome;

type Result<T> = std::result::Result<T, Error>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("btleplug error: {0}")]
    Btleplug(#[from] btleplug::Error),
    #[error("tanuki error: {0}")]
    Tanuki(#[from] tanuki::Error),
}

#[tokio::main]
async fn main() -> Result<()> {
    tanuki::log::init();

    let tanuki = TanukiConnection::connect("tanuki-bthome", "192.168.0.106:1883").await?;

    tokio::spawn({
        let tanuki = tanuki.clone();

        async move {
            loop {
                let packet = tanuki.recv_raw().await;
                tracing::debug!("Received packet: {packet:?}");
            }
        }
    });

    let mut updates = bthome::event_stream().await?;

    let mut devices = HashMap::<String, Sensor<Authority>>::new();

    loop {
        let update = updates
            .recv()
            .await
            .expect("bluetooth event stream ended")?;

        tracing::debug!("BTHome update: {update:#?}");

        let entry = devices.entry(update.address.clone());
        let sensor = match entry {
            Entry::Occupied(entry) => entry,
            Entry::Vacant(entry) => {
                tracing::info!(?update.name, ?update.address, "Registering new device");

                let entity = tanuki.owned_entity(update.name.to_snake_case()).await?;
                entity.publish_meta(meta::Name(update.name.into())).await?;
                entity
                    .publish_meta(meta::Type("BTHome Sensor".into()))
                    .await?;
                entity
                    .publish_meta(meta::Provider("tanuki-bthome".into()))
                    .await?;

                let sensor = entity.capability::<Sensor<_>>().await?;
                entry.insert_entry(sensor)
            }
        };

        let timestamp = update.timestamp.timestamp();

        for object in &update.objects {
            sensor
                .get()
                .publish(object.topic(), SensorPayload {
                    value: object.value(),
                    unit: object.unit().into(),
                    timestamp,
                })
                .await?;
        }
    }
}
