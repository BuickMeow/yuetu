//! Error type shared by the core pipeline.

/// Everything that can go wrong before, during, or after conversion.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("image decode failed: {0}")]
    Image(#[from] image::ImageError),

    #[error("file I/O failed: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid image: {0}")]
    InvalidImage(String),

    #[error("invalid configuration: {0}")]
    Config(String),

    #[error("MIDI writing failed: {0}")]
    Midi(String),

    #[error("conversion cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;
