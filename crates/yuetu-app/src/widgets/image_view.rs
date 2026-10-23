//! Zoomable source-image widget.

use egui::{Color32, CursorIcon, Rect, Stroke, StrokeKind, Ui};

use crate::state::{SourceHover, SourceImage};
use crate::theme;
use crate::widgets::view2d::View2D;

pub struct ImageViewOutput {
    pub hovered: Option<SourceHover>,
}

pub fn show(ui: &mut Ui, source: &SourceImage, view: &mut View2D) -> ImageViewOutput {
    let content_size = egui::vec2(source.texture_size.0 as f32, source.texture_size.1 as f32);
    let interaction = view.interact(ui, content_size, CursorIcon::Crosshair);
    let painter = ui.painter_at(interaction.viewport);

    painter.rect_filled(interaction.viewport, 0.0, theme::view_bg(ui));
    draw_checkerboard(&painter, interaction.content_rect, ui.visuals());

    let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    painter.image(
        source.texture.id(),
        interaction.content_rect,
        uv,
        Color32::WHITE,
    );

    let hovered = interaction.hover_uv().map(|uv| {
        let tx = (uv.x * source.texture_size.0 as f32).floor() as u32;
        let ty = (uv.y * source.texture_size.1 as f32).floor() as u32;
        let tx = tx.min(source.texture_size.0.saturating_sub(1));
        let ty = ty.min(source.texture_size.1.saturating_sub(1));
        let source_pos = source.source_pixel(tx, ty);
        let color = source
            .image
            .pixel(source_pos.0, source_pos.1)
            .unwrap_or(yuetu_core::Color::TRANSPARENT);
        SourceHover {
            texture_pos: (tx, ty),
            source_pos,
            color,
        }
    });

    if let Some(hover) = hovered {
        if interaction.scale >= 4.0 {
            let min = egui::pos2(
                interaction.content_rect.left() + hover.texture_pos.0 as f32 * interaction.scale,
                interaction.content_rect.top() + hover.texture_pos.1 as f32 * interaction.scale,
            );
            let rect = Rect::from_min_size(min, egui::vec2(interaction.scale, interaction.scale));
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0, theme::accent(ui)),
                StrokeKind::Inside,
            );
        }
    }

    ImageViewOutput { hovered }
}

fn draw_checkerboard(painter: &egui::Painter, rect: Rect, visuals: &egui::Visuals) {
    let cell = 12.0;
    let light = visuals.faint_bg_color;
    let cols = (rect.width() / cell).ceil() as i32;
    let rows = (rect.height() / cell).ceil() as i32;
    for y in 0..rows {
        for x in 0..cols {
            if (x + y) % 2 != 0 {
                continue;
            }
            let min = egui::pos2(rect.left() + x as f32 * cell, rect.top() + y as f32 * cell);
            let max = egui::pos2(
                (min.x + cell).min(rect.right()),
                (min.y + cell).min(rect.bottom()),
            );
            painter.rect_filled(Rect::from_min_max(min, max), 0.0, light);
        }
    }
}
