use btleplug::{
    api::{
        Central, CentralEvent, Manager as _, Peripheral as _, ScanFilter, bleuuid::uuid_from_u16,
    },
    platform::Manager,
};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use tokio::sync::mpsc::UnboundedReceiver;

use self::object::Object;
use super::Result;

mod object;

#[derive(Debug)]
pub struct Update {
    pub name: String,
    pub address: String,
    pub objects: Vec<Object>,
    pub timestamp: DateTime<Utc>,
}

pub async fn event_stream() -> Result<UnboundedReceiver<Result<Update>>> {
    let manager = Manager::new().await?;

    let adapters = manager.adapters().await?;
    let central = adapters.into_iter().next().expect("no adapters found");

    let mut events = central.events().await?;

    central.start_scan(ScanFilter::default()).await?;

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    tokio::spawn(async move {
        while let Some(event) = events.next().await {
            let timestamp = Utc::now();

            let CentralEvent::ServiceDataAdvertisement { id, service_data } = event else {
                continue;
            };

            let Some(data) = service_data.get(&uuid_from_u16(0x181c)) else {
                continue;
            };

            let peripherals = match central.peripherals().await {
                Ok(p) => p,
                Err(e) => {
                    tx.send(Err(e.into())).unwrap();
                    continue;
                }
            };

            let Some(peripheral) = peripherals.iter().find(|p| p.id() == id) else {
                tracing::warn!("got ad from unknown peripheral");
                continue;
            };

            let Some(properties) = peripheral.properties().await.unwrap() else {
                tracing::warn!("got ad from peripheral with no properties");
                continue;
            };

            let Some(name) = properties.local_name else {
                tracing::warn!("got ad from peripheral with no name");
                continue;
            };

            let address = peripheral.address().to_string();

            let mut objects = Object::decode(data.as_slice());
            if let Some(rssi) = properties.rssi {
                objects.push(Object::Rssi(rssi));
            }

            tx.send(Ok(Update {
                name: name.clone(),
                address: address.clone(),
                objects,
                timestamp,
            }))
            .unwrap();
        }

        panic!("event stream ended unexpectedly");
    });

    Ok(rx)
}
