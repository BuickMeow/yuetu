//! Piano keyboard strip for the piano-roll preview.
//!
//! The keyboard is deliberately drawn with the same low-contrast line color as
//! the roll's pitch lanes: white-key separators are lines, and black keys are
//! short solid slots. This keeps the keyboard readable without introducing a
//! second palette next to the notes.

use egui::{Align2, Color32, FontId, Painter, Rect, Stroke, Ui};
use yuetu_core::{KeyMap, is_white_key, note_name};

use crate::theme;

/// Keyboard thickness as a fraction of the image's time-axis extent.
pub const THICKNESS_RATIO: f32 = 0.07;

/// How far a black key extends across the keyboard strip.
const BLACK_KEY_DEPTH: f32 = 0.62;
const LABEL_MIN_LANE: f32 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    /// Keys run left-to-right; the keyboard sits under the roll.
    Horizontal,
    /// Keys run bottom-to-top; the keyboard sits to the left of the roll.
    Vertical,
}

/// Draw a keyboard strip aligned with the roll's key axis.
///
/// `content` is the roll's content rect in screen space, so pan/zoom keeps the
/// keyboard aligned with the notes. The caller owns the background and clipping
/// rect; `strip` is the reserved keyboard area.
pub fn show(
    ui: &Ui,
    painter: &Painter,
    keymap: &KeyMap,
    strip: Rect,
    content: Rect,
    key_scale: f32,
    orientation: Orientation,
) {
    if strip.width() <= 0.0 || strip.height() <= 0.0 || key_scale <= 0.0 {
        return;
    }

    // The roll grid line is translucent. Composite it once over the keyboard
    // background so the black-key fill and every stroke use the exact same
    // opaque color instead of stacking alpha at their shared edges.
    let line = theme::view_bg(ui).blend(theme::roll_line(ui));
    let label = theme::faint(ui);
    // The keyboard lives outside the roll content, so clipping to their
    // intersection would erase it. Clip only the key axis to the content.
    let clip = match orientation {
        Orientation::Horizontal => Rect::from_min_max(
            egui::pos2(content.left(), strip.top()),
            egui::pos2(content.right(), strip.bottom()),
        ),
        Orientation::Vertical => Rect::from_min_max(
            egui::pos2(strip.left(), content.top()),
            egui::pos2(strip.right(), content.bottom()),
        ),
    };
    let painter = painter.with_clip_rect(clip.intersect(strip));

    // Solid black keys first. Their full lane width makes both edges coincide
    // with the white-key separators instead of floating inside a lane.
    for position in 0..keymap.width() {
        let Some(key) = keymap.key_at(position) else {
            continue;
        };
        if is_white_key(key) {
            continue;
        }
        let lane = lane_rect(strip, content, key_scale, position, orientation);
        let black = black_key_rect(strip, lane, orientation);
        painter.rect_filled(black, 0.0, line);
        draw_black_tip(&painter, black, orientation, line);
    }

    // White-key separators cover every key lane, including filtered-out
    // columns, so clipped mappings still show the silent gaps.
    let count = keymap.width();
    for position in 0..=count {
        let x_or_y = lane_origin(content, key_scale, position, orientation);
        match orientation {
            Orientation::Horizontal => {
                if x_or_y >= strip.left() && x_or_y <= strip.right() {
                    painter.line_segment(
                        [
                            egui::pos2(x_or_y, strip.top()),
                            egui::pos2(x_or_y, strip.bottom()),
                        ],
                        Stroke::new(1.0, line),
                    );
                }
            }
            Orientation::Vertical => {
                if x_or_y >= strip.top() && x_or_y <= strip.bottom() {
                    painter.line_segment(
                        [
                            egui::pos2(strip.left(), x_or_y),
                            egui::pos2(strip.right(), x_or_y),
                        ],
                        Stroke::new(1.0, line),
                    );
                }
            }
        }
    }

    for position in 0..count {
        let Some(key) = keymap.key_at(position) else {
            continue;
        };
        if key % 12 == 0 {
            draw_label(
                &painter,
                strip,
                content,
                key_scale,
                position,
                key,
                orientation,
                label,
            );
        }
    }

    draw_edge(&painter, strip, orientation, line);
}

fn draw_black_tip(painter: &Painter, black: Rect, orientation: Orientation, line: Color32) {
    let stroke = Stroke::new(1.0, line);
    match orientation {
        Orientation::Horizontal => {
            painter.line_segment(
                [
                    egui::pos2(black.left(), black.bottom()),
                    egui::pos2(black.right(), black.bottom()),
                ],
                stroke,
            );
        }
        Orientation::Vertical => {
            painter.line_segment(
                [
                    egui::pos2(black.right(), black.top()),
                    egui::pos2(black.right(), black.bottom()),
                ],
                stroke,
            );
        }
    }
}

fn draw_edge(painter: &Painter, strip: Rect, orientation: Orientation, line: Color32) {
    let stroke = Stroke::new(1.0, line);
    match orientation {
        Orientation::Horizontal => {
            for y in [strip.top(), strip.bottom()] {
                painter.line_segment(
                    [egui::pos2(strip.left(), y), egui::pos2(strip.right(), y)],
                    stroke,
                );
            }
        }
        Orientation::Vertical => {
            for x in [strip.left(), strip.right()] {
                painter.line_segment(
                    [egui::pos2(x, strip.top()), egui::pos2(x, strip.bottom())],
                    stroke,
                );
            }
        }
    }
}

fn draw_label(
    painter: &Painter,
    strip: Rect,
    content: Rect,
    key_scale: f32,
    position: u32,
    key: u8,
    orientation: Orientation,
    color: Color32,
) {
    if key_scale < LABEL_MIN_LANE {
        return;
    }
    let font = FontId::proportional(8.5);
    match orientation {
        Orientation::Horizontal => {
            let lane = lane_rect(strip, content, key_scale, position, orientation);
            painter.text(
                egui::pos2(lane.center().x, strip.bottom() - 3.0),
                Align2::CENTER_BOTTOM,
                note_name(key),
                font,
                color,
            );
        }
        Orientation::Vertical => {
            let lane = lane_rect(strip, content, key_scale, position, orientation);
            painter.text(
                egui::pos2(strip.right() - 4.0, lane.center().y),
                Align2::RIGHT_CENTER,
                note_name(key),
                font,
                color,
            );
        }
    }
}

fn lane_origin(content: Rect, scale: f32, position: u32, orientation: Orientation) -> f32 {
    match orientation {
        Orientation::Horizontal => content.left() + position as f32 * scale,
        Orientation::Vertical => content.bottom() - position as f32 * scale,
    }
}

fn lane_rect(
    strip: Rect,
    content: Rect,
    scale: f32,
    position: u32,
    orientation: Orientation,
) -> Rect {
    match orientation {
        Orientation::Horizontal => {
            let x = lane_origin(content, scale, position, orientation);
            Rect::from_min_max(
                egui::pos2(x, strip.top()),
                egui::pos2(x + scale, strip.bottom()),
            )
        }
        Orientation::Vertical => {
            let y = lane_origin(content, scale, position, orientation);
            Rect::from_min_max(
                egui::pos2(strip.left(), y - scale),
                egui::pos2(strip.right(), y),
            )
        }
    }
}

fn black_key_rect(strip: Rect, lane: Rect, orientation: Orientation) -> Rect {
    match orientation {
        Orientation::Horizontal => {
            let depth = strip.height() * BLACK_KEY_DEPTH;
            Rect::from_min_max(
                egui::pos2(lane.left(), strip.top()),
                egui::pos2(lane.right(), strip.top() + depth),
            )
        }
        Orientation::Vertical => {
            let depth = strip.width() * BLACK_KEY_DEPTH;
            Rect::from_min_max(
                egui::pos2(strip.left(), lane.top()),
                egui::pos2(strip.left() + depth, lane.bottom()),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yuetu_core::{ColumnMapping, KeyFilter};

    fn keymap() -> KeyMap {
        KeyMap::build(60, 64, KeyFilter::All, ColumnMapping::Filled).unwrap()
    }

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.001
    }

    #[test]
    fn horizontal_keys_align_with_content_columns() {
        let strip = Rect::from_min_max(egui::pos2(20.0, 200.0), egui::pos2(220.0, 246.0));
        let content = Rect::from_min_max(egui::pos2(20.0, 0.0), egui::pos2(220.0, 200.0));
        let lane = lane_rect(strip, content, 20.0, 3, Orientation::Horizontal);
        assert!(approx(lane.left(), 80.0));
        assert!(approx(lane.right(), 100.0));
        assert!(approx(lane.top(), 200.0));
        assert!(approx(lane.bottom(), 246.0));
    }

    #[test]
    fn vertical_keys_ascend_from_the_bottom() {
        let strip = Rect::from_min_max(egui::pos2(0.0, 20.0), egui::pos2(46.0, 220.0));
        let content = Rect::from_min_max(egui::pos2(46.0, 20.0), egui::pos2(246.0, 220.0));
        let lane = lane_rect(strip, content, 20.0, 3, Orientation::Vertical);
        assert!(approx(lane.top(), 140.0));
        assert!(approx(lane.bottom(), 160.0));
        assert!(approx(lane.left(), 0.0));
        assert!(approx(lane.right(), 46.0));
    }

    #[test]
    fn horizontal_black_keys_are_short_and_anchored_to_the_roll() {
        let strip = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 40.0));
        let content = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 200.0));
        let lane = lane_rect(strip, content, 20.0, 1, Orientation::Horizontal);
        let black = black_key_rect(strip, lane, Orientation::Horizontal);
        assert!(approx(black.width(), 20.0));
        assert!(approx(black.height(), 24.8));
        assert!(approx(black.left(), lane.left()));
        assert!(approx(black.right(), lane.right()));
        assert!(approx(black.top(), 0.0));
    }

    #[test]
    fn vertical_black_keys_stay_anchored_to_the_outer_edge() {
        let strip = Rect::from_min_max(egui::pos2(0.0, 20.0), egui::pos2(46.0, 220.0));
        let content = Rect::from_min_max(egui::pos2(46.0, 20.0), egui::pos2(246.0, 220.0));
        let lane = lane_rect(strip, content, 20.0, 1, Orientation::Vertical);
        let black = black_key_rect(strip, lane, Orientation::Vertical);
        assert!(approx(black.width(), 28.52));
        assert!(approx(black.height(), 20.0));
        assert!(approx(black.left(), 0.0));
        assert!(approx(black.top(), lane.top()));
        assert!(approx(black.bottom(), lane.bottom()));
    }

    #[test]
    fn keymap_is_known_to_the_widget() {
        // Geometry is independent of the key map, but this guards the mapping
        // shape used by the labels and black-key pass.
        assert_eq!(keymap().width(), 5);
        assert_eq!(keymap().key_at(0), Some(60));
        assert!(!is_white_key(61));
    }
}
