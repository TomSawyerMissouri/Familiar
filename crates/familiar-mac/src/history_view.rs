//! The history screen: exposure over sessions, coloured by accuracy.

use eframe::egui::{self, Key, RichText};
use egui_plot::{Legend, Line, Plot, Points};
use familiar_core::History;

use crate::theme;

const POINT_RADIUS: f32 = 4.5;

/// Sessions as plot points `[session number, end exposure]`, split by accuracy
/// class: good (at least 85%), fair (at least 65%) and poor.
pub fn points_by_accuracy(history: &History) -> [Vec<[f64; 2]>; 3] {
    let mut classes = [Vec::new(), Vec::new(), Vec::new()];
    for (index, session) in history.sessions.iter().enumerate() {
        let class = if session.balanced_accuracy >= 0.85 {
            0
        } else if session.balanced_accuracy >= 0.65 {
            1
        } else {
            2
        };
        classes[class].push([(index + 1) as f64, f64::from(session.exposure_end_ms)]);
    }
    classes
}

/// Draws the screen; returns `true` when the user wants to go back.
pub fn show(ui: &mut egui::Ui, history: &History) -> bool {
    let mut back = ui.input(|i| i.key_pressed(Key::Escape));
    ui.vertical_centered(|ui| {
        ui.add_space(28.0);
        ui.label(
            RichText::new("HISTORY")
                .font(theme::mono(26.0))
                .color(theme::ACCENT),
        );

        if history.sessions.is_empty() {
            ui.add_space(60.0);
            ui.label(RichText::new("No sessions yet.").color(theme::DIM));
        } else {
            let best = history
                .sessions
                .iter()
                .map(|s| s.exposure_end_ms)
                .min()
                .unwrap_or(0);
            let recent: Vec<f64> = history
                .sessions
                .iter()
                .rev()
                .take(10)
                .map(|s| s.balanced_accuracy)
                .collect();
            let average = recent.iter().sum::<f64>() / recent.len() as f64;
            ui.label(
                RichText::new(format!(
                    "{} sessions · fastest exposure {best} ms · accuracy over the last {} : {:.0}%",
                    history.sessions.len(),
                    recent.len(),
                    average * 100.0
                ))
                .color(theme::DIM),
            );
            ui.add_space(8.0);

            let [good, fair, poor] = points_by_accuracy(history);
            let all: Vec<[f64; 2]> = history
                .sessions
                .iter()
                .enumerate()
                .map(|(i, s)| [(i + 1) as f64, f64::from(s.exposure_end_ms)])
                .collect();
            let size = egui::vec2(
                (ui.available_width() - 64.0).max(200.0),
                (ui.available_height() - 90.0).max(160.0),
            );
            ui.allocate_ui(size, |ui| {
                Plot::new("history-plot")
                    .height(size.y)
                    .allow_zoom(false)
                    .allow_scroll(false)
                    .allow_drag(false)
                    .allow_boxed_zoom(false)
                    .show_background(false)
                    .include_y(0.0)
                    .include_x(0.0)
                    .x_axis_label("session")
                    .y_axis_label("exposure at the end, ms")
                    .legend(Legend::default())
                    .show(ui, |plot| {
                        plot.line(Line::new("exposure", all).color(theme::OUTLINE));
                        for (name, series, color) in [
                            ("accuracy ≥ 85%", good, theme::GOOD),
                            ("accuracy ≥ 65%", fair, theme::WARN),
                            ("accuracy < 65%", poor, theme::BAD),
                        ] {
                            plot.points(
                                Points::new(name, series)
                                    .color(color)
                                    .radius(POINT_RADIUS)
                                    .filled(true),
                            );
                        }
                    });
            });
        }

        ui.add_space(10.0);
        back |= ui.button("Back  (Esc)").clicked();
    });
    back
}

#[cfg(test)]
mod tests {
    use familiar_core::{Counts, Level, SessionRecord};

    use super::*;

    fn session(balanced_accuracy: f64, exposure_end_ms: u32) -> SessionRecord {
        SessionRecord {
            finished_at_unix_ms: 0,
            seed: 0,
            level: Level::MIN,
            trials: 80,
            counts: Counts::default(),
            balanced_accuracy,
            d_prime: 0.0,
            mean_rt_ms: None,
            exposure_start_ms: 1500,
            exposure_end_ms,
        }
    }

    #[test]
    fn sessions_are_split_at_85_and_65_percent() {
        let history = History {
            sessions: vec![
                session(0.85, 1300),
                session(0.849, 1200),
                session(0.65, 1100),
                session(0.649, 1400),
                session(1.0, 900),
            ],
            ..History::default()
        };
        let [good, fair, poor] = points_by_accuracy(&history);
        assert_eq!(good, [[1.0, 1300.0], [5.0, 900.0]]);
        assert_eq!(fair, [[2.0, 1200.0], [3.0, 1100.0]]);
        assert_eq!(poor, [[4.0, 1400.0]]);
    }
}
