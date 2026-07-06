use std::path::Path;

use wasmtime::{
    Engine, Store,
    component::{Component, Linker},
};
use wasmtime_wasi::{WasiCtxView, WasiView};

use crate::bindings::tanuki::plugin;

pub struct PluginDesc {
    pub plugin: crate::bindings::Plugin,
    config: String,
    store: Store<Plugin>,
}

pub struct Plugin {
    ctx: wasmtime_wasi::WasiCtx,
    table: wasmtime_wasi::ResourceTable,
}

impl PluginDesc {
    pub fn load(engine: &Engine, linker: &Linker<Plugin>, path: &Path) -> anyhow::Result<Self> {
        let component = Component::from_file(engine, path)?;

        let mut store = wasmtime::Store::new(engine, Plugin {
            ctx: wasmtime_wasi::WasiCtxBuilder::new().inherit_stdio().build(),
            table: wasmtime_wasi::ResourceTable::new(),
        });

        let plugin = crate::bindings::Plugin::instantiate(&mut store, &component, linker)?;

        Ok(Self { plugin, config: String::new(), store })
    }

    pub async fn run(mut self) -> anyhow::Result<()> {
        self.store
            .run_concurrent(async |acc| {
                self.plugin
                    .tanuki_plugin_provider()
                    .call_run(acc, self.config)
                    .await
            })
            .await??
            .map_err(|e| anyhow::anyhow!(e))
    }
}

// impl AccessorTask<Plugin> for PluginDesc {
//     async fn run(self, accessor: &Accessor<Plugin>) -> wasmtime::Result<()> {
//         self.plugin
//             .tanuki_plugin_provider()
//             .call_run(accessor, self.config)
//             .await?;
//
//         Ok(())
//     }
// }

impl WasiView for Plugin {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

impl plugin::logging::Host for Plugin {
    fn log(&mut self, level: plugin::logging::Level, context: String, message: String) {
        let level = match level {
            plugin::logging::Level::Critical | plugin::logging::Level::Error => log::Level::Error,
            plugin::logging::Level::Warn => log::Level::Warn,
            plugin::logging::Level::Info => log::Level::Info,
            plugin::logging::Level::Debug => log::Level::Debug,
            plugin::logging::Level::Trace => log::Level::Trace,
        };

        log::log!(target: &context, level, "{}", message);
    }
}
