//! Palette generation and nearest-color lookup.
//!
//! M1 deliberately ships one simple, deterministic palette source: a
//! frequency-ranked popularity palette over 5-bit RGB bins. It is not the
//! final clustering story, but it gives real colors and real tracks to the
//! preview without coupling the UI to a clustering experiment.
//!
//! Later palette sources (K-Means, octree, dithering, …) can implement the
//! same `Vec<Color>` output and be dropped in without touching note generation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::color::Color;
use crate::error::{Error, Result};
use crate::image::RgbaImage;
use crate::progress::{Progress, Stage, report};
use crate::quantize::TRANSPARENT_ID;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Palette {
    pub colors: Vec<Color>,
}

impl Palette {
    pub fn new(colors: Vec<Color>) -> Self {
        Self { colors }
    }

    pub fn len(&self) -> usize {
        self.colors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }

    /// Build a popularity palette from the *source* image.
    ///
    /// Colors are grouped into 5-bit-per-channel bins so near-identical
    /// anti-aliased pixels do not each become their own track. The returned
    /// colors are the average of each bin, ordered by pixel count.
    pub fn from_image(image: &RgbaImage, max_colors: usize) -> Self {
        Self::from_image_cancellable(image, max_colors, None, None).unwrap_or_default()
    }

    /// Cancellable variant used by the worker thread.
    ///
    /// Cancellation and progress are checked every 65 536 pixels, which keeps
    /// the scan responsive without adding an atomic load per pixel.
    pub fn from_image_cancellable(
        image: &RgbaImage,
        max_colors: usize,
        cancel: Option<&AtomicBool>,
        progress: Option<&dyn Progress>,
    ) -> Result<Self> {
        let max_colors = max_colors.max(1);
        let mut bins: HashMap<u32, Bin> = HashMap::new();
        let pixel_count = image.pixels.len() / 4;

        report(progress, Stage::Palette, 0.0);
        for (index, px) in image.pixels.chunks_exact(4).enumerate() {
            if index % 65_536 == 0 {
                if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                    return Err(Error::Cancelled);
                }
                if pixel_count > 0 {
                    report(progress, Stage::Palette, index as f32 / pixel_count as f32);
                }
            }

            let color = Color::rgba(px[0], px[1], px[2], px[3]);
            if !color.is_visible() {
                continue;
            }
            let key = (u32::from(color.r >> 3) << 10)
                | (u32::from(color.g >> 3) << 5)
                | u32::from(color.b >> 3);
            let bin = bins.entry(key).or_default();
            bin.count += 1;
            bin.r += u64::from(color.r);
            bin.g += u64::from(color.g);
            bin.b += u64::from(color.b);
        }

        let mut entries: Vec<(u32, Bin)> = bins.into_iter().collect();
        entries
            .sort_by(|(key_a, a), (key_b, b)| b.count.cmp(&a.count).then_with(|| key_a.cmp(key_b)));

        let colors = entries
            .into_iter()
            .take(max_colors)
            .map(|(_, bin)| {
                let count = bin.count.max(1);
                Color::rgb(
                    (bin.r / count) as u8,
                    (bin.g / count) as u8,
                    (bin.b / count) as u8,
                )
            })
            .collect();

        report(progress, Stage::Palette, 1.0);
        Ok(Self { colors })
    }

    /// Index of the nearest palette entry, or `TRANSPARENT_ID`.
    pub fn nearest_index(&self, color: Color) -> i32 {
        if self.colors.is_empty() || !color.is_visible() {
            return TRANSPARENT_ID;
        }

        let mut best = 0usize;
        let mut best_dist = u32::MAX;
        for (index, candidate) in self.colors.iter().enumerate() {
            let dist = color.distance_sq(*candidate);
            if dist < best_dist {
                best_dist = dist;
                best = index;
            }
        }
        best as i32
    }
}

#[derive(Default)]
struct Bin {
    count: u64,
    r: u64,
    g: u64,
    b: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_pixels_are_ignored() {
        let mut image = RgbaImage::new(2, 1, Color::TRANSPARENT);
        image.set_pixel(0, 0, Color::rgb(255, 0, 0));
        let palette = Palette::from_image(&image, 8);
        assert_eq!(palette.len(), 1);
        assert_eq!(palette.colors[0], Color::rgb(255, 0, 0));
    }

    #[test]
    fn palette_is_ordered_by_frequency() {
        let mut image = RgbaImage::new(3, 1, Color::BLACK);
        image.set_pixel(0, 0, Color::rgb(255, 0, 0));
        image.set_pixel(1, 0, Color::rgb(0, 0, 255));
        image.set_pixel(2, 0, Color::rgb(0, 0, 255));
        let palette = Palette::from_image(&image, 2);
        assert_eq!(palette.colors[0], Color::rgb(0, 0, 255));
        assert_eq!(palette.colors[1], Color::rgb(255, 0, 0));
    }
}
