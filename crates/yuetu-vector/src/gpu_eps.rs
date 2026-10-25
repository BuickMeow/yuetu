//! Vello GPU extraction for the path subset of PostScript/EPS display lists.
//!
//! EPS itself has no alpha channel. We render the page on white paper, then
//! replay fill/stroke/clip and simple opacity groups through Vello. Images,
//! shadings, patterns, masks and non-normal blends keep the CPU path.

use stet::DisplayElement;
use stet_fonts::geometry::{Matrix, PathSegment, PsPath};
use stet_graphics::color::{FillRule, LineCap as PsCap, LineJoin as PsJoin};
use vello_common::kurbo::{
    Affine, BezPath, Cap as KurboCap, Join as KurboJoin, Point, Rect, Shape, Stroke,
};
use vello_common::peniko::Color;
use vello_gpu::Scene;

use crate::ViewTransform;

pub(crate) fn build(
    page: &stet::PsDisplayList,
    width: u32,
    height: u32,
    transform: ViewTransform,
) -> Option<Scene> {
    if width == 0 || height == 0 || width > u32::from(u16::MAX) || height > u32::from(u16::MAX) {
        return None;
    }
    // stet's region renderer only handles axis-aligned views. Keep the same
    // restriction for the GPU path; rotated EPS falls back to the full-page
    // raster instead of showing white corners outside the page.
    if transform.b.abs() > 1e-4
        || transform.c.abs() > 1e-4
        || transform.a <= 0.0
        || transform.d <= 0.0
    {
        return None;
    }

    let view = to_affine(transform);
    let mut scene = Scene::new(width as u16, height as u16);
    // PostScript's paper is opaque white. The source-stage texture covers
    // exactly the page, so this also avoids a transparent/checkerboard edge.
    scene.set_transform(Affine::IDENTITY);
    scene.set_paint(Color::WHITE);
    scene.fill_rect(&Rect::new(
        0.0,
        0.0,
        f64::from(width as u16),
        f64::from(height as u16),
    ));
    let page_clip =
        Rect::new(0.0, 0.0, f64::from(width as u16), f64::from(height as u16)).to_path(0.1);
    scene.push_clip_path(&page_clip);

    let mut builder = EpsBuilder {
        scene,
        view,
        unsupported: false,
        clip_depth: 0,
    };
    builder.elements(page.elements());

    if builder.unsupported {
        None
    } else {
        Some(builder.scene)
    }
}

struct EpsBuilder {
    scene: Scene,
    view: Affine,
    unsupported: bool,
    clip_depth: usize,
}

impl EpsBuilder {
    fn elements(&mut self, elements: &[DisplayElement]) {
        for element in elements {
            self.element(element);
        }
    }

    fn element(&mut self, element: &DisplayElement) {
        match element {
            DisplayElement::Fill { path, params } => {
                if params.blend_mode != 0 {
                    self.unsupported = true;
                    return;
                }
                let path = to_bez_path(path);
                self.scene
                    .set_transform(self.view * matrix_to_affine(params.ctm));
                self.scene
                    .set_paint(device_color(&params.color, params.alpha));
                self.scene.set_fill_rule(match params.fill_rule {
                    FillRule::NonZeroWinding => vello_common::peniko::Fill::NonZero,
                    FillRule::EvenOdd => vello_common::peniko::Fill::EvenOdd,
                    _ => vello_common::peniko::Fill::NonZero,
                });
                self.scene.fill_path(&path);
            }
            DisplayElement::Stroke { path, params } => {
                if params.blend_mode != 0 {
                    self.unsupported = true;
                    return;
                }
                let path = to_bez_path(path);
                self.scene
                    .set_transform(self.view * matrix_to_affine(params.ctm));
                self.scene
                    .set_paint(device_color(&params.color, params.alpha));
                let mut stroke = Stroke::new(params.line_width)
                    .with_miter_limit(params.miter_limit)
                    .with_caps(match params.line_cap {
                        PsCap::Butt => KurboCap::Butt,
                        PsCap::Round => KurboCap::Round,
                        PsCap::Square => KurboCap::Square,
                        _ => KurboCap::Butt,
                    })
                    .with_join(match params.line_join {
                        PsJoin::Miter => KurboJoin::Miter,
                        PsJoin::Round => KurboJoin::Round,
                        PsJoin::Bevel => KurboJoin::Bevel,
                        _ => KurboJoin::Miter,
                    });
                if !params.dash_pattern.array.is_empty() {
                    stroke = stroke.with_dashes(
                        params.dash_pattern.offset,
                        params.dash_pattern.array.iter().copied(),
                    );
                }
                self.scene.set_stroke(stroke);
                self.scene.stroke_path(&path);
            }
            DisplayElement::Clip { path, params } => {
                if params.stroke_params.is_some() {
                    self.unsupported = true;
                    return;
                }
                let path = to_bez_path(path);
                self.scene
                    .set_transform(self.view * matrix_to_affine(params.ctm));
                self.scene.set_fill_rule(match params.fill_rule {
                    FillRule::NonZeroWinding => vello_common::peniko::Fill::NonZero,
                    FillRule::EvenOdd => vello_common::peniko::Fill::EvenOdd,
                    _ => vello_common::peniko::Fill::NonZero,
                });
                self.scene.push_clip_path(&path);
                self.clip_depth += 1;
            }
            DisplayElement::InitClip => {
                for _ in 0..self.clip_depth {
                    self.scene.pop_clip();
                }
                self.clip_depth = 0;
            }
            DisplayElement::ErasePage => {
                self.scene.set_transform(Affine::IDENTITY);
                self.scene.set_paint(Color::WHITE);
                self.scene.fill_rect(&Rect::new(
                    0.0,
                    0.0,
                    f64::from(self.scene.width()),
                    f64::from(self.scene.height()),
                ));
            }
            DisplayElement::Group { elements, params } => {
                if params.blend_mode != 0 || params.knockout {
                    self.unsupported = true;
                    return;
                }
                let layered = params.alpha < 1.0;
                if layered {
                    self.scene.push_opacity_layer(params.alpha as f32);
                }
                self.elements(elements.elements());
                if layered {
                    self.scene.pop_layer();
                }
            }
            // Keeping the CPU renderer for these is much safer than silently
            // dropping content that affects the visible page.
            DisplayElement::SoftMasked { .. }
            | DisplayElement::Image { .. }
            | DisplayElement::AxialShading { .. }
            | DisplayElement::RadialShading { .. }
            | DisplayElement::MeshShading { .. }
            | DisplayElement::PatchShading { .. }
            | DisplayElement::PatternFill { .. }
            | DisplayElement::OcgGroup { .. } => {
                self.unsupported = true;
            }
            DisplayElement::Text { .. } | DisplayElement::TextRun { .. } => {}
            _ => self.unsupported = true,
        }
    }
}

fn device_color(color: &stet_graphics::color::DeviceColor, alpha: f64) -> Color {
    let alpha = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color::from_rgba8(
        (color.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.b.clamp(0.0, 1.0) * 255.0).round() as u8,
        alpha,
    )
}

fn to_affine(transform: ViewTransform) -> Affine {
    Affine::new([
        f64::from(transform.a),
        f64::from(transform.b),
        f64::from(transform.c),
        f64::from(transform.d),
        f64::from(transform.e),
        f64::from(transform.f),
    ])
}

fn matrix_to_affine(matrix: Matrix) -> Affine {
    Affine::new([matrix.a, matrix.b, matrix.c, matrix.d, matrix.tx, matrix.ty])
}

fn to_bez_path(path: &PsPath) -> BezPath {
    let mut result = BezPath::new();
    for segment in &path.segments {
        match *segment {
            PathSegment::MoveTo(x, y) => result.move_to(Point::new(x, y)),
            PathSegment::LineTo(x, y) => result.line_to(Point::new(x, y)),
            PathSegment::CurveTo {
                x1,
                y1,
                x2,
                y2,
                x3,
                y3,
            } => result.curve_to(Point::new(x1, y1), Point::new(x2, y2), Point::new(x3, y3)),
            PathSegment::ClosePath => result.close_path(),
        }
    }
    result
}
