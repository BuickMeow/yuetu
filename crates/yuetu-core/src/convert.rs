//! Conversion orchestration.
//!
//! This module is intentionally thin: it validates config, builds the palette
//! and key map, calls `resize` → `quantize` → `generate_notes`, and packages
//! the result. Each stage remains independently testable.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::color::Color;
use crate::error::{Error, Result};
use crate::image::RgbaImage;
use crate::keymap::{ColumnMapping, KeyFilter, KeyMap};
use crate::notegen::{Note, generate_notes_with_progress};
use crate::palette::Palette;
use crate::progress::{Progress, Stage, report};
use crate::quantize::quantize_with_progress;
use crate::resize::{ResizeAlgorithm, resize};

/// Everything that affects the *preview* (not MIDI timing).
#[derive(Clone, Debug, PartialEq)]
pub struct ConversionConfig {
    pub key_start: u8,
    pub key_end: u8,
    pub key_filter: KeyFilter,
    pub column_mapping: ColumnMapping,
    /// Number of colors when the palette is generated automatically.
    pub palette_colors: usize,
    /// Optional manual palette. If present, `palette_colors` is ignored.
    pub manual_palette: Option<Palette>,
    pub resize: ResizeAlgorithm,
    /// `0` = derive height from the source aspect ratio.
    pub target_height: u32,
    /// `0` = notes ring for the whole color run.
    pub max_note_length: u32,
    /// Grid-anchored splitting instead of "flow with color" retriggering.
    pub measure_from_image_start: bool,
}

impl Default for ConversionConfig {
    fn default() -> Self {
        Self {
            key_start: 0,
            key_end: 127,
            key_filter: KeyFilter::All,
            column_mapping: ColumnMapping::Filled,
            palette_colors: 16,
            manual_palette: None,
            resize: ResizeAlgorithm::Area,
            target_height: 0,
            max_note_length: 0,
            measure_from_image_start: false,
        }
    }
}

/// The single snapshot the UI/preview and the MIDI writer consume.
#[derive(Clone, Debug)]
pub struct ConversionResult {
    pub notes: Vec<Note>,
    pub palette: Palette,
    pub keymap: KeyMap,
    pub key_start: u8,
    pub key_end: u8,
    /// Total duration in pixel-ticks (rows of the resized image).
    pub duration_px: u32,
    pub source_size: (u32, u32),
    pub resized_size: (u32, u32),
    pub note_count_by_track: Vec<usize>,
}

impl ConversionResult {
    pub fn note_count(&self) -> usize {
        self.notes.len()
    }

    pub fn track_count(&self) -> usize {
        self.palette.len()
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    /// Palette color for a note track, with a safe fallback.
    pub fn track_color(&self, track: u16) -> Color {
        self.palette
            .colors
            .get(track as usize)
            .copied()
            .unwrap_or(Color::WHITE)
    }
}

pub fn convert(image: &RgbaImage, config: &ConversionConfig) -> Result<ConversionResult> {
    convert_with_progress(image, config, None, &AtomicBool::new(false))
}

/// Convert with optional progress reporting and cooperative cancellation.
///
/// The progress callback is invoked from the calling thread. The app moves
/// this call to a worker thread and forwards reports to the UI via a channel.
pub fn convert_with_progress(
    image: &RgbaImage,
    config: &ConversionConfig,
    progress: Option<&dyn Progress>,
    cancel: &AtomicBool,
) -> Result<ConversionResult> {
    if image.is_empty() {
        return Err(Error::InvalidImage("源图为空".into()));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }

    let keymap = KeyMap::build(
        config.key_start,
        config.key_end,
        config.key_filter,
        config.column_mapping,
    )?;

    report(progress, Stage::Palette, 0.0);
    let palette = match &config.manual_palette {
        Some(palette) if !palette.is_empty() => palette.clone(),
        Some(_) => return Err(Error::Config("手动调色板不能为空".into())),
        None => Palette::from_image_cancellable(
            image,
            config.palette_colors.max(1),
            Some(cancel),
            progress,
        )?,
    };
    report(progress, Stage::Palette, 1.0);
    if palette.is_empty() {
        return Err(Error::Config("图像中没有不透明像素，无法生成调色板".into()));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }

    let target_width = keymap.width();
    if target_width == 0 {
        return Err(Error::Config("键映射没有产生任何列".into()));
    }

    let target_height = if config.target_height > 0 {
        config.target_height
    } else {
        ((image.height as f64) * (target_width as f64) / (image.width as f64))
            .round()
            .max(1.0) as u32
    };

    // A pathological aspect ratio (e.g. a 1 x 100000 strip) would otherwise
    // allocate hundreds of millions of pixels. Fail loudly instead of OOM.
    const MAX_CONVERTED_HEIGHT: u32 = 65_535;
    if target_height > MAX_CONVERTED_HEIGHT {
        return Err(Error::Config(format!(
            "缩放后高度 {target_height}px 超过上限 {MAX_CONVERTED_HEIGHT}px；请先裁剪图片"
        )));
    }

    report(progress, Stage::Resize, 0.0);
    let resized = resize(image, target_width, target_height, config.resize)?;
    report(progress, Stage::Resize, 1.0);
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }

    let indexed = quantize_with_progress(&resized, &palette, cancel, progress)?;
    let (notes, note_count_by_track) = generate_notes_with_progress(
        &indexed,
        &keymap,
        palette.len(),
        config.max_note_length,
        config.measure_from_image_start,
        cancel,
        progress,
    )?;

    Ok(ConversionResult {
        notes,
        palette,
        keymap,
        key_start: config.key_start,
        key_end: config.key_end,
        duration_px: target_height,
        source_size: (image.width, image.height),
        resized_size: (target_width, target_height),
        note_count_by_track,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_red(width: u32, height: u32) -> RgbaImage {
        RgbaImage::new(width, height, Color::rgb(255, 0, 0))
    }

    #[test]
    fn one_column_one_note() {
        let image = solid_red(1, 4);
        let config = ConversionConfig {
            key_start: 60,
            key_end: 60,
            palette_colors: 1,
            manual_palette: Some(Palette::new(vec![Color::rgb(255, 0, 0)])),
            ..ConversionConfig::default()
        };
        let result = convert(&image, &config).unwrap();

        assert_eq!(result.notes.len(), 1);
        assert_eq!(result.notes[0].key, 60);
        assert_eq!(result.notes[0].start, 0);
        assert_eq!(result.notes[0].end, 4);
        assert_eq!(result.resized_size, (1, 4));
        assert_eq!(result.track_count(), 1);
    }

    #[test]
    fn width_is_stretched_to_the_key_range() {
        let image = solid_red(10, 10);
        let config = ConversionConfig {
            key_start: 60,
            key_end: 64,
            palette_colors: 1,
            manual_palette: Some(Palette::new(vec![Color::rgb(255, 0, 0)])),
            ..ConversionConfig::default()
        };
        let result = convert(&image, &config).unwrap();

        assert_eq!(result.resized_size.0, 5);
        assert_eq!(result.resized_size.1, 5);
        assert_eq!(result.notes.len(), 5); // one note per column
    }

    #[test]
    fn white_key_filled_mapping_reduces_width() {
        let image = solid_red(10, 10);
        let config = ConversionConfig {
            key_start: 60,
            key_end: 63,
            key_filter: KeyFilter::White,
            column_mapping: ColumnMapping::Filled,
            palette_colors: 1,
            manual_palette: Some(Palette::new(vec![Color::rgb(255, 0, 0)])),
            ..ConversionConfig::default()
        };
        let result = convert(&image, &config).unwrap();

        assert_eq!(result.resized_size.0, 2);
        assert_eq!(result.notes.len(), 2);
        assert!(
            result
                .notes
                .iter()
                .all(|note| note.key == 60 || note.key == 62)
        );
    }

    #[test]
    fn invalid_range_is_rejected() {
        let image = solid_red(2, 2);
        let config = ConversionConfig {
            key_start: 64,
            key_end: 60,
            ..ConversionConfig::default()
        };
        assert!(convert(&image, &config).is_err());
    }

    #[test]
    fn cancelled_conversion_returns_cancelled() {
        let image = solid_red(2, 2);
        let cancel = AtomicBool::new(true);
        let result = convert_with_progress(&image, &ConversionConfig::default(), None, &cancel);
        assert!(matches!(result, Err(Error::Cancelled)));
    }

    #[test]
    fn progress_reports_every_pipeline_stage() {
        let image = solid_red(8, 8);
        let config = ConversionConfig {
            key_start: 60,
            key_end: 63,
            palette_colors: 1,
            manual_palette: Some(Palette::new(vec![Color::rgb(255, 0, 0)])),
            ..ConversionConfig::default()
        };
        let reports = std::sync::Mutex::new(Vec::new());
        let progress = |stage, fraction| {
            reports.lock().unwrap().push((stage, fraction));
        };
        let cancel = AtomicBool::new(false);

        convert_with_progress(&image, &config, Some(&progress), &cancel).unwrap();

        let reports = reports.into_inner().unwrap();
        for stage in [Stage::Palette, Stage::Resize, Stage::Quantize, Stage::Notes] {
            assert!(
                reports.iter().any(|(reported, _)| *reported == stage),
                "missing progress for {stage:?}"
            );
        }
        assert!(
            reports
                .iter()
                .all(|(_, fraction)| (0.0..=1.0).contains(fraction))
        );
    }

    #[test]
    fn palette_progress_reports_intermediate_fractions() {
        let image = solid_red(512, 512);
        let config = ConversionConfig {
            key_start: 60,
            key_end: 60,
            palette_colors: 1,
            ..ConversionConfig::default()
        };
        let reports = std::sync::Mutex::new(Vec::new());
        let progress = |stage, fraction| {
            reports.lock().unwrap().push((stage, fraction));
        };
        let cancel = AtomicBool::new(false);

        convert_with_progress(&image, &config, Some(&progress), &cancel).unwrap();

        let reports = reports.into_inner().unwrap();
        assert!(reports.iter().any(|(stage, fraction)| {
            *stage == Stage::Palette && *fraction > 0.0 && *fraction < 1.0
        }));
    }
}
