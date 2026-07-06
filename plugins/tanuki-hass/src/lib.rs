use tanuki_plugin::exports::tanuki::plugin;

struct Plugin;

impl plugin::provider::Guest for Plugin {
    fn get_info() -> plugin::provider::Info {
        plugin::provider::Info {
            name: "example".to_string(),
            version: "0.1.0".to_string(),
            description: "An example plugin".to_string(),
        }
    }

    async fn run(config: String) -> Result<(), String> {
        tanuki_plugin::log::init();
        tanuki_plugin::log::info!("hello, world!");
        tanuki_plugin::log::info!("config: {config}");

        Ok(())
    }
}

tanuki_plugin::export!(Plugin);
