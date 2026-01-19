use tanuki_app::TanukiApp;

fn main() -> eframe::Result {
    pretty_env_logger::init_timed();

    eframe::run_native(
        "tanuki",
        Default::default(),
        Box::new(|cc| Ok(Box::new(TanukiApp::new(cc)))),
    )
}
