//! Progress reporting for long-running conversion stages.
//!
//! The trait deliberately has no `Send + Sync` bound: the caller decides
//! where it runs. The worker thread in the app sends reports to the UI through
//! a channel.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Build the popularity palette.
    Palette,
    /// Scale the image to the key width.
    Resize,
    /// Map every pixel to a palette index.
    Quantize,
    /// Scan columns and emit notes.
    Notes,
}

/// Receives per-stage progress in `0.0..=1.0`.
pub trait Progress {
    fn report(&self, stage: Stage, fraction: f32);
}

impl<F> Progress for F
where
    F: Fn(Stage, f32),
{
    fn report(&self, stage: Stage, fraction: f32) {
        self(stage, fraction.clamp(0.0, 1.0));
    }
}

pub(crate) fn report(progress: Option<&dyn Progress>, stage: Stage, fraction: f32) {
    if let Some(progress) = progress {
        progress.report(stage, fraction.clamp(0.0, 1.0));
    }
}
