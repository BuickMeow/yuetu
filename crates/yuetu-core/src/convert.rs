//! Conversion orchestration.
//!
//! This module is intentionally thin: it validates config, builds the palette
//! and key map, calls `resize` → `quantize` → `generate_notes`, and packages
//! the result. Each stage remains independently testable.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::color::Color;
use crate::error::{Error, Result};
use crate::image::RgbaImage;
use crate::keymap::{ColumnMapping, KeyFilter, KeyMap};
use crate::notegen::{Note, generate_notes_with_progress};
use crate::palette::Palette;
use crate::progress::{Progress, Stage, report};
use crate::quantize::{IndexedImage, quantize_with_progress};
use crate::resize::{ResizeAlgorithm, resize};
use crate::rotate::{normalize_degrees, rotate_image};

/// Which pixels feed the automatic palette.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaletteScope {
    /// Cluster the current frame only.
    CurrentFrame,
    /// Cluster every frame of the animation, weighted by frame delay, so all
    /// frames share one palette and one set of note tracks.
    #[default]
    AllFrames,
}

impl PaletteScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::CurrentFrame => "当前帧",
            Self::AllFrames => "所有帧",
        }
    }
}

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
    pub palette_scope: PaletteScope,
    pub resize: ResizeAlgorithm,
    /// Clockwise source rotation in degrees before resizing.
    pub rotation_deg: f32,
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
            palette_scope: PaletteScope::AllFrames,
            resize: ResizeAlgorithm::Area,
            rotation_deg: 0.0,
            target_height: 0,
            max_note_length: 0,
            measure_from_image_start: false,
        }
    }
}

/// Artifacts that can be reused when only a downstream stage changed.
///
/// The caller is responsible for only filling fields whose upstream keys are
/// unchanged. This keeps the core stateless while still allowing the app to
/// turn the pipeline into a keyed stage cache.
#[derive(Clone, Debug, Default)]
pub struct ConversionSeed {
    pub keymap: Option<KeyMap>,
    pub palette: Option<Palette>,
    pub resized: Option<RgbaImage>,
    pub resized_size: Option<(u32, u32)>,
    pub source_size: Option<(u32, u32)>,
    pub indexed: Option<IndexedImage>,
}

/// Per-stage wall-clock timings. They are diagnostic data, not part of the
/// conversion contract.
#[derive(Clone, Copy, Debug, Default)]
pub struct StageTimings {
    pub palette_ms: f32,
    pub transform_ms: f32,
    pub quantize_ms: f32,
    pub notes_ms: f32,
    pub total_ms: f32,
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
    /// The image after the key-map width resize, kept for pipeline previews.
    pub resized_image: RgbaImage,
    /// The nearest-palette-color index for every resized pixel.
    pub indexed_image: IndexedImage,
    pub note_count_by_track: Vec<usize>,
    /// False when the preview deliberately skipped note generation.
    pub notes_generated: bool,
    pub timings: StageTimings,
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

/// Convert one frame, optionally using a shared palette built from a whole
/// animation sequence.
///
/// `palette_frames` is the complete, *unrotated* sequence. Rotation is a
/// geometric operation and does not change the color distribution, so the
/// shared palette can be computed without rotating every frame. The selected
/// `image` is rotated before resizing.
pub fn convert_frames_with_progress(
    image: &RgbaImage,
    palette_frames: &[&RgbaImage],
    delays_ms: &[u32],
    config: &ConversionConfig,
    progress: Option<&dyn Progress>,
    cancel: &AtomicBool,
) -> Result<ConversionResult> {
    convert_frames_with_palette_and_progress(
        image,
        palette_frames,
        delays_ms,
        None,
        config,
        progress,
        cancel,
    )
}

/// Like [`convert_frames_with_progress`], but can reuse a palette that was
/// already clustered for the same animation sequence.
///
/// When a shared palette is supplied, the palette stage is skipped entirely,
/// which is the expensive part of scrubbing through a long animation.
#[allow(clippy::too_many_arguments)]
pub fn convert_frames_with_palette_and_progress(
    image: &RgbaImage,
    palette_frames: &[&RgbaImage],
    delays_ms: &[u32],
    shared_palette: Option<&Palette>,
    config: &ConversionConfig,
    progress: Option<&dyn Progress>,
    cancel: &AtomicBool,
) -> Result<ConversionResult> {
    convert_seeded_with_progress(
        image,
        palette_frames,
        delays_ms,
        shared_palette,
        None,
        config,
        progress,
        cancel,
    )
}

/// Convert with optional reusable artifacts from upstream stages.
///
/// The caller is responsible for matching the seed against `config` and the
/// source revision. Invalid seeds only affect quality, not memory safety.
#[allow(clippy::too_many_arguments)]
pub fn convert_seeded_with_progress(
    image: &RgbaImage,
    palette_frames: &[&RgbaImage],
    delays_ms: &[u32],
    shared_palette: Option<&Palette>,
    seed: Option<&ConversionSeed>,
    config: &ConversionConfig,
    progress: Option<&dyn Progress>,
    cancel: &AtomicBool,
) -> Result<ConversionResult> {
    convert_seeded_with_options(
        image,
        palette_frames,
        delays_ms,
        shared_palette,
        seed,
        config,
        true,
        progress,
        cancel,
    )
}

/// Like [`convert_seeded_with_progress`], but note generation can be deferred.
#[allow(clippy::too_many_arguments)]
pub fn convert_seeded_with_options(
    image: &RgbaImage,
    palette_frames: &[&RgbaImage],
    delays_ms: &[u32],
    shared_palette: Option<&Palette>,
    seed: Option<&ConversionSeed>,
    config: &ConversionConfig,
    generate_notes: bool,
    progress: Option<&dyn Progress>,
    cancel: &AtomicBool,
) -> Result<ConversionResult> {
    let total_start = Instant::now();
    if image.is_empty() {
        return Err(Error::InvalidImage("源图为空".into()));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }

    let mut timings = StageTimings::default();
    let keymap_start = Instant::now();
    let keymap = if let Some(keymap) = seed.and_then(|seed| seed.keymap.clone()) {
        keymap
    } else {
        KeyMap::build(
            config.key_start,
            config.key_end,
            config.key_filter,
            config.column_mapping,
        )?
    };
    timings.transform_ms += keymap_start.elapsed().as_secs_f32() * 1000.0;

    let can_reuse_resized = seed.is_some_and(|seed| {
        seed.resized.is_some() && seed.source_size.is_some() && seed.resized_size.is_some()
    });
    let palette_available = config.manual_palette.is_some()
        || shared_palette.is_some_and(|palette| !palette.is_empty())
        || seed
            .and_then(|seed| seed.palette.as_ref())
            .is_some_and(|palette| !palette.is_empty());
    let needs_current_source = !palette_available
        && (config.palette_scope == PaletteScope::CurrentFrame || palette_frames.is_empty());
    let needs_rotated_source = !can_reuse_resized || needs_current_source;

    let rotation_start = Instant::now();
    let rotated = if needs_rotated_source && normalize_degrees(config.rotation_deg) != 0.0 {
        Some(rotate_image(image, config.rotation_deg)?)
    } else {
        None
    };
    let source = rotated.as_ref().unwrap_or(image);
    timings.transform_ms += rotation_start.elapsed().as_secs_f32() * 1000.0;

    let palette_start = Instant::now();
    report(progress, Stage::Palette, 0.0);
    let palette = match &config.manual_palette {
        Some(palette) if !palette.is_empty() => palette.clone(),
        Some(_) => return Err(Error::Config("手动调色板不能为空".into())),
        None => {
            if let Some(palette) = shared_palette.filter(|palette| !palette.is_empty()) {
                palette.clone()
            } else if let Some(palette) = seed
                .and_then(|seed| seed.palette.as_ref())
                .filter(|palette| !palette.is_empty())
            {
                palette.clone()
            } else if config.palette_scope == PaletteScope::AllFrames && !palette_frames.is_empty()
            {
                Palette::from_frames_cancellable(
                    palette_frames.iter().copied(),
                    delays_ms,
                    config.palette_colors.max(1),
                    Some(cancel),
                    progress,
                )?
            } else {
                Palette::from_image_cancellable(
                    source,
                    config.palette_colors.max(1),
                    Some(cancel),
                    progress,
                )?
            }
        }
    };
    report(progress, Stage::Palette, 1.0);
    timings.palette_ms = palette_start.elapsed().as_secs_f32() * 1000.0;
    if palette.is_empty() {
        return Err(Error::Config("图像中没有不透明像素，无法生成调色板".into()));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }

    let transform_start = Instant::now();
    let (resized, source_size, resized_size) = if let Some((resized, source_size, resized_size)) =
        seed.and_then(|seed| {
            Some((
                seed.resized.as_ref()?,
                seed.source_size?,
                seed.resized_size?,
            ))
        }) {
        (resized.clone(), source_size, resized_size)
    } else {
        let target_width = keymap.width();
        if target_width == 0 {
            return Err(Error::Config("键映射没有产生任何列".into()));
        }

        let target_height = if config.target_height > 0 {
            config.target_height
        } else {
            ((source.height as f64) * (target_width as f64) / (source.width as f64))
                .round()
                .max(1.0) as u32
        };

        // A pathological aspect ratio (e.g. a 1 x 100000 strip) would
        // otherwise allocate hundreds of millions of pixels. Fail loudly
        // instead of OOM.
        const MAX_CONVERTED_HEIGHT: u32 = 65_535;
        if target_height > MAX_CONVERTED_HEIGHT {
            return Err(Error::Config(format!(
                "缩放后高度 {target_height}px 超过上限 {MAX_CONVERTED_HEIGHT}px；请先裁剪图片"
            )));
        }

        report(progress, Stage::Resize, 0.0);
        let resized = resize(source, target_width, target_height, config.resize)?;
        report(progress, Stage::Resize, 1.0);
        (
            resized,
            (source.width, source.height),
            (target_width, target_height),
        )
    };
    timings.transform_ms += transform_start.elapsed().as_secs_f32() * 1000.0;
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }

    let quantize_start = Instant::now();
    let indexed = if let Some(indexed) = seed.and_then(|seed| seed.indexed.clone()) {
        indexed
    } else {
        quantize_with_progress(&resized, &palette, cancel, progress)?
    };
    timings.quantize_ms = quantize_start.elapsed().as_secs_f32() * 1000.0;

    let notes_start = Instant::now();
    let (notes, note_count_by_track) = if generate_notes {
        generate_notes_with_progress(
            &indexed,
            &keymap,
            palette.len(),
            config.max_note_length,
            config.measure_from_image_start,
            cancel,
            progress,
        )?
    } else {
        (Vec::new(), vec![0usize; palette.len()])
    };
    timings.notes_ms = if generate_notes {
        notes_start.elapsed().as_secs_f32() * 1000.0
    } else {
        0.0
    };
    timings.total_ms = total_start.elapsed().as_secs_f32() * 1000.0;

    Ok(ConversionResult {
        notes,
        palette,
        keymap,
        key_start: config.key_start,
        key_end: config.key_end,
        duration_px: resized_size.1,
        source_size,
        resized_size,
        resized_image: resized,
        indexed_image: indexed,
        note_count_by_track,
        notes_generated: generate_notes,
        timings,
    })
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
    convert_frames_with_progress(image, &[], &[], config, progress, cancel)
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

    #[test]
    fn shared_palette_contains_colors_from_every_frame() {
        let selected = RgbaImage::new(2, 2, Color::rgb(255, 0, 0));
        let other = RgbaImage::new(2, 2, Color::rgb(0, 0, 255));
        let config = ConversionConfig {
            key_start: 60,
            key_end: 60,
            palette_colors: 2,
            palette_scope: PaletteScope::AllFrames,
            ..ConversionConfig::default()
        };
        let cancel = AtomicBool::new(false);

        let result = convert_frames_with_progress(
            &selected,
            &[&selected, &other],
            &[100, 100],
            &config,
            None,
            &cancel,
        )
        .unwrap();

        assert_eq!(result.palette.len(), 2);
        assert!(result.palette.colors.iter().any(|color| color.r > 200));
        assert!(result.palette.colors.iter().any(|color| color.b > 200));
    }

    #[test]
    fn shared_palette_is_identical_for_every_selected_frame() {
        let red = RgbaImage::new(2, 2, Color::rgb(255, 0, 0));
        let blue = RgbaImage::new(2, 2, Color::rgb(0, 0, 255));
        let frames = [&red, &blue];
        let delays = [100, 300];
        let config = ConversionConfig {
            key_start: 60,
            key_end: 60,
            palette_colors: 2,
            palette_scope: PaletteScope::AllFrames,
            ..ConversionConfig::default()
        };
        let cancel = AtomicBool::new(false);

        let first =
            convert_frames_with_progress(&red, &frames, &delays, &config, None, &cancel).unwrap();
        let second =
            convert_frames_with_progress(&blue, &frames, &delays, &config, None, &cancel).unwrap();

        assert_eq!(first.palette, second.palette);
    }

    #[test]
    fn current_frame_scope_ignores_other_frames() {
        let selected = RgbaImage::new(2, 2, Color::rgb(255, 0, 0));
        let other = RgbaImage::new(2, 2, Color::rgb(0, 0, 255));
        let config = ConversionConfig {
            key_start: 60,
            key_end: 60,
            palette_colors: 2,
            palette_scope: PaletteScope::CurrentFrame,
            ..ConversionConfig::default()
        };
        let cancel = AtomicBool::new(false);

        let result = convert_frames_with_progress(
            &selected,
            &[&selected, &other],
            &[100, 100],
            &config,
            None,
            &cancel,
        )
        .unwrap();

        assert_eq!(result.palette.len(), 1);
        assert_eq!(result.palette.colors[0], Color::rgb(255, 0, 0));
    }

    #[test]
    fn seeded_conversion_reuses_upstream_stages() {
        let image = solid_red(8, 8);
        let config = ConversionConfig {
            key_start: 60,
            key_end: 63,
            palette_colors: 1,
            ..ConversionConfig::default()
        };
        let cancel = AtomicBool::new(false);
        let first = convert_with_progress(&image, &config, None, &cancel).unwrap();
        assert!(first.timings.total_ms >= 0.0);

        let seed = ConversionSeed {
            keymap: Some(first.keymap.clone()),
            palette: Some(first.palette.clone()),
            resized: Some(first.resized_image.clone()),
            resized_size: Some(first.resized_size),
            source_size: Some(first.source_size),
            indexed: Some(first.indexed_image.clone()),
        };
        let next_config = ConversionConfig {
            max_note_length: 2,
            ..config
        };
        let second = convert_seeded_with_progress(
            &image,
            &[],
            &[],
            None,
            Some(&seed),
            &next_config,
            None,
            &cancel,
        )
        .unwrap();

        assert_eq!(second.palette, first.palette);
        assert_eq!(second.keymap, first.keymap);
        assert_eq!(second.resized_size, first.resized_size);
        assert_ne!(second.notes.len(), first.notes.len());
    }

    #[test]
    fn notes_can_be_deferred_without_losing_the_indexed_image() {
        let image = solid_red(8, 8);
        let config = ConversionConfig {
            key_start: 60,
            key_end: 63,
            palette_colors: 1,
            ..ConversionConfig::default()
        };
        let cancel = AtomicBool::new(false);
        let result = convert_seeded_with_options(
            &image,
            &[],
            &[],
            None,
            None,
            &config,
            false,
            None,
            &cancel,
        )
        .unwrap();

        assert!(!result.notes_generated);
        assert!(result.notes.is_empty());
        assert_eq!(result.indexed_image.width, result.resized_size.0);
        assert_eq!(result.note_count_by_track.len(), result.palette.len());
    }

    #[test]
    fn supplied_shared_palette_overrides_frame_clustering() {
        let image = RgbaImage::new(2, 2, Color::rgb(255, 0, 0));
        let other = RgbaImage::new(2, 2, Color::rgb(0, 0, 255));
        let palette = Palette::new(vec![Color::rgb(0, 255, 0), Color::rgb(255, 255, 0)]);
        let config = ConversionConfig {
            key_start: 60,
            key_end: 60,
            palette_colors: 2,
            palette_scope: PaletteScope::AllFrames,
            ..ConversionConfig::default()
        };
        let cancel = AtomicBool::new(false);

        let result = convert_frames_with_palette_and_progress(
            &image,
            &[&image, &other],
            &[100, 100],
            Some(&palette),
            &config,
            None,
            &cancel,
        )
        .unwrap();

        assert_eq!(result.palette, palette);
    }

    #[test]
    fn rotation_expands_the_source_before_resizing() {
        let image = RgbaImage::new(4, 2, Color::rgb(255, 0, 0));
        let config = ConversionConfig {
            key_start: 60,
            key_end: 60,
            palette_colors: 1,
            rotation_deg: 90.0,
            ..ConversionConfig::default()
        };
        let cancel = AtomicBool::new(false);

        let result = convert_with_progress(&image, &config, None, &cancel).unwrap();
        assert_eq!(result.source_size, (2, 4));
        assert_eq!(result.resized_size, (1, 2));
    }
}
