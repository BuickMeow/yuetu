//! UI-facing conversion settings.

use yuetu_core::{
    ColumnMapping, ConversionConfig, KeyFilter, KeyMap, MidiOutputConfig, PaletteScope,
    ResizeAlgorithm, TimeAxis, TimeLength, TimeSignature, normalize_degrees, resolve_time_rows,
};

/// How the time-axis length is chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LengthMode {
    /// Preserve one source time pixel per output row.
    #[default]
    Source,
    /// Scale both axes equally, keeping square pixels.
    Square,
    /// Use `fixed_rows`.
    Fixed,
}

impl LengthMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Source => "原图长度",
            Self::Square => "正方形像素",
            Self::Fixed => "固定高度",
        }
    }

    fn time_length(self, fixed_rows: u32) -> TimeLength {
        match self {
            Self::Source => TimeLength::Source,
            Self::Square => TimeLength::Square,
            Self::Fixed => TimeLength::Fixed(fixed_rows.max(1)),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub key_start: u8,
    pub key_end: u8,
    pub key_filter: KeyFilter,
    pub column_mapping: ColumnMapping,
    pub palette_colors: usize,
    pub resize: ResizeAlgorithm,
    pub rotation_deg: f32,
    pub time_axis: TimeAxis,
    pub length_mode: LengthMode,
    pub fixed_rows: u32,
    pub limit_note_length: bool,
    pub max_note_length: u32,
    pub measure_from_image_start: bool,

    pub ticks_per_pixel: u16,
    pub ppq: u16,
    pub time_signature: TimeSignature,
    pub bpm: u16,
    pub start_offset: u32,
    pub emit_color_events: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            key_start: 0,
            key_end: 127,
            key_filter: KeyFilter::All,
            column_mapping: ColumnMapping::Filled,
            palette_colors: 16,
            resize: ResizeAlgorithm::Area,
            rotation_deg: 0.0,
            time_axis: TimeAxis::Vertical,
            length_mode: LengthMode::Source,
            fixed_rows: 800,
            limit_note_length: false,
            max_note_length: 5,
            measure_from_image_start: false,

            ticks_per_pixel: 5,
            ppq: 960,
            time_signature: TimeSignature::default(),
            bpm: 120,
            start_offset: 0,
            emit_color_events: true,
        }
    }
}

impl Settings {
    pub fn conversion_config(&self) -> ConversionConfig {
        ConversionConfig {
            key_start: self.key_start.min(self.key_end),
            key_end: self.key_end.max(self.key_start),
            key_filter: self.key_filter,
            column_mapping: self.column_mapping,
            palette_colors: self.palette_colors.max(1),
            manual_palette: None,
            palette_scope: PaletteScope::AllFrames,
            resize: self.resize,
            rotation_deg: normalize_degrees(self.rotation_deg),
            time_axis: self.time_axis,
            time_length: self.length_mode.time_length(self.fixed_rows),
            max_note_length: if self.limit_note_length {
                self.max_note_length.max(1)
            } else {
                0
            },
            measure_from_image_start: self.measure_from_image_start,
        }
    }

    pub fn midi_config(&self) -> MidiOutputConfig {
        MidiOutputConfig {
            ticks_per_pixel: self.ticks_per_pixel.max(1),
            ppq: self.ppq.clamp(1, 0x7FFF),
            time_signature: self.valid_time_signature(),
            bpm: self.bpm.clamp(1, 1000),
            start_offset: self.start_offset,
            emit_color_events: self.emit_color_events,
        }
    }

    /// Number of time rows that the current settings resolve to for a rotated
    /// source canvas. Used for the live output readout in the controls panel.
    pub fn time_rows(&self, source_size: (u32, u32)) -> u32 {
        let Ok(keymap) = KeyMap::build(
            self.key_start.min(self.key_end),
            self.key_end.max(self.key_start),
            self.key_filter,
            self.column_mapping,
        ) else {
            return 1;
        };
        resolve_time_rows(
            source_size.0,
            source_size.1,
            keymap.width(),
            self.time_axis,
            self.length_mode.time_length(self.fixed_rows),
        )
    }

    pub fn total_ticks(&self, source_size: (u32, u32)) -> u64 {
        u64::from(self.time_rows(source_size))
            .saturating_mul(u64::from(self.ticks_per_pixel.max(1)))
    }

    pub fn bar_count(&self, source_size: (u32, u32)) -> f64 {
        let bar_ticks = self
            .valid_time_signature()
            .bar_ticks(self.ppq.clamp(1, 0x7FFF));
        if bar_ticks <= 0.0 {
            return 0.0;
        }
        self.total_ticks(source_size) as f64 / bar_ticks
    }

    pub fn valid_time_signature(&self) -> TimeSignature {
        let numerator = self.time_signature.numerator.clamp(1, 32);
        let denominator = if matches!(self.time_signature.denominator, 1 | 2 | 4 | 8 | 16 | 32) {
            self.time_signature.denominator
        } else {
            4
        };
        TimeSignature {
            numerator,
            denominator,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.key_start > self.key_end {
            return Err("起始键不能大于结束键".into());
        }
        if self.key_end > 127 {
            return Err("MIDI 键号不能超过 127".into());
        }
        if self.palette_colors == 0 {
            return Err("调色板颜色数至少为 1".into());
        }
        if self.ppq == 0 || self.ticks_per_pixel == 0 {
            return Err("PPQ 与 Ticks per Pixel 必须大于 0".into());
        }
        if self.length_mode == LengthMode::Fixed && !(1..=65_535).contains(&self.fixed_rows) {
            return Err("固定高度必须在 1 到 65535 行之间".into());
        }
        if self.time_signature.numerator == 0 {
            return Err("拍号分子必须大于 0".into());
        }
        if !matches!(self.time_signature.denominator, 1 | 2 | 4 | 8 | 16 | 32) {
            return Err("拍号分母必须是 1、2、4、8、16 或 32".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_wraps_when_converted() {
        let settings = Settings {
            rotation_deg: 360.0,
            ..Settings::default()
        };
        assert_eq!(settings.conversion_config().rotation_deg, 0.0);

        let settings = Settings {
            rotation_deg: 725.0,
            ..Settings::default()
        };
        assert_eq!(settings.conversion_config().rotation_deg, 5.0);
    }

    #[test]
    fn defaults_use_source_length_and_four_four() {
        let settings = Settings::default();
        assert_eq!(settings.length_mode, LengthMode::Source);
        assert_eq!(settings.conversion_config().time_length, TimeLength::Source);
        assert_eq!(settings.valid_time_signature(), TimeSignature::default());
    }

    #[test]
    fn fixed_height_is_independent_of_ppq() {
        let settings = Settings {
            length_mode: LengthMode::Fixed,
            fixed_rows: 1001,
            ppq: 960,
            ticks_per_pixel: 5,
            ..Settings::default()
        };
        assert_eq!(
            settings.conversion_config().time_length,
            TimeLength::Fixed(1001)
        );
        assert_eq!(settings.time_rows((800, 800)), 1001);
        assert_eq!(settings.total_ticks((800, 800)), 5005);
        assert_eq!(settings.midi_config().ticks_per_pixel, 5);
    }

    #[test]
    fn ppq_and_ticks_change_bars_but_not_image_rows() {
        let settings = Settings {
            length_mode: LengthMode::Fixed,
            fixed_rows: 1001,
            ppq: 960,
            ticks_per_pixel: 5,
            ..Settings::default()
        };
        assert_eq!(settings.time_rows((800, 800)), 1001);
        assert_eq!(settings.total_ticks((800, 800)), 5005);
        let bars = settings.bar_count((800, 800));
        assert!((bars - 1.303_385).abs() < 1e-5);

        let slower_ppq = Settings {
            ppq: 480,
            ..settings.clone()
        };
        assert_eq!(slower_ppq.time_rows((800, 800)), 1001);
        assert_eq!(slower_ppq.total_ticks((800, 800)), 5005);
        assert!(slower_ppq.bar_count((800, 800)) > bars);

        let more_ticks = Settings {
            ticks_per_pixel: 10,
            ..settings
        };
        assert_eq!(more_ticks.time_rows((800, 800)), 1001);
        assert_eq!(more_ticks.total_ticks((800, 800)), 10_010);
        assert!(more_ticks.bar_count((800, 800)) > bars);
    }

    #[test]
    fn time_axis_controls_which_source_extent_is_length() {
        let source = (1200, 800);
        let vertical = Settings {
            key_start: 0,
            key_end: 127,
            ..Settings::default()
        };
        assert_eq!(vertical.time_rows(source), 800);

        let horizontal = Settings {
            time_axis: TimeAxis::Horizontal,
            ..vertical.clone()
        };
        assert_eq!(horizontal.time_rows(source), 1200);

        let vertical_square = Settings {
            length_mode: LengthMode::Square,
            ..vertical
        };
        assert_eq!(vertical_square.time_rows(source), 85);

        let horizontal_square = Settings {
            length_mode: LengthMode::Square,
            ..horizontal
        };
        assert_eq!(horizontal_square.time_rows(source), 192);
    }
}
