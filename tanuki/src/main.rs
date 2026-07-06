use std::ffi::OsStr;

use wasmtime::component::HasSelf;

use crate::plugin::{Plugin, PluginDesc};

mod plugin;

mod bindings {
    wasmtime::component::bindgen!("plugin" in "../wit");
}

#[tokio::main]
pub async fn main() -> anyhow::Result<()> {
    pretty_env_logger::init();

    let engine = wasmtime::Engine::default();
    let mut linker = wasmtime::component::Linker::<Plugin>::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
    wasmtime_wasi::p3::add_to_linker(&mut linker)?;
    bindings::tanuki::plugin::logging::add_to_linker::<Plugin, HasSelf<Plugin>>(
        &mut linker,
        |s| s,
    )?;

    let plugins_dir = "target/wasm32-wasip2/debug";
    for entry in std::fs::read_dir(plugins_dir)? {
        let path = entry?.path();
        if path.is_file() && path.extension().and_then(OsStr::to_str) == Some("wasm") {
            log::debug!("Loading plugin: {:?}", path);
            PluginDesc::load(&engine, &linker, &path)?.run().await?;
            log::debug!("Finished running plugin: {:?}", path);
        }
    }

    Ok(())
}
