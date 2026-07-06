mod bindings {
    wit_bindgen::generate!({
        world: "plugin",
        path: "../wit",
        pub_export_macro: true,
        default_bindings_module: "::tanuki_plugin",
    });
}

pub use bindings::export;
pub use bindings::exports::tanuki::plugin::*;
pub use bindings::tanuki::plugin::*;

/// for the export macro
#[doc(hidden)]
pub use bindings::exports;

pub mod log;
