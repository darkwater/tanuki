use std::collections::{HashMap, hash_map::Entry};

use heck::ToSnakeCase as _;
use tanuki::{
    TanukiConnection,
    capabilities::{Authority, sensor::Sensor},
};
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

pub async fn bridge(
    addr: &str,
    id_map: impl IntoIterator<Item = (impl AsRef<str>, impl AsRef<str>, impl AsRef<str>)>,
) -> Result<()> {
    let id_map = id_map
        .into_iter()
        .map(|(k, id, name)| {
            (k.as_ref().to_owned(), (id.as_ref().to_owned(), name.as_ref().to_owned()))
        })
        .collect::<HashMap<_, _>>();

    let tanuki = TanukiConnection::connect("tanuki-bthome", addr).await?;

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

                let (id, name) = id_map
                    .get(update.name.as_str())
                    .or_else(|| id_map.get(update.address.as_str()))
                    .cloned()
                    .unwrap_or((update.name.to_snake_case(), update.name));

                let entity = tanuki.entity(id).await?;

                entity.publish_meta(meta::Name(name.into())).await?;
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

        for object in &update.objects {
            sensor
                .get()
                .publish(object.topic(), SensorPayload {
                    value: object.value(),
                    unit: object.unit().into(),
                    timestamp: update.timestamp,
                })
                .await?;
        }
    }
}
