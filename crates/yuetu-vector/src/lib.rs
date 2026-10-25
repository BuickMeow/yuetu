//! Vector document loading and rasterization.
//!
//! SVG uses `usvg`/`resvg`, PDF uses `hayro`, and PostScript/EPS uses `stet`.
//! AI files are dispatched by their real container: modern AI files are PDF,
//! older ones are PostScript/EPS.

use std::path::Path;

use resvg::tiny_skia;
use thiserror::Error;
use yuetu_core::{ResizeAlgorithm, RgbaImage, resize::resize, rotate::rotate_image};

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
    Svg(usvg::Tree),
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
        let trimmed = bytes
            .iter()
            .copied()
            .skip_while(u8::is_ascii_whitespace)
            .collect::<Vec<_>>();
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
                backend: Backend::Svg(tree),
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

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn page_size(&self, page: usize) -> Option<(f32, f32)> {
        self.pages.get(page).map(|page| (page.width, page.height))
    }

    /// Whether the backend can render an arbitrary cropped view instead of a
    /// full page. EPS currently uses the full-page fallback.
    pub fn supports_view_render(&self) -> bool {
        true
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
                let mut pixmap =
                    tiny_skia::Pixmap::new(width, height).ok_or(VectorError::InvalidSize)?;
                resvg::render(tree, transform.tiny_skia(), &mut pixmap.as_mut());
                Ok(pixmap_to_rgba(&pixmap))
            }
            Backend::Pdf(pdf) => render_pdf_page_view(pdf, page, width, height, transform),
            Backend::Eps(eps) => render_eps_view(eps, page, width, height, transform),
        }
    }

    /// Render the first page into a raster with the requested long side.
    pub fn render_to_long_side(&self, long_side: u32) -> Result<RgbaImage, VectorError> {
        self.render_page_to_long_side(0, long_side)
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

    /// Render the first page rotated clockwise into a fitted RGBA raster.
    pub fn render_rotated_to_fit(
        &self,
        width: u32,
        height: u32,
        degrees: f32,
    ) -> Result<RgbaImage, VectorError> {
        self.render_page_rotated_to_fit(0, width, height, degrees)
    }

    /// Render a page rotated clockwise into a fitted RGBA raster.
    pub fn render_page_rotated_to_fit(
        &self,
        page: usize,
        width: u32,
        height: u32,
        degrees: f32,
    ) -> Result<RgbaImage, VectorError> {
        let (page_w, page_h) = self.page_size(page).ok_or(VectorError::InvalidPage)?;
        if page_w <= 0.0 || page_h <= 0.0 || width == 0 || height == 0 {
            return Err(VectorError::InvalidSize);
        }
        let angle = normalize_degrees(degrees);
        let (rotated_w, rotated_h) = rotated_page_size(page_w, page_h, angle);
        let scale = (width as f32 / rotated_w).min(height as f32 / rotated_h);
        let base_width = (page_w * scale).round().max(1.0) as u32;
        let base_height = (page_h * scale).round().max(1.0) as u32;
        let base = self.render_page_exact(page, base_width, base_height)?;
        let rotated = if angle.abs() < 1e-4 {
            base
        } else {
            rotate_image(&base, angle).map_err(|error| VectorError::Parse(error.to_string()))?
        };
        Ok(center_into(rotated, width, height))
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
    let cache = hayro::RenderCache::new();
    let settings = hayro::hayro_interpret::InterpreterSettings::default();
    let render_settings = hayro::RenderSettings::default();
    let pixmap = hayro::render(
        page,
        &cache,
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
        image = center_into_resized(image, width, height);
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
    let cache = hayro::RenderCache::new();
    let mut ctx = hayro::vello_cpu::RenderContext::new(width as u16, height as u16);
    use hayro::hayro_interpret::util::TransformExt;
    let page_transform = transform.kurbo() * page.initial_transform(true).to_kurbo();
    hayro::render_into(
        page,
        &cache,
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
    let prepared = stet::prepare_display_list(&page.display_list);
    let rgba = stet::render_region_prepared(
        &page.display_list,
        &prepared,
        0.0,
        0.0,
        page.width as f64,
        page.height as f64,
        width,
        height,
        72.0,
        None,
        None,
        false,
    );
    RgbaImage::from_rgba(width, height, rgba).map_err(|error| VectorError::Parse(error.to_string()))
}

fn render_eps_view(
    eps: &EpsBackend,
    page: usize,
    width: u32,
    height: u32,
    transform: ViewTransform,
) -> Result<RgbaImage, VectorError> {
    let page = eps.pages.get(page).ok_or(VectorError::InvalidPage)?;
    // The stet region renderer works on axis-aligned regions. Rotated
    // PostScript pages fall back to the full-page texture path.
    if transform.b.abs() > 1e-4
        || transform.c.abs() > 1e-4
        || transform.a <= 0.0
        || transform.d <= 0.0
    {
        return Err(VectorError::Parse("EPS 旋转视口暂不支持".into()));
    }
    let vp_x = (0.0 - transform.e) / transform.a;
    let vp_y = (0.0 - transform.f) / transform.d;
    let vp_w = width as f32 / transform.a;
    let vp_h = height as f32 / transform.d;
    let prepared = stet::prepare_display_list(&page.display_list);
    let rgba = stet::render_region_prepared(
        &page.display_list,
        &prepared,
        f64::from(vp_x),
        f64::from(vp_y),
        f64::from(vp_w),
        f64::from(vp_h),
        width,
        height,
        72.0,
        None,
        None,
        false,
    );
    RgbaImage::from_rgba(width, height, rgba).map_err(|error| VectorError::Parse(error.to_string()))
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

fn center_into_resized(image: RgbaImage, width: u32, height: u32) -> RgbaImage {
    let scale = (width as f32 / image.width as f32).min(height as f32 / image.height as f32);
    let resized_w = (image.width as f32 * scale).round().max(1.0) as u32;
    let resized_h = (image.height as f32 * scale).round().max(1.0) as u32;
    let resized = resize(&image, resized_w, resized_h, ResizeAlgorithm::Area).unwrap_or(image);
    center_into(resized, width, height)
}

fn normalize_degrees(degrees: f32) -> f32 {
    if !degrees.is_finite() {
        return 0.0;
    }
    let mut angle = degrees % 360.0;
    if angle <= -180.0 {
        angle += 360.0;
    } else if angle > 180.0 {
        angle -= 360.0;
    }
    angle
}

fn rotated_page_size(width: f32, height: f32, degrees: f32) -> (f32, f32) {
    let radians = degrees.to_radians();
    let (sin, cos) = radians.sin_cos();
    (
        width * cos.abs() + height * sin.abs(),
        width * sin.abs() + height * cos.abs(),
    )
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
    fn unsupported_files_are_rejected() {
        assert!(VectorDocument::from_bytes(b"not a vector".to_vec(), "txt").is_err());
    }
}
