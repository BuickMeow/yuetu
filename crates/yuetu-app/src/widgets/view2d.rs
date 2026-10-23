//! Shared 2D pan/zoom viewport used by every pipeline stage view.
//!
//! Trackpad two-finger scrolling pans in both axes; pinch zooms around the
//! cursor. Mouse-wheel lines and Ctrl/Cmd+scroll zoom, drag pans, arrows pan,
//! double-click fits.

use egui::{CursorIcon, Event, Key, MouseWheelUnit, Pos2, Rect, Response, Sense, Ui, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScrollSource {
    Wheel,
    Touchpad,
}

#[derive(Clone, Debug)]
pub struct View2D {
    pub zoom: f32,
    pub pan: Vec2,
    fit_pending: bool,
    last_scale: f32,
    last_fit: f32,
    last_viewport: Rect,
    last_content_size: Vec2,
    scroll_source: Option<ScrollSource>,
}

impl Default for View2D {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: Vec2::ZERO,
            fit_pending: true,
            last_scale: 1.0,
            last_fit: 1.0,
            last_viewport: Rect::NOTHING,
            last_content_size: Vec2::splat(1.0),
            scroll_source: None,
        }
    }
}

impl View2D {
    pub fn reset_fit(&mut self) {
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
        self.fit_pending = true;
    }

    /// Current absolute zoom in percent, where 100% means one image pixel per
    /// logical point.
    pub fn absolute_zoom_percent(&self) -> f32 {
        self.last_scale * 100.0
    }

    pub fn set_absolute_zoom_percent(&mut self, percent: f32) {
        if self.last_fit <= 1e-6 {
            return;
        }
        let target_scale = (percent * 0.01).clamp(0.01, 64.0);
        self.zoom = (target_scale / self.last_fit).clamp(0.02, 120.0);
        self.fit_pending = false;
        self.last_scale = self.last_fit * self.zoom;
    }

    pub fn fit_width(&mut self) {
        if self.last_content_size.x <= 0.0 || self.last_viewport.width() <= 0.0 {
            return;
        }
        let target_scale = (self.last_viewport.width() / self.last_content_size.x) * 0.98;
        self.zoom = (target_scale / self.last_fit).clamp(0.02, 120.0);
        self.pan = Vec2::ZERO;
        self.fit_pending = false;
        self.last_scale = self.last_fit * self.zoom;
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
            let (scroll, pinch, has_wheel, has_point, wheel_y) = ui.input(|input| {
                // Trackpads report Point deltas; a classic mouse wheel reports
                // Line/Page. Remember the gesture source across frames: egui
                // smooths scroll over several frames, and a wheel must never
                // become a pan just because its event frame has passed.
                let mut has_wheel = false;
                let mut has_point = false;
                let mut wheel_y = 0.0;
                for event in &input.events {
                    if let Event::MouseWheel { unit, delta, .. } = event {
                        match unit {
                            MouseWheelUnit::Line | MouseWheelUnit::Page => {
                                has_wheel = true;
                                wheel_y += delta.y;
                            }
                            MouseWheelUnit::Point => has_point = true,
                        }
                    }
                }
                (
                    input.smooth_scroll_delta,
                    input.zoom_delta(),
                    has_wheel,
                    has_point,
                    wheel_y,
                )
            });

            if has_wheel {
                self.scroll_source = Some(ScrollSource::Wheel);
            } else if has_point {
                self.scroll_source = Some(ScrollSource::Touchpad);
            } else if scroll == Vec2::ZERO && (pinch - 1.0).abs() <= 1e-4 {
                self.scroll_source = None;
            }

            if self.scroll_source == Some(ScrollSource::Wheel) {
                let dy = if scroll.y.abs() > 1e-6 {
                    scroll.y
                } else {
                    wheel_y * 20.0
                };
                let factor = (pinch * (dy * 0.0018).exp()).clamp(0.5, 2.0);
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

        self.last_scale = scale;
        self.last_fit = fit;
        self.last_viewport = viewport;
        self.last_content_size = content_size;

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
