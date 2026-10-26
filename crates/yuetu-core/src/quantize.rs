//! Pixel → palette-index quantization.
//!
//! The output `IndexedImage` is the boundary between preprocessing and the
//! note generator. The generator never sees raw pixels or palette colors; it
//! only sees small integers (`-2` = silence).

use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

use crate::color::Color;
use crate::error::{Error, Result};
use crate::image::RgbaImage;
use crate::palette::Palette;
use crate::progress::{Progress, Stage, report};

/// Sentinel for "this pixel does not sound".
pub const TRANSPARENT_ID: i32 = -2;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndexedImage {
    pub width: u32,
    pub height: u32,
    /// Row-major palette indices, `TRANSPARENT_ID` for silence.
    pub indices: Vec<i32>,
}

impl IndexedImage {
    pub fn at(&self, column: u32, row: u32) -> i32 {
        self.indices[(row as usize * self.width as usize) + column as usize]
    }
}

/// Map every opaque pixel onto its nearest palette entry.
pub fn quantize(image: &RgbaImage, palette: &Palette) -> IndexedImage {
    quantize_with_progress(image, palette, &AtomicBool::new(false), None)
        .expect("quantization without cancellation cannot fail")
}

/// Cancellable, progress-reporting variant used by the worker thread.
pub fn quantize_with_progress(
    image: &RgbaImage,
    palette: &Palette,
    cancel: &AtomicBool,
    progress: Option<&dyn Progress>,
) -> Result<IndexedImage> {
    report(progress, Stage::Quantize, 0.0);

    let mut indices = vec![TRANSPARENT_ID; image.width as usize * image.height as usize];
    if palette.is_empty() {
        report(progress, Stage::Quantize, 1.0);
        return Ok(IndexedImage {
            width: image.width,
            height: image.height,
            indices,
        });
    }

    let width = image.width as usize;
    let height = image.height as usize;
    if width == 0 || height == 0 {
        report(progress, Stage::Quantize, 1.0);
        return Ok(IndexedImage {
            width: image.width,
            height: image.height,
            indices,
        });
    }
    let rows_per_batch = height.div_ceil(rayon::current_num_threads().max(1)).max(1);
    for (batch_index, batch) in indices.chunks_mut(rows_per_batch * width).enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let base_y = batch_index * rows_per_batch;
        batch
            .par_chunks_mut(width)
            .enumerate()
            .for_each(|(row_index, row)| {
                let y = (base_y + row_index) as u32;
                for (x, index) in row.iter_mut().enumerate() {
                    let color: Color = image.pixel_unchecked(x as u32, y);
                    *index = palette.nearest_index(color);
                }
            });
        report(
            progress,
            Stage::Quantize,
            ((base_y + rows_per_batch).min(height)) as f32 / height as f32,
        );
    }

    report(progress, Stage::Quantize, 1.0);
    Ok(IndexedImage {
        width: image.width,
        height: image.height,
        indices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_pixels_become_silence() {
        let image = RgbaImage::new(1, 1, Color::TRANSPARENT);
        let palette = Palette::new(vec![Color::BLACK]);
        let indexed = quantize(&image, &palette);
        assert_eq!(indexed.at(0, 0), TRANSPARENT_ID);
    }

    #[test]
    fn nearest_color_is_selected() {
        let mut image = RgbaImage::new(2, 1, Color::BLACK);
        image.set_pixel(1, 0, Color::rgb(240, 10, 10));
        let palette = Palette::new(vec![Color::BLACK, Color::rgb(255, 0, 0)]);
        let indexed = quantize(&image, &palette);
        assert_eq!(indexed.at(0, 0), 0);
        assert_eq!(indexed.at(1, 0), 1);
    }
}
