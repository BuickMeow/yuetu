//! Custom split handles.
//!
//! egui's built-in panel separator is a thin line drawn by `Panel` with a
//! grab radius controlled by the style. This widget instead gives a fixed
//! visual line plus a small, predictable hit area, and returns the drag
//! response so the caller can own the split ratio/size.
//!
//! Adapted from the same interaction model used in Yinhe's split handle.

use egui::{Area, CursorIcon, Id, Order, Rangef, Rect, Response, Sense, Ui};

/// Extra hit area on each side of the 2 px visual line.
const HIT_PAD: f32 = 4.0;
const LINE_W: f32 = 2.0;

pub fn vertical(
    ctx: &egui::Context,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    x: f32,
    y_range: Rangef,
) -> Response {
    let line = Rect::from_x_y_ranges((x - LINE_W * 0.5)..=(x + LINE_W * 0.5), y_range);
    let hit = line.expand2(egui::vec2(HIT_PAD, 0.0));

    Area::new(Id::unique(id_salt))
        .order(Order::Foreground)
        .fixed_pos(hit.min)
        .show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(hit.size(), Sense::click_and_drag());
            let on_rect = ui.input(|input| {
                input
                    .pointer
                    .interact_pos()
                    .is_some_and(|pos| hit.contains(pos))
            });
            // The handle moves with the panel during a drag, so do not gate
            // `active` on the original press position.
            let active = response.dragged();
            let hovered = on_rect && response.hovered();

            ui.painter().rect_filled(
                Rect::from_min_size(
                    rect.min + egui::vec2((hit.width() - LINE_W) * 0.5, 0.0),
                    egui::vec2(LINE_W, rect.height()),
                ),
                0.0,
                split_color(ui, active, hovered),
            );
            if hovered || active {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            response
        })
        .inner
}

fn split_color(ui: &Ui, active: bool, hovered: bool) -> egui::Color32 {
    let visuals = ui.visuals();
    if active {
        visuals.widgets.active.fg_stroke.color
    } else if hovered {
        visuals.widgets.hovered.fg_stroke.color
    } else {
        visuals
            .widgets
            .noninteractive
            .fg_stroke
            .color
            .gamma_multiply(0.45)
    }
}
