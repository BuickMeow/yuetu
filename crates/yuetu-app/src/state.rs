//! Application state that outlives a single frame.

use egui::{TextureFilter, TextureHandle, TextureOptions, TextureWrapMode};
use std::path::PathBuf;
use std::sync::Arc;
use yuetu_core::resize::resize;
use yuetu_core::{
    Color, ConversionConfig, ConversionResult, ResizeAlgorithm, RgbaImage, load_image_sequence,
};

/// Longest side of the texture uploaded to the GPU. The conversion still uses
/// the full-resolution CPU image; this is display-only.
const MAX_TEXTURE_SIDE: u32 = 4096;

pub struct SourceImage {
    pub path: PathBuf,
    pub name: String,
    /// The currently selected frame, shared with the conversion job.
    pub image: Arc<RgbaImage>,
    frames: Vec<Arc<RgbaImage>>,
    frame_index: usize,
    pub texture: TextureHandle,
    /// Dimensions of the (possibly downscaled) texture.
    pub texture_size: (u32, u32),
}

impl SourceImage {
    pub fn load(ctx: &egui::Context, path: PathBuf) -> yuetu_core::Result<Self> {
        let sequence = load_image_sequence(&path)?;
        let frames: Vec<Arc<RgbaImage>> = sequence.frames.into_iter().map(Arc::new).collect();
        let image = frames[0].clone();
        let texture_name = format!("source:{}:0", path.display());
        let (texture, texture_size) = upload_texture(ctx, &texture_name, &image)?;

        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("image")
            .to_owned();

        Ok(Self {
            path,
            name,
            image,
            frames,
            frame_index: 0,
            texture_size,
            texture,
        })
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn frame_index(&self) -> usize {
        self.frame_index
    }

    pub fn select_frame(&mut self, ctx: &egui::Context, index: usize) -> yuetu_core::Result<bool> {
        if self.frames.len() <= 1 {
            return Ok(false);
        }
        let index = index.min(self.frames.len() - 1);
        if index == self.frame_index {
            return Ok(false);
        }

        let image = self.frames[index].clone();
        let texture_name = format!("source:{}:{index}", self.path.display());
        let (texture, texture_size) = upload_texture(ctx, &texture_name, &image)?;
        self.image = image;
        self.frame_index = index;
        self.texture = texture;
        self.texture_size = texture_size;
        Ok(true)
    }

    /// Map a pixel in the display texture back to the full-resolution image.
    pub fn source_pixel(&self, x: u32, y: u32) -> (u32, u32) {
        let sx = (u64::from(x) * u64::from(self.image.width)
            / u64::from(self.texture_size.0.max(1))) as u32;
        let sy = (u64::from(y) * u64::from(self.image.height)
            / u64::from(self.texture_size.1.max(1))) as u32;
        (
            sx.min(self.image.width.saturating_sub(1)),
            sy.min(self.image.height.saturating_sub(1)),
        )
    }
}

fn upload_texture(
    ctx: &egui::Context,
    name: &str,
    image: &RgbaImage,
) -> yuetu_core::Result<(TextureHandle, (u32, u32))> {
    // Never assume 4096: some backends (and egui's defaults) cap at 2048.
    // Loading a larger texture is a debug assertion/panic inside egui.
    let max_texture_side = ctx
        .input(|input| input.max_texture_side)
        .clamp(256, MAX_TEXTURE_SIDE as usize) as u32;
    let display = if image.width.max(image.height) > max_texture_side {
        let scale = max_texture_side as f64 / f64::from(image.width.max(image.height));
        let width = (f64::from(image.width) * scale)
            .round()
            .clamp(1.0, f64::from(max_texture_side)) as u32;
        let height = (f64::from(image.height) * scale)
            .round()
            .clamp(1.0, f64::from(max_texture_side)) as u32;
        resize(image, width, height, ResizeAlgorithm::Area)?
    } else {
        image.clone()
    };

    let color_image = egui::ColorImage::from_rgba_unmultiplied(
        [display.width as usize, display.height as usize],
        display.as_rgba(),
    );
    // Crisp texels when zoomed in, smooth reduction when zoomed out.
    let options = TextureOptions {
        magnification: TextureFilter::Nearest,
        minification: TextureFilter::Linear,
        wrap_mode: TextureWrapMode::ClampToEdge,
        mipmap_mode: None,
    };
    let texture = ctx.load_texture(name, color_image, options);
    Ok((texture, (display.width, display.height)))
}

pub struct Preview {
    pub result: ConversionResult,
    /// The config that produced this preview; used to gate export while an
    /// automatic update is still running.
    pub config: ConversionConfig,
    /// Source generation, so selecting another GIF frame also invalidates it.
    pub source_revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug)]
pub struct StatusMessage {
    pub kind: StatusKind,
    pub text: String,
}

impl StatusMessage {
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            kind: StatusKind::Info,
            text: text.into(),
        }
    }

    pub fn success(text: impl Into<String>) -> Self {
        Self {
            kind: StatusKind::Success,
            text: text.into(),
        }
    }

    pub fn warning(text: impl Into<String>) -> Self {
        Self {
            kind: StatusKind::Warning,
            text: text.into(),
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            kind: StatusKind::Error,
            text: text.into(),
        }
    }
}

impl Default for StatusMessage {
    fn default() -> Self {
        Self::info("就绪")
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SourceHover {
    pub texture_pos: (u32, u32),
    pub source_pos: (u32, u32),
    pub color: Color,
}

#[derive(Clone, Copy, Debug)]
pub struct PreviewHover {
    pub key: u8,
    pub tick: u32,
}
