//! CPU image type and file loading.
//!
//! Pixels are stored as straight RGBA bytes, row-major, exactly like the
//! `image` crate's `RgbaImage`. The conversion pipeline only uses this type;
//! `egui::ColorImage` / GPU textures are a UI concern.

use std::path::Path;

use crate::color::Color;
use crate::error::{Error, Result};

#[derive(Clone, Debug)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, row-major, RGBA.
    pub pixels: Vec<u8>,
}

impl RgbaImage {
    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self> {
        let expected = width as usize * height as usize * 4;
        if width == 0 || height == 0 {
            return Err(Error::InvalidImage(
                "image dimensions must be non-zero".into(),
            ));
        }
        if pixels.len() != expected {
            return Err(Error::InvalidImage(format!(
                "expected {expected} RGBA bytes for {width}x{height}, got {}",
                pixels.len()
            )));
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    pub fn new(width: u32, height: u32, color: Color) -> Self {
        let mut pixels = vec![0; width as usize * height as usize * 4];
        for chunk in pixels.chunks_exact_mut(4) {
            chunk.copy_from_slice(&[color.r, color.g, color.b, color.a]);
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    pub fn pixel(&self, x: u32, y: u32) -> Option<Color> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(self.pixel_unchecked(x, y))
    }

    /// Caller must guarantee `x < width && y < height`.
    pub fn pixel_unchecked(&self, x: u32, y: u32) -> Color {
        let i = ((y as usize * self.width as usize) + x as usize) * 4;
        Color::rgba(
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        )
    }

    pub fn set_pixel(&mut self, x: u32, y: u32, color: Color) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        let i = ((y as usize * self.width as usize) + x as usize) * 4;
        self.pixels[i..i + 4].copy_from_slice(&[color.r, color.g, color.b, color.a]);
        true
    }

    pub fn as_rgba(&self) -> &[u8] {
        &self.pixels
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// Decode the first frame of a raster image into RGBA8.
///
/// Animated GIF/WebP support can be added later by returning a frame list;
/// the rest of the pipeline never needs to know where the frame came from.
pub fn load_image(path: &Path) -> Result<RgbaImage> {
    let reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let decoded = reader.decode()?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    RgbaImage::from_rgba(width, height, rgba.into_raw())
}

/// Decode an in-memory image (used by drag-and-drop / pasted bytes later).
pub fn load_image_from_memory(bytes: &[u8]) -> Result<RgbaImage> {
    let decoded = image::load_from_memory(bytes)?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    RgbaImage::from_rgba(width, height, rgba.into_raw())
}
