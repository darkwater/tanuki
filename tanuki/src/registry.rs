use std::{
    collections::{HashMap, hash_map::Entry},
    sync::Arc,
};

use tanuki_common::EntityId;

use crate::{
    Authority, Result, TanukiConnection, TanukiEntity,
    capabilities::{CapabilityImpl, light::Light, on_off::OnOff, sensor::Sensor},
};

pub struct Registry {
    tanuki: Arc<TanukiConnection>,
    entities: HashMap<EntityId, Arc<TanukiEntity<Authority>>>,
    sensors: HashMap<EntityId, Sensor<Authority>>,
    on_offs: HashMap<EntityId, OnOff<Authority>>,
    lights: HashMap<EntityId, Light<Authority>>,
}

async fn get<'a, T: CapabilityImpl<Authority>>(
    tanuki: &Arc<TanukiConnection>,
    entities: &mut HashMap<EntityId, Arc<TanukiEntity<Authority>>>,
    cap_map: &'a mut HashMap<EntityId, T>,
    id: &EntityId,
    init: impl AsyncFnOnce(&TanukiEntity<Authority>) -> Result<()>,
) -> Result<&'a mut T> {
    let cap = cap_map.entry(id.clone());
    let out = match cap {
        Entry::Occupied(cap) => cap.into_mut(),
        Entry::Vacant(entry) => {
            let entity = entities.entry(id.clone());
            let entity = match entity {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    let entity = tanuki.owned_entity(id.clone()).await?;
                    init(&entity).await?;
                    entry.insert(entity)
                }
            };

            let sensor = entity.capability::<T>().await?;
            entry.insert(sensor)
        }
    };

    Ok(out)
}

impl Registry {
    pub fn new(tanuki: Arc<TanukiConnection>) -> Self {
        Self {
            tanuki,
            entities: HashMap::new(),
            sensors: HashMap::new(),
            on_offs: HashMap::new(),
            lights: HashMap::new(),
        }
    }

    pub async fn sensor(
        &mut self,
        id: &EntityId,
        entity_init: impl AsyncFnOnce(&TanukiEntity<Authority>) -> Result<()>,
    ) -> Result<&mut Sensor<Authority>> {
        get(&self.tanuki, &mut self.entities, &mut self.sensors, id, entity_init).await
    }

    pub async fn on_off(
        &mut self,
        id: &EntityId,
        entity_init: impl AsyncFnOnce(&TanukiEntity<Authority>) -> Result<()>,
    ) -> Result<&mut OnOff<Authority>> {
        get(&self.tanuki, &mut self.entities, &mut self.on_offs, id, entity_init).await
    }

    pub async fn light(
        &mut self,
        id: &EntityId,
        entity_init: impl AsyncFnOnce(&TanukiEntity<Authority>) -> Result<()>,
    ) -> Result<&mut Light<Authority>> {
        get(&self.tanuki, &mut self.entities, &mut self.lights, id, entity_init).await
    }
}
