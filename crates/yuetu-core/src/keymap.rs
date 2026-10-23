//! Column → MIDI key mapping.
//!
//! The original tool has a surprisingly large number of key modes. They are
//! really two orthogonal decisions:
//!
//! * **which keys may sound** (`KeyFilter`), and
//! * **how image columns land on keys** (`ColumnMapping`).
//!
//! `KeyMap` flattens both into one validated lookup table, so the note
//! generator only needs `key_at(column)`.

use crate::error::{Error, Result};

pub const MAX_MIDI_KEY: u8 = 127;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyFilter {
    /// Every chromatic key in the range.
    #[default]
    All,
    /// Only white piano keys (C D E F G A B).
    White,
    /// Only black piano keys (C# D# F# G# A#).
    Black,
}

impl KeyFilter {
    pub fn allows(self, key: u8) -> bool {
        match self {
            Self::All => true,
            Self::White => is_white_key(key),
            Self::Black => !is_white_key(key),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "全部键",
            Self::White => "仅白键",
            Self::Black => "仅黑键",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColumnMapping {
    /// Image width is stretched to the number of *usable* keys.
    #[default]
    Filled,
    /// Image width equals the full key range; filtered-out columns are silent.
    Clipped,
}

impl ColumnMapping {
    pub fn label(self) -> &'static str {
        match self {
            Self::Filled => "拉伸到可用键",
            Self::Clipped => "按完整键范围裁剪",
        }
    }
}

/// A precomputed column → key table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyMap {
    columns: Vec<Option<u8>>,
}

impl KeyMap {
    pub fn build(
        start_key: u8,
        end_key: u8,
        filter: KeyFilter,
        mapping: ColumnMapping,
    ) -> Result<Self> {
        if start_key > end_key {
            return Err(Error::Config(format!(
                "起始键 {start_key} 不能大于结束键 {end_key}"
            )));
        }
        if end_key > MAX_MIDI_KEY {
            return Err(Error::Config(format!(
                "结束键 {end_key} 超过 MIDI 最大值 {MAX_MIDI_KEY}"
            )));
        }

        let range = start_key..=end_key;
        let columns: Vec<Option<u8>> = match mapping {
            ColumnMapping::Filled => range.filter(|key| filter.allows(*key)).map(Some).collect(),
            ColumnMapping::Clipped => range.map(|key| filter.allows(key).then_some(key)).collect(),
        };

        if columns.iter().all(Option::is_none) {
            return Err(Error::Config("当前键范围与过滤器没有可用键".into()));
        }

        Ok(Self { columns })
    }

    /// Number of image columns after mapping (the width the image is resized to).
    pub fn width(&self) -> u32 {
        self.columns.len() as u32
    }

    /// MIDI key for an image column, or `None` for a filtered-out column.
    pub fn key_at(&self, column: u32) -> Option<u8> {
        self.columns.get(column as usize).copied().flatten()
    }

    /// Number of columns that actually produce sound.
    pub fn sounding_width(&self) -> u32 {
        self.columns.iter().filter(|key| key.is_some()).count() as u32
    }
}

/// C D E F G A B are white keys; everything else is black.
pub fn is_white_key(key: u8) -> bool {
    matches!(key % 12, 0 | 2 | 4 | 5 | 7 | 9 | 11)
}

/// Human-readable note name such as `C4` or `F#3`.
pub fn note_name(key: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let octave = i32::from(key) / 12 - 1;
    format!("{}{}", NAMES[(key % 12) as usize], octave)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chromatic_range_maps_one_to_one() {
        let map = KeyMap::build(60, 63, KeyFilter::All, ColumnMapping::Filled).unwrap();
        assert_eq!(map.width(), 4);
        assert_eq!(
            (0..4).map(|c| map.key_at(c)).collect::<Vec<_>>(),
            vec![Some(60), Some(61), Some(62), Some(63)]
        );
    }

    #[test]
    fn white_keys_filled_shrink_the_image_width() {
        let map = KeyMap::build(60, 63, KeyFilter::White, ColumnMapping::Filled).unwrap();
        assert_eq!(map.width(), 2);
        assert_eq!(map.key_at(0), Some(60));
        assert_eq!(map.key_at(1), Some(62));
    }

    #[test]
    fn white_keys_clipped_keep_silent_columns() {
        let map = KeyMap::build(60, 63, KeyFilter::White, ColumnMapping::Clipped).unwrap();
        assert_eq!(map.width(), 4);
        assert_eq!(map.key_at(0), Some(60));
        assert_eq!(map.key_at(1), None);
        assert_eq!(map.key_at(2), Some(62));
        assert_eq!(map.key_at(3), None);
    }

    #[test]
    fn invalid_ranges_are_rejected() {
        assert!(KeyMap::build(64, 60, KeyFilter::All, ColumnMapping::Filled).is_err());
        assert!(KeyMap::build(0, 128, KeyFilter::All, ColumnMapping::Filled).is_err());
    }

    #[test]
    fn note_names_are_correct() {
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(61), "C#4");
        assert_eq!(note_name(21), "A0");
        assert_eq!(note_name(108), "C8");
    }
}
