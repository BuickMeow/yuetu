//! Shared 2D pan/zoom viewport used by the source image and the MIDI preview.
//!
//! Trackpad two-finger scrolling pans in both axes; pinch zooms around the
//! cursor. Mouse-wheel lines and Ctrl/Cmd+scroll zoom, drag pans, arrows pan,
//! double-click fits.

use egui::{CursorIcon, Event, Key, MouseWheelUnit, Pos2, Rect, Response, Sense, Ui, Vec2};

#[derive(Clone, Debug)]
pub struct View2D {
    pub zoom: f32,
    pub pan: Vec2,
    fit_pending: bool,
}

impl Default for View2D {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: Vec2::ZERO,
            fit_pending: true,
        }
    }
}

impl View2D {
    pub fn reset_fit(&mut self) {
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
        self.fit_pending = true;
    }

    /// Scale relative to the fit-to-viewport scale (1.0 = fully fitted).
    pub fn relative_zoom(&self) -> f32 {
        self.zoom
    }

    pub fn interact(
        &mut self,
        ui: &mut Ui,
        content_size: Vec2,
        cursor: CursorIcon,
    ) -> ViewInteraction {
        let available = ui.available_size();
        let size = Vec2::new(available.x.max(1.0), available.y.max(1.0));
        let (viewport, response) = ui.allocate_exact_size(size, Sense::click_and_drag());

        let content_size = Vec2::new(content_size.x.max(1.0), content_size.y.max(1.0));
        let fit =
            (viewport.width() / content_size.x).min(viewport.height() / content_size.y) * 0.97;
        let fit = if fit.is_finite() && fit > 0.0 {
            fit
        } else {
            1.0
        };

        if self.fit_pending {
            self.zoom = 1.0;
            self.pan = Vec2::ZERO;
            self.fit_pending = false;
        }

        if response.double_clicked() {
            self.zoom = 1.0;
            self.pan = Vec2::ZERO;
        }

        if response.dragged_by(egui::PointerButton::Primary) {
            self.pan += response.drag_delta();
        }

        if response.hovered() {
            let (scroll, pinch, wheel_is_zoom) = ui.input(|input| {
                // Trackpads report Point deltas; a classic mouse wheel reports
                // Line/Page. Treat the former as panning, the latter as zoom.
                let wheel_is_zoom = input.modifiers.command
                    || input.modifiers.ctrl
                    || input.events.iter().any(|event| {
                        matches!(
                            event,
                            Event::MouseWheel {
                                unit: MouseWheelUnit::Line | MouseWheelUnit::Page,
                                ..
                            }
                        )
                    });
                (input.smooth_scroll_delta, input.zoom_delta(), wheel_is_zoom)
            });

            if wheel_is_zoom {
                let factor = (pinch * (scroll.y * 0.0018).exp()).clamp(0.5, 2.0);
                self.zoom_at(viewport, fit, response.hover_pos(), factor);
            } else {
                // Trackpad two-finger scroll: move the content in both axes.
                self.pan += scroll;
                self.zoom_at(viewport, fit, response.hover_pos(), pinch);
            }

            if !ui.ctx().egui_wants_keyboard_input() {
                let arrow = ui.input(|input| {
                    let mut delta = Vec2::ZERO;
                    if input.key_down(Key::ArrowLeft) {
                        delta.x += 1.0;
                    }
                    if input.key_down(Key::ArrowRight) {
                        delta.x -= 1.0;
                    }
                    if input.key_down(Key::ArrowUp) {
                        delta.y += 1.0;
                    }
                    if input.key_down(Key::ArrowDown) {
                        delta.y -= 1.0;
                    }
                    delta * 12.0
                });
                if arrow != Vec2::ZERO {
                    self.pan += arrow;
                    ui.ctx().request_repaint();
                }
            }
        }

        if response.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        } else if response.hovered() {
            ui.ctx().set_cursor_icon(cursor);
        }

        let scale = fit * self.zoom;
        let center = viewport.center() + self.pan;
        let content_rect = Rect::from_center_size(center, content_size * scale);

        ViewInteraction {
            response,
            viewport,
            content_rect,
            scale,
        }
    }

    fn zoom_at(&mut self, viewport: Rect, fit: f32, pointer: Option<Pos2>, factor: f32) {
        let factor = factor.clamp(0.5, 2.0);
        if (factor - 1.0).abs() <= 1e-4 {
            return;
        }

        let old_scale = fit * self.zoom;
        let new_zoom = (self.zoom * factor).clamp(0.02, 120.0);
        let new_scale = fit * new_zoom;

        if let Some(pointer) = pointer {
            let old_center = viewport.center() + self.pan;
            let content_offset = (pointer - old_center) / old_scale.max(1e-6);
            let new_center = pointer - content_offset * new_scale;
            self.pan = new_center - viewport.center();
        }
        self.zoom = new_zoom;
    }
}

pub struct ViewInteraction {
    pub response: Response,
    pub viewport: Rect,
    pub content_rect: Rect,
    pub scale: f32,
}

impl ViewInteraction {
    /// Pointer position inside the content, normalized to `[0, 1)`, if any.
    pub fn hover_uv(&self) -> Option<Vec2> {
        if !self.response.hovered() {
            return None;
        }
        let pointer = self.response.hover_pos()?;
        if !self.content_rect.contains(pointer) {
            return None;
        }
        Some(Vec2::new(
            ((pointer.x - self.content_rect.left()) / self.content_rect.width())
                .clamp(0.0, 0.999_999),
            ((pointer.y - self.content_rect.top()) / self.content_rect.height())
                .clamp(0.0, 0.999_999),
        ))
    }

    pub fn hover_pos_in_content(&self) -> Option<Pos2> {
        if !self.response.hovered() {
            return None;
        }
        let pointer = self.response.hover_pos()?;
        self.content_rect.contains(pointer).then_some(pointer)
    }
}
