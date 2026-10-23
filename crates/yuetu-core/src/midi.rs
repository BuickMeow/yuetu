//! Standard MIDI file writing.
//!
//! One palette color maps to one MIDI channel, 16 colors per MIDI track —
//! the same layout as the original tool. A Format-1 file is written so a DAW
//! can show each color group as a separate track.

use std::path::Path;

use midly::num::{u4, u7, u15, u24, u28};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};

use crate::convert::ConversionResult;
use crate::error::{Error, Result};

/// MIDI-specific output settings. Kept separate from conversion settings so
/// the preview can be regenerated without touching timing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiOutputConfig {
    /// MIDI ticks per image pixel (row).
    pub ticks_per_pixel: u16,
    /// Pulses per quarter note.
    pub ppq: u16,
    /// Tempo written at tick 0.
    pub bpm: u16,
    /// Silence before the first note, in MIDI ticks.
    pub start_offset: u32,
    /// Emit the original tool's custom `FF 0A` color meta events.
    pub emit_color_events: bool,
}

impl Default for MidiOutputConfig {
    fn default() -> Self {
        Self {
            ticks_per_pixel: 5,
            ppq: 960,
            bpm: 120,
            start_offset: 0,
            emit_color_events: true,
        }
    }
}

pub fn write_midi(path: &Path, result: &ConversionResult, config: &MidiOutputConfig) -> Result<()> {
    if result.palette.is_empty() {
        return Err(Error::Midi("调色板为空，无法生成音轨".into()));
    }

    let palette_len = result.palette.len();
    let midi_track_count = palette_len.div_ceil(16);
    let ticks_per_pixel = u64::from(config.ticks_per_pixel.max(1));
    let start_offset = u64::from(config.start_offset);
    if start_offset > MAX_DELTA {
        return Err(Error::Midi(format!(
            "起始偏移 {start_offset} 超过单事件 delta 上限 {MAX_DELTA}"
        )));
    }

    // Color meta events borrow their payload. Keep the arena alive until
    // `Smf::save` below; this avoids leaking `Box<[u8]>` for every export.
    let color_payloads: Vec<[u8; 8]> = if config.emit_color_events {
        result
            .palette
            .colors
            .iter()
            .enumerate()
            .map(|(index, color)| {
                [
                    0x00,
                    0x0F,
                    (index % 16) as u8,
                    0x00,
                    color.r,
                    color.g,
                    color.b,
                    color.a,
                ]
            })
            .collect()
    } else {
        Vec::new()
    };

    let mut tracks: Vec<Vec<TimedEvent>> = (0..midi_track_count)
        .map(|_| Vec::with_capacity(64))
        .collect();

    tracks[0].push(TimedEvent {
        tick: 0,
        priority: 0,
        kind: EventKind::Tempo(config.bpm.max(1)),
    });

    for payload_index in 0..color_payloads.len() {
        tracks[payload_index / 16].push(TimedEvent {
            tick: 0,
            priority: 0,
            kind: EventKind::Color { payload_index },
        });
    }

    for note in &result.notes {
        let color_index = note.track as usize;
        if color_index >= palette_len {
            continue;
        }
        let track = color_index / 16;
        let channel = (color_index % 16) as u8;
        let on_tick = start_offset + u64::from(note.start) * ticks_per_pixel;
        let off_tick = start_offset + u64::from(note.end) * ticks_per_pixel;

        tracks[track].push(TimedEvent {
            tick: on_tick,
            priority: 2,
            kind: EventKind::NoteOn {
                channel,
                key: note.key,
            },
        });
        tracks[track].push(TimedEvent {
            tick: off_tick,
            priority: 1,
            kind: EventKind::NoteOff {
                channel,
                key: note.key,
            },
        });
    }

    let mut midi_tracks: Vec<Vec<TrackEvent<'_>>> = Vec::with_capacity(midi_track_count);
    for mut events in tracks {
        events.sort_by_key(|event| (event.tick, event.priority));

        let mut last_tick = 0u64;
        let mut out = Vec::with_capacity(events.len() + 1);
        for event in events {
            let delta = event.tick.saturating_sub(last_tick);
            if delta > MAX_DELTA {
                return Err(Error::Midi(format!(
                    "事件间隔 {delta} ticks 超过 SMF 的 u28 上限；请减小 Ticks/px 或起始偏移"
                )));
            }
            out.push(TrackEvent {
                delta: u28::new(delta as u32),
                kind: event.kind.into_track_event(&color_payloads),
            });
            last_tick = event.tick;
        }
        out.push(TrackEvent {
            delta: u28::new(0),
            kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
        });
        midi_tracks.push(out);
    }

    let header = Header {
        format: Format::Parallel,
        timing: Timing::Metrical(u15::new(config.ppq.clamp(1, 0x7FFF))),
    };
    let smf = Smf {
        header,
        tracks: midi_tracks,
    };
    smf.save(path)
        .map_err(|error| Error::Midi(error.to_string()))
}

#[derive(Clone, Copy)]
struct TimedEvent {
    tick: u64,
    /// Tempo/color (0), note-off (1), note-on (2). Ensures note-offs come
    /// before note-ons when a color changes at the same tick.
    priority: u8,
    kind: EventKind,
}

#[derive(Clone, Copy)]
enum EventKind {
    Tempo(u16),
    Color { payload_index: usize },
    NoteOn { channel: u8, key: u8 },
    NoteOff { channel: u8, key: u8 },
}

impl EventKind {
    fn into_track_event<'a>(self, color_payloads: &'a [[u8; 8]]) -> TrackEventKind<'a> {
        match self {
            Self::Tempo(bpm) => {
                let micros = (60_000_000u32 / u32::from(bpm.max(1))).min(0x00FF_FFFF);
                TrackEventKind::Meta(MetaMessage::Tempo(u24::new(micros)))
            }
            Self::Color { payload_index } => {
                // Matches the original ImageToMidi ColorEvent layout:
                // 00 0F <channel> 00 <r> <g> <b> <a>
                TrackEventKind::Meta(MetaMessage::Unknown(0x0A, &color_payloads[payload_index]))
            }
            Self::NoteOn { channel, key } => TrackEventKind::Midi {
                channel: u4::new(channel & 0x0F),
                message: MidiMessage::NoteOn {
                    key: u7::new(key & 0x7F),
                    vel: u7::new(1),
                },
            },
            Self::NoteOff { channel, key } => TrackEventKind::Midi {
                channel: u4::new(channel & 0x0F),
                message: MidiMessage::NoteOff {
                    key: u7::new(key & 0x7F),
                    vel: u7::new(0),
                },
            },
        }
    }
}

const MAX_DELTA: u64 = 0x0FFF_FFFF;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
    use crate::convert::{ConversionConfig, convert};
    use crate::image::RgbaImage;
    use crate::palette::Palette;

    #[test]
    fn writes_a_parseable_format_one_file() {
        let image = RgbaImage::new(1, 2, Color::rgb(255, 0, 0));
        let mut config = ConversionConfig {
            key_start: 60,
            key_end: 60,
            palette_colors: 1,
            ..ConversionConfig::default()
        };
        config.manual_palette = Some(Palette::new(vec![Color::rgb(255, 0, 0)]));
        let result = convert(&image, &config).unwrap();

        let path = std::env::temp_dir().join(format!("yuetu-test-{}.mid", std::process::id()));
        write_midi(&path, &result, &MidiOutputConfig::default()).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let smf = Smf::parse(&bytes).unwrap();
        assert_eq!(smf.header.format, Format::Parallel);
        assert_eq!(smf.tracks.len(), 1);
        let note_ons = smf.tracks[0]
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    TrackEventKind::Midi {
                        message: MidiMessage::NoteOn { .. },
                        ..
                    }
                )
            })
            .count();
        assert_eq!(note_ons, 1);

        let _ = std::fs::remove_file(path);
    }
}
