//! Horizontal frame browser shown below the main stage for animations.
//!
//! The strip is intentionally lightweight: only the visible cells ask
//! `SourceImage` for a thumbnail, so a 1000-frame GIF does not upload 1000
//! textures when it is opened. Cells are fixed-width like a timeline, and the
//! thumbnail keeps its aspect ratio inside each cell.

use egui::scroll_area::ScrollBarVisibility;
use egui::{Align2, Color32, FontId, Rect, Sense, Ui};

use crate::state::SourceImage;
use crate::theme;

const CELL_WIDTH: f32 = 64.0;
const CELL_HEIGHT: f32 = 56.0;
const CELL_GAP: f32 = 4.0;
const STRIDE: f32 = CELL_WIDTH + CELL_GAP;

/// Returns the frame clicked in this frame.
pub fn show(ui: &mut Ui, source: &mut SourceImage, last_selected: &mut usize) -> Option<usize> {
    let frame_count = source.frame_count();
    if frame_count <= 1 {
        return None;
    }

    let total_width = frame_count as f32 * STRIDE - CELL_GAP;
    // Use an explicit bar for long strips instead of relying on the
    // "visible when needed" heuristic; short animations get no bar at all.
    let scroll_bar_visibility = if total_width > ui.available_width() {
        ScrollBarVisibility::AlwaysVisible
    } else {
        ScrollBarVisibility::AlwaysHidden
    };
    let selected = source.frame_index();
    let scroll_to_selected = selected != *last_selected;
    *last_selected = selected;
    let selected_before = selected;
    let mut chosen = None;

    egui::ScrollArea::horizontal()
        .id_salt("frame_strip_scroll")
        .auto_shrink([false, true])
        .scroll_bar_visibility(scroll_bar_visibility)
        .show_viewport(ui, |ui, viewport| {
            let (content_rect, _) =
                ui.allocate_exact_size(egui::vec2(total_width, CELL_HEIGHT), Sense::hover());

            let first = ((viewport.min.x / STRIDE).floor() as usize).min(frame_count);
            let last = ((viewport.max.x / STRIDE).ceil() as usize).min(frame_count);

            for index in first..last {
                let cell = Rect::from_min_size(
                    content_rect.min + egui::vec2(index as f32 * STRIDE, 0.0),
                    egui::vec2(CELL_WIDTH, CELL_HEIGHT),
                );
                let response =
                    ui.interact(cell, ui.scope_id().with(("frame", index)), Sense::click());
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }

                let is_selected = index == selected_before;
                let background = if response.hovered() {
                    ui.visuals().widgets.hovered.weak_bg_fill
                } else {
                    theme::view_bg(ui)
                };
                ui.painter().rect_filled(cell, 4.0, background);

                let image_area = Rect::from_min_max(
                    cell.min + egui::vec2(3.0, 3.0),
                    cell.max - egui::vec2(3.0, 12.0),
                );
                if let Some((texture, size)) = source.thumbnail(ui.ctx(), index) {
                    let scale = (image_area.width() / size.x.max(1.0))
                        .min(image_area.height() / size.y.max(1.0));
                    let image_rect =
                        Rect::from_center_size(image_area.center(), size * scale.max(0.01));
                    ui.painter().image(
                        texture,
                        image_rect,
                        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }

                if is_selected {
                    // A filled number band instead of a border: selected state
                    // is expressed by color, matching the app's control style.
                    let band = Rect::from_min_max(
                        egui::pos2(cell.left() + 3.0, cell.bottom() - 11.0),
                        egui::pos2(cell.right() - 3.0, cell.bottom() - 3.0),
                    );
                    ui.painter()
                        .rect_filled(band, 2.0, ui.visuals().selection.bg_fill);
                }

                let number_color = if is_selected {
                    ui.visuals().strong_text_color()
                } else {
                    theme::faint(ui)
                };
                ui.painter().text(
                    cell.left_bottom() + egui::vec2(5.0, -3.0),
                    Align2::LEFT_BOTTOM,
                    format!("{}", index + 1),
                    FontId::monospace(9.0),
                    number_color,
                );

                if response.clicked() {
                    chosen = Some(index);
                }
            }

            // If the current frame changed outside the strip (for example via
            // the frame DragValue), bring it into view. A frame clicked inside
            // the strip is already visible, so it does not jump.
            if scroll_to_selected && (selected_before < first || selected_before >= last) {
                let selected_rect = Rect::from_min_size(
                    content_rect.min + egui::vec2(selected_before as f32 * STRIDE, 0.0),
                    egui::vec2(CELL_WIDTH, CELL_HEIGHT),
                );
                ui.scroll_to_rect(selected_rect, Some(egui::Align::Center));
            }
        });

    if let Some(index) = chosen {
        *last_selected = index;
    }
    chosen
}
