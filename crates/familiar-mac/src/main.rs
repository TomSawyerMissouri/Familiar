//! Familiar: a face recognition trainer for macOS.

mod app;
mod history_view;
mod loader;
mod render;
mod session_view;
mod sound;
mod theme;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Familiar")
            .with_inner_size([1000.0, 780.0])
            .with_min_inner_size([720.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Familiar",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(&cc.egui_ctx)))),
    )
}
