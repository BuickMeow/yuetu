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
    let interaction = view.interact(ui, display_size, CursorIcon::Crosshair);
    let painter = ui.painter_at(interaction.viewport);

    painter.rect_filled(interaction.viewport, 0.0, theme::view_bg(ui));

    if result.notes.is_empty() {
        painter.text(
            interaction.viewport.center(),
            egui::Align2::CENTER_CENTER,
            "没有生成音符",
            FontId::proportional(14.0),
            theme::muted(ui),
        );
        return PreviewOutput { hovered: None };
    }

    let content = interaction.content_rect;
    let horizontal = result.time_axis == TimeAxis::Horizontal;
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
        let stroke = Stroke::new(1.0, Color32::from_white_alpha(18));
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

    // ── Hover ──────────────────────────────────────────────────────
    let mut hovered = None;
    if let Some(pointer) = interaction.hover_pos_in_content() {
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
        if key_position >= 0 && time_row >= 0 {
            if let Some(key) = keymap.key_at(key_position as u32) {
                hovered = Some(StageHover::Midi {
                    key,
                    tick: (time_row as u32)
                        .min(result.time_rows)
                        .saturating_mul(u32::from(midi_config.ticks_per_pixel.max(1))),
                });
            }
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
