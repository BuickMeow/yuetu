//! Translation of the loaded SVG tree into a Vello GPU scene.
//!
//! This is deliberately conservative: anything whose appearance cannot be
//! represented faithfully yet (text, embedded images, patterns, clipping,
//! masks, filters and blend modes) asks the caller to use the existing CPU
//! renderer instead. Ordinary filled/stroked paths, opacity groups and linear
//! or radial gradients run through Vello.

use vello_common::kurbo::{Affine, BezPath, Cap as KurboCap, Join as KurboJoin, Point, Stroke};
use vello_common::paint::PaintType;
use vello_common::peniko::{Color, Extend, Gradient};
use vello_gpu::Scene;

use crate::ViewTransform;

pub(crate) fn build(
    tree: &usvg::Tree,
    width: u32,
    height: u32,
    transform: ViewTransform,
) -> Option<Scene> {
    if width == 0 || height == 0 || width > u32::from(u16::MAX) || height > u32::from(u16::MAX) {
        return None;
    }

    let view = Affine::new([
        f64::from(transform.a),
        f64::from(transform.b),
        f64::from(transform.c),
        f64::from(transform.d),
        f64::from(transform.e),
        f64::from(transform.f),
    ]);
    let mut builder = SceneBuilder {
        scene: Scene::new(width as u16, height as u16),
        view,
    };
    builder.group(tree.root())?;
    Some(builder.scene)
}

struct SceneBuilder {
    scene: Scene,
    view: Affine,
}

impl SceneBuilder {
    fn group(&mut self, group: &usvg::Group) -> Option<()> {
        if group.clip_path().is_some()
            || group.mask().is_some()
            || !group.filters().is_empty()
            || group.blend_mode() != usvg::BlendMode::Normal
        {
            return None;
        }

        let opacity = group.opacity().get();
        let layered = opacity < 1.0;
        if layered {
            self.scene.push_opacity_layer(opacity);
        }
        for child in group.children() {
            self.node(child)?;
        }
        if layered {
            self.scene.pop_layer();
        }
        Some(())
    }

    fn node(&mut self, node: &usvg::Node) -> Option<()> {
        match node {
            usvg::Node::Group(group) => self.group(group),
            usvg::Node::Path(path) => self.path(path),
            // Text is already laid out into glyphs by usvg, and images need an
            // external texture binding. Keep those on the CPU path for now.
            usvg::Node::Text(_) | usvg::Node::Image(_) => None,
        }
    }

    fn path(&mut self, path: &usvg::Path) -> Option<()> {
        if !path.is_visible() {
            return Some(());
        }

        let data = to_bez_path(path.data());
        let transform = self.view * to_affine(path.abs_transform());
        self.scene.set_transform(transform);

        match path.paint_order() {
            usvg::PaintOrder::FillAndStroke => {
                self.fill(path, &data)?;
                self.stroke(path, &data)?;
            }
            usvg::PaintOrder::StrokeAndFill => {
                self.stroke(path, &data)?;
                self.fill(path, &data)?;
            }
        }
        Some(())
    }

    fn fill(&mut self, path: &usvg::Path, data: &BezPath) -> Option<()> {
        let Some(fill) = path.fill() else {
            return Some(());
        };
        let color = paint(fill.paint(), fill.opacity().get())?;
        self.scene.set_paint(color.paint);
        self.scene.set_paint_transform(color.transform);
        self.scene.set_fill_rule(match fill.rule() {
            usvg::FillRule::NonZero => vello_common::peniko::Fill::NonZero,
            usvg::FillRule::EvenOdd => vello_common::peniko::Fill::EvenOdd,
        });
        self.scene.fill_path(data);
        Some(())
    }

    fn stroke(&mut self, path: &usvg::Path, data: &BezPath) -> Option<()> {
        let Some(stroke) = path.stroke() else {
            return Some(());
        };
        let color = paint(stroke.paint(), stroke.opacity().get())?;
        self.scene.set_paint(color.paint);
        self.scene.set_paint_transform(color.transform);

        let mut style = Stroke::new(f64::from(stroke.width().get()))
            .with_miter_limit(f64::from(stroke.miterlimit().get()))
            .with_caps(match stroke.linecap() {
                usvg::LineCap::Butt => KurboCap::Butt,
                usvg::LineCap::Round => KurboCap::Round,
                usvg::LineCap::Square => KurboCap::Square,
            })
            .with_join(match stroke.linejoin() {
                usvg::LineJoin::Miter | usvg::LineJoin::MiterClip => KurboJoin::Miter,
                usvg::LineJoin::Round => KurboJoin::Round,
                usvg::LineJoin::Bevel => KurboJoin::Bevel,
            });

        if let Some(dashes) = stroke.dasharray()
            && !dashes.is_empty()
        {
            style = style.with_dashes(
                f64::from(stroke.dashoffset()),
                dashes.iter().copied().map(f64::from),
            );
        }

        self.scene.set_stroke(style);
        self.scene.stroke_path(data);
        Some(())
    }
}

struct GpuPaint {
    paint: PaintType,
    transform: Affine,
}

fn paint(source: &usvg::Paint, opacity: f32) -> Option<GpuPaint> {
    let opacity = opacity.clamp(0.0, 1.0);
    let color = |color: usvg::Color, alpha: f32| {
        Color::from_rgba8(
            color.red,
            color.green,
            color.blue,
            (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
        )
    };

    match source {
        usvg::Paint::Color(value) => Some(GpuPaint {
            paint: PaintType::Solid(color(*value, opacity)),
            transform: Affine::IDENTITY,
        }),
        usvg::Paint::LinearGradient(gradient) => {
            let stops = gradient_stops(gradient.stops(), opacity, color)?;
            let value = Gradient::new_linear(
                Point::new(f64::from(gradient.x1()), f64::from(gradient.y1())),
                Point::new(f64::from(gradient.x2()), f64::from(gradient.y2())),
            )
            .with_extend(extend(gradient.spread_method()))
            .with_stops(stops.as_slice());
            Some(GpuPaint {
                paint: PaintType::Gradient(value),
                transform: to_affine(gradient.transform()),
            })
        }
        usvg::Paint::RadialGradient(gradient) => {
            let stops = gradient_stops(gradient.stops(), opacity, color)?;
            let value = Gradient::new_two_point_radial(
                Point::new(f64::from(gradient.fx()), f64::from(gradient.fy())),
                gradient.fr().get(),
                Point::new(f64::from(gradient.cx()), f64::from(gradient.cy())),
                gradient.r().get(),
            )
            .with_extend(extend(gradient.spread_method()))
            .with_stops(stops.as_slice());
            Some(GpuPaint {
                paint: PaintType::Gradient(value),
                transform: to_affine(gradient.transform()),
            })
        }
        usvg::Paint::Pattern(_) => None,
    }
}

fn gradient_stops(
    stops: &[usvg::Stop],
    opacity: f32,
    color: impl Fn(usvg::Color, f32) -> Color,
) -> Option<Vec<(f32, Color)>> {
    if stops.len() < 2 {
        return None;
    }
    Some(
        stops
            .iter()
            .map(|stop| {
                (
                    stop.offset().get(),
                    color(stop.color(), stop.opacity().get() * opacity),
                )
            })
            .collect(),
    )
}

fn extend(spread: usvg::SpreadMethod) -> Extend {
    match spread {
        usvg::SpreadMethod::Pad => Extend::Pad,
        usvg::SpreadMethod::Reflect => Extend::Reflect,
        usvg::SpreadMethod::Repeat => Extend::Repeat,
    }
}

fn to_affine(transform: usvg::Transform) -> Affine {
    Affine::new([
        f64::from(transform.sx),
        f64::from(transform.ky),
        f64::from(transform.kx),
        f64::from(transform.sy),
        f64::from(transform.tx),
        f64::from(transform.ty),
    ])
}

fn to_bez_path(path: &usvg::tiny_skia_path::Path) -> BezPath {
    let mut result = BezPath::new();
    for segment in path.segments() {
        match segment {
            usvg::tiny_skia_path::PathSegment::MoveTo(point) => {
                result.move_to(Point::new(f64::from(point.x), f64::from(point.y)));
            }
            usvg::tiny_skia_path::PathSegment::LineTo(point) => {
                result.line_to(Point::new(f64::from(point.x), f64::from(point.y)));
            }
            usvg::tiny_skia_path::PathSegment::QuadTo(control, point) => {
                result.quad_to(
                    Point::new(f64::from(control.x), f64::from(control.y)),
                    Point::new(f64::from(point.x), f64::from(point.y)),
                );
            }
            usvg::tiny_skia_path::PathSegment::CubicTo(control1, control2, point) => {
                result.curve_to(
                    Point::new(f64::from(control1.x), f64::from(control1.y)),
                    Point::new(f64::from(control2.x), f64::from(control2.y)),
                    Point::new(f64::from(point.x), f64::from(point.y)),
                );
            }
            usvg::tiny_skia_path::PathSegment::Close => result.close_path(),
        }
    }
    result
}
