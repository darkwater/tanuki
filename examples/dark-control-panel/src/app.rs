use eframe::NativeOptions;
#[cfg(target_os = "android")]
use egui::Vec2;
use egui::{Color32, RichText};

#[derive(Default)]
pub struct ControlPanelApp {
    delay_us: u32,
    power: f32,
    events: Vec<egui::Event>,

    #[cfg(target_os = "android")]
    last_raw_pos: Option<egui::Pos2>,
    #[cfg(target_os = "android")]
    last_raw_delta: Vec2,
}

impl ControlPanelApp {
    pub fn run(options: NativeOptions) -> Result<(), eframe::Error> {
        eframe::run_native(
            "dark-control-panel",
            options,
            Box::new(|_cc| Ok(Box::<ControlPanelApp>::default())),
        )
    }
}

impl eframe::App for ControlPanelApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        ctx.style_mut(|s| s.visuals.panel_fill = Color32::BLACK);

        let now = chrono::Local::now();

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label(RichText::new(now.format("%H:%M").to_string()).size(400.));

            ui.spacing_mut().slider_width = ui.available_width() - 150.;

            ui.add(egui::Slider::new(&mut self.delay_us, 0..=16_667).text("Delay us"));
            ui.add(egui::Slider::new(&mut self.power, 0.1..=5.0).text("Power"));

            if let Some(time) = frame.info().cpu_usage {
                ui.label(format!("{:.2}", time * 1000.));
            }

            for event in &self.events {
                ui.label(format!("{:?}", event));
            }

            let delta = ctx.input(|i| i.pointer.delta());
            ui.label(format!("Pointer delta: {:?}", delta));
        });

        if let Some(pos) = ctx.pointer_hover_pos() {
            ctx.debug_painter()
                .circle_stroke(pos, 50., (1., Color32::RED));
        }

        std::thread::sleep(std::time::Duration::from_micros(self.delay_us as u64));
    }

    #[cfg(target_os = "android")]
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.events = raw_input.events.clone();

        let last_pos = self.last_raw_pos;
        let last_delta = self.last_raw_delta;
        self.last_raw_pos = None;
        self.last_raw_delta = Vec2::ZERO;

        for event in &mut raw_input.events {
            match event {
                egui::Event::PointerMoved(pos) | egui::Event::Touch { pos, .. } => {
                    self.last_raw_pos = Some(*pos);

                    if let Some(last_pos) = last_pos {
                        let now_delta = *pos - last_pos;
                        let predicted_delta = now_delta + (now_delta - last_delta);

                        self.last_raw_delta = now_delta;

                        let power = (predicted_delta.length_sq() / (25. * 25.)).clamp(0., 1.);

                        *pos += predicted_delta * self.power * power;
                    }
                }
                _ => {}
            }
        }
    }
}
