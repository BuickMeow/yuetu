use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Key, Modifiers};
use yuetu_core::{ConversionConfig, ConversionSeed, Palette, PaletteScope, Stage};

use crate::jobs::{JobEvent, JobRunner, ProgressView};
use crate::settings::Settings;
use crate::state::{
    CoordinateOrigin, PipelineStage, Preview, SourceImage, StageHover, StageStop, StatusMessage,
    ZoomPreset,
};
use crate::theme;
use crate::ui;
use crate::widgets::{View2D, split_handle, stage_rail};

/// Wait this long after the last parameter edit before starting a new job.
/// It keeps a slider drag from spawning one conversion per frame while still
/// feeling immediate.
const AUTO_CONVERT_DEBOUNCE: Duration = Duration::from_millis(80);

/// Palette clustered from a whole animation sequence. Frame changes do not
/// invalidate it: only loading a new source or changing the palette settings
/// does. This keeps timeline scrubbing from re-scanning every frame.
struct SharedPaletteCache {
    source_generation: u64,
    palette_colors: usize,
    palette: Palette,
}

pub struct YuetuApp {
    source: Option<SourceImage>,
    /// Increments whenever the source sequence changes (new file).
    source_generation: u64,
    /// Increments whenever the source pixels change (new image or GIF frame).
    source_revision: u64,
    shared_palette_cache: Option<SharedPaletteCache>,
    preview: Option<Preview>,
    settings: Settings,

    stage: PipelineStage,
    stage_stops: Vec<StageStop>,
    stage_hover: Option<StageHover>,
    coordinate_origin: CoordinateOrigin,
    frame_strip_last_frame: usize,
    frame_strip_mode: ui::frame_strip::FrameStripMode,
    frame_scrub_pending: bool,
    checker_texture: egui::TextureHandle,
    gpu_target_format: eframe::wgpu::TextureFormat,
    source_view: View2D,
    key_view: View2D,
    palette_view: View2D,
    midi_view: View2D,
    zoom_modes: [ZoomPreset; 4],

    status: StatusMessage,

    jobs: JobRunner,
    job_progress: Option<ProgressView>,
    job_config: Option<ConversionConfig>,
    job_source_revision: Option<u64>,
    job_source_generation: Option<u64>,
    job_started: Option<Instant>,
    /// When the next automatic conversion should start. `None` means no
    /// parameters changed since the last result/job.
    auto_convert_at: Option<Instant>,

    left_panel_width: f32,
}

impl YuetuApp {
    pub fn new(cc: &eframe::CreationContext<'_>, startup_image: Option<PathBuf>) -> Self {
        theme::apply(&cc.egui_ctx);
        let checker_texture = theme::checker_texture(&cc.egui_ctx);
        let gpu_target_format = cc
            .wgpu_render_state
            .as_ref()
            .map(|state| state.target_format)
            .unwrap_or(eframe::wgpu::TextureFormat::Bgra8UnormSrgb);

        let mut app = Self {
            source: None,
            source_generation: 0,
            source_revision: 0,
            shared_palette_cache: None,
            preview: None,
            settings: Settings::default(),
            stage: PipelineStage::Source,
            stage_stops: Vec::new(),
            stage_hover: None,
            coordinate_origin: CoordinateOrigin::TopLeft,
            frame_strip_last_frame: 0,
            frame_strip_mode: ui::frame_strip::FrameStripMode::default(),
            frame_scrub_pending: false,
            checker_texture,
            gpu_target_format,
            source_view: View2D::default(),
            key_view: View2D::default(),
            palette_view: View2D::default(),
            midi_view: View2D::default(),
            zoom_modes: [ZoomPreset::Fit; 4],
            status: StatusMessage::default(),
            jobs: JobRunner::new(),
            job_progress: None,
            job_config: None,
            job_source_revision: None,
            job_source_generation: None,
            job_started: None,
            auto_convert_at: None,
            left_panel_width: 310.0,
        };

        if let Some(path) = startup_image {
            app.load_path(&cc.egui_ctx, path);
        }

        app
    }

    fn open_dialog(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "图片",
                &["png", "jpg", "jpeg", "bmp", "gif", "webp", "tif", "tiff"],
            )
            .pick_file()
        {
            self.load_path(ctx, path);
        }
    }

    fn load_path(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.jobs.cancel();
        self.job_progress = None;
        self.job_config = None;
        self.job_source_revision = None;
        self.job_source_generation = None;
        self.job_started = None;
        self.auto_convert_at = None;

        match SourceImage::load(ctx, path.clone()) {
            Ok(source) => {
                self.source_generation = self.source_generation.wrapping_add(1);
                self.shared_palette_cache = None;
                self.settings.rotation_deg = 0.0;
                self.status = StatusMessage::success(format!(
                    "已加载 {} · {} × {} px",
                    source.name, source.display_size.0, source.display_size.1
                ));
                self.source = Some(source);
                self.source_revision = self.source_revision.wrapping_add(1);
                self.preview = None;
                self.stage = PipelineStage::Source;
                self.stage_hover = None;
                self.frame_strip_last_frame = 0;
                self.frame_scrub_pending = false;
                self.source_view.reset_fit();
                self.key_view.reset_fit();
                self.palette_view.reset_fit();
                self.midi_view.reset_fit();
                self.zoom_modes = [ZoomPreset::Fit; 4];
                self.schedule_auto_conversion(ctx, Duration::ZERO);
            }
            Err(error) => {
                tracing::warn!("failed to load image {}: {error}", path.display());
                self.status = StatusMessage::error(format!("加载失败：{error}"));
            }
        }
    }

    fn start_conversion(&mut self, ctx: &egui::Context) {
        self.auto_convert_at = None;

        let Some(source) = self.source.as_ref() else {
            self.status = StatusMessage::warning("未打开图片");
            return;
        };
        if let Err(error) = self.settings.validate() {
            self.status = StatusMessage::error(error);
            return;
        }

        let config = self.settings.conversion_config();
        let seed = self.conversion_seed(&config);
        let seed_has_palette = seed
            .as_ref()
            .and_then(|seed| seed.palette.as_ref())
            .is_some();
        let shared_palette =
            if config.palette_scope == PaletteScope::AllFrames && config.manual_palette.is_none() {
                self.shared_palette_cache
                    .as_ref()
                    .filter(|cache| {
                        cache.source_generation == self.source_generation
                            && cache.palette_colors == config.palette_colors
                    })
                    .map(|cache| cache.palette.clone())
            } else {
                None
            };
        let need_palette_frames = config.palette_scope == PaletteScope::AllFrames
            && shared_palette.is_none()
            && !seed_has_palette;
        let palette_frames = if need_palette_frames {
            source.palette_frames().to_vec()
        } else {
            Vec::new()
        };
        let delays_ms = if need_palette_frames {
            source.delays_ms().to_vec()
        } else {
            Vec::new()
        };
        let image = source.image.clone();
        let source_generation = self.source_generation;
        match self.jobs.start(
            image,
            palette_frames,
            delays_ms,
            shared_palette,
            seed,
            config.clone(),
        ) {
            Ok(_) => {
                self.job_config = Some(config);
                self.job_source_revision = Some(self.source_revision);
                self.job_source_generation = Some(source_generation);
                self.job_started = Some(Instant::now());
                self.job_progress = Some(ProgressView::new(Stage::Palette, 0.0));
                self.status = StatusMessage::info("正在生成");
                ctx.request_repaint();
            }
            Err(error) => {
                tracing::warn!("failed to spawn conversion job: {error}");
                self.job_progress = None;
                self.job_config = None;
                self.job_source_revision = None;
                self.job_source_generation = None;
                self.job_started = None;
                self.status = StatusMessage::error(format!("无法启动转换任务：{error}"));
            }
        }
    }

    /// Build a stage-cache seed from the current preview when its upstream
    /// keys still match the requested configuration.
    fn conversion_seed(&self, config: &ConversionConfig) -> Option<ConversionSeed> {
        let preview = self.preview.as_ref()?;
        let previous = &preview.config;
        let same_source = preview.source_revision == self.source_revision;
        let same_keymap = keymap_config_eq(previous, config);
        let same_palette = palette_config_eq(previous, config);
        let same_transform = same_source && transform_config_eq(previous, config);

        let mut seed = ConversionSeed::default();
        if same_keymap {
            seed.keymap = Some(preview.result.keymap.clone());
        }
        if same_palette {
            seed.palette = Some(preview.result.palette.clone());
        }
        if same_transform {
            seed.resized = Some(preview.result.resized_image.clone());
            seed.resized_size = Some(preview.result.resized_size);
            seed.source_size = Some(preview.result.source_size);
            if same_palette {
                seed.indexed = Some(preview.result.indexed_image.clone());
            }
        }

        if seed.keymap.is_some()
            || seed.palette.is_some()
            || seed.resized.is_some()
            || seed.indexed.is_some()
        {
            Some(seed)
        } else {
            None
        }
    }

    /// Mark the current settings as dirty. A short debounce collapses slider
    /// drags into one conversion instead of cancelling and restarting per frame.
    fn schedule_auto_conversion(&mut self, ctx: &egui::Context, delay: Duration) {
        let deadline = Instant::now() + delay;
        self.auto_convert_at = Some(deadline);
        if !delay.is_zero() {
            ctx.request_repaint_after(delay);
        }
    }

    fn tick_auto_conversion(&mut self, ctx: &egui::Context) {
        let Some(deadline) = self.auto_convert_at else {
            return;
        };
        let now = Instant::now();
        if now < deadline {
            ctx.request_repaint_after(deadline - now);
            return;
        }

        self.auto_convert_at = None;
        if self.source.is_none() {
            return;
        }

        let config = self.settings.conversion_config();
        // A job for exactly these settings and source frame is already doing
        // the work.
        if self.jobs.is_running()
            && self.job_config.as_ref() == Some(&config)
            && self.job_source_revision == Some(self.source_revision)
        {
            return;
        }
        // A finished job already produced this exact result.
        if !self.jobs.is_running()
            && self.preview.as_ref().is_some_and(|preview| {
                preview.config == config && preview.source_revision == self.source_revision
            })
        {
            return;
        }

        self.start_conversion(ctx);
    }

    fn cancel_conversion(&mut self) {
        self.auto_convert_at = None;
        if !self.jobs.is_running() {
            return;
        }
        self.jobs.cancel();
        self.frame_scrub_pending = false;
        self.job_progress = None;
        self.job_config = None;
        self.job_source_revision = None;
        self.job_source_generation = None;
        self.job_started = None;
        self.status = StatusMessage::warning("已取消生成");
    }

    /// Select a GIF/WebP frame from the sidebar or the frame strip.
    ///
    /// A live frame-strip drag updates the source texture immediately but
    /// suspends conversion until the drag ends. This is what makes intermediate
    /// frames visible without spending the whole gesture on cancelled jobs.
    fn select_source_frame(&mut self, ctx: &egui::Context, frame: usize, live: bool) -> bool {
        let changed = {
            let Some(source) = self.source.as_mut() else {
                return false;
            };
            match source.select_frame(ctx, frame) {
                Ok(true) => true,
                Ok(false) => false,
                Err(error) => {
                    self.status = StatusMessage::error(format!("切换帧失败：{error}"));
                    false
                }
            }
        };
        if !changed {
            return false;
        }

        self.source_revision = self.source_revision.wrapping_add(1);
        self.stage_hover = None;
        if live {
            self.suspend_conversion_for_scrub();
            self.frame_scrub_pending = true;
            self.status = StatusMessage::info("浏览帧…");
            if matches!(self.stage, PipelineStage::KeyMap | PipelineStage::Palette) {
                self.update_scrub_preview(ctx);
            }
        } else {
            self.frame_scrub_pending = false;
            self.status = StatusMessage::info("正在更新预览…");
            self.schedule_auto_conversion(ctx, AUTO_CONVERT_DEBOUNCE);
        }
        true
    }

    /// Refresh only the resized/indexed images while scrubbing. This bypasses
    /// the background conversion so the key-map and palette stages follow the
    /// pointer without recomputing notes.
    fn update_scrub_preview(&mut self, ctx: &egui::Context) {
        let Some(source) = self.source.as_ref() else {
            return;
        };
        let image = source.interaction_image().clone();
        let full_source_size = source.display_size;
        let config = self.settings.conversion_config();
        let Some(preview) = self.preview.as_mut() else {
            return;
        };
        if preview.config != config {
            return;
        }
        // `display_image` is already rotated for the source stage. Avoid
        // rotating the full-resolution frame again just to rebuild a 128px
        // preview.
        let mut preview_config = config;
        preview_config.rotation_deg = 0.0;
        let palette = preview.result.palette.clone();
        if let Err(error) =
            preview.update_scrub_frame(ctx, &image, full_source_size, &preview_config, &palette)
        {
            tracing::warn!("failed to build scrub preview: {error}");
        }
    }

    fn suspend_conversion_for_scrub(&mut self) {
        self.auto_convert_at = None;
        if self.jobs.is_running() {
            self.jobs.cancel();
        }
        self.job_progress = None;
        self.job_config = None;
        self.job_source_revision = None;
        self.job_source_generation = None;
        self.job_started = None;
    }

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        for event in self.jobs.poll() {
            match event {
                JobEvent::Progress {
                    stage, fraction, ..
                } => {
                    let progress = ProgressView::new(stage, fraction);
                    self.job_progress = Some(progress);
                    self.status = StatusMessage::info(format!(
                        "正在生成：{} {:.0}%",
                        progress.label(),
                        progress.overall * 100.0
                    ));
                }
                JobEvent::Finished { result, .. } => {
                    let elapsed = self
                        .job_started
                        .take()
                        .map(|started| started.elapsed())
                        .unwrap_or_default();
                    let config = self.job_config.take().unwrap_or_default();
                    let source_revision = self.job_source_revision.take().unwrap_or_default();
                    let job_source_generation = self.job_source_generation.take();
                    let current = self.settings.conversion_config();
                    let had_preview = self.preview.is_some();
                    self.job_progress = None;
                    tracing::debug!(
                        palette_ms = result.timings.palette_ms,
                        transform_ms = result.timings.transform_ms,
                        quantize_ms = result.timings.quantize_ms,
                        notes_ms = result.timings.notes_ms,
                        total_ms = result.timings.total_ms,
                        "conversion stage timings"
                    );
                    if config.palette_scope == PaletteScope::AllFrames
                        && config.manual_palette.is_none()
                        && job_source_generation == Some(self.source_generation)
                    {
                        self.shared_palette_cache = Some(SharedPaletteCache {
                            source_generation: self.source_generation,
                            palette_colors: config.palette_colors,
                            palette: result.palette.clone(),
                        });
                    }
                    if config == current && source_revision == self.source_revision {
                        let timings = result.timings;
                        self.status = StatusMessage::success(format!(
                            "生成完成：{} 音符 · {} 音轨 · 用时 {:.1} ms",
                            result.note_count(),
                            result.track_count(),
                            elapsed.as_secs_f64() * 1000.0
                        ))
                        .with_detail(format!(
                            "调色板 {:.1} ms\n键位映射/缩放 {:.1} ms\n量化 {:.1} ms\n音符 {:.1} ms",
                            timings.palette_ms,
                            timings.transform_ms,
                            timings.quantize_ms,
                            timings.notes_ms
                        ));
                    } else {
                        self.status = StatusMessage::info("正在更新预览…");
                    }
                    match Preview::new(ctx, result, config, source_revision) {
                        Ok(preview) => {
                            self.preview = Some(preview);
                            // Auto updates should keep the user's zoom/pan;
                            // only fit raster previews when first created.
                            if !had_preview {
                                self.key_view.reset_fit();
                                self.palette_view.reset_fit();
                                self.midi_view.reset_fit();
                            }
                        }
                        Err(error) => {
                            tracing::warn!("failed to build stage textures: {error}");
                            self.status =
                                StatusMessage::error(format!("无法创建预览纹理：{error}"));
                        }
                    }
                }
                JobEvent::Failed { message, .. } => {
                    self.job_progress = None;
                    self.job_config = None;
                    self.job_source_revision = None;
                    self.job_source_generation = None;
                    self.job_started = None;
                    tracing::warn!("conversion failed: {message}");
                    self.status = StatusMessage::error(format!("转换失败：{message}"));
                }
                JobEvent::Cancelled { .. } => {
                    self.job_progress = None;
                    self.job_config = None;
                    self.job_source_revision = None;
                    self.job_source_generation = None;
                    self.job_started = None;
                    self.status = StatusMessage::warning("已取消生成");
                }
            }
        }

        if self.jobs.is_running() {
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }

    fn export_dialog(&mut self) {
        let Some(preview) = self.preview.as_ref() else {
            self.status = StatusMessage::warning("暂无可导出的预览");
            return;
        };
        if preview.config != self.settings.conversion_config()
            || preview.source_revision != self.source_revision
        {
            self.status = StatusMessage::warning("预览正在更新，请稍候");
            return;
        }

        let default_name = self
            .source
            .as_ref()
            .and_then(|source| source.path.file_stem())
            .and_then(|stem| stem.to_str())
            .map(|stem| format!("{stem}.mid"))
            .unwrap_or_else(|| "yuetu.mid".to_owned());

        let Some(path) = rfd::FileDialog::new()
            .add_filter("MIDI", &["mid", "midi"])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };

        match yuetu_core::write_midi(&path, &preview.result, &self.settings.midi_config()) {
            Ok(()) => {
                self.status = StatusMessage::success(format!("已导出 {}", path.display()));
                tracing::info!("wrote MIDI to {}", path.display());
            }
            Err(error) => {
                tracing::warn!("failed to write MIDI: {error}");
                self.status = StatusMessage::error(format!("导出失败：{error}"));
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let open = ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::O));
        let export = ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::S));
        let cancel = self.jobs.is_running()
            && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));

        if open {
            self.open_dialog(ctx);
        }
        if export {
            self.export_dialog();
        }
        if cancel {
            self.cancel_conversion();
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect()
        });
        if let Some(path) = dropped.into_iter().next() {
            self.load_path(ctx, path);
        }
    }

    fn preview_is_current(&self) -> bool {
        let config = self.settings.conversion_config();
        self.preview.as_ref().is_some_and(|preview| {
            preview.config == config && preview.source_revision == self.source_revision
        })
    }

    fn clamp_panel_widths(&mut self, total_width: f32) {
        const MIN_CENTER: f32 = 420.0;
        let max_left = (total_width - stage_rail::WIDTH - MIN_CENTER).clamp(220.0, 560.0);
        self.left_panel_width = self.left_panel_width.clamp(220.0, max_left);
    }

    fn current_view_mut(&mut self) -> &mut View2D {
        match self.stage {
            PipelineStage::Source => &mut self.source_view,
            PipelineStage::KeyMap => &mut self.key_view,
            PipelineStage::Palette => &mut self.palette_view,
            PipelineStage::Midi => &mut self.midi_view,
        }
    }

    fn current_zoom_percent(&self) -> f32 {
        match self.stage {
            PipelineStage::Source => self.source_view.absolute_zoom_percent(),
            PipelineStage::KeyMap => self.key_view.absolute_zoom_percent(),
            PipelineStage::Palette => self.palette_view.absolute_zoom_percent(),
            PipelineStage::Midi => self.midi_view.absolute_zoom_percent(),
        }
    }
}

impl eframe::App for YuetuApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_shortcuts(&ctx);
        self.handle_dropped_files(&ctx);
        self.poll_jobs(&ctx);

        let root_rect = ui.max_rect();
        let total_width = root_rect.width();
        self.clamp_panel_widths(total_width);
        let config_before = self.settings.conversion_config();
        let updating = self.jobs.is_running() || self.auto_convert_at.is_some();
        let can_export = self.preview_is_current();

        // The status bar deliberately shows the previous frame's hover;
        // the main text and zoom are updated by actions later in this frame.
        let mut status_action = ui::status_bar::Action::default();
        let zoom_enabled = self.source.is_some()
            && (self.stage == PipelineStage::Source || self.preview.is_some());
        egui::Panel::bottom("status")
            .show_separator_line(false)
            .frame(theme::panel_frame(ui, theme::PanelKind::Bottom))
            .show(ui, |ui| {
                status_action = ui::status_bar::show(
                    ui,
                    &self.status,
                    self.stage,
                    self.stage_hover,
                    self.coordinate_origin,
                    self.current_zoom_percent(),
                    self.zoom_modes[self.stage.index()],
                    zoom_enabled,
                );
            });

        // Reserved strip for the pipeline rail. The dot is drawn afterwards
        // because it needs the card centers from the controls panel.
        egui::Panel::left("stage_rail")
            .exact_size(stage_rail::WIDTH)
            .resizable(false)
            .show_separator_line(false)
            .frame(theme::panel_frame(ui, theme::PanelKind::Side))
            .show(ui, |_ui| {});

        let left_panel = egui::Panel::left("controls")
            .exact_size(self.left_panel_width)
            .resizable(false)
            .show_separator_line(false)
            .frame(theme::panel_frame(ui, theme::PanelKind::Side))
            .show(ui, |ui| {
                ui::controls::show(
                    ui,
                    &mut self.settings,
                    &mut self.coordinate_origin,
                    self.source.as_ref(),
                    self.preview.as_ref(),
                    self.jobs.is_running(),
                    self.job_progress.as_ref(),
                    can_export,
                )
            });
        let controls_output = left_panel.inner;
        let controls_action = controls_output.action;
        self.stage_stops = controls_output.stops;
        let left_rect = left_panel.response.rect;

        let mut acted = false;
        if let Some(stage) = stage_rail::show(&ctx, &self.stage_stops, self.stage, root_rect.left())
        {
            if stage != self.stage {
                self.stage = stage;
                self.stage_hover = None;
                acted = true;
            }
        }

        let checker_texture = self.checker_texture.id();
        let gpu_target_format = self.gpu_target_format;
        let mut stage_action = ui::stage_panel::Action::default();
        let mut frame_strip_output = ui::frame_strip::Output::default();
        egui::CentralPanel::default()
            .frame(theme::panel_frame(ui, theme::PanelKind::Central))
            .show(ui, |ui| {
                // The frame strip lives inside the central panel so it spans
                // only the stage width, directly below the main view. It is
                // absent for still images.
                if let Some(source) = self.source.as_mut() {
                    if source.frame_count() > 1 {
                        let last_frame = &mut self.frame_strip_last_frame;
                        let mode = &mut self.frame_strip_mode;
                        egui::Panel::bottom("frame_strip")
                            .show_separator_line(false)
                            .frame(theme::panel_frame(ui, theme::PanelKind::FrameStrip))
                            .show(ui, |ui| {
                                frame_strip_output =
                                    ui::frame_strip::show(ui, source, last_frame, mode);
                            });
                    }
                }

                let stage = self.stage;
                let source = self.source.as_ref();
                let preview = self.preview.as_ref();
                let progress = self.job_progress.as_ref();
                let view = match stage {
                    PipelineStage::Source => &mut self.source_view,
                    PipelineStage::KeyMap => &mut self.key_view,
                    PipelineStage::Palette => &mut self.palette_view,
                    PipelineStage::Midi => &mut self.midi_view,
                };
                stage_action = ui::stage_panel::show(
                    ui,
                    stage,
                    self.coordinate_origin,
                    source,
                    preview,
                    view,
                    &mut self.stage_hover,
                    updating,
                    progress,
                    checker_texture,
                    gpu_target_format,
                );
            });

        let split_left = split_handle::vertical(
            &ctx,
            "split.left",
            root_rect.left() + stage_rail::WIDTH + self.left_panel_width,
            egui::Rangef::new(left_rect.top(), left_rect.bottom()),
        );
        if split_left.dragged() {
            let max_left = (total_width - stage_rail::WIDTH - 420.0).clamp(220.0, 560.0);
            self.left_panel_width =
                (self.left_panel_width + split_left.drag_delta().x).clamp(220.0, max_left);
            self.clamp_panel_widths(total_width);
            acted = true;
        }
        if split_left.double_clicked() {
            self.left_panel_width = 310.0;
            self.clamp_panel_widths(total_width);
            acted = true;
        }

        if controls_action.open || stage_action.open {
            self.open_dialog(&ctx);
            acted = true;
        }
        if let Some(frame) = controls_action.frame {
            acted |= self.select_source_frame(&ctx, frame, false);
        }
        if let Some(frame) = frame_strip_output.frame {
            acted |= self.select_source_frame(&ctx, frame, frame_strip_output.scrubbing);
        }
        if frame_strip_output.scrubbing {
            self.frame_scrub_pending = true;
        }
        if let Some(degrees) = controls_action.rotation {
            if let Some(source) = self.source.as_mut() {
                match source.set_rotation(&ctx, degrees) {
                    Ok(true) => {
                        self.settings.rotation_deg = source.rotation_deg();
                        self.source_revision = self.source_revision.wrapping_add(1);
                        self.stage_hover = None;
                        self.source_view.reset_fit();
                        self.status = StatusMessage::info("正在更新预览…");
                        self.schedule_auto_conversion(&ctx, AUTO_CONVERT_DEBOUNCE);
                        acted = true;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        self.settings.rotation_deg = source.rotation_deg();
                        self.status = StatusMessage::error(format!("旋转失败：{error}"));
                    }
                }
            }
        }
        if controls_action.reset {
            self.settings = Settings::default();
            self.coordinate_origin = CoordinateOrigin::TopLeft;
            if let Some(source) = self.source.as_mut() {
                match source.set_rotation(&ctx, 0.0) {
                    Ok(true) => {
                        self.source_revision = self.source_revision.wrapping_add(1);
                        self.source_view.reset_fit();
                        self.stage_hover = None;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        self.status = StatusMessage::error(format!("重置旋转失败：{error}"));
                    }
                }
            }
            self.schedule_auto_conversion(&ctx, AUTO_CONVERT_DEBOUNCE);
            acted = true;
        }
        if controls_action.export {
            self.export_dialog();
            acted = true;
        }
        if controls_action.cancel {
            self.cancel_conversion();
            acted = true;
        }
        if let Some(preset) = status_action.zoom_preset {
            match preset {
                ZoomPreset::Fit => self.current_view_mut().reset_fit(),
                ZoomPreset::Fill => self.current_view_mut().fill(),
                ZoomPreset::Actual => self.current_view_mut().set_absolute_zoom_percent(100.0),
            }
            self.zoom_modes[self.stage.index()] = preset;
            acted = true;
        }
        if let Some(percent) = status_action.set_zoom_percent {
            self.current_view_mut().set_absolute_zoom_percent(percent);
            acted = true;
        }

        if self.frame_scrub_pending && !ctx.input(|input| input.pointer.primary_down()) {
            self.frame_scrub_pending = false;
            self.status = StatusMessage::info("正在更新预览…");
            self.schedule_auto_conversion(&ctx, AUTO_CONVERT_DEBOUNCE);
            acted = true;
        }

        if self.source.is_some() && self.settings.conversion_config() != config_before {
            self.status = StatusMessage::info("正在更新预览…");
            self.schedule_auto_conversion(&ctx, AUTO_CONVERT_DEBOUNCE);
            acted = true;
        }
        self.tick_auto_conversion(&ctx);

        if acted {
            ctx.request_repaint();
        }
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }
}

fn keymap_config_eq(a: &ConversionConfig, b: &ConversionConfig) -> bool {
    a.key_start == b.key_start
        && a.key_end == b.key_end
        && a.key_filter == b.key_filter
        && a.column_mapping == b.column_mapping
}

fn palette_config_eq(a: &ConversionConfig, b: &ConversionConfig) -> bool {
    a.palette_colors == b.palette_colors
        && a.manual_palette == b.manual_palette
        && a.palette_scope == b.palette_scope
}

fn transform_config_eq(a: &ConversionConfig, b: &ConversionConfig) -> bool {
    keymap_config_eq(a, b)
        && a.resize == b.resize
        && a.rotation_deg == b.rotation_deg
        && a.target_height == b.target_height
}
