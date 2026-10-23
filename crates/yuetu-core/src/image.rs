//! CPU image type and file loading.
//!
//! Pixels are stored as straight RGBA bytes, row-major, exactly like the
//! `image` crate's `RgbaImage`. The conversion pipeline only uses this type;
//! `egui::ColorImage` / GPU textures are a UI concern.

use std::path::Path;

use image::AnimationDecoder;

use crate::color::Color;
use crate::error::{Error, Result};

/// Decoded frames from a still image or an animated GIF/WebP.
#[derive(Clone, Debug)]
pub struct ImageSequence {
    pub frames: Vec<RgbaImage>,
    /// Per-frame delay in milliseconds; the static-image case has one zero.
    pub delays_ms: Vec<u32>,
}

const MAX_ANIMATION_FRAMES: usize = 1024;
const MAX_ANIMATION_BYTES: usize = 512 * 1024 * 1024;

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

/// Decode an image, including every frame of an animated GIF/WebP.
pub fn load_image_sequence(path: &Path) -> Result<ImageSequence> {
    let reader = image::ImageReader::open(path)?.with_guessed_format()?;
    match reader.format() {
        Some(image::ImageFormat::Gif) => {
            let decoder = image::codecs::gif::GifDecoder::new(reader.into_inner())?;
            decode_animation(decoder)
        }
        Some(image::ImageFormat::WebP) => {
            let decoder = image::codecs::webp::WebPDecoder::new(reader.into_inner())?;
            if decoder.has_animation() {
                decode_animation(decoder)
            } else {
                decode_still(path)
            }
        }
        _ => sequence_from_dynamic(reader.decode()?),
    }
}

fn decode_still(path: &Path) -> Result<ImageSequence> {
    let decoded = image::ImageReader::open(path)?
        .with_guessed_format()?
        .decode()?;
    sequence_from_dynamic(decoded)
}

fn sequence_from_dynamic(decoded: image::DynamicImage) -> Result<ImageSequence> {
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(ImageSequence {
        frames: vec![RgbaImage::from_rgba(width, height, rgba.into_raw())?],
        delays_ms: vec![0],
    })
}

fn decode_animation<'a>(decoder: impl AnimationDecoder<'a>) -> Result<ImageSequence> {
    let mut frames = Vec::new();
    let mut delays_ms = Vec::new();
    let mut total_bytes = 0usize;

    for frame in decoder.into_frames() {
        let frame = frame?;
        if frames.len() >= MAX_ANIMATION_FRAMES {
            return Err(Error::InvalidImage(format!(
                "动画帧数超过 {MAX_ANIMATION_FRAMES}"
            )));
        }

        let (numerator, denominator) = frame.delay().numer_denom_ms();
        let delay_ms = if denominator == 0 {
            0
        } else {
            numerator / denominator
        };
        let buffer = frame.into_buffer();
        let (width, height) = buffer.dimensions();
        let image = RgbaImage::from_rgba(width, height, buffer.into_raw())?;
        total_bytes = total_bytes.saturating_add(image.pixels.len());
        if total_bytes > MAX_ANIMATION_BYTES {
            return Err(Error::InvalidImage(format!(
                "动画帧数据超过 {} MiB 上限",
                MAX_ANIMATION_BYTES / (1024 * 1024)
            )));
        }

        frames.push(image);
        delays_ms.push(delay_ms);
    }

    if frames.is_empty() {
        return Err(Error::InvalidImage("动画没有可解码的帧".into()));
    }

    Ok(ImageSequence { frames, delays_ms })
}

/// Decode the first frame of a raster image into RGBA8.
pub fn load_image(path: &Path) -> Result<RgbaImage> {
    let mut sequence = load_image_sequence(path)?;
    Ok(sequence.frames.remove(0))
}

/// Decode an in-memory image (used by drag-and-drop / pasted bytes later).
pub fn load_image_from_memory(bytes: &[u8]) -> Result<RgbaImage> {
    let decoded = image::load_from_memory(bytes)?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    RgbaImage::from_rgba(width, height, rgba.into_raw())
}
