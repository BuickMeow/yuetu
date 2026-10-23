//! Vertical pipeline rail: one dot, snapping to card centers.

use egui::{Area, CursorIcon, Id, Order, Rect, Sense};

use crate::state::{PipelineStage, StageStop};
use crate::theme;

pub const WIDTH: f32 = 26.0;

/// Draw the rail over the reserved left strip. Returns the newly selected
/// stage while the dot is clicked/dragged; the dot itself always rests on a
/// card center, never between stops.
pub fn show(
    ctx: &egui::Context,
    stops: &[StageStop],
    current: PipelineStage,
    x: f32,
) -> Option<PipelineStage> {
    let top = stops
        .iter()
        .map(|stop| stop.center_y)
        .fold(f32::INFINITY, f32::min);
    let bottom = stops
        .iter()
        .map(|stop| stop.center_y)
        .fold(f32::NEG_INFINITY, f32::max);
    if !top.is_finite() || !bottom.is_finite() || bottom <= top {
        return None;
    }

    let hit = Rect::from_min_max(
        egui::pos2(x, top - 18.0),
        egui::pos2(x + WIDTH, bottom + 18.0),
    );

    Area::new(Id::unique("stage.rail"))
        .order(Order::Foreground)
        .fixed_pos(hit.min)
        .show(ctx, |ui| {
            let (_, response) = ui.allocate_exact_size(hit.size(), Sense::click_and_drag());
            let mut chosen = None;
            if response.clicked() || response.dragged() {
                if let Some(pointer) = response.interact_pointer_pos() {
                    chosen = stops
                        .iter()
                        .min_by(|a, b| {
                            (a.center_y - pointer.y)
                                .abs()
                                .total_cmp(&(b.center_y - pointer.y).abs())
                        })
                        .map(|stop| stop.stage);
                }
            }

            let center_x = x + WIDTH * 0.5;
            let track = Rect::from_min_max(
                egui::pos2(center_x - 3.0, top),
                egui::pos2(center_x + 3.0, bottom),
            );
            ui.painter()
                .rect_filled(track, 3.0, theme::accent(ui).gamma_multiply(0.28));

            let line_color = theme::muted(ui).gamma_multiply(0.8);
            ui.painter().line_segment(
                [egui::pos2(center_x, top), egui::pos2(center_x, bottom)],
                egui::Stroke::new(1.0, line_color),
            );

            for stop in stops {
                ui.painter().line_segment(
                    [
                        egui::pos2(center_x - 4.0, stop.center_y),
                        egui::pos2(center_x + 4.0, stop.center_y),
                    ],
                    egui::Stroke::new(1.0, theme::muted(ui).gamma_multiply(0.65)),
                );
            }

            let selected_y = chosen
                .and_then(|stage| stops.iter().find(|stop| stop.stage == stage))
                .or_else(|| stops.iter().find(|stop| stop.stage == current))
                .map(|stop| stop.center_y)
                .unwrap_or(top);
            let radius = if response.hovered() || response.dragged() {
                5.5
            } else {
                4.5
            };
            ui.painter()
                .circle_filled(egui::pos2(center_x, selected_y), radius, theme::accent(ui));

            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
            }
            chosen
        })
        .inner
}
