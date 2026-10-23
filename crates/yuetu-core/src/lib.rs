//! Pure, UI-independent image → MIDI conversion.
//!
//! The crate is deliberately split along the pipeline:
//!
//! ```text
//! image load ──► resize ──► quantize ──► note generation
//!                                              │
//!                                       ConversionResult
//!                                              │
//!                                   MIDI writer / UI preview
//! ```
//!
//! No module here knows about egui, wgpu, windows, or files dialogs.
//!
//! The core idea is the same as ImageToMidi: each column of the resized image
//! is a piano key, each palette color is a track, and every maximal vertical
//! run of one color becomes a note. The scan runs bottom-to-top so time flows
//! upward, like a piano roll.

pub mod color;
pub mod convert;
pub mod error;
pub mod image;
pub mod keymap;
pub mod midi;
pub mod notegen;
pub mod palette;
pub mod progress;
pub mod quantize;
pub mod resize;

pub use color::Color;
pub use convert::{ConversionConfig, ConversionResult, convert, convert_with_progress};
pub use error::{Error, Result};
pub use image::{
    ImageSequence, RgbaImage, load_image, load_image_from_memory, load_image_sequence,
};
pub use keymap::{ColumnMapping, KeyFilter, KeyMap, is_white_key, note_name};
pub use midi::{MidiOutputConfig, write_midi};
pub use notegen::{generate_notes, generate_notes_with_progress};
pub use palette::Palette;
pub use progress::{Progress, Stage};
pub use quantize::{IndexedImage, TRANSPARENT_ID, quantize, quantize_with_progress};
pub use resize::ResizeAlgorithm;
