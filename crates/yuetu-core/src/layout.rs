//! Time-axis orientation and time-axis length resolution.
//!
//! The image is always interpreted with the origin at the bottom-left:
//!
//! * [`TimeAxis::Vertical`] — keys run left-to-right, time runs bottom-to-top.
//! * [`TimeAxis::Horizontal`] — time runs left-to-right, keys run bottom-to-top.
//!
//! The key axis is resized to the mapped key count. The time axis length is
//! resolved independently, which is what makes non-square grid cells possible.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimeAxis {
    /// Keys on X, time on Y. Output grid is `keys × rows`.
    #[default]
    Vertical,
    /// Time on X, keys on Y. Output grid is `rows × keys`.
    Horizontal,
}

impl TimeAxis {
    pub fn label(self) -> &'static str {
        match self {
            Self::Vertical => "纵向时间",
            Self::Horizontal => "横向时间",
        }
    }

    /// Extent of the source canvas along the key and time axes.
    pub fn source_extents(self, width: u32, height: u32) -> (u32, u32) {
        match self {
            Self::Vertical => (width.max(1), height.max(1)),
            Self::Horizontal => (height.max(1), width.max(1)),
        }
    }

    /// Dimensions of an oriented grid for `keys` key positions and `rows`
    /// time rows.
    pub fn grid_size(self, keys: u32, rows: u32) -> (u32, u32) {
        match self {
            Self::Vertical => (keys.max(1), rows.max(1)),
            Self::Horizontal => (rows.max(1), keys.max(1)),
        }
    }

    pub fn time_rows(self, width: u32, height: u32) -> u32 {
        match self {
            Self::Vertical => height.max(1),
            Self::Horizontal => width.max(1),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimeLength {
    /// Preserve one source time pixel per output time row.
    #[default]
    Source,
    /// Resize both axes by `keys / key_extent`, keeping square pixels.
    Square,
    /// Use an explicit number of time rows.
    Fixed(u32),
}

impl TimeLength {
    pub fn label(self) -> &'static str {
        match self {
            Self::Source => "原图长度",
            Self::Square => "正方形像素",
            Self::Fixed(_) => "固定高度",
        }
    }
}

/// Resolve the number of time rows from the rotated source canvas and key
/// count. This is intentionally independent of MIDI PPQ and ticks per row.
pub fn resolve_time_rows(
    source_width: u32,
    source_height: u32,
    key_count: u32,
    axis: TimeAxis,
    length: TimeLength,
) -> u32 {
    let (key_extent, time_extent) = axis.source_extents(source_width, source_height);
    match length {
        TimeLength::Source => time_extent,
        TimeLength::Square => ((f64::from(time_extent) * f64::from(key_count.max(1)))
            / f64::from(key_extent))
        .round()
        .max(1.0) as u32,
        TimeLength::Fixed(rows) => rows.max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_length_preserves_time_pixels() {
        assert_eq!(
            resolve_time_rows(800, 800, 128, TimeAxis::Vertical, TimeLength::Source),
            800
        );
        assert_eq!(
            resolve_time_rows(800, 800, 128, TimeAxis::Horizontal, TimeLength::Source),
            800
        );
        assert_eq!(
            resolve_time_rows(1200, 800, 128, TimeAxis::Vertical, TimeLength::Source),
            800
        );
        assert_eq!(
            resolve_time_rows(1200, 800, 128, TimeAxis::Horizontal, TimeLength::Source),
            1200
        );
    }

    #[test]
    fn square_pixels_scale_the_time_axis_with_the_key_axis() {
        assert_eq!(
            resolve_time_rows(1200, 800, 128, TimeAxis::Vertical, TimeLength::Square),
            85
        );
        assert_eq!(
            resolve_time_rows(1200, 800, 128, TimeAxis::Horizontal, TimeLength::Square),
            192
        );
    }

    #[test]
    fn fixed_length_uses_rows_directly() {
        assert_eq!(
            resolve_time_rows(800, 800, 128, TimeAxis::Vertical, TimeLength::Fixed(1001)),
            1001
        );
        assert_eq!(TimeAxis::Vertical.grid_size(128, 1001), (128, 1001));
        assert_eq!(TimeAxis::Horizontal.grid_size(128, 1001), (1001, 128));
    }
}
