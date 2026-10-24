//! Shared 2D pan/zoom viewport used by every pipeline stage view.
//!
//! Trackpad two-finger scrolling pans in both axes; pinch zooms around the
//! cursor. Mouse-wheel lines and Ctrl/Cmd+scroll zoom, drag pans, arrows pan,
//! double-click fits.

use egui::{
    CursorIcon, Event, Key, MouseWheelSource, MouseWheelUnit, Pos2, Rect, Response, Sense, Ui, Vec2,
};

const MIN_ABSOLUTE_SCALE: f64 = 0.01;
const MAX_ABSOLUTE_SCALE: f64 = 64.0;
const MAX_RELATIVE_ZOOM: f64 = 120.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScrollSource {
    Wheel,
    Touchpad,
}

fn scroll_source_for(unit: MouseWheelUnit, source: MouseWheelSource) -> Option<ScrollSource> {
    match source {
        MouseWheelSource::Wheel => Some(ScrollSource::Wheel),
        MouseWheelSource::Trackpad | MouseWheelSource::Momentum => Some(ScrollSource::Touchpad),
        MouseWheelSource::Unknown => match unit {
            MouseWheelUnit::Line | MouseWheelUnit::Page => Some(ScrollSource::Wheel),
            MouseWheelUnit::Point => None,
        },
    }
}

#[derive(Clone, Copy, Debug)]
struct ZoomAnchor {
    /// Content coordinate captured at the start of the zoom gesture.
    content: [f64; 2],
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
    zoom_anchor: Option<ZoomAnchor>,
    /// Set while a trackpad pinch is active (and until the scroll settles), so
    /// residual scroll deltas cannot pan the image immediately after zooming.
    pinch_zoom_gesture: bool,
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
            zoom_anchor: None,
            pinch_zoom_gesture: false,
        }
    }
}

impl View2D {
    pub fn reset_fit(&mut self) {
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
        self.fit_pending = true;
        self.zoom_anchor = None;
        self.pinch_zoom_gesture = false;
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
        self.zoom_anchor = None;
        self.last_scale = self.last_fit * self.zoom;
    }

    /// Cover the viewport. This is intentionally `max(width_ratio,
    /// height_ratio)`, so tall images fill the width instead of shrinking to
    /// fit vertically.
    pub fn fill(&mut self) {
        if self.last_content_size.x <= 0.0
            || self.last_content_size.y <= 0.0
            || self.last_viewport.width() <= 0.0
            || self.last_viewport.height() <= 0.0
        {
            return;
        }
        let target_scale = (self.last_viewport.width() / self.last_content_size.x)
            .max(self.last_viewport.height() / self.last_content_size.y)
            .clamp(MIN_ABSOLUTE_SCALE as f32, MAX_ABSOLUTE_SCALE as f32);
        self.zoom = (target_scale / self.last_fit).clamp(0.02, 120.0);
        self.pan = Vec2::ZERO;
        self.fit_pending = false;
        self.zoom_anchor = None;
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
            self.zoom_anchor = None;
        }

        if response.double_clicked() {
            self.zoom = 1.0;
            self.pan = Vec2::ZERO;
            self.zoom_anchor = None;
        }

        if response.dragged_by(egui::PointerButton::Primary) {
            self.pan += response.drag_delta();
            self.zoom_anchor = None;
        }

        if response.hovered() {
            let (scroll, pinch, event_source, wheel_y) = ui.input(|input| {
                // Use the backend's wheel source instead of the delta unit:
                // macOS can report a physical wheel as Point deltas. A wheel
                // must zoom even when its unit is Point, and a trackpad must
                // pan even when its unit is Line/Page.
                let mut event_source = None;
                let mut wheel_y = 0.0;
                for event in &input.events {
                    if let Event::MouseWheel {
                        unit,
                        delta,
                        source,
                        ..
                    } = event
                    {
                        if let Some(source) = scroll_source_for(*unit, *source) {
                            event_source = Some(source);
                            if source == ScrollSource::Wheel {
                                wheel_y += delta.y;
                            }
                        }
                    }
                }
                (
                    input.smooth_scroll_delta,
                    input.zoom_delta(),
                    event_source,
                    wheel_y,
                )
            });

            if let Some(source) = event_source {
                self.scroll_source = Some(source);
            } else if scroll == Vec2::ZERO && (pinch - 1.0).abs() <= 1e-4 {
                self.scroll_source = None;
            }

            let pinching = (pinch - 1.0).abs() > 1e-4;
            if pinching {
                self.pinch_zoom_gesture = true;
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
                // Suppress pan for the whole pinch gesture, including the
                // smoothed tail after the pinch has ended; otherwise a tiny
                // residual scroll shifts the image right after zooming.
                if !pinching && !self.pinch_zoom_gesture {
                    self.pan += scroll;
                }
                self.zoom_at(viewport, fit, response.hover_pos(), pinch);
            }

            if (pinch - 1.0).abs() <= 1e-4 && self.scroll_source != Some(ScrollSource::Wheel) {
                self.zoom_anchor = None;
            }
            if !pinching && scroll == Vec2::ZERO && self.scroll_source.is_none() {
                self.pinch_zoom_gesture = false;
                self.zoom_anchor = None;
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
                    self.zoom_anchor = None;
                    ui.ctx().request_repaint();
                }
            }
        } else {
            self.scroll_source = None;
            self.zoom_anchor = None;
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

        let old_scale = f64::from(fit) * f64::from(self.zoom);
        let min_zoom = (MIN_ABSOLUTE_SCALE / f64::from(fit)).max(0.02);
        let max_zoom = (MAX_ABSOLUTE_SCALE / f64::from(fit)).min(MAX_RELATIVE_ZOOM);
        let new_zoom = (f64::from(self.zoom) * f64::from(factor)).clamp(min_zoom, max_zoom);
        let new_zoom_f32 = new_zoom as f32;

        // At a zoom limit (or after a subpixel step that rounds to the same
        // f32 zoom), there is no zoom change. Never recompute `pan` in that
        // case, otherwise continuing to scroll at the end stop would translate
        // the viewport.
        if new_zoom_f32 == self.zoom {
            self.zoom_anchor = None;
            return;
        }

        let new_scale = f64::from(fit) * new_zoom;

        if let Some(pointer) = pointer {
            // Capture the content point once per gesture, then keep that exact
            // content point under the cursor. Recomputing it from the rounded
            // f32 pan on every wheel event accumulates a visible drift at
            // extreme zoom. The anchor math itself uses f64.
            let anchor = self.zoom_anchor.unwrap_or_else(|| {
                let old_center_x = f64::from(viewport.center().x) + f64::from(self.pan.x);
                let old_center_y = f64::from(viewport.center().y) + f64::from(self.pan.y);
                ZoomAnchor {
                    content: [
                        (f64::from(pointer.x) - old_center_x) / old_scale.max(1e-12),
                        (f64::from(pointer.y) - old_center_y) / old_scale.max(1e-12),
                    ],
                }
            });
            self.zoom_anchor = Some(anchor);

            let new_center_x = f64::from(pointer.x) - anchor.content[0] * new_scale;
            let new_center_y = f64::from(pointer.y) - anchor.content[1] * new_scale;
            self.pan = Vec2::new(
                (new_center_x - f64::from(viewport.center().x)) as f32,
                (new_center_y - f64::from(viewport.center().y)) as f32,
            );
        } else {
            self.zoom_anchor = None;
        }
        self.zoom = new_zoom_f32;
    }
}

pub struct ViewInteraction {
    pub response: Response,
    pub viewport: Rect,
    pub content_rect: Rect,
    pub scale: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_anchor_stays_stable_during_subpixel_zooms() {
        let mut view = View2D {
            zoom: 100.0,
            pan: egui::vec2(123.4, -789.1),
            fit_pending: false,
            ..View2D::default()
        };
        let viewport = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 800.0));
        let pointer = egui::pos2(870.0, 120.0);
        let fit = 0.5;

        let content_point = |view: &View2D| {
            let center = viewport.center() + view.pan;
            let scale = fit * view.zoom;
            (pointer - center) / scale
        };
        let before = content_point(&view);

        for _ in 0..2000 {
            view.zoom_at(viewport, fit, Some(pointer), 1.00005);
        }

        let after = content_point(&view);
        assert!(
            (before - after).length() < 0.01,
            "anchor drifted from {before:?} to {after:?}"
        );
    }

    #[test]
    fn wheel_source_wins_over_delta_unit() {
        assert_eq!(
            scroll_source_for(MouseWheelUnit::Point, MouseWheelSource::Wheel),
            Some(ScrollSource::Wheel)
        );
        assert_eq!(
            scroll_source_for(MouseWheelUnit::Line, MouseWheelSource::Trackpad),
            Some(ScrollSource::Touchpad)
        );
        assert_eq!(
            scroll_source_for(MouseWheelUnit::Point, MouseWheelSource::Trackpad),
            Some(ScrollSource::Touchpad)
        );
        assert_eq!(
            scroll_source_for(MouseWheelUnit::Line, MouseWheelSource::Unknown),
            Some(ScrollSource::Wheel)
        );
        assert_eq!(
            scroll_source_for(MouseWheelUnit::Point, MouseWheelSource::Unknown),
            None
        );
    }

    #[test]
    fn zooming_at_the_content_center_does_not_translate() {
        let mut view = View2D::default();
        let viewport = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 800.0));
        let pointer = viewport.center();

        view.zoom_at(viewport, 0.5, Some(pointer), 1.25);

        assert!(
            view.pan.length() < 1e-5,
            "center zoom moved pan: {:?}",
            view.pan
        );
    }

    #[test]
    fn zoom_at_limit_does_not_move_pan() {
        let mut view = View2D {
            zoom: 120.0,
            pan: egui::vec2(321.0, -654.0),
            fit_pending: false,
            ..View2D::default()
        };
        let viewport = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 800.0));
        let before = view.pan;

        for _ in 0..100 {
            view.zoom_at(viewport, 0.5, Some(egui::pos2(800.0, 150.0)), 1.25);
        }

        assert_eq!(view.pan, before);
        assert_eq!(view.zoom, 120.0);
    }
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
