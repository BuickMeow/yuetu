//! Vector document loading and rasterization.
//!
//! SVG uses `usvg`/`resvg`, PDF uses `hayro`, and PostScript/EPS uses `stet`.
//! AI files are dispatched by their real container: modern AI files are PDF,
//! older ones are PostScript/EPS.

use std::path::Path;

use resvg::tiny_skia;
use thiserror::Error;
use yuetu_core::{ResizeAlgorithm, RgbaImage, resize::resize};

mod gpu;
mod gpu_pdf;

#[derive(Debug, Error)]
pub enum VectorError {
    #[error("failed to read vector file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse vector file: {0}")]
    Parse(String),
    #[error("vector page has invalid dimensions")]
    InvalidSize,
    #[error("vector page index is out of range")]
    InvalidPage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VectorFormat {
    Svg,
    Pdf,
    Ai,
    Eps,
}

struct PageInfo {
    width: f32,
    height: f32,
}

struct EpsBackend {
    pages: Vec<stet::DisplayListPage>,
}

enum Backend {
    Svg(Box<usvg::Tree>),
    Pdf(hayro::hayro_syntax::Pdf),
    Eps(EpsBackend),
}

pub struct VectorDocument {
    format: VectorFormat,
    backend: Backend,
    pages: Vec<PageInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewTransform {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl ViewTransform {
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn tiny_skia(self) -> tiny_skia::Transform {
        tiny_skia::Transform::from_row(self.a, self.b, self.c, self.d, self.e, self.f)
    }

    fn kurbo(self) -> hayro::kurbo::Affine {
        hayro::kurbo::Affine::new([
            f64::from(self.a),
            f64::from(self.b),
            f64::from(self.c),
            f64::from(self.d),
            f64::from(self.e),
            f64::from(self.f),
        ])
    }

    fn inverse(self) -> Option<Self> {
        let a = f64::from(self.a);
        let b = f64::from(self.b);
        let c = f64::from(self.c);
        let d = f64::from(self.d);
        let determinant = a * d - b * c;
        if determinant.abs() <= f64::EPSILON {
            return None;
        }
        let inv = 1.0 / determinant;
        Some(Self {
            a: (d * inv) as f32,
            b: (-b * inv) as f32,
            c: (-c * inv) as f32,
            d: (a * inv) as f32,
            e: ((c * f64::from(self.f) - d * f64::from(self.e)) * inv) as f32,
            f: ((b * f64::from(self.e) - a * f64::from(self.f)) * inv) as f32,
        })
    }

    fn apply(self, x: f64, y: f64) -> (f64, f64) {
        (
            f64::from(self.a) * x + f64::from(self.c) * y + f64::from(self.e),
            f64::from(self.b) * x + f64::from(self.d) * y + f64::from(self.f),
        )
    }

    fn is_axis_aligned(self) -> bool {
        self.b == 0.0 && self.c == 0.0 && self.a > 0.0 && self.d > 0.0
    }

    /// Map a page rotated clockwise into an output canvas.
    ///
    /// `scale_x`/`scale_y` are output pixels per rotated-canvas unit and
    /// `offset_x`/`offset_y` place that canvas inside the output.
    pub fn rotated_page(
        page_width: f32,
        page_height: f32,
        degrees: f32,
        scale_x: f32,
        scale_y: f32,
        offset_x: f32,
        offset_y: f32,
    ) -> Option<Self> {
        if page_width <= 0.0
            || page_height <= 0.0
            || scale_x <= 0.0
            || scale_y <= 0.0
            || !scale_x.is_finite()
            || !scale_y.is_finite()
        {
            return None;
        }
        let angle = yuetu_core::normalize_degrees(degrees);
        let (rotated_w, rotated_h) =
            yuetu_core::rotated_dimensions_f32(page_width, page_height, angle);
        if rotated_w <= 0.0 || rotated_h <= 0.0 {
            return None;
        }
        let radians = angle.to_radians();
        let (sin, cos) = radians.sin_cos();
        let ex = rotated_w * 0.5 - cos * page_width * 0.5 + sin * page_height * 0.5;
        let ey = rotated_h * 0.5 - sin * page_width * 0.5 - cos * page_height * 0.5;
        Some(Self {
            a: scale_x * cos,
            b: scale_y * sin,
            c: -scale_x * sin,
            d: scale_y * cos,
            e: scale_x * ex + offset_x,
            f: scale_y * ey + offset_y,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PageFit {
    Contain,
    Stretch,
}

impl VectorDocument {
    pub fn load(path: &Path) -> Result<Self, VectorError> {
        let bytes = std::fs::read(path)?;
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        Self::from_bytes(bytes, &extension)
    }

    pub fn from_bytes(bytes: Vec<u8>, extension: &str) -> Result<Self, VectorError> {
        let trimmed = &bytes[bytes
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())
            .unwrap_or(bytes.len())..];
        if trimmed.starts_with(b"%PDF-") || find_header(&bytes, b"%PDF-", 1024) {
            let pdf = hayro::hayro_syntax::Pdf::new(bytes)
                .map_err(|error| VectorError::Parse(format!("{error:?}")))?;
            let pages = pdf
                .pages()
                .iter()
                .map(|page| {
                    let (width, height) = page.render_dimensions();
                    PageInfo { width, height }
                })
                .collect::<Vec<_>>();
            if pages.is_empty() {
                return Err(VectorError::Parse("PDF 没有页面".into()));
            }
            return Ok(Self {
                format: if extension == "ai" {
                    VectorFormat::Ai
                } else {
                    VectorFormat::Pdf
                },
                backend: Backend::Pdf(pdf),
                pages,
            });
        }
        if trimmed.starts_with(b"%!PS") {
            let mut interpreter = stet::Interpreter::builder()
                .page_background(stet::PageBackground::Transparent)
                .suppress_output()
                .build();
            let rendered = interpreter
                .render_to_display_list(&bytes, 72.0)
                .map_err(|error| VectorError::Parse(error.to_string()))?;
            let pages = rendered
                .iter()
                .map(|page| PageInfo {
                    width: page.width as f32,
                    height: page.height as f32,
                })
                .collect::<Vec<_>>();
            if pages.is_empty() {
                return Err(VectorError::Parse(
                    "PostScript 没有输出页面（可能缺少 showpage）".into(),
                ));
            }
            return Ok(Self {
                format: if extension == "ai" {
                    VectorFormat::Ai
                } else {
                    VectorFormat::Eps
                },
                backend: Backend::Eps(EpsBackend { pages: rendered }),
                pages,
            });
        }
        if trimmed.starts_with(b"<svg") || trimmed.starts_with(b"<?xml") {
            let mut options = usvg::Options::default();
            options.fontdb_mut().load_system_fonts();
            let tree = usvg::Tree::from_data(&bytes, &options)
                .map_err(|error| VectorError::Parse(error.to_string()))?;
            let size = tree.size();
            return Ok(Self {
                format: VectorFormat::Svg,
                backend: Backend::Svg(Box::new(tree)),
                pages: vec![PageInfo {
                    width: size.width(),
                    height: size.height(),
                }],
            });
        }
        Err(VectorError::Parse("unsupported vector format".into()))
    }

    pub fn format(&self) -> VectorFormat {
        self.format
    }

    /// Whether the page can contain transparent pixels. PostScript/EPS paper
    /// is opaque; we still clip the GPU scene to the page polygon, but the
    /// source stage should show the empty view background around a rotated
    /// page instead of a transparency checkerboard.
    pub fn page_has_alpha(&self) -> bool {
        !matches!(self.backend, Backend::Eps(_))
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn page_size(&self, page: usize) -> Option<(f32, f32)> {
        self.pages.get(page).map(|page| (page.width, page.height))
    }

    /// Render the page using a page-user-unit -> output-pixel transform. Only
    /// the pixels needed by the viewport are touched; this is what keeps deep
    /// zoom from being limited by a full-page texture.
    pub fn render_page_view(
        &self,
        page: usize,
        width: u32,
        height: u32,
        transform: ViewTransform,
    ) -> Result<RgbaImage, VectorError> {
        if width == 0 || height == 0 {
            return Err(VectorError::InvalidSize);
        }
        match &self.backend {
            Backend::Svg(tree) => {
                if page != 0 {
                    return Err(VectorError::InvalidPage);
                }
                let mut pixmap =
                    tiny_skia::Pixmap::new(width, height).ok_or(VectorError::InvalidSize)?;
                resvg::render(tree, transform.tiny_skia(), &mut pixmap.as_mut());
                Ok(pixmap_to_rgba(&pixmap))
            }
            Backend::Pdf(pdf) => render_pdf_page_view(pdf, page, width, height, transform),
            Backend::Eps(eps) => render_eps_view(eps, page, width, height, transform),
        }
    }

    /// Build a Vello GPU scene using a page-user-unit -> output-pixel
    /// transform when the backend can express the page's drawing operations
    /// as vector paths. `None` means the caller should keep using the CPU
    /// renderer for this page.
    pub fn build_gpu_scene(
        &self,
        page: usize,
        width: u32,
        height: u32,
        transform: ViewTransform,
    ) -> Option<vello_gpu::Scene> {
        match &self.backend {
            Backend::Svg(tree) => {
                if page != 0 {
                    return None;
                }
                gpu::build(tree, width, height, transform)
            }
            Backend::Pdf(pdf) => {
                let pages = pdf.pages();
                let page = pages.get(page)?;
                gpu_pdf::build(page, width, height, transform)
            }
            // EPS paper is composited in the CPU renderer. Using a separate
            // white paper path in Vello double-antialiases shared page edges.
            Backend::Eps(_) => None,
        }
    }

    pub fn render_page_to_long_side(
        &self,
        page: usize,
        long_side: u32,
    ) -> Result<RgbaImage, VectorError> {
        let (page_w, page_h) = self.page_size(page).ok_or(VectorError::InvalidPage)?;
        if page_w <= 0.0 || page_h <= 0.0 || long_side == 0 {
            return Err(VectorError::InvalidSize);
        }
        let scale = long_side as f32 / page_w.max(page_h);
        let width = (page_w * scale).round().max(1.0) as u32;
        let height = (page_h * scale).round().max(1.0) as u32;
        self.render_page_exact(page, width, height)
    }

    /// Render a page rotated clockwise at an exact target size. The rotated
    /// page canvas is stretched to fill the target, matching the conversion
    /// resize used by the key-map and palette previews.
    pub fn render_page_rotated_to_size(
        &self,
        page: usize,
        width: u32,
        height: u32,
        degrees: f32,
    ) -> Result<RgbaImage, VectorError> {
        let transform = self
            .rotation_transform(page, width, height, degrees, PageFit::Stretch)
            .ok_or(VectorError::InvalidSize)?;
        self.render_page_view(page, width, height, transform)
    }

    /// Render a page rotated clockwise into a fitted RGBA raster.
    pub fn render_page_rotated_to_fit(
        &self,
        page: usize,
        width: u32,
        height: u32,
        degrees: f32,
    ) -> Result<RgbaImage, VectorError> {
        let transform = self
            .rotation_transform(page, width, height, degrees, PageFit::Contain)
            .ok_or(VectorError::InvalidSize)?;
        self.render_page_view(page, width, height, transform)
    }

    fn rotation_transform(
        &self,
        page: usize,
        width: u32,
        height: u32,
        degrees: f32,
        fit: PageFit,
    ) -> Option<ViewTransform> {
        let (page_w, page_h) = self.page_size(page)?;
        if page_w <= 0.0 || page_h <= 0.0 || width == 0 || height == 0 {
            return None;
        }
        let angle = yuetu_core::normalize_degrees(degrees);
        let (rotated_w, rotated_h) = yuetu_core::rotated_dimensions_f32(page_w, page_h, angle);
        if rotated_w <= 0.0 || rotated_h <= 0.0 {
            return None;
        }
        let (sx, sy, offset_x, offset_y) = match fit {
            PageFit::Contain => {
                let scale = (width as f32 / rotated_w).min(height as f32 / rotated_h);
                (
                    scale,
                    scale,
                    (width as f32 - rotated_w * scale) * 0.5,
                    (height as f32 - rotated_h * scale) * 0.5,
                )
            }
            PageFit::Stretch => (
                width as f32 / rotated_w,
                height as f32 / rotated_h,
                0.0,
                0.0,
            ),
        };
        ViewTransform::rotated_page(page_w, page_h, angle, sx, sy, offset_x, offset_y)
    }

    /// Render an exact-aspect page into the requested raster dimensions.
    pub fn render_page_exact(
        &self,
        page: usize,
        width: u32,
        height: u32,
    ) -> Result<RgbaImage, VectorError> {
        if width == 0 || height == 0 {
            return Err(VectorError::InvalidSize);
        }
        match &self.backend {
            Backend::Svg(tree) => {
                if page != 0 {
                    return Err(VectorError::InvalidPage);
                }
                let size = tree.size();
                if size.width() <= 0.0 || size.height() <= 0.0 {
                    return Err(VectorError::InvalidSize);
                }
                let scale = (width as f32 / size.width()).min(height as f32 / size.height());
                let offset_x = (width as f32 - size.width() * scale) * 0.5;
                let offset_y = (height as f32 - size.height() * scale) * 0.5;
                let transform = tiny_skia::Transform::from_translate(offset_x, offset_y)
                    .pre_scale(scale, scale);
                let mut pixmap =
                    tiny_skia::Pixmap::new(width, height).ok_or(VectorError::InvalidSize)?;
                resvg::render(tree, transform, &mut pixmap.as_mut());
                Ok(pixmap_to_rgba(&pixmap))
            }
            Backend::Pdf(pdf) => render_pdf_page(pdf, page, width, height),
            Backend::Eps(eps) => render_eps_exact(eps, page, width, height),
        }
    }
}

fn render_pdf_page(
    pdf: &hayro::hayro_syntax::Pdf,
    page: usize,
    width: u32,
    height: u32,
) -> Result<RgbaImage, VectorError> {
    let pages = pdf.pages();
    let page = pages.get(page).ok_or(VectorError::InvalidPage)?;
    let (page_w, page_h) = page.render_dimensions();
    if page_w <= 0.0 || page_h <= 0.0 {
        return Err(VectorError::InvalidSize);
    }
    let scale_x = width as f32 / page_w;
    let scale_y = height as f32 / page_h;
    // hayro requires a cache object for the duration of this call; it is
    // dropped with the render and never retained between frames.
    let render_cache = hayro::RenderCache::new();
    let settings = hayro::hayro_interpret::InterpreterSettings::default();
    let render_settings = hayro::RenderSettings::default();
    let pixmap = hayro::render(
        page,
        &render_cache,
        &settings,
        &render_settings,
        &hayro::PixmapSettings {
            x_scale: scale_x,
            y_scale: scale_y,
            bg_color: hayro::vello_cpu::color::palette::css::TRANSPARENT,
        },
    );
    let mut image = vello_pixmap_to_rgba(&pixmap);
    if image.width != width || image.height != height {
        image = center_into_resized(image, width, height)?;
    }
    Ok(image)
}

fn render_pdf_page_view(
    pdf: &hayro::hayro_syntax::Pdf,
    page: usize,
    width: u32,
    height: u32,
    transform: ViewTransform,
) -> Result<RgbaImage, VectorError> {
    let pages = pdf.pages();
    let page = pages.get(page).ok_or(VectorError::InvalidPage)?;
    if width > u32::from(u16::MAX) || height > u32::from(u16::MAX) {
        return Err(VectorError::InvalidSize);
    }
    let settings = hayro::hayro_interpret::InterpreterSettings::default();
    let render_settings = hayro::RenderSettings::default();
    // Per-call scratch required by hayro; not retained across renders.
    let render_cache = hayro::RenderCache::new();
    let mut ctx = hayro::vello_cpu::RenderContext::new(width as u16, height as u16);
    use hayro::hayro_interpret::util::TransformExt;
    let page_transform = transform.kurbo() * page.initial_transform(true).to_kurbo();
    hayro::render_into(
        page,
        &render_cache,
        &settings,
        &render_settings,
        &mut ctx,
        page_transform,
    );
    ctx.flush();
    let mut pixmap = hayro::vello_cpu::Pixmap::new(ctx.width(), ctx.height());
    let mut resources = hayro::vello_cpu::Resources::default();
    ctx.render_with(
        &mut pixmap,
        &mut resources,
        hayro::vello_cpu::RasterizerSettings {
            target_init: hayro::vello_cpu::TargetInit::Clear(
                hayro::vello_cpu::color::palette::css::TRANSPARENT,
            ),
            ..Default::default()
        },
    );
    Ok(vello_pixmap_to_rgba(&pixmap))
}

fn render_eps_exact(
    eps: &EpsBackend,
    page: usize,
    width: u32,
    height: u32,
) -> Result<RgbaImage, VectorError> {
    let page = eps.pages.get(page).ok_or(VectorError::InvalidPage)?;
    if page.width == 0 || page.height == 0 {
        return Err(VectorError::InvalidSize);
    }
    render_eps_region(
        page,
        0.0,
        0.0,
        page.width as f64,
        page.height as f64,
        width,
        height,
    )
}

fn render_eps_view(
    eps: &EpsBackend,
    page: usize,
    width: u32,
    height: u32,
    transform: ViewTransform,
) -> Result<RgbaImage, VectorError> {
    let page = eps.pages.get(page).ok_or(VectorError::InvalidPage)?;
    if transform.is_axis_aligned() {
        let vp_x = (0.0 - transform.e) / transform.a;
        let vp_y = (0.0 - transform.f) / transform.d;
        let vp_w = width as f32 / transform.a;
        let vp_h = height as f32 / transform.d;
        let mut image = render_eps_region(
            page,
            f64::from(vp_x),
            f64::from(vp_y),
            f64::from(vp_w),
            f64::from(vp_h),
            width,
            height,
        )?;
        unmatte_eps_page(
            &mut image,
            f64::from(page.width),
            f64::from(page.height),
            transform,
        );
        return Ok(image);
    }
    render_eps_rotated_view(page, width, height, transform)
}

fn render_eps_region(
    page: &stet::DisplayListPage,
    vp_x: f64,
    vp_y: f64,
    vp_w: f64,
    vp_h: f64,
    width: u32,
    height: u32,
) -> Result<RgbaImage, VectorError> {
    let prepared = stet::prepare_display_list(&page.display_list);
    let rgba = stet::render_region_prepared(
        &page.display_list,
        &prepared,
        vp_x,
        vp_y,
        vp_w,
        vp_h,
        width,
        height,
        72.0,
        None,
        None,
        false,
    );
    RgbaImage::from_rgba(width, height, rgba).map_err(|error| VectorError::Parse(error.to_string()))
}

fn render_eps_rotated_view(
    page: &stet::DisplayListPage,
    width: u32,
    height: u32,
    transform: ViewTransform,
) -> Result<RgbaImage, VectorError> {
    let inverse = transform
        .inverse()
        .ok_or_else(|| VectorError::Parse("EPS 视口变换不可逆".into()))?;
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for (x, y) in [
        (0.0, 0.0),
        (f64::from(width), 0.0),
        (0.0, f64::from(height)),
        (f64::from(width), f64::from(height)),
    ] {
        let (px, py) = inverse.apply(x, y);
        min_x = min_x.min(px);
        min_y = min_y.min(py);
        max_x = max_x.max(px);
        max_y = max_y.max(py);
    }
    let region_w = max_x - min_x;
    let region_h = max_y - min_y;
    if !region_w.is_finite() || !region_h.is_finite() || region_w <= 0.0 || region_h <= 0.0 {
        return Err(VectorError::InvalidSize);
    }
    let density_x = f64::from(transform.a).hypot(f64::from(transform.b));
    let density_y = f64::from(transform.c).hypot(f64::from(transform.d));
    if !density_x.is_finite() || !density_y.is_finite() || density_x <= 0.0 || density_y <= 0.0 {
        return Err(VectorError::InvalidSize);
    }
    let raster_w = (region_w * density_x).ceil().max(1.0) as u32;
    let raster_h = (region_h * density_y).ceil().max(1.0) as u32;
    if raster_w > u32::from(u16::MAX) || raster_h > u32::from(u16::MAX) {
        return Err(VectorError::InvalidSize);
    }
    let scale_x = f64::from(raster_w) / region_w;
    let scale_y = f64::from(raster_h) / region_h;
    let mut raster = render_eps_region(page, min_x, min_y, region_w, region_h, raster_w, raster_h)?;
    unmatte_eps_page(
        &mut raster,
        f64::from(page.width),
        f64::from(page.height),
        ViewTransform {
            a: scale_x as f32,
            b: 0.0,
            c: 0.0,
            d: scale_y as f32,
            e: (-min_x * scale_x) as f32,
            f: (-min_y * scale_y) as f32,
        },
    );

    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let (px, py) = inverse.apply(f64::from(x) + 0.5, f64::from(y) + 0.5);
            let color = yuetu_core::sample_bilinear(
                &raster,
                (px - min_x) * scale_x,
                (py - min_y) * scale_y,
            );
            pixels.extend_from_slice(&color);
        }
    }
    RgbaImage::from_rgba(width, height, pixels)
        .map_err(|error| VectorError::Parse(error.to_string()))
}

/// stet's region renderer always composites the page onto white paper, even
/// when only part of a page edge covers an output pixel. That is correct for a
/// fully opaque page, but when the page is shown over the app background the
/// partial edge pixels keep a white matte and read as a one-pixel white fringe.
///
/// Inverse the matte using the page rectangle's coverage: `out = content +
/// white * (1 - page_coverage)` becomes `content = out - white * (1 - c)` and
/// `alpha = c`. Pixels inside the page keep their original result; only page
/// boundary pixels change. The white paper visible *inside* the page is part
/// of the content and stays intact.
fn unmatte_eps_page(
    image: &mut RgbaImage,
    page_width: f64,
    page_height: f64,
    transform: ViewTransform,
) {
    if page_width <= 0.0 || page_height <= 0.0 || image.width == 0 || image.height == 0 {
        return;
    }
    let x0 = f64::from(transform.e);
    let x1 = x0 + f64::from(transform.a) * page_width;
    let y0 = f64::from(transform.f);
    let y1 = y0 + f64::from(transform.d) * page_height;
    let width = f64::from(image.width);
    let height = f64::from(image.height);
    // The common crop covers the whole texture; avoid touching every pixel.
    if x0 <= 0.0 && x1 >= width && y0 <= 0.0 && y1 >= height {
        return;
    }

    for y in 0..image.height {
        let cy = interval_coverage(y, y0, y1);
        if cy <= 0.0 {
            clear_scanline(image, y);
            continue;
        }
        for x in 0..image.width {
            let cx = interval_coverage(x, x0, x1);
            let coverage = cx * cy;
            if coverage >= 1.0 - 1e-4 {
                continue;
            }
            let index = (y as usize * image.width as usize + x as usize) * 4;
            if coverage <= 0.0 {
                image.pixels[index..index + 4].fill(0);
                continue;
            }
            // stet added `255 - alpha` to each premultiplied channel to put
            // the page on white. Remove the same amount for the page area the
            // output pixel does not cover.
            let white = (1.0 - coverage) * 255.0;
            for channel in &mut image.pixels[index..index + 3] {
                let premultiplied = (f64::from(*channel) - white).max(0.0);
                *channel = (premultiplied / coverage).round().clamp(0.0, 255.0) as u8;
            }
            image.pixels[index + 3] = (coverage * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

fn interval_coverage(pixel: u32, start: f64, end: f64) -> f64 {
    let (start, end) = if start <= end {
        (start, end)
    } else {
        (end, start)
    };
    let pixel_start = f64::from(pixel);
    let pixel_end = pixel_start + 1.0;
    (end.min(pixel_end) - start.max(pixel_start)).clamp(0.0, 1.0)
}

fn clear_scanline(image: &mut RgbaImage, y: u32) {
    let start = y as usize * image.width as usize * 4;
    let end = start + image.width as usize * 4;
    image.pixels[start..end].fill(0);
}

fn vello_pixmap_to_rgba(pixmap: &hayro::vello_cpu::Pixmap) -> RgbaImage {
    let width = pixmap.width() as u32;
    let height = pixmap.height() as u32;
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for pixel in pixmap.data() {
        let a = pixel.a;
        if a == 0 {
            pixels.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let r = ((u16::from(pixel.r) * 255 + u16::from(a) / 2) / u16::from(a)) as u8;
            let g = ((u16::from(pixel.g) * 255 + u16::from(a) / 2) / u16::from(a)) as u8;
            let b = ((u16::from(pixel.b) * 255 + u16::from(a) / 2) / u16::from(a)) as u8;
            pixels.extend_from_slice(&[r, g, b, a]);
        }
    }
    RgbaImage {
        width,
        height,
        pixels,
    }
}

fn pixmap_to_rgba(pixmap: &tiny_skia::Pixmap) -> RgbaImage {
    let mut pixels = Vec::with_capacity(pixmap.data().len());
    for pixel in pixmap.pixels() {
        let color = pixel.demultiply();
        pixels.extend_from_slice(&[color.red(), color.green(), color.blue(), color.alpha()]);
    }
    RgbaImage {
        width: pixmap.width(),
        height: pixmap.height(),
        pixels,
    }
}

fn center_into(image: RgbaImage, width: u32, height: u32) -> RgbaImage {
    if image.width == width && image.height == height {
        return image;
    }
    let mut out = RgbaImage::new(width, height, yuetu_core::Color::TRANSPARENT);
    let dx = (width as i64 - image.width as i64) / 2;
    let dy = (height as i64 - image.height as i64) / 2;
    for y in 0..image.height {
        let ty = y as i64 + dy;
        if ty < 0 || ty >= i64::from(height) {
            continue;
        }
        for x in 0..image.width {
            let tx = x as i64 + dx;
            if tx < 0 || tx >= i64::from(width) {
                continue;
            }
            if let Some(color) = image.pixel(x, y) {
                out.set_pixel(tx as u32, ty as u32, color);
            }
        }
    }
    out
}

fn center_into_resized(
    image: RgbaImage,
    width: u32,
    height: u32,
) -> Result<RgbaImage, VectorError> {
    let scale = (width as f32 / image.width as f32).min(height as f32 / image.height as f32);
    let resized_w = (image.width as f32 * scale).round().max(1.0) as u32;
    let resized_h = (image.height as f32 * scale).round().max(1.0) as u32;
    let resized = resize(&image, resized_w, resized_h, ResizeAlgorithm::Area)
        .map_err(|error| VectorError::Parse(error.to_string()))?;
    Ok(center_into(resized, width, height))
}

fn find_header(bytes: &[u8], needle: &[u8], limit: usize) -> bool {
    let end = bytes.len().min(limit);
    bytes[..end]
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> VectorDocument {
        let path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata")).join(name);
        VectorDocument::load(&path).unwrap()
    }

    #[test]
    fn svg_renders_to_fit_with_straight_rgba() {
        let document = fixture("vector-red.svg");
        let image = document.render_page_exact(0, 100, 100).unwrap();
        assert_eq!((image.width, image.height), (100, 100));
        assert_eq!(image.pixel(50, 50).unwrap().r, 255);
        assert_eq!(image.pixel(50, 50).unwrap().g, 0);
        assert_eq!(image.pixel(50, 50).unwrap().b, 0);
        assert_eq!(image.pixel(50, 50).unwrap().a, 255);
    }

    #[test]
    fn render_to_long_side_preserves_page_aspect() {
        let document = fixture("vector-ratio.svg");
        let image = document.render_page_to_long_side(0, 200).unwrap();
        assert_eq!((image.width, image.height), (200, 100));
        assert_eq!(image.pixel(100, 10).unwrap().g, 255);
        assert_eq!(image.pixel(100, 10).unwrap().a, 255);
    }

    #[test]
    fn rotated_render_uses_the_expanded_canvas() {
        let bytes = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="100" height="50" fill="#00ff00"/></svg>"##;
        let document = VectorDocument::from_bytes(bytes.to_vec(), "svg").unwrap();
        let image = document
            .render_page_rotated_to_fit(0, 100, 100, 90.0)
            .unwrap();
        assert_eq!(image.pixel(50, 50).unwrap().g, 255);
        assert_eq!(image.pixel(5, 50).unwrap().a, 0);
        assert_eq!(image.pixel(95, 50).unwrap().a, 0);
    }

    #[test]
    fn svg_view_render_maps_the_page_transform() {
        let document = fixture("vector-red.svg");
        let image = document
            .render_page_view(
                0,
                50,
                50,
                ViewTransform {
                    a: 0.5,
                    b: 0.0,
                    c: 0.0,
                    d: 0.5,
                    e: 0.0,
                    f: 0.0,
                },
            )
            .unwrap();
        assert_eq!((image.width, image.height), (50, 50));
        assert_eq!(image.pixel(25, 25).unwrap().r, 255);
        assert_eq!(image.pixel(25, 25).unwrap().a, 255);
    }

    #[test]
    fn pdf_view_render_renders_a_crop() {
        let document = fixture("vector.pdf");
        let image = document
            .render_page_view(
                0,
                200,
                150,
                ViewTransform {
                    a: 0.5,
                    b: 0.0,
                    c: 0.0,
                    d: 0.5,
                    e: 0.0,
                    f: 0.0,
                },
            )
            .unwrap();
        assert_eq!((image.width, image.height), (200, 150));
        assert!(image.pixels.iter().any(|byte| *byte != 0));
    }

    #[test]
    fn pdf_view_render_respects_translation_and_scale() {
        let document = fixture("vector.pdf");
        let image = document
            .render_page_view(
                0,
                200,
                200,
                ViewTransform {
                    a: 2.0,
                    b: 0.0,
                    c: 0.0,
                    d: 2.0,
                    e: -120.0,
                    f: -180.0,
                },
            )
            .unwrap();
        // The first page's red circle is centered at (130, 160) in PDF points.
        let pixel = image.pixel(140, 140).unwrap();
        assert!(pixel.r > 150, "{pixel:?}");
        assert!(pixel.a > 200, "{pixel:?}");
    }

    #[test]
    fn pdf_view_render_uses_a_viewport_offset() {
        let document = fixture("vector.pdf");
        let image = document
            .render_page_view(
                0,
                1000,
                800,
                ViewTransform {
                    a: 1.2275,
                    b: 0.0,
                    c: 0.0,
                    d: 1.23,
                    e: 300.0,
                    f: 200.0,
                },
            )
            .unwrap();
        let pixel = image.pixel(459, 396).unwrap();
        assert!(pixel.r > 150, "{pixel:?}");
        assert!(pixel.a > 200, "{pixel:?}");
    }

    #[test]
    fn eps_view_render_renders_an_axis_aligned_crop() {
        let document = fixture("vector.eps");
        let image = document
            .render_page_view(
                0,
                200,
                150,
                ViewTransform {
                    a: 0.5,
                    b: 0.0,
                    c: 0.0,
                    d: 0.5,
                    e: 0.0,
                    f: 0.0,
                },
            )
            .unwrap();
        assert_eq!((image.width, image.height), (200, 150));
        let pixel = image.pixel(65, 80).unwrap();
        assert!(pixel.r > 150, "{pixel:?}");
        assert!(pixel.a > 200, "{pixel:?}");
    }

    #[test]
    fn eps_view_render_removes_white_paper_at_fractional_page_edges() {
        let document = fixture("vector.eps");
        let image = document
            .render_page_view(
                0,
                800,
                600,
                ViewTransform {
                    a: 2.0,
                    b: 0.0,
                    c: 0.0,
                    d: 2.0,
                    e: 0.3,
                    f: 0.3,
                },
            )
            .unwrap();
        // The page background is dark and fills the page edge. The boundary
        // pixel must keep that color with partial coverage instead of being
        // matted onto stet's white paper.
        let corner = image.pixel(0, 0).unwrap();
        assert!(
            corner.r < 40 && corner.g < 40 && corner.b < 40,
            "{corner:?}"
        );
        assert!(corner.a > 0 && corner.a < 255, "{corner:?}");
        let inner = image.pixel(10, 10).unwrap();
        assert_eq!(inner.a, 255);
        assert!(inner.r < 40, "{inner:?}");
    }

    #[test]
    fn eps_view_render_keeps_white_paper_inside_the_page() {
        let bytes =
            b"%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 100 100\nshowpage\n%%EOF".to_vec();
        let document = VectorDocument::from_bytes(bytes, "eps").unwrap();
        let image = document
            .render_page_view(
                0,
                200,
                200,
                ViewTransform {
                    a: 2.0,
                    b: 0.0,
                    c: 0.0,
                    d: 2.0,
                    e: 0.3,
                    f: 0.3,
                },
            )
            .unwrap();
        // An unpainted page is white paper. Only its coverage is removed at
        // the page edge; the paper color itself must survive.
        let corner = image.pixel(0, 0).unwrap();
        assert!(
            corner.r > 240 && corner.g > 240 && corner.b > 240,
            "{corner:?}"
        );
        assert!(corner.a > 0 && corner.a < 255, "{corner:?}");
    }

    #[test]
    fn eps_view_render_rotates_without_a_white_fringe() {
        let document = fixture("vector.eps");
        let image = document
            .render_page_view(
                0,
                300,
                400,
                ViewTransform {
                    a: 0.0,
                    b: 1.0,
                    c: -1.0,
                    d: 0.0,
                    e: 300.0,
                    f: 0.0,
                },
            )
            .unwrap();
        // The dark page background reaches every edge. A rotated page corner
        // may be partially covered, but its color must stay dark rather than
        // being matted onto white paper.
        for (x, y) in [(0, 0), (299, 0), (0, 399), (299, 399)] {
            let pixel = image.pixel(x, y).unwrap();
            assert!(pixel.r < 40 && pixel.g < 40 && pixel.b < 40, "{pixel:?}");
            assert!(pixel.a > 0, "{pixel:?}");
        }
        let center = image.pixel(150, 200).unwrap();
        assert_eq!(center.a, 255);
    }

    #[test]
    fn eps_rotated_target_render_fills_the_exact_preview_size() {
        let document = fixture("vector.eps");
        let image = document
            .render_page_rotated_to_size(0, 128, 1536, 31.0)
            .unwrap();
        assert_eq!((image.width, image.height), (128, 1536));
        assert!(
            image
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] > 0)
        );
    }

    #[test]
    fn svg_builds_a_gpu_scene_for_solid_paths() {
        let document = fixture("vector-red.svg");
        let scene = document.build_gpu_scene(
            0,
            64,
            64,
            ViewTransform {
                a: 0.64,
                b: 0.0,
                c: 0.0,
                d: 0.64,
                e: 0.0,
                f: 0.0,
            },
        );
        assert!(scene.is_some());
    }

    #[test]
    fn svg_builds_a_gpu_scene_for_gradients() {
        let document = fixture("vector.svg");
        let scene = document.build_gpu_scene(
            0,
            800,
            600,
            ViewTransform {
                a: 1.0,
                b: 0.0,
                c: 0.0,
                d: 1.0,
                e: 0.0,
                f: 0.0,
            },
        );
        assert!(scene.is_some());
    }

    #[test]
    fn pdf_builds_a_gpu_scene_for_paths() {
        let document = fixture("vector.pdf");
        let scene = document.build_gpu_scene(
            0,
            800,
            600,
            ViewTransform {
                a: 2.0,
                b: 0.0,
                c: 0.0,
                d: 2.0,
                e: 0.0,
                f: 0.0,
            },
        );
        assert!(scene.is_some());
        let page_two = document.build_gpu_scene(
            1,
            800,
            600,
            ViewTransform {
                a: 2.0,
                b: 0.0,
                c: 0.0,
                d: 2.0,
                e: 0.0,
                f: 0.0,
            },
        );
        assert!(page_two.is_some());
    }

    #[test]
    fn eps_uses_the_cpu_renderer_for_every_transform() {
        for name in ["vector.eps", "vector-eps.ai"] {
            let document = fixture(name);
            let scene = document.build_gpu_scene(
                0,
                800,
                600,
                ViewTransform {
                    a: 2.0,
                    b: 0.0,
                    c: 0.0,
                    d: 2.0,
                    e: 0.0,
                    f: 0.0,
                },
            );
            assert!(scene.is_none(), "{name}");
        }
    }

    #[test]
    fn modern_ai_builds_a_gpu_scene_through_pdf() {
        let document = fixture("vector.ai");
        let scene = document.build_gpu_scene(
            0,
            800,
            600,
            ViewTransform {
                a: 2.0,
                b: 0.0,
                c: 0.0,
                d: 2.0,
                e: 0.0,
                f: 0.0,
            },
        );
        assert!(scene.is_some());
    }

    #[test]
    fn rotated_target_render_fills_the_exact_preview_size() {
        let document = fixture("vector.svg");
        let image = document
            .render_page_rotated_to_size(0, 128, 1536, 31.0)
            .unwrap();
        assert_eq!((image.width, image.height), (128, 1536));
        assert!(image.pixels.iter().any(|byte| *byte != 0));

        let document = fixture("vector.pdf");
        let image = document
            .render_page_rotated_to_size(0, 128, 1536, 31.0)
            .unwrap();
        assert_eq!((image.width, image.height), (128, 1536));
        assert!(image.pixels.iter().any(|byte| *byte != 0));
    }

    #[test]
    fn unsupported_files_are_rejected() {
        assert!(VectorDocument::from_bytes(b"not a vector".to_vec(), "txt").is_err());
    }
}
