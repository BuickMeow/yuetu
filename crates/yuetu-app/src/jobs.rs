//! Background conversion jobs.
//!
//! The UI thread never runs `convert()` for a large image. It starts a job,
//! then polls progress/result events. Cancellation is cooperative through an
//! `AtomicBool` checked by the core stages.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use yuetu_core::{
    ConversionConfig, ConversionResult, Error, Palette, RgbaImage, Stage,
    convert_frames_with_palette_and_progress,
};

/// UI-facing progress snapshot.
#[derive(Clone, Copy, Debug)]
pub struct ProgressView {
    pub stage: Stage,
    /// 0..1 weighted across the whole pipeline.
    pub overall: f32,
}

impl ProgressView {
    pub fn new(stage: Stage, stage_fraction: f32) -> Self {
        let stage_fraction = stage_fraction.clamp(0.0, 1.0);
        let (base, weight) = match stage {
            Stage::Palette => (0.0, 0.20),
            Stage::Resize => (0.20, 0.10),
            Stage::Quantize => (0.30, 0.35),
            Stage::Notes => (0.65, 0.35),
        };
        Self {
            stage,
            overall: (base + weight * stage_fraction).clamp(0.0, 1.0),
        }
    }

    pub fn label(self) -> &'static str {
        match self.stage {
            Stage::Palette => "分析调色板",
            Stage::Resize => "缩放图片",
            Stage::Quantize => "量化颜色",
            Stage::Notes => "生成音符",
        }
    }
}

pub enum JobEvent {
    Progress {
        id: u64,
        stage: Stage,
        fraction: f32,
    },
    Finished {
        id: u64,
        result: ConversionResult,
    },
    Failed {
        id: u64,
        message: String,
    },
    Cancelled {
        id: u64,
    },
}

impl JobEvent {
    pub fn id(&self) -> u64 {
        match self {
            Self::Progress { id, .. }
            | Self::Finished { id, .. }
            | Self::Failed { id, .. }
            | Self::Cancelled { id } => *id,
        }
    }

    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Progress { .. })
    }
}

struct ActiveJob {
    id: u64,
    cancel: Arc<AtomicBool>,
}

/// Spawns one conversion at a time and relays events back to the UI thread.
pub struct JobRunner {
    event_tx: Sender<JobEvent>,
    event_rx: Receiver<JobEvent>,
    active: Option<ActiveJob>,
    next_id: u64,
}

impl Default for JobRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl JobRunner {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        Self {
            event_tx,
            event_rx,
            active: None,
            next_id: 1,
        }
    }

    pub fn start(
        &mut self,
        image: Arc<RgbaImage>,
        palette_frames: Vec<Arc<RgbaImage>>,
        delays_ms: Vec<u32>,
        shared_palette: Option<Palette>,
        config: ConversionConfig,
    ) -> Result<u64, String> {
        self.cancel();

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let tx = self.event_tx.clone();

        thread::Builder::new()
            .name(format!("yuetu-convert-{id}"))
            .spawn(move || {
                let progress = |stage: Stage, fraction: f32| {
                    let _ = tx.send(JobEvent::Progress {
                        id,
                        stage,
                        fraction,
                    });
                };

                let frame_refs: Vec<&RgbaImage> = palette_frames.iter().map(Arc::as_ref).collect();
                let event = match convert_frames_with_palette_and_progress(
                    image.as_ref(),
                    &frame_refs,
                    &delays_ms,
                    shared_palette.as_ref(),
                    &config,
                    Some(&progress),
                    &worker_cancel,
                ) {
                    Ok(result) => JobEvent::Finished { id, result },
                    Err(Error::Cancelled) => JobEvent::Cancelled { id },
                    Err(error) => JobEvent::Failed {
                        id,
                        message: error.to_string(),
                    },
                };
                let _ = tx.send(event);
            })
            .map_err(|error| error.to_string())?;

        self.active = Some(ActiveJob { id, cancel });
        Ok(id)
    }

    /// Cooperative cancel. The worker may take a few rows to notice.
    pub fn cancel(&mut self) {
        if let Some(active) = self.active.take() {
            active.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn is_running(&self) -> bool {
        self.active.is_some()
    }

    /// Drain events belonging to the active job. Events from cancelled jobs
    /// are discarded.
    pub fn poll(&mut self) -> Vec<JobEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            if self.active.as_ref().map(|active| active.id) != Some(event.id()) {
                continue;
            }

            let terminal = event.is_terminal();
            events.push(event);
            if terminal {
                self.active = None;
            }
        }
        events
    }
}

impl Drop for JobRunner {
    fn drop(&mut self) {
        self.cancel();
    }
}
