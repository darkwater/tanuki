use core::fmt::Debug;

use serde::{Deserialize, Serialize};

#[macro_export]
macro_rules! property {
    attr($namespace:ty, key = $key:expr) (pub struct $ident:ident(pub $($inner:ty)*);) => {
        #[derive(Debug, Clone, PartialEq, $crate::_serde::Serialize, $crate::_serde::Deserialize)]
        #[serde(transparent)]
        pub struct $ident(pub $($inner)*);

        impl $crate::Property for $ident {
            const KEY: &str = $key;
        }

        impl $namespace for $ident {}
    };
}

pub trait Property: Debug + Clone + Serialize + for<'de> Deserialize<'de> {
    const KEY: &str;
}
