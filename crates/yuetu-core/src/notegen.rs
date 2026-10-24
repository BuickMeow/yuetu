//! The core note-generation algorithm.
//!
//! This is a pure function: `IndexedImage + KeyMap` in, notes out. It knows
//! nothing about palettes (only track indices), files, timing units, or UI.
//!
//! Rows are scanned **bottom to top**. Each row step advances time by one
//! pixel-tick. Whenever a column changes color (or a note-length limit forces
//! a retrigger), the previous note is closed and the new one is opened.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{Error, Result};
use crate::keymap::KeyMap;
use crate::progress::{Progress, Stage, report};
use crate::quantize::{IndexedImage, TRANSPARENT_ID};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Note {
    /// MIDI key number.
    pub key: u8,
    /// Palette color index; also the MIDI track/channel source.
    pub track: u16,
    /// Start time in pixel-ticks, measured from the bottom of the image.
    pub start: u32,
    /// End time in pixel-ticks (exclusive), from the bottom.
    pub end: u32,
}

impl Note {
    pub fn duration(self) -> u32 {
        self.end.saturating_sub(self.start)
    }
}

/// Generate all notes from a quantized image.
///
/// * `max_note_length == 0` disables length limiting.
/// * `measure_from_start == true` splits on a fixed grid anchored at the
///   bottom; otherwise a note is retriggered after it has been held for
///   `max_note_length` ticks (the original "flow with color" behavior).
pub fn generate_notes(
    indexed: &IndexedImage,
    keymap: &KeyMap,
    track_count: usize,
    max_note_length: u32,
    measure_from_start: bool,
) -> (Vec<Note>, Vec<usize>) {
    generate_notes_with_progress(
        indexed,
        keymap,
        track_count,
        max_note_length,
        measure_from_start,
        &AtomicBool::new(false),
        None,
    )
    .expect("note generation without cancellation cannot fail")
}

/// Cancellable, progress-reporting variant used by the worker thread.
#[allow(clippy::too_many_arguments)]
pub fn generate_notes_with_progress(
    indexed: &IndexedImage,
    keymap: &KeyMap,
    track_count: usize,
    max_note_length: u32,
    measure_from_start: bool,
    cancel: &AtomicBool,
    progress: Option<&dyn Progress>,
) -> Result<(Vec<Note>, Vec<usize>)> {
    report(progress, Stage::Notes, 0.0);

    let width = indexed.width as usize;
    let mut notes = Vec::new();
    let mut count_by_track = vec![0usize; track_count];

    // Resolve the key map once. Silent columns are skipped entirely instead of
    // paying a `key_at` call for every pixel row.
    let mut columns: Vec<(usize, u8)> = Vec::with_capacity(width);
    for column in 0..width {
        if let Some(key) = keymap.key_at(column as u32) {
            columns.push((column, key));
        }
    }

    let mut column_color = vec![TRANSPARENT_ID; columns.len()];
    let mut column_start = vec![0u32; columns.len()];
    let mut column_last_on = vec![0u32; columns.len()];

    let height = indexed.height;
    let mut time = 0u32;

    for row in (0..height).rev() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let row_start = row as usize * width;
        let grid_retrigger = max_note_length > 0
            && measure_from_start
            && time > 0
            && time.is_multiple_of(max_note_length);

        for (position, &(column, key)) in columns.iter().enumerate() {
            let mut new_color = indexed.indices[row_start + column];
            if new_color < 0 || new_color as usize >= track_count {
                new_color = TRANSPARENT_ID;
            }

            let old_color = column_color[position];
            let force_retrigger = max_note_length > 0
                && old_color >= 0
                && if measure_from_start {
                    grid_retrigger
                } else {
                    time.saturating_sub(column_last_on[position]) >= max_note_length
                };

            if new_color != old_color || force_retrigger {
                if old_color >= 0 {
                    push_note(
                        &mut notes,
                        &mut count_by_track,
                        Note {
                            key,
                            track: old_color as u16,
                            start: column_start[position],
                            end: time,
                        },
                    );
                }

                column_color[position] = new_color;
                if new_color >= 0 {
                    column_start[position] = time;
                    column_last_on[position] = time;
                }
            }
        }

        time = time.saturating_add(1);

        if row % 16 == 0 && height > 0 {
            report(
                progress,
                Stage::Notes,
                (height - row) as f32 / height as f32,
            );
        }
    }

    // Close every note still ringing at the top edge.
    for (position, &(_, key)) in columns.iter().enumerate() {
        let color = column_color[position];
        if color < 0 {
            continue;
        }
        push_note(
            &mut notes,
            &mut count_by_track,
            Note {
                key,
                track: color as u16,
                start: column_start[position],
                end: time,
            },
        );
    }

    notes.sort_unstable_by_key(|note| (note.start, note.track, note.key, note.end));

    report(progress, Stage::Notes, 1.0);
    Ok((notes, count_by_track))
}

fn push_note(notes: &mut Vec<Note>, count_by_track: &mut [usize], note: Note) {
    if note.end <= note.start {
        return;
    }
    if let Some(count) = count_by_track.get_mut(note.track as usize) {
        *count += 1;
    }
    notes.push(note);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{ColumnMapping, KeyFilter};
    use crate::quantize::quantize;
    use crate::{Color, Palette, RgbaImage};

    fn keymap(start: u8, end: u8) -> KeyMap {
        KeyMap::build(start, end, KeyFilter::All, ColumnMapping::Filled).unwrap()
    }

    #[test]
    fn solid_column_is_one_long_note() {
        let image = RgbaImage::new(1, 4, Color::rgb(255, 0, 0));
        let palette = Palette::new(vec![Color::rgb(255, 0, 0)]);
        let indexed = quantize(&image, &palette);
        let (notes, counts) = generate_notes(&indexed, &keymap(60, 60), 1, 0, false);

        assert_eq!(notes.len(), 1);
        assert_eq!(
            notes[0],
            Note {
                key: 60,
                track: 0,
                start: 0,
                end: 4
            }
        );
        assert_eq!(counts, vec![1]);
    }

    #[test]
    fn color_change_closes_and_opens_at_the_same_tick() {
        let mut image = RgbaImage::new(1, 2, Color::rgb(0, 0, 255));
        image.set_pixel(0, 0, Color::rgb(255, 0, 0)); // top
        let palette = Palette::new(vec![Color::rgb(255, 0, 0), Color::rgb(0, 0, 255)]);
        let indexed = quantize(&image, &palette);
        let (notes, counts) = generate_notes(&indexed, &keymap(60, 60), 2, 0, false);

        assert_eq!(notes.len(), 2);
        assert_eq!(
            notes[0],
            Note {
                key: 60,
                track: 1,
                start: 0,
                end: 1
            }
        );
        assert_eq!(
            notes[1],
            Note {
                key: 60,
                track: 0,
                start: 1,
                end: 2
            }
        );
        assert_eq!(counts, vec![1, 1]);
    }

    #[test]
    fn transparent_gap_produces_two_notes() {
        let mut image = RgbaImage::new(1, 3, Color::rgb(255, 0, 0));
        image.set_pixel(0, 1, Color::TRANSPARENT);
        let palette = Palette::new(vec![Color::rgb(255, 0, 0)]);
        let indexed = quantize(&image, &palette);
        let (notes, _) = generate_notes(&indexed, &keymap(60, 60), 1, 0, false);

        assert_eq!(notes.len(), 2);
        assert_eq!((notes[0].start, notes[0].end), (0, 1));
        assert_eq!((notes[1].start, notes[1].end), (2, 3));
    }

    #[test]
    fn max_length_retriggers_a_held_note() {
        let image = RgbaImage::new(1, 5, Color::rgb(255, 0, 0));
        let palette = Palette::new(vec![Color::rgb(255, 0, 0)]);
        let indexed = quantize(&image, &palette);
        let (notes, _) = generate_notes(&indexed, &keymap(60, 60), 1, 2, false);

        assert_eq!(notes.len(), 3);
        assert_eq!((notes[0].start, notes[0].end), (0, 2));
        assert_eq!((notes[1].start, notes[1].end), (2, 4));
        assert_eq!((notes[2].start, notes[2].end), (4, 5));
    }
}
