use eframe::NativeOptions;
use egui::ViewportBuilder;
use tanuki_app::TanukiApp;

fn main() -> eframe::Result {
    pretty_env_logger::init_timed();

    let opts = NativeOptions {
        viewport: ViewportBuilder::default().with_transparent(true),
        ..Default::default()
    };

    eframe::run_native("tanuki", opts, Box::new(|cc| Ok(Box::new(TanukiApp::new(cc)))))
}
