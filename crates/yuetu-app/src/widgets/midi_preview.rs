//! MIDI preview: a piano-roll where X is pitch and Y is time.
//!
//! This orientation deliberately mirrors the source image: the bottom of the
//! preview is tick 0, and time flows upward. Each note is colored by its
//! palette track.

use std::sync::Arc;

use egui::{Color32, CursorIcon, FontId, Rect, Stroke, Ui};
use yuetu_core::{ConversionResult, is_white_key, note_name};

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
    midi_gpu: &Arc<MidiGpuData>,
    view: &mut View2D,
    gpu_target_format: eframe::wgpu::TextureFormat,
) -> PreviewOutput {
    let key_span = result.keymap.width().max(1) as f32;
    let duration = result.duration_px.max(1) as f32;
    let interaction = view.interact(ui, egui::vec2(key_span, duration), CursorIcon::Crosshair);
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
    let scale = interaction.scale;
    let keymap = &result.keymap;

    // ── Pitch lanes ────────────────────────────────────────────────
    for column in 0..keymap.width() {
        let x0 = content.left() + column as f32 * scale;
        let x1 = x0 + scale;
        if x1 < interaction.viewport.left() || x0 > interaction.viewport.right() {
            continue;
        }
        let fill = match keymap.key_at(column) {
            Some(key) if is_white_key(key) => theme::lane_white(ui),
            Some(_) => theme::lane_black(ui),
            None => theme::lane_off(ui),
        };
        painter.rect_filled(
            Rect::from_min_max(
                egui::pos2(x0, content.top()),
                egui::pos2(x1, content.bottom()),
            ),
            0.0,
            fill,
        );
    }

    // ── Octave lines + note labels ─────────────────────────────────
    for column in 0..keymap.width() {
        let Some(key) = keymap.key_at(column) else {
            continue;
        };
        let x = content.left() + column as f32 * scale;
        if x < interaction.viewport.left() || x > interaction.viewport.right() {
            continue;
        }
        if key % 12 == 0 {
            painter.line_segment(
                [
                    egui::pos2(x, content.top().max(interaction.viewport.top())),
                    egui::pos2(x, content.bottom().min(interaction.viewport.bottom())),
                ],
                Stroke::new(1.0, Color32::from_white_alpha(18)),
            );
            if scale >= 9.0 {
                painter.text(
                    egui::pos2(x + 2.0, interaction.viewport.top() + 3.0),
                    egui::Align2::LEFT_TOP,
                    note_name(key),
                    FontId::monospace(9.0),
                    theme::faint(ui),
                );
            }
        }
    }

    // ── Time grid ──────────────────────────────────────────────────
    let step = grid_step(result.duration_px);
    let mut tick = 0u32;
    while tick <= result.duration_px {
        let y = content.bottom() - tick as f32 * scale;
        if y >= interaction.viewport.top() && y <= interaction.viewport.bottom() {
            let major = tick % (step * 4) == 0;
            painter.line_segment(
                [
                    egui::pos2(interaction.viewport.left(), y),
                    egui::pos2(interaction.viewport.right(), y),
                ],
                Stroke::new(1.0, Color32::from_white_alpha(if major { 22 } else { 10 })),
            );
        }
        tick = tick.saturating_add(step);
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
                scale,
                gpu_target_format,
            ),
        );
        painter.add(egui::Shape::Callback(callback));
    }

    // ── Hover ──────────────────────────────────────────────────────
    let mut hovered = None;
    if let Some(pointer) = interaction.hover_pos_in_content() {
        let column = ((pointer.x - content.left()) / scale.max(1e-6)).floor() as i32;
        let tick = ((content.bottom() - pointer.y) / scale.max(1e-6)).floor() as i32;
        if column >= 0 && tick >= 0 {
            if let Some(key) = keymap.key_at(column as u32) {
                hovered = Some(StageHover::Midi {
                    key,
                    tick: (tick as u32).min(result.duration_px),
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

fn grid_step(duration: u32) -> u32 {
    let target = (duration / 8).max(1);
    let mut magnitude = 1u32;
    while magnitude * 10 <= target {
        magnitude *= 10;
    }
    for factor in [1, 2, 5, 10] {
        let step = magnitude * factor;
        if step >= target {
            return step.max(1);
        }
    }
    magnitude * 10
}
