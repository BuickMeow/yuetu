use egui::{RichText, Ui};
use yuetu_core::{Color, note_name};

use crate::state::{PipelineStage, StageHover, StatusKind, StatusMessage};
use crate::theme;

#[derive(Clone, Copy, Debug, Default)]
pub struct Action {
    pub set_zoom_percent: Option<f32>,
    pub fit_width: bool,
}

pub fn show(
    ui: &mut Ui,
    status: &StatusMessage,
    stage: PipelineStage,
    hover: Option<StageHover>,
    zoom_percent: f32,
    zoom_enabled: bool,
) -> Action {
    let mut action = Action::default();
    let status_color = match status.kind {
        StatusKind::Info => theme::muted(ui),
        StatusKind::Success => theme::success(ui),
        StatusKind::Warning => theme::warning(ui),
        StatusKind::Error => theme::danger(ui),
    };

    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.5, status_color);
        ui.label(RichText::new(status.text.as_str()).color(status_color));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(zoom_enabled, egui::Button::new("适宽"))
                .clicked()
            {
                action.fit_width = true;
            }
            if ui
                .add_enabled(zoom_enabled, egui::Button::new("100%"))
                .clicked()
            {
                action.set_zoom_percent = Some(100.0);
            }

            let mut percent = zoom_percent;
            let response = ui.add_enabled(
                zoom_enabled,
                egui::DragValue::new(&mut percent)
                    .range(1.0..=6400.0)
                    .speed(1.0)
                    .max_decimals(0)
                    .suffix("%"),
            );
            if response.changed() {
                action.set_zoom_percent = Some(percent);
            }

            ui.add_space(6.0);
            ui.label(stage.label());

            if let Some(hover) = hover {
                ui.add_space(10.0);
                match hover {
                    StageHover::Source { pos, color } => {
                        color_swatch(ui, color);
                        ui.label(format!("({}, {}) {}", pos.0, pos.1, color.hex()));
                    }
                    StageHover::KeyMap { pos, color, key } => {
                        color_swatch(ui, color);
                        let mapped = key.map(note_name).unwrap_or_else(|| "静音".to_owned());
                        ui.label(format!("({}, {}) · {}", pos.0, pos.1, mapped));
                    }
                    StageHover::Palette { index, color } => {
                        color_swatch(ui, color);
                        if index < 0 {
                            ui.label(format!("索引 {index} · 静音"));
                        } else {
                            ui.label(format!("索引 {index} · {}", color.hex()));
                        }
                    }
                    StageHover::Midi { key, tick } => {
                        ui.label(format!("{} · tick {}", note_name(key), tick));
                    }
                }
            }
        });
    });

    action
}

fn color_swatch(ui: &mut Ui, color: Color) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    let color = egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a);
    ui.painter().rect_filled(rect, 2.0, color);
}
