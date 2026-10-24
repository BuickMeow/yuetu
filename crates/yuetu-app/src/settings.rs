//! UI-facing conversion settings.

use yuetu_core::{
    ColumnMapping, ConversionConfig, KeyFilter, MidiOutputConfig, PaletteScope, ResizeAlgorithm,
    normalize_degrees,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub key_start: u8,
    pub key_end: u8,
    pub key_filter: KeyFilter,
    pub column_mapping: ColumnMapping,
    pub palette_colors: usize,
    pub resize: ResizeAlgorithm,
    pub rotation_deg: f32,
    pub target_height: u32,
    pub limit_note_length: bool,
    pub max_note_length: u32,
    pub measure_from_image_start: bool,

    pub ticks_per_pixel: u16,
    pub ppq: u16,
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
            target_height: 0,
            limit_note_length: false,
            max_note_length: 5,
            measure_from_image_start: false,

            ticks_per_pixel: 5,
            ppq: 960,
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
            target_height: self.target_height,
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
            bpm: self.bpm.clamp(1, 1000),
            start_offset: self.start_offset,
            emit_color_events: self.emit_color_events,
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
}
