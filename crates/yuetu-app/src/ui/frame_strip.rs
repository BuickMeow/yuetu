//! Horizontal frame browser shown below the main stage for animations.
//!
//! Small animations use thumbnails; long sequences switch to a compact
//! timeline with adaptive frame numbers and a playhead. This keeps a
//! 5000-frame sequence usable instead of allocating 5000 thumbnail cells.
//!
//! Both modes report whether the frame was changed by an active drag so the
//! app can show intermediate source frames without restarting conversion for
//! every pixel of the scrub gesture.

use egui::scroll_area::{DragScroll, ScrollBarVisibility, ScrollSource};
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, Ui};
use egui_extras::{Size, StripBuilder};
use egui_material_icons::MaterialIcon;
use egui_material_icons::icons::{ICON_VIEW_MODULE, ICON_VIEW_TIMELINE};

use crate::state::SourceImage;
use crate::theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrameStripMode {
    #[default]
    Thumbnails,
    Timeline,
}

impl FrameStripMode {
    fn toggled(self) -> Self {
        match self {
            Self::Thumbnails => Self::Timeline,
            Self::Timeline => Self::Thumbnails,
        }
    }

    fn target_label(self) -> &'static str {
        match self {
            Self::Thumbnails => "切换到时间轴",
            Self::Timeline => "切换到缩略图",
        }
    }

    fn target_icon(self) -> MaterialIcon {
        match self {
            Self::Thumbnails => ICON_VIEW_TIMELINE,
            Self::Timeline => ICON_VIEW_MODULE,
        }
    }
}

const THUMBNAIL_CELL_WIDTH: f32 = 64.0;
const THUMBNAIL_CELL_HEIGHT: f32 = 56.0;
const THUMBNAIL_CELL_GAP: f32 = 4.0;
const THUMBNAIL_STRIDE: f32 = THUMBNAIL_CELL_WIDTH + THUMBNAIL_CELL_GAP;

const TIMELINE_FRAME_WIDTH: f32 = 10.0;
const TIMELINE_HEIGHT: f32 = 56.0;
const TIMELINE_RULER_HEIGHT: f32 = 18.0;
const MODE_BUTTON_SIZE: f32 = 28.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct Output {
    /// Frame selected by a click or drag.
    pub frame: Option<usize>,
    /// Pointer is currently dragging. The source frame should update, but
    /// conversion should wait until the drag ends.
    pub scrubbing: bool,
}

/// Returns the frame clicked or dragged to in this frame.
pub fn show(
    ui: &mut Ui,
    source: &mut SourceImage,
    last_selected: &mut usize,
    mode: &mut FrameStripMode,
) -> Output {
    let frame_count = source.frame_count();
    if frame_count <= 1 {
        return Output::default();
    }

    let selected = source.frame_index();
    let scroll_to_selected = selected != *last_selected;
    *last_selected = selected;

    let current_mode = *mode;
    let mut output = Output::default();
    StripBuilder::new(ui)
        .size(Size::remainder())
        .size(Size::exact(MODE_BUTTON_SIZE))
        .horizontal(|mut strip| {
            strip.cell(|ui| {
                output = match current_mode {
                    FrameStripMode::Thumbnails => {
                        show_thumbnails(ui, source, selected, scroll_to_selected)
                    }
                    FrameStripMode::Timeline => {
                        show_timeline(ui, frame_count, selected, scroll_to_selected)
                    }
                };
            });
            strip.cell(|ui| {
                let clicked = ui
                    .centered_and_justified(|ui| mode_button(ui, current_mode))
                    .inner;
                if clicked {
                    *mode = current_mode.toggled();
                    // The two ScrollAreas keep independent offsets. Force the
                    // new mode to reveal the current frame on its first frame.
                    *last_selected = usize::MAX;
                }
            });
        });

    if let Some(index) = output.frame {
        *last_selected = index;
    }
    output
}

fn show_thumbnails(
    ui: &mut Ui,
    source: &mut SourceImage,
    selected: usize,
    scroll_to_selected: bool,
) -> Output {
    source.begin_thumbnails();
    let frame_count = source.frame_count();
    let total_width = frame_count as f32 * THUMBNAIL_STRIDE - THUMBNAIL_CELL_GAP;
    let scroll_bar_visibility = scroll_bar_visibility(ui, total_width);
    let mut output = Output::default();

    egui::ScrollArea::horizontal()
        .id_salt("frame_strip_scroll")
        .auto_shrink([false, true])
        .scroll_bar_visibility(scroll_bar_visibility)
        .scroll_source(ScrollSource {
            drag: DragScroll::Never,
            ..Default::default()
        })
        .show_viewport(ui, |ui, viewport| {
            let (content_rect, response) = ui.allocate_exact_size(
                egui::vec2(total_width, THUMBNAIL_CELL_HEIGHT),
                Sense::click_and_drag(),
            );
            let pointer = response
                .interact_pointer_pos()
                .or_else(|| response.hover_pos());
            let hovered = pointer
                .filter(|pos| content_rect.contains(*pos))
                .map(|pos| frame_at(pos.x, content_rect, THUMBNAIL_STRIDE, frame_count));

            let first = ((viewport.min.x / THUMBNAIL_STRIDE).floor() as usize).min(frame_count);
            let last = ((viewport.max.x / THUMBNAIL_STRIDE).ceil() as usize).min(frame_count);

            for index in first..last {
                let cell = Rect::from_min_size(
                    content_rect.min + egui::vec2(index as f32 * THUMBNAIL_STRIDE, 0.0),
                    egui::vec2(THUMBNAIL_CELL_WIDTH, THUMBNAIL_CELL_HEIGHT),
                );
                let is_selected = index == selected;
                let background = if hovered == Some(index) {
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
                    FontId::proportional(9.0),
                    number_color,
                );
            }

            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() || response.dragged() || response.drag_stopped() {
                if let Some(pos) = pointer {
                    output.frame =
                        Some(frame_at(pos.x, content_rect, THUMBNAIL_STRIDE, frame_count));
                }
            }
            output.scrubbing = response.dragged();

            if scroll_to_selected && (selected < first || selected >= last) {
                let selected_rect = Rect::from_min_size(
                    content_rect.min + egui::vec2(selected as f32 * THUMBNAIL_STRIDE, 0.0),
                    egui::vec2(THUMBNAIL_CELL_WIDTH, THUMBNAIL_CELL_HEIGHT),
                );
                ui.scroll_to_rect(selected_rect, Some(egui::Align::Center));
            }
        });

    output
}

fn show_timeline(
    ui: &mut Ui,
    frame_count: usize,
    selected: usize,
    scroll_to_selected: bool,
) -> Output {
    let total_width = frame_count as f32 * TIMELINE_FRAME_WIDTH;
    let scroll_bar_visibility = scroll_bar_visibility(ui, total_width);
    let mut output = Output::default();

    egui::ScrollArea::horizontal()
        .id_salt("frame_timeline_scroll")
        .auto_shrink([false, true])
        .scroll_bar_visibility(scroll_bar_visibility)
        .scroll_source(ScrollSource {
            drag: DragScroll::Never,
            ..Default::default()
        })
        .show_viewport(ui, |ui, viewport| {
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(total_width, TIMELINE_HEIGHT),
                Sense::click_and_drag(),
            );
            let painter = ui.painter();
            painter.rect_filled(rect, 0.0, theme::view_bg(ui));

            let ruler = Rect::from_min_max(
                rect.min,
                egui::pos2(rect.right(), rect.top() + TIMELINE_RULER_HEIGHT),
            );
            let track = Rect::from_min_max(egui::pos2(rect.left(), ruler.bottom()), rect.max);
            painter.rect_filled(ruler, 0.0, ui.visuals().panel_fill);
            painter.rect_filled(track, 0.0, theme::view_bg(ui));

            let first = ((viewport.min.x / TIMELINE_FRAME_WIDTH).floor() as usize).min(frame_count);
            let last = ((viewport.max.x / TIMELINE_FRAME_WIDTH).ceil() as usize).min(frame_count);
            let label_step = timeline_label_step(TIMELINE_FRAME_WIDTH);
            let tick_color = theme::faint(ui).gamma_multiply(0.55);
            let label_color = theme::faint(ui);
            let separator = Color32::from_white_alpha(14);

            for index in first..last {
                let x = rect.left() + index as f32 * TIMELINE_FRAME_WIDTH;
                let cell = Rect::from_min_max(
                    egui::pos2(x, track.top()),
                    egui::pos2(x + TIMELINE_FRAME_WIDTH, track.bottom()),
                );

                // Alternate five-frame blocks, like a compressed timeline.
                if (index / 5) % 2 == 0 {
                    painter.rect_filled(cell, 0.0, ui.visuals().faint_bg_color);
                }
                painter.line_segment(
                    [egui::pos2(x, track.top()), egui::pos2(x, track.bottom())],
                    Stroke::new(1.0, separator),
                );

                let major = (index + 1) % label_step == 0;
                painter.line_segment(
                    [
                        egui::pos2(x, ruler.bottom() - if major { 7.0 } else { 4.0 }),
                        egui::pos2(x, ruler.bottom()),
                    ],
                    Stroke::new(1.0, tick_color),
                );
                if major {
                    painter.text(
                        egui::pos2(x + 3.0, ruler.top() + 2.0),
                        Align2::LEFT_TOP,
                        format!("{}", index + 1),
                        FontId::proportional(9.0),
                        label_color,
                    );
                }
            }

            let selected_x = rect.left() + selected as f32 * TIMELINE_FRAME_WIDTH;
            let selected_cell = Rect::from_min_max(
                egui::pos2(selected_x, track.top()),
                egui::pos2(selected_x + TIMELINE_FRAME_WIDTH, track.bottom()),
            );
            painter.rect_filled(selected_cell, 0.0, ui.visuals().selection.bg_fill);

            let playhead_x = selected_x + TIMELINE_FRAME_WIDTH * 0.5;
            let playhead_color = ui.visuals().selection.stroke.color;
            painter.line_segment(
                [
                    egui::pos2(playhead_x, ruler.top()),
                    egui::pos2(playhead_x, track.bottom()),
                ],
                Stroke::new(1.0, playhead_color),
            );
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(playhead_x - 4.0, ruler.top()),
                    egui::pos2(playhead_x + 4.0, ruler.top()),
                    egui::pos2(playhead_x, ruler.top() + 6.0),
                ],
                playhead_color,
                Stroke::NONE,
            ));

            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() || response.dragged() || response.drag_stopped() {
                if let Some(pointer) = response
                    .interact_pointer_pos()
                    .or_else(|| response.hover_pos())
                {
                    output.frame =
                        Some(frame_at(pointer.x, rect, TIMELINE_FRAME_WIDTH, frame_count));
                }
            }
            output.scrubbing = response.dragged();

            if scroll_to_selected && (selected < first || selected >= last) {
                let selected_rect = Rect::from_min_size(
                    egui::pos2(selected_x, rect.top()),
                    egui::vec2(TIMELINE_FRAME_WIDTH, TIMELINE_HEIGHT),
                );
                ui.scroll_to_rect(selected_rect, Some(egui::Align::Center));
            }
        });

    output
}

fn frame_at(x: f32, content_rect: Rect, stride: f32, frame_count: usize) -> usize {
    (((x - content_rect.left()) / stride).floor() as i32)
        .clamp(0, frame_count.saturating_sub(1) as i32) as usize
}

fn timeline_label_step(frame_width: f32) -> usize {
    let target = (44.0 / frame_width.max(1.0)).ceil() as usize;
    for step in [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000] {
        if step >= target {
            return step;
        }
    }
    target.max(1)
}

fn mode_button(ui: &mut Ui, mode: FrameStripMode) -> bool {
    let icon = mode.target_icon();
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(MODE_BUTTON_SIZE, MODE_BUTTON_SIZE),
        Sense::click(),
    );
    let background = if response.hovered() {
        ui.visuals().widgets.hovered.weak_bg_fill
    } else {
        ui.visuals().widgets.inactive.weak_bg_fill
    };
    ui.painter().rect_filled(rect, 6.0, background);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon.codepoint,
        FontId::new(18.0, icon.font_family()),
        ui.visuals().text_color(),
    );
    response.on_hover_text(mode.target_label()).clicked()
}

fn scroll_bar_visibility(ui: &Ui, content_width: f32) -> ScrollBarVisibility {
    if content_width > ui.available_width() {
        ScrollBarVisibility::AlwaysVisible
    } else {
        ScrollBarVisibility::AlwaysHidden
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_labels_stay_readable_at_any_scale() {
        assert_eq!(timeline_label_step(10.0), 5);
        assert_eq!(timeline_label_step(1.0), 50);
        assert_eq!(timeline_label_step(0.0), 50);
    }

    #[test]
    fn frame_at_clamps_to_sequence() {
        let rect = Rect::from_min_size(egui::pos2(10.0, 0.0), egui::vec2(80.0, 20.0));
        assert_eq!(frame_at(10.0, rect, 10.0, 8), 0);
        assert_eq!(frame_at(89.0, rect, 10.0, 8), 7);
        assert_eq!(frame_at(200.0, rect, 10.0, 8), 7);
        assert_eq!(frame_at(0.0, rect, 10.0, 8), 0);
    }
}
