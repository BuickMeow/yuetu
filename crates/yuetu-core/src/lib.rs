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
//! The core idea is the same as ImageToMidi: the key axis of the resized
//! image maps to piano keys, each palette color is a track, and every maximal
//! run of one color along the time axis becomes a note. The key and time axes
//! can be horizontal or vertical; time always starts at the bottom-left.

pub mod color;
pub mod convert;
pub mod error;
pub mod image;
pub mod keymap;
pub mod layout;
pub mod midi;
pub mod notegen;
pub mod palette;
pub mod progress;
pub mod quantize;
pub mod resize;
pub mod rotate;

pub use color::Color;
pub use convert::{
    ConversionConfig, ConversionResult, PaletteScope, StageTimings, convert,
    convert_frames_with_progress, convert_with_options, convert_with_progress,
};
pub use error::{Error, Result};
pub use image::{
    ImageSequence, RgbaImage, load_image, load_image_from_memory, load_image_sequence,
};
pub use keymap::{ColumnMapping, KeyFilter, KeyMap, is_white_key, note_name};
pub use layout::{TimeAxis, TimeLength, resolve_time_rows};
pub use midi::{MidiOutputConfig, TimeSignature, write_midi};
pub use notegen::{
    generate_notes, generate_notes_on_grid, generate_notes_with_progress,
    generate_notes_with_progress_on_grid,
};
pub use palette::Palette;
pub use progress::{Progress, Stage};
pub use quantize::{IndexedImage, TRANSPARENT_ID, quantize, quantize_with_progress};
pub use resize::ResizeAlgorithm;
pub use rotate::{
    normalize_degrees, rotate_image, rotated_dimensions, rotated_dimensions_f32, sample_bilinear,
};
