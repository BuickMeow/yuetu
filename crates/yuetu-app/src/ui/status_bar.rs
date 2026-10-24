use egui::{RichText, Ui};
use egui_extras::{Size, StripBuilder};
use egui_material_icons::MaterialIcon;
use egui_material_icons::icons::{ICON_FIT_SCREEN, ICON_FULLSCREEN, ICON_PERCENT};
use yuetu_core::{Color, note_name};

use crate::state::{
    CoordinateOrigin, PipelineStage, StageHover, StatusKind, StatusMessage, ZoomPreset,
};
use crate::theme;

const CONTROL_SIZE: egui::Vec2 = egui::vec2(64.0, 26.0);
const STAGE_CELL_WIDTH: f32 = 80.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct Action {
    pub set_zoom_percent: Option<f32>,
    pub zoom_preset: Option<ZoomPreset>,
}

pub fn show(
    ui: &mut Ui,
    status: &StatusMessage,
    stage: PipelineStage,
    hover: Option<StageHover>,
    coordinate_origin: CoordinateOrigin,
    zoom_percent: f32,
    zoom_mode: ZoomPreset,
    zoom_enabled: bool,
) -> Action {
    let mut action = Action::default();
    let status_color = match status.kind {
        StatusKind::Info => theme::muted(ui),
        StatusKind::Success => theme::success(ui),
        StatusKind::Warning => theme::warning(ui),
        StatusKind::Error => theme::danger(ui),
    };

    // One exact-height row. StripBuilder owns the column allocation; the
    // panel frame owns the outer top/right/bottom margins.
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), CONTROL_SIZE.y),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            StripBuilder::new(ui)
                .size(Size::remainder())
                .size(Size::exact(STAGE_CELL_WIDTH))
                .size(Size::exact(CONTROL_SIZE.x))
                .size(Size::exact(CONTROL_SIZE.x))
                .horizontal(|mut strip| {
                    strip.cell(|ui| {
                        status_cell(ui, status, status_color, hover, coordinate_origin);
                    });

                    strip.cell(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(stage.label());
                        });
                    });

                    strip.cell(|ui| {
                        let mut percent = zoom_percent;
                        let response = ui
                            .add_enabled_ui(zoom_enabled, |ui| {
                                ui.add_sized(
                                    CONTROL_SIZE,
                                    egui::DragValue::new(&mut percent)
                                        .range(1.0..=6400.0)
                                        .speed(1.0)
                                        .max_decimals(0)
                                        .suffix("%"),
                                )
                            })
                            .inner;
                        if response.changed() {
                            action.set_zoom_percent = Some(percent);
                        }
                    });

                    strip.cell(|ui| {
                        let response =
                            icon_button(ui, zoom_icon(zoom_mode), zoom_enabled, zoom_mode.label());
                        if response.clicked() {
                            action.zoom_preset = Some(zoom_mode.next());
                        }
                    });
                });
        },
    );

    action
}

fn status_cell(
    ui: &mut Ui,
    status: &StatusMessage,
    status_color: egui::Color32,
    hover: Option<StageHover>,
    coordinate_origin: CoordinateOrigin,
) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.5, status_color);
        ui.label(RichText::new(status.text.as_str()).color(status_color));

        if let Some(hover) = hover {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                hover_info(ui, hover, coordinate_origin)
            });
        }
    });
}

fn hover_info(ui: &mut Ui, hover: StageHover, coordinate_origin: CoordinateOrigin) {
    match hover {
        StageHover::Source { pos, color, height } => {
            color_swatch(ui, color);
            let y = coordinate_origin.display_y(pos.1, height);
            ui.label(format!("({}, {}) {}", pos.0, y, color.hex()));
        }
        StageHover::KeyMap {
            pos,
            color,
            key,
            height,
        } => {
            color_swatch(ui, color);
            let mapped = key.map(note_name).unwrap_or_else(|| "静音".to_owned());
            let y = coordinate_origin.display_y(pos.1, height);
            ui.label(format!("({}, {}) · {}", pos.0, y, mapped));
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

fn zoom_icon(mode: ZoomPreset) -> MaterialIcon {
    match mode {
        ZoomPreset::Fit => ICON_FIT_SCREEN,
        ZoomPreset::Fill => ICON_FULLSCREEN,
        ZoomPreset::Actual => ICON_PERCENT,
    }
}

fn icon_button(ui: &mut Ui, icon: MaterialIcon, enabled: bool, tooltip: &str) -> egui::Response {
    let text = RichText::new(icon.codepoint)
        .family(icon.font_family())
        .size(18.0);
    ui.add_enabled_ui(enabled, |ui| {
        ui.add_sized(CONTROL_SIZE, egui::Button::new(text))
    })
    .inner
    .on_hover_text(tooltip)
}

fn color_swatch(ui: &mut Ui, color: Color) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    let color = egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a);
    ui.painter().rect_filled(rect, 2.0, color);
}
