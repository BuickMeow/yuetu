//! Palette generation and nearest-color lookup.
//!
//! The palette is built from a weighted RGB histogram followed by weighted
//! k-means clustering. For animations every frame contributes to the same
//! histogram, weighted by its display duration, so a single palette can be
//! shared by all frames. A static image is simply a one-frame sequence.
//!
//! The palette is intentionally a `Vec<Color>`: later sources (octree, median
//! cut, perceptual clustering) can implement the same output without touching
//! note generation.

use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

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

    /// Build a palette from one image.
    pub fn from_image(image: &RgbaImage, max_colors: usize) -> Self {
        Self::from_image_cancellable(image, max_colors, None, None).unwrap_or_default()
    }

    /// Cancellable variant used by the worker thread.
    pub fn from_image_cancellable(
        image: &RgbaImage,
        max_colors: usize,
        cancel: Option<&AtomicBool>,
        progress: Option<&dyn Progress>,
    ) -> Result<Self> {
        Self::from_frames_cancellable(std::iter::once(image), &[0], max_colors, cancel, progress)
    }

    /// Build one palette shared by a sequence of frames.
    ///
    /// Each frame's pixels are weighted by its delay, so colors that stay on
    /// screen longer matter more. Frames with a zero delay (still images or
    /// malformed GIFs) get weight 1. All colors are clustered together, which
    /// is what makes the same track indices meaningful across frames.
    pub fn from_frames_cancellable<'a>(
        frames: impl IntoIterator<Item = &'a RgbaImage>,
        delays_ms: &[u32],
        max_colors: usize,
        cancel: Option<&AtomicBool>,
        progress: Option<&dyn Progress>,
    ) -> Result<Self> {
        let frames: Vec<&RgbaImage> = frames.into_iter().collect();
        let max_colors = max_colors.max(1);
        if frames.is_empty() {
            report(progress, Stage::Palette, 1.0);
            return Ok(Self::default());
        }

        let total_pixels: u64 = frames
            .iter()
            .map(|frame| frame.pixels.len() as u64 / 4)
            .sum();

        report(progress, Stage::Palette, 0.0);

        // Split every frame into row bands. Bands from all frames are one
        // parallel task pool; each worker keeps a private integer histogram
        // and the merge is exact regardless of task order.
        const BAND_ROWS: usize = 128;
        let mut tasks: Vec<(&RgbaImage, usize, usize, u64)> = Vec::new();
        for (frame_index, frame) in frames.iter().enumerate() {
            let frame_weight = u64::from(delays_ms.get(frame_index).copied().unwrap_or(0).max(1));
            let height = frame.height as usize;
            let mut start = 0;
            while start < height {
                let end = (start + BAND_ROWS).min(height);
                tasks.push((*frame, start, end, frame_weight));
                start = end;
            }
        }

        if total_pixels == 0 {
            report(progress, Stage::Palette, 1.0);
            return Ok(Self::default());
        }

        let batch_size = tasks
            .len()
            .div_ceil(rayon::current_num_threads().max(1))
            .max(1);
        let mut bins = vec![Bin::default(); BIN_COUNT];
        let mut processed = 0u64;
        for batch in tasks.chunks(batch_size) {
            if is_cancelled(cancel) {
                return Err(Error::Cancelled);
            }
            let partial = batch
                .par_iter()
                .try_fold(
                    || vec![Bin::default(); BIN_COUNT],
                    |mut bins, &(frame, start, end, frame_weight)| {
                        if is_cancelled(cancel) {
                            return Err(Error::Cancelled);
                        }
                        let width = frame.width as usize;
                        for y in start..end {
                            let row = &frame.pixels[y * width * 4..(y + 1) * width * 4];
                            for px in row.as_chunks::<4>().0 {
                                let alpha = px[3];
                                if alpha < 128 {
                                    continue;
                                }
                                let r = u16::from(px[0]);
                                let g = u16::from(px[1]);
                                let b = u16::from(px[2]);
                                let key = (u32::from(px[0] >> 3) << 10)
                                    | (u32::from(px[1] >> 3) << 5)
                                    | u32::from(px[2] >> 3);
                                let bin = &mut bins[key as usize];
                                bin.weight += frame_weight;
                                bin.r += u64::from(r) * frame_weight;
                                bin.g += u64::from(g) * frame_weight;
                                bin.b += u64::from(b) * frame_weight;
                            }
                        }
                        Ok(bins)
                    },
                )
                .try_reduce(
                    || vec![Bin::default(); BIN_COUNT],
                    |mut left, right| {
                        for (left, right) in left.iter_mut().zip(right) {
                            left.weight += right.weight;
                            left.r += right.r;
                            left.g += right.g;
                            left.b += right.b;
                        }
                        Ok(left)
                    },
                )?;
            for (left, right) in bins.iter_mut().zip(partial) {
                left.weight += right.weight;
                left.r += right.r;
                left.g += right.g;
                left.b += right.b;
            }
            for &(frame, start, end, _) in batch {
                processed += (end - start) as u64 * u64::from(frame.width);
            }
            report(
                progress,
                Stage::Palette,
                (processed as f32 / total_pixels as f32).min(1.0),
            );
        }

        let mut entries: Vec<WeightedColor> = bins
            .into_iter()
            .enumerate()
            .filter(|(_, bin)| bin.weight > 0)
            .map(|(key, bin)| {
                let weight = bin.weight as f64;
                WeightedColor {
                    key: key as u32,
                    weight,
                    rgb: [
                        bin.r as f64 / weight,
                        bin.g as f64 / weight,
                        bin.b as f64 / weight,
                    ],
                }
            })
            .collect();

        // Stable order: most common bins first, with the bin key as a
        // deterministic tie-breaker.
        entries.sort_by(|a, b| {
            b.weight
                .total_cmp(&a.weight)
                .then_with(|| a.key.cmp(&b.key))
        });

        if entries.is_empty() {
            report(progress, Stage::Palette, 1.0);
            return Ok(Self::default());
        }

        if entries.len() <= max_colors {
            let colors = entries.iter().map(|entry| entry.to_color()).collect();
            report(progress, Stage::Palette, 1.0);
            return Ok(Self { colors });
        }

        let colors = weighted_kmeans(&entries, max_colors, cancel)?;
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

const BIN_BITS: u32 = 5;
const BIN_COUNT: usize = 1 << (BIN_BITS * 3);
const KMEANS_ITERATIONS: usize = 24;
const KMEANS_EPSILON: f64 = 0.05;

#[derive(Clone, Copy, Default)]
struct Bin {
    weight: u64,
    r: u64,
    g: u64,
    b: u64,
}

#[derive(Clone, Copy)]
struct WeightedColor {
    key: u32,
    weight: f64,
    rgb: [f64; 3],
}

impl WeightedColor {
    fn to_color(self) -> Color {
        Color::rgb(
            self.rgb[0].round().clamp(0.0, 255.0) as u8,
            self.rgb[1].round().clamp(0.0, 255.0) as u8,
            self.rgb[2].round().clamp(0.0, 255.0) as u8,
        )
    }
}

fn weighted_kmeans(
    entries: &[WeightedColor],
    max_colors: usize,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<Color>> {
    let mut centers = initial_centers(entries, max_colors, cancel)?;
    if centers.is_empty() {
        return Ok(Vec::new());
    }

    for _ in 0..KMEANS_ITERATIONS {
        if is_cancelled(cancel) {
            return Err(Error::Cancelled);
        }

        let mut sums = vec![[0.0f64; 3]; centers.len()];
        let mut weights = vec![0.0f64; centers.len()];
        let assignments: Vec<usize> = entries
            .par_iter()
            .map(|entry| nearest_center(entry.rgb, &centers))
            .collect();
        for (entry, &index) in entries.iter().zip(&assignments) {
            weights[index] += entry.weight;
            for (sum, value) in sums[index].iter_mut().zip(entry.rgb) {
                *sum += value * entry.weight;
            }
        }

        let mut max_movement = 0.0f64;
        for (index, center) in centers.iter_mut().enumerate() {
            if weights[index] <= 0.0 {
                continue;
            }
            let next = [
                sums[index][0] / weights[index],
                sums[index][1] / weights[index],
                sums[index][2] / weights[index],
            ];
            let movement = distance_sq(next, *center).sqrt();
            max_movement = max_movement.max(movement);
            *center = next;
        }

        if max_movement <= KMEANS_EPSILON {
            break;
        }
    }

    // Final cluster weights decide the track order. This keeps the most
    // frequently sounding colors at the front, like the old popularity
    // palette, while the colors themselves are cluster centroids.
    let mut cluster_weights = vec![0.0f64; centers.len()];
    let assignments: Vec<usize> = entries
        .par_iter()
        .map(|entry| nearest_center(entry.rgb, &centers))
        .collect();
    for (entry, &index) in entries.iter().zip(&assignments) {
        cluster_weights[index] += entry.weight;
    }

    let mut order: Vec<usize> = (0..centers.len()).collect();
    order.sort_by(|&a, &b| {
        cluster_weights[b]
            .total_cmp(&cluster_weights[a])
            .then_with(|| a.cmp(&b))
    });

    let mut colors = Vec::with_capacity(centers.len());
    for index in order {
        let color = Color::rgb(
            centers[index][0].round().clamp(0.0, 255.0) as u8,
            centers[index][1].round().clamp(0.0, 255.0) as u8,
            centers[index][2].round().clamp(0.0, 255.0) as u8,
        );
        if !colors.contains(&color) {
            colors.push(color);
        }
    }
    Ok(colors)
}

fn initial_centers(
    entries: &[WeightedColor],
    max_colors: usize,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<[f64; 3]>> {
    let mut centers = Vec::with_capacity(max_colors);
    let Some(first) = entries.first() else {
        return Ok(centers);
    };
    centers.push(first.rgb);

    // Weighted k-means++ initialization: the next center is the bin with the
    // largest "distance × weight", which favors both distant and common
    // colors. Selection is deterministic because entries are already sorted.
    while centers.len() < max_colors {
        if is_cancelled(cancel) {
            return Err(Error::Cancelled);
        }
        let best = entries
            .par_iter()
            .enumerate()
            .map(|(index, entry)| {
                let nearest = centers
                    .iter()
                    .map(|center| distance_sq(entry.rgb, *center))
                    .fold(f64::INFINITY, f64::min);
                (index, nearest * entry.weight)
            })
            .reduce(
                || (usize::MAX, 0.0),
                |a, b| {
                    if b.1 > a.1 || (b.1 == a.1 && b.0 < a.0) {
                        b
                    } else {
                        a
                    }
                },
            );
        let (index, best_score) = best;
        if index == usize::MAX || best_score <= f64::EPSILON {
            break;
        }
        centers.push(entries[index].rgb);
    }

    Ok(centers)
}

fn nearest_center(color: [f64; 3], centers: &[[f64; 3]]) -> usize {
    let mut best = 0usize;
    let mut best_distance = f64::INFINITY;
    for (index, center) in centers.iter().enumerate() {
        let distance = distance_sq(color, *center);
        if distance < best_distance {
            best_distance = distance;
            best = index;
        }
    }
    best
}

fn distance_sq(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dr = a[0] - b[0];
    let dg = a[1] - b[1];
    let db = a[2] - b[2];
    dr * dr + dg * dg + db * db
}

fn is_cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|flag| flag.load(Ordering::Relaxed))
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
    fn palette_is_ordered_by_weight() {
        let mut image = RgbaImage::new(3, 1, Color::BLACK);
        image.set_pixel(0, 0, Color::rgb(255, 0, 0));
        image.set_pixel(1, 0, Color::rgb(0, 0, 255));
        image.set_pixel(2, 0, Color::rgb(0, 0, 255));
        let palette = Palette::from_image(&image, 2);
        assert_eq!(palette.len(), 2);
        // Blue is the heavier cluster; the exact centroid can include the
        // black pixel because k-means minimizes error rather than preserving
        // popularity colors verbatim.
        assert!(palette.colors[0].b > palette.colors[0].r);
        assert!(palette.colors[1].r > palette.colors[1].b);
    }

    #[test]
    fn all_frames_share_one_palette() {
        let red = RgbaImage::new(2, 2, Color::rgb(255, 0, 0));
        let blue = RgbaImage::new(2, 2, Color::rgb(0, 0, 255));
        let palette =
            Palette::from_frames_cancellable([&red, &blue], &[100, 100], 2, None, None).unwrap();

        assert_eq!(palette.len(), 2);
        assert!(
            palette
                .colors
                .iter()
                .any(|color| color.r > 200 && color.b == 0)
        );
        assert!(
            palette
                .colors
                .iter()
                .any(|color| color.b > 200 && color.r == 0)
        );
    }

    #[test]
    fn frame_delay_weights_palette_colors() {
        let red = RgbaImage::new(1, 1, Color::rgb(255, 0, 0));
        let blue = RgbaImage::new(1, 1, Color::rgb(0, 0, 255));
        let palette =
            Palette::from_frames_cancellable([&red, &blue], &[1000, 10], 1, None, None).unwrap();

        assert_eq!(palette.len(), 1);
        assert!(palette.colors[0].r > palette.colors[0].b);
    }
}
