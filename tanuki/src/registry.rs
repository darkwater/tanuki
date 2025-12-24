use core::any::{Any, TypeId};
use std::{
    collections::{HashMap, hash_map::Entry},
    sync::Arc,
};

use tanuki_common::EntityId;

use crate::{Authority, Result, TanukiConnection, TanukiEntity, capabilities::Capability};

pub struct Registry {
    tanuki: Arc<TanukiConnection>,
    entities: HashMap<EntityId, Arc<TanukiEntity<Authority>>>,
    caps: HashMap<(EntityId, TypeId), Box<dyn Any + Send + Sync + 'static>>,
}

impl Registry {
    pub fn new(tanuki: Arc<TanukiConnection>) -> Self {
        Self {
            tanuki,
            entities: HashMap::new(),
            caps: HashMap::new(),
        }
    }

    pub async fn get<T: Capability<Authority> + Send + Sync + 'static>(
        &mut self,
        id: &EntityId,
        entity_init: impl AsyncFnOnce(&TanukiEntity<Authority>) -> Result<()>,
    ) -> Result<&mut T> {
        let cap = self.caps.entry((id.clone(), TypeId::of::<T>()));
        let out = match cap {
            Entry::Occupied(cap) => cap.into_mut(),
            Entry::Vacant(entry) => {
                let entity = self.entities.entry(id.clone());
                let entity = match entity {
                    Entry::Occupied(entry) => entry.into_mut(),
                    Entry::Vacant(entry) => {
                        let entity = self.tanuki.entity(id.clone()).await?;
                        entity_init(&entity).await?;
                        entry.insert(entity)
                    }
                };

                let sensor = entity.capability::<T>().await?;
                entry.insert(Box::new(sensor))
            }
        };

        Ok(out
            .downcast_mut::<T>()
            .expect("wrong type stored in registry"))
    }
}
