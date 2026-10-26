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
use crate::layout::{TimeAxis, TimeLength, resolve_time_rows};
use crate::notegen::{Note, generate_notes_with_progress_on_grid};
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
    /// Orientation of the time axis. The key axis is the other axis.
    pub time_axis: TimeAxis,
    /// How the number of time rows is chosen.
    pub time_length: TimeLength,
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
            time_axis: TimeAxis::Vertical,
            time_length: TimeLength::Source,
            max_note_length: 0,
            measure_from_image_start: false,
        }
    }
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
    /// Time-axis orientation used to build `resized_image` and the notes.
    pub time_axis: TimeAxis,
    /// Total duration in time rows.
    pub duration_px: u32,
    /// Number of time rows.
    pub time_rows: u32,
    /// Rotated source canvas dimensions.
    pub source_size: (u32, u32),
    /// Oriented resized image dimensions. For horizontal time the first value
    /// is the time extent and the second is the key count.
    pub resized_size: (u32, u32),
    /// The image after the key/time-axis resize, kept for pipeline previews.
    /// Its orientation follows `time_axis`.
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
    convert_with_options(
        image,
        palette_frames,
        delays_ms,
        config,
        true,
        progress,
        cancel,
    )
}

/// Convert one frame, optionally using all frames of an animation sequence
/// for the palette.
///
/// `palette_frames` is the complete, *unrotated* sequence. Rotation is a
/// geometric operation and does not change the color distribution, so the
/// palette can be computed without rotating every frame. The selected
/// `image` is rotated before resizing.
#[allow(clippy::too_many_arguments)]
pub fn convert_with_options(
    image: &RgbaImage,
    palette_frames: &[&RgbaImage],
    delays_ms: &[u32],
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
    let keymap = KeyMap::build(
        config.key_start,
        config.key_end,
        config.key_filter,
        config.column_mapping,
    )?;
    timings.transform_ms += keymap_start.elapsed().as_secs_f32() * 1000.0;

    let rotation_start = Instant::now();
    let rotated = if normalize_degrees(config.rotation_deg) != 0.0 {
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
            if config.palette_scope == PaletteScope::AllFrames && !palette_frames.is_empty() {
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
    let key_count = keymap.width();
    if key_count == 0 {
        return Err(Error::Config("键映射没有产生任何列".into()));
    }

    let time_rows = resolve_time_rows(
        source.width,
        source.height,
        key_count,
        config.time_axis,
        config.time_length,
    );
    let (target_width, target_height) = config.time_axis.grid_size(key_count, time_rows);

    // A pathological custom height would otherwise allocate hundreds of
    // millions of pixels. Fail loudly instead of OOM.
    const MAX_CONVERTED_SIDE: u32 = 65_535;
    if target_width > MAX_CONVERTED_SIDE || target_height > MAX_CONVERTED_SIDE {
        return Err(Error::Config(format!(
            "缩放后尺寸 {target_width}×{target_height}px 超过单边上限 {MAX_CONVERTED_SIDE}px；请先裁剪图片或减小时间行数"
        )));
    }

    report(progress, Stage::Resize, 0.0);
    let resized = resize(source, target_width, target_height, config.resize)?;
    report(progress, Stage::Resize, 1.0);
    let source_size = (source.width, source.height);
    let resized_size = (target_width, target_height);
    let time_rows = match config.time_axis {
        TimeAxis::Vertical => resized_size.1,
        TimeAxis::Horizontal => resized_size.0,
    };
    timings.transform_ms += transform_start.elapsed().as_secs_f32() * 1000.0;
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }

    let quantize_start = Instant::now();
    let indexed = quantize_with_progress(&resized, &palette, cancel, progress)?;
    timings.quantize_ms = quantize_start.elapsed().as_secs_f32() * 1000.0;

    let notes_start = Instant::now();
    let (notes, note_count_by_track) = if generate_notes {
        generate_notes_with_progress_on_grid(
            &indexed,
            &keymap,
            palette.len(),
            config.max_note_length,
            config.measure_from_image_start,
            config.time_axis,
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
        time_axis: config.time_axis,
        duration_px: time_rows,
        time_rows,
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
            time_length: TimeLength::Square,
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
    fn notes_can_be_deferred_without_losing_the_indexed_image() {
        let image = solid_red(8, 8);
        let config = ConversionConfig {
            key_start: 60,
            key_end: 63,
            palette_colors: 1,
            ..ConversionConfig::default()
        };
        let cancel = AtomicBool::new(false);
        let result = convert_with_options(&image, &[], &[], &config, false, None, &cancel).unwrap();

        assert!(!result.notes_generated);
        assert!(result.notes.is_empty());
        assert_eq!(result.indexed_image.width, result.resized_size.0);
        assert_eq!(result.note_count_by_track.len(), result.palette.len());
    }

    #[test]
    fn source_length_preserves_time_pixels() {
        let image = solid_red(10, 10);
        let config = ConversionConfig {
            key_start: 60,
            key_end: 64,
            palette_colors: 1,
            manual_palette: Some(Palette::new(vec![Color::rgb(255, 0, 0)])),
            ..ConversionConfig::default()
        };
        let result = convert(&image, &config).unwrap();

        assert_eq!(result.time_rows, 10);
        assert_eq!(result.resized_size, (5, 10));
    }

    #[test]
    fn fixed_height_is_exact() {
        let image = solid_red(10, 10);
        let config = ConversionConfig {
            key_start: 60,
            key_end: 61,
            palette_colors: 1,
            time_length: TimeLength::Fixed(7),
            manual_palette: Some(Palette::new(vec![Color::rgb(255, 0, 0)])),
            ..ConversionConfig::default()
        };
        let result = convert(&image, &config).unwrap();

        assert_eq!(result.time_rows, 7);
        assert_eq!(result.resized_size, (2, 7));
    }

    #[test]
    fn horizontal_time_maps_rows_to_keys_bottom_up() {
        let mut image = RgbaImage::new(3, 2, Color::rgb(0, 0, 255));
        for x in 0..3 {
            image.set_pixel(x, 0, Color::rgb(255, 0, 0));
        }
        let config = ConversionConfig {
            key_start: 60,
            key_end: 61,
            time_axis: TimeAxis::Horizontal,
            palette_colors: 2,
            manual_palette: Some(Palette::new(vec![
                Color::rgb(255, 0, 0),
                Color::rgb(0, 0, 255),
            ])),
            ..ConversionConfig::default()
        };
        let result = convert(&image, &config).unwrap();

        assert_eq!(result.time_rows, 3);
        assert_eq!(result.resized_size, (3, 2));
        assert_eq!(result.notes.len(), 2);
        let bottom = result.notes.iter().find(|note| note.key == 60).unwrap();
        let top = result.notes.iter().find(|note| note.key == 61).unwrap();
        assert_eq!((bottom.track, bottom.start, bottom.end), (1, 0, 3));
        assert_eq!((top.track, top.start, top.end), (0, 0, 3));
    }

    #[test]
    fn rotation_and_time_axis_choose_the_rotated_time_extent() {
        let image = RgbaImage::new(4, 2, Color::rgb(255, 0, 0));
        let base = ConversionConfig {
            key_start: 60,
            key_end: 60,
            palette_colors: 1,
            rotation_deg: 90.0,
            manual_palette: Some(Palette::new(vec![Color::rgb(255, 0, 0)])),
            ..ConversionConfig::default()
        };
        let cancel = AtomicBool::new(false);

        let vertical = convert_with_progress(&image, &base, None, &cancel).unwrap();
        assert_eq!(vertical.source_size, (2, 4));
        assert_eq!(vertical.time_axis, TimeAxis::Vertical);
        assert_eq!(vertical.time_rows, 4);
        assert_eq!(vertical.resized_size, (1, 4));

        let horizontal = convert_with_progress(
            &image,
            &ConversionConfig {
                time_axis: TimeAxis::Horizontal,
                ..base
            },
            None,
            &cancel,
        )
        .unwrap();
        assert_eq!(horizontal.source_size, (2, 4));
        assert_eq!(horizontal.time_axis, TimeAxis::Horizontal);
        assert_eq!(horizontal.time_rows, 2);
        assert_eq!(horizontal.resized_size, (2, 1));
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
        assert_eq!(result.time_rows, 4);
        assert_eq!(result.resized_size, (1, 4));
    }
}
