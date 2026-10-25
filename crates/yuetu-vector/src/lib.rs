//! Vector document loading and rasterization.

use std::path::Path;

use resvg::tiny_skia;
use thiserror::Error;
use yuetu_core::RgbaImage;

#[derive(Debug, Error)]
pub enum VectorError {
    #[error("failed to read vector file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse SVG: {0}")]
    Svg(String),
    #[error("vector page has invalid dimensions")]
    InvalidSize,
}

pub struct VectorDocument {
    format: VectorFormat,
    tree: usvg::Tree,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VectorFormat {
    Svg,
}

impl VectorDocument {
    pub fn load(path: &Path) -> Result<Self, VectorError> {
        let bytes = std::fs::read(path)?;
        if looks_like_svg(&bytes) {
            let mut options = usvg::Options::default();
            options.fontdb_mut().load_system_fonts();
            let tree = usvg::Tree::from_data(&bytes, &options)
                .map_err(|error| VectorError::Svg(error.to_string()))?;
            return Ok(Self {
                format: VectorFormat::Svg,
                tree,
            });
        }
        Err(VectorError::Svg("unsupported vector format".into()))
    }

    pub fn format(&self) -> VectorFormat {
        self.format
    }

    pub fn page_count(&self) -> usize {
        1
    }

    /// Logical page size in SVG user units.
    pub fn page_size(&self) -> (f32, f32) {
        let size = self.tree.size();
        (size.width(), size.height())
    }

    /// Render the page into an exact-aspect raster with the requested long side.
    pub fn render_to_long_side(&self, long_side: u32) -> Result<RgbaImage, VectorError> {
        let (page_w, page_h) = self.page_size();
        if page_w <= 0.0 || page_h <= 0.0 || long_side == 0 {
            return Err(VectorError::InvalidSize);
        }
        let scale = long_side as f32 / page_w.max(page_h);
        let width = (page_w * scale).round().max(1.0) as u32;
        let height = (page_h * scale).round().max(1.0) as u32;
        self.render_to_fit(width, height)
    }

    /// Render the page into an exact-aspect raster with the requested width.
    pub fn render_to_width(&self, width: u32) -> Result<RgbaImage, VectorError> {
        let (page_w, page_h) = self.page_size();
        if page_w <= 0.0 || page_h <= 0.0 || width == 0 {
            return Err(VectorError::InvalidSize);
        }
        let height = ((width as f32 * page_h / page_w).round() as u32).max(1);
        self.render_to_fit(width, height)
    }

    /// Render the page into a fitted RGBA raster.
    pub fn render_to_fit(&self, width: u32, height: u32) -> Result<RgbaImage, VectorError> {
        let (page_w, page_h) = self.page_size();
        if page_w <= 0.0 || page_h <= 0.0 || width == 0 || height == 0 {
            return Err(VectorError::InvalidSize);
        }
        let scale = (width as f32 / page_w).min(height as f32 / page_h);
        let offset_x = (width as f32 - page_w * scale) * 0.5;
        let offset_y = (height as f32 - page_h * scale) * 0.5;
        self.render_with_transform(
            width,
            height,
            tiny_skia::Transform::from_translate(offset_x, offset_y).pre_scale(scale, scale),
        )
    }

    /// Render the page rotated clockwise into a fitted RGBA raster.
    pub fn render_rotated_to_fit(
        &self,
        width: u32,
        height: u32,
        degrees: f32,
    ) -> Result<RgbaImage, VectorError> {
        let (page_w, page_h) = self.page_size();
        if page_w <= 0.0 || page_h <= 0.0 || width == 0 || height == 0 {
            return Err(VectorError::InvalidSize);
        }
        let angle = normalize_degrees(degrees);
        let radians = angle.to_radians();
        let (sin, cos) = radians.sin_cos();
        let rotated_w = page_w * cos.abs() + page_h * sin.abs();
        let rotated_h = page_w * sin.abs() + page_h * cos.abs();
        let scale = (width as f32 / rotated_w).min(height as f32 / rotated_h);
        let offset_x = (width as f32 - rotated_w * scale) * 0.5;
        let offset_y = (height as f32 - rotated_h * scale) * 0.5;
        let center_x = width as f32 * 0.5;
        let center_y = height as f32 * 0.5;
        let transform = tiny_skia::Transform::from_translate(center_x, center_y)
            .pre_scale(scale, scale)
            .pre_rotate(angle)
            .pre_translate(-page_w * 0.5, -page_h * 0.5)
            .post_translate(
                offset_x + rotated_w * scale * 0.5 - center_x,
                offset_y + rotated_h * scale * 0.5 - center_y,
            );
        self.render_with_transform(width, height, transform)
    }

    /// Render using a caller-provided SVG-user-unit to pixel transform.
    pub fn render_with_transform(
        &self,
        width: u32,
        height: u32,
        transform: tiny_skia::Transform,
    ) -> Result<RgbaImage, VectorError> {
        if width == 0 || height == 0 {
            return Err(VectorError::InvalidSize);
        }
        let mut pixmap = tiny_skia::Pixmap::new(width, height).ok_or(VectorError::InvalidSize)?;
        resvg::render(&self.tree, transform, &mut pixmap.as_mut());
        Ok(pixmap_to_rgba(&pixmap))
    }
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

fn looks_like_svg(bytes: &[u8]) -> bool {
    let start = bytes
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .collect::<Vec<_>>();
    start.starts_with(b"<svg") || start.starts_with(b"<?xml")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn red_square() -> VectorDocument {
        let bytes = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="100" height="100" fill="#ff0000"/></svg>"##;
        let tree = usvg::Tree::from_data(bytes, &usvg::Options::default()).unwrap();
        VectorDocument {
            format: VectorFormat::Svg,
            tree,
        }
    }

    #[test]
    fn svg_renders_to_fit_with_straight_rgba() {
        let document = red_square();
        let image = document.render_to_fit(100, 100).unwrap();
        assert_eq!((image.width, image.height), (100, 100));
        assert_eq!(image.pixel(50, 50).unwrap().r, 255);
        assert_eq!(image.pixel(50, 50).unwrap().g, 0);
        assert_eq!(image.pixel(50, 50).unwrap().b, 0);
        assert_eq!(image.pixel(50, 50).unwrap().a, 255);
    }

    #[test]
    fn rotated_render_uses_the_expanded_canvas() {
        let bytes = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="100" height="50" fill="#00ff00"/></svg>"##;
        let tree = usvg::Tree::from_data(bytes, &usvg::Options::default()).unwrap();
        let document = VectorDocument {
            format: VectorFormat::Svg,
            tree,
        };
        let image = document.render_rotated_to_fit(100, 100, 90.0).unwrap();
        assert_eq!(image.pixel(50, 50).unwrap().g, 255);
        assert_eq!(image.pixel(5, 50).unwrap().a, 0);
        assert_eq!(image.pixel(95, 50).unwrap().a, 0);
    }

    #[test]
    fn render_to_fit_preserves_page_aspect() {
        let bytes = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="100" height="50" fill="#00ff00"/></svg>"##;
        let tree = usvg::Tree::from_data(bytes, &usvg::Options::default()).unwrap();
        let document = VectorDocument {
            format: VectorFormat::Svg,
            tree,
        };
        let image = document.render_to_width(200).unwrap();
        assert_eq!((image.width, image.height), (200, 100));
        assert_eq!(image.pixel(100, 10).unwrap().g, 255);
        assert_eq!(image.pixel(100, 10).unwrap().a, 255);
    }
}
