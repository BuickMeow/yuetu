//! Raster pipeline-stage view with Photoshop-style pixel rulers.

use egui::{Color32, CursorIcon, FontId, Rect, Stroke, StrokeKind, Ui};

use crate::theme;
use crate::widgets::view2d::View2D;

#[derive(Clone, Copy, Debug)]
pub struct RasterHover {
    /// Full-resolution pixel coordinate inside this stage image.
    pub x: u32,
    pub y: u32,
}

pub fn show(
    ui: &mut Ui,
    view: &mut View2D,
    texture: egui::TextureId,
    display_size: (u32, u32),
    grid_size: (u32, u32),
    checkerboard: bool,
    checker_texture: egui::TextureId,
) -> Option<RasterHover> {
    let size = egui::vec2(display_size.0.max(1) as f32, display_size.1.max(1) as f32);
    let interaction = view.interact(ui, size, CursorIcon::Crosshair);
    let painter = ui.painter_at(interaction.viewport);

    painter.rect_filled(interaction.viewport, 0.0, theme::view_bg(ui));
    if checkerboard {
        draw_checkerboard_quad(
            &painter,
            interaction.content_rect,
            interaction.viewport,
            checker_texture,
        );
    }

    painter.image(
        texture,
        interaction.content_rect,
        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );

    let hovered = interaction.hover_uv().map(|uv| RasterHover {
        x: ((uv.x * grid_size.0 as f32).floor() as u32).min(grid_size.0.saturating_sub(1)),
        y: ((uv.y * grid_size.1 as f32).floor() as u32).min(grid_size.1.saturating_sub(1)),
    });

    if let Some(hover) = hovered {
        let cell_width = interaction.content_rect.width() / grid_size.0.max(1) as f32;
        let cell_height = interaction.content_rect.height() / grid_size.1.max(1) as f32;
        if cell_width >= 4.0 && cell_height >= 4.0 {
            let min = egui::pos2(
                interaction.content_rect.left() + hover.x as f32 * cell_width,
                interaction.content_rect.top() + hover.y as f32 * cell_height,
            );
            let rect = Rect::from_min_size(min, egui::vec2(cell_width, cell_height));
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0, theme::accent(ui)),
                StrokeKind::Inside,
            );
        }
    }

    draw_rulers(ui, &painter, &interaction, display_size);
    hovered
}

fn draw_checkerboard_quad(
    painter: &egui::Painter,
    rect: Rect,
    viewport: Rect,
    checker_texture: egui::TextureId,
) {
    let visible = rect.intersect(viewport);
    if !visible.is_positive() {
        return;
    }

    // The 2x2 checker texture repeats. One quad covers the visible transparent
    // area; the GPU sampler does all the tiling.
    const CHECKER_PERIOD: f32 = 24.0;
    let uv = Rect::from_min_max(
        egui::pos2(
            (visible.left() - rect.left()) / CHECKER_PERIOD,
            (visible.top() - rect.top()) / CHECKER_PERIOD,
        ),
        egui::pos2(
            (visible.right() - rect.left()) / CHECKER_PERIOD,
            (visible.bottom() - rect.top()) / CHECKER_PERIOD,
        ),
    );
    let mut mesh = egui::Mesh::default();
    mesh.texture_id = checker_texture;
    mesh.add_rect_with_uv(visible, uv, Color32::WHITE);
    painter.add(egui::Shape::mesh(mesh));
}

fn draw_rulers(
    ui: &Ui,
    painter: &egui::Painter,
    interaction: &crate::widgets::view2d::ViewInteraction,
    content_size: (u32, u32),
) {
    if interaction.scale <= 1e-6 {
        return;
    }

    const TOP: f32 = 17.0;
    const LEFT: f32 = 28.0;
    let viewport = interaction.viewport;
    if viewport.width() <= LEFT + 4.0 || viewport.height() <= TOP + 4.0 {
        return;
    }

    let corner = Rect::from_min_max(
        viewport.min,
        egui::pos2(viewport.left() + LEFT, viewport.top() + TOP),
    );
    let top_band = Rect::from_min_max(
        egui::pos2(corner.right(), viewport.top()),
        egui::pos2(viewport.right(), viewport.top() + TOP),
    );
    let left_band = Rect::from_min_max(
        egui::pos2(viewport.left(), corner.bottom()),
        egui::pos2(viewport.left() + LEFT, viewport.bottom()),
    );
    let ruler_fill = ui.visuals().panel_fill;
    painter.rect_filled(corner, 0.0, ruler_fill);
    painter.rect_filled(top_band, 0.0, ruler_fill);
    painter.rect_filled(left_band, 0.0, ruler_fill);

    let step = ruler_step(interaction.scale);
    let tick_color = theme::muted(ui).gamma_multiply(0.75);
    let label_color = theme::faint(ui);
    let font = FontId::proportional(9.0);

    let content = interaction.content_rect;
    let first_x =
        (((viewport.left() - content.left()) / interaction.scale).floor()).max(0.0) as u32;
    let last_x = (((viewport.right() - content.left()) / interaction.scale).ceil())
        .clamp(0.0, content_size.0 as f32) as u32;
    let start_x = (first_x / step) * step;
    let mut x = start_x;
    while x <= last_x {
        let sx = content.left() + x as f32 * interaction.scale;
        if sx >= viewport.left() + LEFT && sx <= viewport.right() {
            painter.line_segment(
                [
                    egui::pos2(sx, viewport.top() + TOP - 4.0),
                    egui::pos2(sx, viewport.top() + TOP),
                ],
                Stroke::new(1.0, tick_color),
            );
            if sx + 30.0 <= viewport.right() {
                painter.text(
                    egui::pos2(sx + 2.0, viewport.top() + 1.0),
                    egui::Align2::LEFT_TOP,
                    x.to_string(),
                    font.clone(),
                    label_color,
                );
            }
        }
        x = x.saturating_add(step);
    }

    // Time/Y ruler starts at 0 on the bottom edge and runs upward.
    let height = content_size.1 as f32;
    let first_y = (((content.bottom() - viewport.bottom()) / interaction.scale).floor())
        .clamp(0.0, height) as u32;
    let last_y = (((content.bottom() - viewport.top()) / interaction.scale).ceil())
        .clamp(0.0, height) as u32;
    let start_y = (first_y / step) * step;
    let mut y = start_y;
    while y <= last_y {
        draw_y_tick(
            painter,
            &font,
            label_color,
            tick_color,
            viewport,
            content,
            interaction.scale,
            LEFT,
            TOP,
            y,
        );
        y = y.saturating_add(step);
    }
    let top = content_size.1;
    if top % step != 0 && top >= first_y && top <= last_y {
        draw_y_tick(
            painter,
            &font,
            label_color,
            tick_color,
            viewport,
            content,
            interaction.scale,
            LEFT,
            TOP,
            top,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_y_tick(
    painter: &egui::Painter,
    font: &FontId,
    label_color: egui::Color32,
    tick_color: egui::Color32,
    viewport: Rect,
    content: Rect,
    scale: f32,
    left: f32,
    top: f32,
    coordinate: u32,
) {
    let sy = content.bottom() - coordinate as f32 * scale;
    if sy < viewport.top() + top || sy > viewport.bottom() {
        return;
    }
    painter.line_segment(
        [
            egui::pos2(viewport.left() + left - 4.0, sy),
            egui::pos2(viewport.left() + left, sy),
        ],
        Stroke::new(1.0, tick_color),
    );
    let label_top = (sy + 1.0).min(viewport.bottom() - 11.0);
    painter.text(
        egui::pos2(viewport.left() + left - 4.0, label_top),
        egui::Align2::RIGHT_TOP,
        coordinate.to_string(),
        font.clone(),
        label_color,
    );
}

fn ruler_step(scale: f32) -> u32 {
    let target = 72.0 / scale.max(1e-6);
    let power = 10f32.powf(target.log10().floor());
    let ratio = target / power;
    let multiplier = if ratio <= 1.0 {
        1.0
    } else if ratio <= 2.0 {
        2.0
    } else if ratio <= 5.0 {
        5.0
    } else {
        10.0
    };
    (power * multiplier).max(1.0).round() as u32
}
