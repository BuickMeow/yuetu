//! MIDI preview: a piano roll in the same orientation and aspect ratio as the
//! source image.
//!
//! Vertical time: X=keys, Y=time, time flows upward.
//! Horizontal time: X=time, Y=keys, time flows right.
//!
//! Bar lines follow the MIDI output settings, not the image grid.

use std::sync::Arc;

use egui::{Color32, CursorIcon, FontId, Rect, Stroke, Ui};
use yuetu_core::{ConversionResult, MidiOutputConfig, TimeAxis, is_white_key, note_name};

use crate::state::StageHover;
use crate::theme;
use crate::widgets::midi_gpu::{MidiGpuCallback, MidiGpuData};
use crate::widgets::piano;
use crate::widgets::view2d::View2D;

pub struct PreviewOutput {
    pub hovered: Option<StageHover>,
}

pub fn show(
    ui: &mut Ui,
    result: &ConversionResult,
    midi_config: &MidiOutputConfig,
    midi_gpu: &Arc<MidiGpuData>,
    view: &mut View2D,
    gpu_target_format: eframe::wgpu::TextureFormat,
) -> PreviewOutput {
    let display_size = egui::vec2(
        result.source_size.0.max(1) as f32,
        result.source_size.1.max(1) as f32,
    );
    let horizontal = result.time_axis == TimeAxis::Horizontal;
    let ratio = piano::THICKNESS_RATIO;
    // Feed View2D a virtual canvas that includes the keyboard. The image and
    // keyboard therefore scale as one fixed-ratio object: no viewport-edge gap,
    // no separate fixed-height strip.
    let layout_size = if horizontal {
        egui::vec2(display_size.x * (1.0 + ratio), display_size.y)
    } else {
        egui::vec2(display_size.x, display_size.y * (1.0 + ratio))
    };
    let interaction = view.interact(ui, layout_size, CursorIcon::Crosshair);
    let painter = ui.painter_at(interaction.viewport);
    painter.rect_filled(interaction.viewport, 0.0, theme::view_bg(ui));

    let (content, piano_rect) = split_keyboard(interaction.content_rect, horizontal);

    let key_count = result.keymap.width().max(1) as f32;
    let time_rows = result.time_rows.max(1) as f32;
    let (x_units, y_units) = if horizontal {
        (time_rows, key_count)
    } else {
        (key_count, time_rows)
    };
    let scale_x = content.width() / x_units;
    let scale_y = content.height() / y_units;
    let keymap = &result.keymap;

    if result.notes.is_empty() {
        painter.text(
            interaction.viewport.center(),
            egui::Align2::CENTER_CENTER,
            "没有生成音符",
            FontId::proportional(14.0),
            theme::muted(ui),
        );
        draw_keyboard(
            ui, result, content, piano_rect, horizontal, scale_x, scale_y,
        );
        return PreviewOutput { hovered: None };
    }

    // ── Pitch lanes ────────────────────────────────────────────────
    for position in 0..keymap.width() {
        let fill = match keymap.key_at(position) {
            Some(key) if is_white_key(key) => theme::lane_white(ui),
            Some(_) => theme::lane_black(ui),
            None => theme::lane_off(ui),
        };
        let rect = if horizontal {
            let y1 = content.bottom() - position as f32 * scale_y;
            let y0 = y1 - scale_y;
            Rect::from_min_max(
                egui::pos2(content.left(), y0),
                egui::pos2(content.right(), y1),
            )
        } else {
            let x0 = content.left() + position as f32 * scale_x;
            let x1 = x0 + scale_x;
            Rect::from_min_max(
                egui::pos2(x0, content.top()),
                egui::pos2(x1, content.bottom()),
            )
        };
        if rect.intersects(interaction.viewport) {
            painter.rect_filled(rect, 0.0, fill);
        }
    }

    // ── Octave lines + note labels ─────────────────────────────────
    for position in 0..keymap.width() {
        let Some(key) = keymap.key_at(position) else {
            continue;
        };
        if key % 12 != 0 {
            continue;
        }
        let stroke = Stroke::new(1.0, theme::roll_line(ui));
        if horizontal {
            let y = content.bottom() - position as f32 * scale_y;
            if y >= interaction.viewport.top() && y <= interaction.viewport.bottom() {
                painter.line_segment(
                    [
                        egui::pos2(interaction.viewport.left(), y),
                        egui::pos2(interaction.viewport.right(), y),
                    ],
                    stroke,
                );
                if scale_y >= 9.0 {
                    painter.text(
                        egui::pos2(interaction.viewport.left() + 3.0, y - 2.0),
                        egui::Align2::LEFT_BOTTOM,
                        note_name(key),
                        FontId::proportional(9.0),
                        theme::faint(ui),
                    );
                }
            }
        } else {
            let x = content.left() + position as f32 * scale_x;
            if x >= interaction.viewport.left() && x <= interaction.viewport.right() {
                painter.line_segment(
                    [
                        egui::pos2(x, interaction.viewport.top()),
                        egui::pos2(x, interaction.viewport.bottom()),
                    ],
                    stroke,
                );
                if scale_x >= 9.0 {
                    painter.text(
                        egui::pos2(x + 2.0, interaction.viewport.top() + 3.0),
                        egui::Align2::LEFT_TOP,
                        note_name(key),
                        FontId::proportional(9.0),
                        theme::faint(ui),
                    );
                }
            }
        }
    }

    // ── Notes: one instanced GPU draw ──────────────────────────────
    // Instances live in Preview and are re-uploaded only when a new
    // ConversionResult arrives. Pan/zoom only updates the uniform.
    if !midi_gpu.is_empty() {
        let callback = eframe::egui_wgpu::Callback::new_paint_callback(
            interaction.viewport,
            MidiGpuCallback::new(
                Arc::clone(midi_gpu),
                interaction.viewport,
                content,
                scale_x,
                scale_y,
                horizontal,
                gpu_target_format,
                ui.ctx().pixels_per_point(),
            ),
        );
        painter.add(egui::Shape::Callback(callback));
    }

    draw_bar_lines(
        &painter,
        interaction.viewport,
        content,
        horizontal,
        scale_x,
        scale_y,
        result.time_rows,
        midi_config,
    );
    draw_keyboard(
        ui, result, content, piano_rect, horizontal, scale_x, scale_y,
    );

    // ── Hover ──────────────────────────────────────────────────────
    let mut hovered = None;
    if let Some(pointer) = interaction
        .response
        .hover_pos()
        .filter(|pointer| content.contains(*pointer))
    {
        let (key_position, time_row) = if horizontal {
            (
                ((content.bottom() - pointer.y) / scale_y.max(1e-6)).floor() as i32,
                ((pointer.x - content.left()) / scale_x.max(1e-6)).floor() as i32,
            )
        } else {
            (
                ((pointer.x - content.left()) / scale_x.max(1e-6)).floor() as i32,
                ((content.bottom() - pointer.y) / scale_y.max(1e-6)).floor() as i32,
            )
        };
        if key_position >= 0
            && time_row >= 0
            && let Some(key) = keymap.key_at(key_position as u32)
        {
            hovered = Some(StageHover::Midi {
                key,
                tick: (time_row as u32)
                    .min(result.time_rows)
                    .saturating_mul(u32::from(midi_config.ticks_per_pixel.max(1))),
            });
        }

        painter.line_segment(
            [
                egui::pos2(pointer.x, interaction.viewport.top()),
                egui::pos2(pointer.x, interaction.viewport.bottom()),
            ],
            Stroke::new(1.0, Color32::from_white_alpha(24)),
        );
        painter.line_segment(
            [
                egui::pos2(interaction.viewport.left(), pointer.y),
                egui::pos2(interaction.viewport.right(), pointer.y),
            ],
            Stroke::new(1.0, Color32::from_white_alpha(24)),
        );
    }

    PreviewOutput { hovered }
}

fn split_keyboard(group: Rect, horizontal: bool) -> (Rect, Rect) {
    let ratio = piano::THICKNESS_RATIO;
    if horizontal {
        let keys_width = group.width() * ratio / (1.0 + ratio);
        let content = Rect::from_min_max(
            egui::pos2(group.left() + keys_width, group.top()),
            group.max,
        );
        let keys = Rect::from_min_max(
            group.min,
            egui::pos2(group.left() + keys_width, group.bottom()),
        );
        (content, keys)
    } else {
        let keys_height = group.height() * ratio / (1.0 + ratio);
        let content = Rect::from_min_max(
            group.min,
            egui::pos2(group.right(), group.bottom() - keys_height),
        );
        let keys = Rect::from_min_max(
            egui::pos2(group.left(), group.bottom() - keys_height),
            group.max,
        );
        (content, keys)
    }
}

fn draw_keyboard(
    ui: &Ui,
    result: &ConversionResult,
    content: Rect,
    rect: Rect,
    horizontal: bool,
    scale_x: f32,
    scale_y: f32,
) {
    if !rect.is_positive() {
        return;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::view_bg(ui));
    let (orientation, scale) = if horizontal {
        (piano::Orientation::Vertical, scale_y)
    } else {
        (piano::Orientation::Horizontal, scale_x)
    };
    piano::show(
        ui,
        &painter,
        &result.keymap,
        rect,
        content,
        scale,
        orientation,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_bar_lines(
    painter: &egui::Painter,
    viewport: Rect,
    content: Rect,
    horizontal: bool,
    scale_x: f32,
    scale_y: f32,
    time_rows: u32,
    midi_config: &MidiOutputConfig,
) {
    let ticks_per_row = u64::from(midi_config.ticks_per_pixel.max(1)) as f64;
    let bar_ticks = midi_config.time_signature.bar_ticks(midi_config.ppq);
    let bar_rows = bar_ticks / ticks_per_row;
    if bar_rows <= 0.0 || !bar_rows.is_finite() {
        return;
    }
    let scale = if horizontal { scale_x } else { scale_y };
    if bar_rows as f32 * scale < 2.0 {
        return;
    }

    let total_rows = f64::from(time_rows);
    let (first_row, last_row) = if horizontal {
        (
            (f64::from(viewport.left() - content.left()) / f64::from(scale_x))
                .clamp(0.0, total_rows),
            (f64::from(viewport.right() - content.left()) / f64::from(scale_x))
                .clamp(0.0, total_rows),
        )
    } else {
        (
            (f64::from(content.bottom() - viewport.bottom()) / f64::from(scale_y))
                .clamp(0.0, total_rows),
            (f64::from(content.bottom() - viewport.top()) / f64::from(scale_y))
                .clamp(0.0, total_rows),
        )
    };
    let first_bar = (first_row / bar_rows).ceil().max(0.0) as u64;
    let last_bar = (last_row / bar_rows).floor().max(0.0) as u64;
    if first_bar > last_bar {
        return;
    }

    let stroke = Stroke::new(1.0, Color32::from_white_alpha(58));
    for bar in first_bar..=last_bar {
        let row = bar_rows * bar as f64;
        if horizontal {
            let x = content.left() + row as f32 * scale_x;
            painter.line_segment(
                [
                    egui::pos2(x, viewport.top()),
                    egui::pos2(x, viewport.bottom()),
                ],
                stroke,
            );
        } else {
            let y = content.bottom() - row as f32 * scale_y;
            painter.line_segment(
                [
                    egui::pos2(viewport.left(), y),
                    egui::pos2(viewport.right(), y),
                ],
                stroke,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn keyboard_hugs_the_image_and_scales_with_it() {
        let group = Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(800.0, 856.0));
        let (content, keys) = split_keyboard(group, false);
        assert!(close(content.width(), 800.0));
        assert!(close(content.height(), 800.0));
        assert!(close(keys.height(), 56.0));
        assert!(close(keys.top(), content.bottom()));
        assert!(close(keys.left(), content.left()));
        assert!(close(keys.right(), content.right()));

        let group = Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(856.0, 800.0));
        let (content, keys) = split_keyboard(group, true);
        assert!(close(content.width(), 800.0));
        assert!(close(content.height(), 800.0));
        assert!(close(keys.width(), 56.0));
        assert!(close(keys.right(), content.left()));
        assert!(close(keys.top(), content.top()));
        assert!(close(keys.bottom(), content.bottom()));
    }
}
