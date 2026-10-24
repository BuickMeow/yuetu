//! Vertical pipeline rail with Material Icons at every stage.

use egui::{Area, CursorIcon, FontId, Id, Order, Rect, Sense};
use egui_material_icons::MaterialIcon;
use egui_material_icons::icons::{ICON_IMAGE, ICON_MUSIC_NOTE, ICON_PALETTE, ICON_PIANO};

use crate::state::{PipelineStage, StageStop};
use crate::theme;

pub const WIDTH: f32 = 32.0;
const THUMB_RADIUS: f32 = 12.0;
/// Keep the track's width and corner radius identical to the active circle.
const TRACK_W: f32 = THUMB_RADIUS * 2.0;
const TRACK_RADIUS: f32 = THUMB_RADIUS;
const ICON_SIZE: f32 = 18.0;

fn stage_icon(stage: PipelineStage) -> MaterialIcon {
    match stage {
        PipelineStage::Source => ICON_IMAGE,
        PipelineStage::KeyMap => ICON_PIANO,
        PipelineStage::Palette => ICON_PALETTE,
        PipelineStage::Midi => ICON_MUSIC_NOTE,
    }
}

/// Draw the rail over the reserved left strip. Returns the newly selected
/// stage while the thumb is clicked/dragged; it always rests on a card center.
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
        egui::pos2(x, top - 20.0),
        egui::pos2(x + WIDTH, bottom + 20.0),
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
                egui::pos2(center_x - TRACK_W * 0.5, top),
                egui::pos2(center_x + TRACK_W * 0.5, bottom),
            );
            ui.painter()
                .rect_filled(track, TRACK_RADIUS, ui.visuals().widgets.inactive.bg_fill);

            let selected_stage = chosen.unwrap_or(current);
            let selected_y = stops
                .iter()
                .find(|stop| stop.stage == selected_stage)
                .map(|stop| stop.center_y)
                .unwrap_or(top);
            ui.painter().circle_filled(
                egui::pos2(center_x, selected_y),
                THUMB_RADIUS,
                theme::accent(ui),
            );

            let pointer_y = response.hover_pos().map(|pos| pos.y);
            let hovered_stage = pointer_y.and_then(|y| {
                stops
                    .iter()
                    .min_by(|a, b| (a.center_y - y).abs().total_cmp(&(b.center_y - y).abs()))
                    .map(|stop| stop.stage)
            });

            for stop in stops {
                let icon = stage_icon(stop.stage);
                let color = if stop.stage == selected_stage {
                    ui.visuals().panel_fill
                } else if hovered_stage == Some(stop.stage) {
                    ui.visuals().text_color()
                } else {
                    theme::muted(ui)
                };
                ui.painter().text(
                    egui::pos2(center_x, stop.center_y),
                    egui::Align2::CENTER_CENTER,
                    icon.codepoint,
                    FontId::new(ICON_SIZE, icon.font_family()),
                    color,
                );
            }

            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
            }
            chosen
        })
        .inner
}
