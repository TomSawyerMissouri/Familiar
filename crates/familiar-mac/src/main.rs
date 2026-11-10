//! Familiar: a face recognition trainer for macOS.

mod app;
mod history_view;
mod loader;
mod render;
mod session_view;
mod sound;
mod theme;

use eframe::egui;

/// The app icon, drawn by `scripts/make_icon.py`.
const ICON_PNG: &[u8] = include_bytes!("../icons/icon_256x256@2x.png");

/// The icon shown in the Dock while the app runs.
///
/// eframe sets the application icon at startup and, given none, uses its own
/// (a white "e"), which would replace the icon of the bundle.
fn app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(ICON_PNG).unwrap_or_else(|error| {
        eprintln!("familiar: the app icon could not be decoded: {error}");
        // An empty icon tells eframe to leave the system's icon alone.
        egui::IconData::default()
    })
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Familiar")
            .with_icon(app_icon())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_icon_decodes_to_a_square_rgba_image() {
        let icon = app_icon();
        assert_eq!((icon.width, icon.height), (512, 512));
        assert_eq!(icon.rgba.len(), 512 * 512 * 4);
        // Transparent corner, opaque centre: it is the icon, not a blank.
        assert_eq!(icon.rgba[3], 0);
        let centre = (256 * 512 + 256) * 4;
        assert_eq!(icon.rgba[centre + 3], 255);
    }
}
