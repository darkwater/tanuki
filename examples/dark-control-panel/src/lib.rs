mod app;
pub use app::ControlPanelApp;

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: egui_winit::winit::platform::android::activity::AndroidApp) {
    use eframe::{NativeOptions, Renderer};
    use egui_winit::winit::platform::android::activity::WindowManagerFlags;

    unsafe {
        std::env::set_var("RUST_BACKTRACE", "full");
    }

    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );

    app.set_window_flags(WindowManagerFlags::FULLSCREEN, WindowManagerFlags::empty());

    let options = NativeOptions {
        android_app: Some(app),
        renderer: Renderer::Wgpu,
        vsync: false,
        hardware_acceleration: eframe::HardwareAcceleration::Required,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            present_mode: wgpu::PresentMode::AutoNoVsync,
            desired_maximum_frame_latency: Some(1),
            ..Default::default()
        },
        ..Default::default()
    };
    ControlPanelApp::run(options).unwrap();
}
