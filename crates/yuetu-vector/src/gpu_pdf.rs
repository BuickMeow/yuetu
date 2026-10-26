//! Opt-in Vello GPU scene extraction for the path-based subset of PDF pages.
//!
//! PDF is an interpreted format, not a scene graph. `hayro` exposes its
//! `Device` interface, which is enough to turn ordinary paths, glyph outlines,
//! clips and opacity groups into Vello commands. Images, patterns, soft masks,
//! Type3 glyphs and non-normal blend modes still need the CPU renderer, so a
//! page containing any of them is rejected as a whole.

use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::util::{RectExt, TransformExt};
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, FillRule, Image, ImageDrawProps,
    InterpreterCache, InterpreterSettings, Paint, SoftMask, StrokeProps, interpret_page,
};
use hayro::hayro_syntax::page::Page;
use hayro::kurbo;
use vello_common::kurbo::{
    Affine, BezPath, Cap as KurboCap, Join as KurboJoin, Rect, Shape, Stroke,
};
use vello_common::peniko::Color;
use vello_gpu::Scene;

use crate::ViewTransform;

pub(crate) fn build(
    page: &Page<'_>,
    width: u32,
    height: u32,
    transform: ViewTransform,
) -> Option<Scene> {
    if width == 0 || height == 0 || width > u32::from(u16::MAX) || height > u32::from(u16::MAX) {
        return None;
    }

    let initial = to_affine(transform) * page.initial_transform(true).to_kurbo();
    // Per-call scratch required by hayro; not retained across scenes.
    let interpreter_cache = InterpreterCache::new();
    let settings = InterpreterSettings::default();
    let bbox = Rect::new(0.0, 0.0, f64::from(width as u16), f64::from(height as u16));
    let mut context = Context::new(initial, bbox, &interpreter_cache, page.xref(), settings);

    let mut scene = Scene::new(width as u16, height as u16);
    let mut clip = page.intersected_crop_box().to_kurbo().to_path(0.1);
    clip.apply_affine(initial);
    scene.push_clip_path(&clip);

    let mut device = PdfDevice {
        scene,
        groups: Vec::new(),
        unsupported: false,
    };
    interpret_page(page, &mut context, &mut device);

    if device.unsupported {
        None
    } else {
        Some(device.scene)
    }
}

struct PdfDevice {
    scene: Scene,
    groups: Vec<bool>,
    unsupported: bool,
}

impl PdfDevice {
    fn paint(&mut self, paint: &Paint<'_>) -> Option<Color> {
        match paint {
            Paint::Color(color) => {
                let [r, g, b, a] = color.to_rgba().to_rgba8();
                Some(Color::from_rgba8(r, g, b, a))
            }
            Paint::Pattern(_) => {
                self.unsupported = true;
                None
            }
        }
    }

    fn path_style(&mut self, props: &DrawProps<'_>) -> Option<Color> {
        if props.soft_mask.is_some() || props.blend_mode != BlendMode::Normal {
            self.unsupported = true;
            return None;
        }
        self.paint(&props.paint)
    }

    fn fill(&mut self, path: &BezPath, props: &DrawProps<'_>, rule: FillRule) {
        let Some(color) = self.path_style(props) else {
            return;
        };
        self.scene.set_transform(props.transform);
        self.scene.set_paint(color);
        self.scene.set_fill_rule(match rule {
            FillRule::NonZero => vello_common::peniko::Fill::NonZero,
            FillRule::EvenOdd => vello_common::peniko::Fill::EvenOdd,
        });
        self.scene.fill_path(path);
    }

    fn stroke(&mut self, path: &BezPath, props: &DrawProps<'_>, style: &StrokeProps) {
        let Some(color) = self.path_style(props) else {
            return;
        };
        self.scene.set_transform(props.transform);
        self.scene.set_paint(color);
        let mut stroke = Stroke::new(f64::from(style.line_width))
            .with_miter_limit(f64::from(style.miter_limit))
            .with_caps(match style.line_cap {
                kurbo::Cap::Butt => KurboCap::Butt,
                kurbo::Cap::Round => KurboCap::Round,
                kurbo::Cap::Square => KurboCap::Square,
            })
            .with_join(match style.line_join {
                kurbo::Join::Miter => KurboJoin::Miter,
                kurbo::Join::Round => KurboJoin::Round,
                kurbo::Join::Bevel => KurboJoin::Bevel,
            });
        if !style.dash_array.is_empty() {
            stroke = stroke.with_dashes(
                f64::from(style.dash_offset),
                style.dash_array.iter().copied().map(f64::from),
            );
        }
        self.scene.set_stroke(stroke);
        self.scene.stroke_path(path);
    }

    fn draw(&mut self, path: &BezPath, props: &DrawProps<'_>, mode: &DrawMode) {
        match mode {
            DrawMode::Fill(rule) => self.fill(path, props, *rule),
            DrawMode::Stroke(style) => self.stroke(path, props, style),
            DrawMode::FillAndStroke(rule, style) => {
                self.fill(path, props, *rule);
                self.stroke(path, props, style);
            }
            DrawMode::Invisible => {}
        }
    }
}

impl<'a> Device<'a> for PdfDevice {
    fn draw_image(&mut self, _image: Image<'a, '_>, _props: ImageDrawProps<'a>) {
        self.unsupported = true;
    }

    fn push_clip_path(&mut self, clip_path: &ClipPath) {
        self.scene.set_transform(Affine::IDENTITY);
        self.scene.set_fill_rule(match clip_path.fill {
            FillRule::NonZero => vello_common::peniko::Fill::NonZero,
            FillRule::EvenOdd => vello_common::peniko::Fill::EvenOdd,
        });
        self.scene.push_clip_path(&clip_path.path);
    }

    fn push_transparency_group(
        &mut self,
        opacity: f32,
        mask: Option<SoftMask<'a>>,
        blend_mode: BlendMode,
    ) {
        if mask.is_some() || blend_mode != BlendMode::Normal {
            self.unsupported = true;
        }
        let layered = opacity < 1.0 && mask.is_none() && blend_mode == BlendMode::Normal;
        if layered {
            self.scene.push_opacity_layer(opacity);
        }
        self.groups.push(layered);
    }

    fn draw_path(&mut self, path: &BezPath, props: DrawProps<'a>, draw_mode: &DrawMode) {
        self.draw(path, &props, draw_mode);
    }

    fn draw_rect(&mut self, rect: &Rect, props: DrawProps<'a>, draw_mode: &DrawMode) {
        self.draw(&rect.to_path(0.1), &props, draw_mode);
    }

    fn draw_glyph_run(
        &mut self,
        glyph_run: &GlyphRun<'_, 'a>,
        props: DrawProps<'a>,
        draw_mode: &DrawMode,
    ) {
        if props.soft_mask.is_some() || props.blend_mode != BlendMode::Normal {
            self.unsupported = true;
            return;
        }
        for glyph in glyph_run.glyphs() {
            let Glyph::Outline(outline) = &**glyph else {
                // Type3 glyphs execute more drawing instructions; keep them on
                // the CPU path instead of duplicating that interpreter here.
                self.unsupported = true;
                continue;
            };
            let path = outline.outline();
            let transform = props.transform * glyph.transform();
            let positioned = DrawProps {
                transform,
                ..props.clone()
            };
            self.draw(&path, &positioned, draw_mode);
        }
    }

    fn pop_clip(&mut self) {
        self.scene.pop_clip();
    }

    fn pop_transparency_group(&mut self) {
        if self.groups.pop().unwrap_or(false) {
            self.scene.pop_layer();
        }
    }
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
