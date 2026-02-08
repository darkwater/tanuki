use core::ops::Deref;
use std::sync::Arc;

use serde::Serialize;
use tanuki_common::{
    EntityId, Property, TanukiString, ToTanukiString, Topic,
    meta::{self, MetaField},
};

use crate::{PublishOpts, Result, TanukiEntity};

pub mod buttons;
pub mod light;
pub mod media;
pub mod on_off;
pub mod sensor;

pub struct TanukiCapability<R: EntityRole> {
    pub(crate) entity: Arc<TanukiEntity<R>>,
    pub(crate) capability: TanukiString,
}

impl<R: EntityRole> TanukiCapability<R> {
    pub fn entity(&self) -> Arc<TanukiEntity<R>> {
        self.entity.clone()
    }

    pub fn entity_id(&self) -> &EntityId {
        self.entity.id()
    }

    pub fn id(&self) -> &str {
        &self.capability
    }

    pub async fn initialize(&self, version: i32) -> Result<()> {
        self.publish_meta(meta::Version(version)).await?;

        Ok(())
    }

    pub async fn publish_raw(
        &self,
        topic: impl ToTanukiString,
        payload: impl Serialize,
        opts: PublishOpts,
    ) -> Result<()> {
        let topic = Topic::CapabilityData {
            entity: self.entity.id().clone(),
            capability: self.capability.clone(),
            rest: topic.to_tanuki_string(),
        };

        self.entity.conn.publish(topic, payload, opts).await
    }

    pub(crate) async fn publish_property<T: Property>(
        &self,
        property: T,
        opts: PublishOpts,
    ) -> Result<()> {
        self.publish_raw(T::KEY, property, opts).await
    }

    pub(crate) async fn publish_meta<T: MetaField>(&self, meta: T) -> Result<()> {
        let topic = Topic::CapabilityMeta {
            entity: self.entity.id().clone(),
            capability: self.capability.clone(),
            key: TanukiString::const_new(T::KEY),
        };

        self.entity
            .conn
            .publish(topic, meta, PublishOpts::metadata())
            .await
    }

    pub(crate) async fn listen<T: Property>(
        &self,
        mut listener: impl FnMut(T) + Send + Sync + 'static,
        oneshot: bool,
    ) -> Result<()> {
        self.entity
            .conn
            .subscribe_with_handler(
                Topic::CapabilityData {
                    entity: self.entity.id().clone(),
                    capability: self.capability.clone(),
                    rest: TanukiString::const_new(T::KEY),
                },
                Box::new(move |ev| match serde_json::from_value::<T>(ev.payload) {
                    Ok(payload) => {
                        listener(payload);
                        !oneshot
                    }
                    Err(e) => {
                        tracing::error!("Failed to deserialize property {}: {e}", T::KEY);
                        false
                    }
                }),
            )
            .await
    }

    pub(crate) async fn listen_oneshot<T: Property>(
        &self,
        listener: impl FnOnce(T) + Send + Sync + 'static,
    ) -> Result<()> {
        let mut listener = Some(listener);
        self.listen(
            move |v| {
                if let Some(listener) = listener.take() {
                    listener(v);
                }
            },
            true,
        )
        .await
    }

    pub(crate) async fn get<T: Property + Send + 'static>(&self) -> Result<T> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.listen_oneshot(move |prop: T| {
            let _ = tx.send(prop);
        })
        .await?;

        Ok(rx.await.unwrap())
    }
}

pub trait EntityRole {
    const AUTHORITY: bool;
}

pub struct Authority;
pub struct User;

impl EntityRole for Authority {
    const AUTHORITY: bool = true;
}
impl EntityRole for User {
    const AUTHORITY: bool = false;
}

pub trait Capability<R: EntityRole>:
    From<TanukiCapability<R>> + Deref<Target = TanukiCapability<R>>
{
    const ID: &'static str;
    const VERSION: i32 = 0;
}

#[macro_export]
macro_rules! capability {
    attr(id = $id:expr) (pub struct $name:ident $($tt:tt)*) => {
        pub struct $name $($tt)*

        impl<R: EntityRole> Capability<R> for $name<R> {
            const ID: &'static str = $id;
        }

        impl<R: EntityRole> From<$crate::TanukiCapability<R>> for $name<R> {
            fn from(cap: $crate::TanukiCapability<R>) -> Self {
                Self { cap }
            }
        }

        impl<R: EntityRole> ::core::ops::Deref for $name<R> {
            type Target = $crate::TanukiCapability<R>;

            fn deref(&self) -> &Self::Target {
                &self.cap
            }
        }
    };
}
