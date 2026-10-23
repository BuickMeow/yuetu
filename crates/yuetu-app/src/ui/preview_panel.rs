use egui::{RichText, Ui};

use crate::jobs::ProgressView;
use crate::state::{Preview, PreviewHover};
use crate::theme;
use crate::widgets::{View2D, midi_preview};

#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    preview: Option<&Preview>,
    stale: bool,
    is_running: bool,
    progress: Option<&ProgressView>,
    view: &mut View2D,
    hover: &mut Option<PreviewHover>,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("MIDI 预览").strong());
        if let Some(preview) = preview {
            ui.label(format!(
                "{} 音符 · {} 音轨",
                preview.result.note_count(),
                preview.result.track_count()
            ));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("适配").clicked() {
                view.reset_fit();
            }
        });
    });

    if is_running {
        if let Some(progress) = progress {
            ui.add(
                egui::ProgressBar::new(progress.overall)
                    .desired_width(ui.available_width())
                    .text(format!(
                        "{} {:.0}%",
                        progress.label(),
                        progress.overall * 100.0
                    ))
                    .animate(true),
            );
        } else {
            ui.add(
                egui::ProgressBar::new(0.0)
                    .desired_width(ui.available_width())
                    .text("准备中…")
                    .animate(true),
            );
        }
    } else if stale {
        ui.label(RichText::new("预览已过期").color(theme::warning(ui)));
    }
    ui.add_space(6.0);

    let Some(preview) = preview else {
        *hover = None;
        theme::view_frame(ui).show(ui, |ui| {
            let available = ui.available_size();
            ui.set_min_size(egui::vec2(available.x, available.y.max(200.0)));
            ui.vertical_centered(|ui| {
                ui.add_space((available.y * 0.3).max(30.0));
                ui.label(RichText::new("暂无预览").size(15.0));
            });
        });
        return;
    };

    let output = midi_preview::show(ui, &preview.result, view);
    *hover = output.hovered;
}
