use core::ops::Deref;

use crate::{EntityRole, TanukiCapability};

pub mod light;
pub mod on_off;
pub mod sensor;

pub trait CapabilityImpl<R: EntityRole>:
    From<TanukiCapability<R>> + Deref<Target = TanukiCapability<R>>
{
    const ID: &'static str;
    const VERSION: i32 = 0;
}

#[macro_export]
macro_rules! capability {
    attr(id = $id:expr) (pub struct $name:ident $($tt:tt)*) => {
        pub struct $name $($tt)*

        impl<R: EntityRole> CapabilityImpl<R> for $name<R> {
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
