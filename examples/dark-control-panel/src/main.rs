use dark_control_panel::ControlPanelApp;
use eframe::NativeOptions;

fn main() -> Result<(), eframe::Error> {
    let options = NativeOptions::default();
    ControlPanelApp::run(options)
}
