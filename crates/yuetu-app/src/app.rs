use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Key, Modifiers};
use yuetu_core::{ConversionConfig, Stage};

use crate::jobs::{JobEvent, JobRunner, ProgressView};
use crate::settings::Settings;
use crate::state::{Preview, PreviewHover, SourceHover, SourceImage, StatusMessage};
use crate::theme;
use crate::ui;
use crate::widgets::{View2D, split_handle};

/// Wait this long after the last parameter edit before starting a new job.
/// It keeps a slider drag from spawning one conversion per frame while still
/// feeling immediate.
const AUTO_CONVERT_DEBOUNCE: Duration = Duration::from_millis(80);

pub struct YuetuApp {
    source: Option<SourceImage>,
    /// Increments whenever the source pixels change (new image or GIF frame).
    source_revision: u64,
    preview: Option<Preview>,
    settings: Settings,

    image_view: View2D,
    preview_view: View2D,

    status: StatusMessage,
    source_hover: Option<SourceHover>,
    preview_hover: Option<PreviewHover>,

    jobs: JobRunner,
    job_progress: Option<ProgressView>,
    job_config: Option<ConversionConfig>,
    job_source_revision: Option<u64>,
    job_started: Option<Instant>,
    /// When the next automatic conversion should start. `None` means no
    /// parameters changed since the last result/job.
    auto_convert_at: Option<Instant>,

    left_panel_width: f32,
    right_panel_width: f32,
}

impl YuetuApp {
    pub fn new(cc: &eframe::CreationContext<'_>, startup_image: Option<PathBuf>) -> Self {
        theme::apply(&cc.egui_ctx);

        let mut app = Self {
            source: None,
            source_revision: 0,
            preview: None,
            settings: Settings::default(),
            image_view: View2D::default(),
            preview_view: View2D::default(),
            status: StatusMessage::default(),
            source_hover: None,
            preview_hover: None,
            jobs: JobRunner::new(),
            job_progress: None,
            job_config: None,
            job_source_revision: None,
            job_started: None,
            auto_convert_at: None,
            left_panel_width: 310.0,
            right_panel_width: 560.0,
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
        self.job_started = None;
        self.auto_convert_at = None;

        match SourceImage::load(ctx, path.clone()) {
            Ok(source) => {
                self.status = StatusMessage::success(format!(
                    "已加载 {} · {} × {} px",
                    source.name, source.image.width, source.image.height
                ));
                self.source = Some(source);
                self.source_revision = self.source_revision.wrapping_add(1);
                self.preview = None;
                self.source_hover = None;
                self.preview_hover = None;
                self.image_view.reset_fit();
                self.preview_view.reset_fit();
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
        match self.jobs.start(source.image.clone(), config.clone()) {
            Ok(_) => {
                self.job_config = Some(config);
                self.job_source_revision = Some(self.source_revision);
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
                self.job_started = None;
                self.status = StatusMessage::error(format!("无法启动转换任务：{error}"));
            }
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
        self.job_progress = None;
        self.job_config = None;
        self.job_source_revision = None;
        self.job_started = None;
        self.status = StatusMessage::warning("已取消生成");
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
                    let current = self.settings.conversion_config();
                    let had_preview = self.preview.is_some();
                    self.job_progress = None;
                    if config == current && source_revision == self.source_revision {
                        self.status = StatusMessage::success(format!(
                            "生成完成：{} 音符 · {} 音轨 · 用时 {:.1} ms",
                            result.note_count(),
                            result.track_count(),
                            elapsed.as_secs_f64() * 1000.0
                        ));
                    } else {
                        self.status = StatusMessage::info("正在更新预览…");
                    }
                    self.preview = Some(Preview {
                        result,
                        config,
                        source_revision,
                    });
                    // Auto updates should keep the user's zoom/pan; only fit a
                    // newly displayed preview.
                    if !had_preview {
                        self.preview_view.reset_fit();
                    }
                }
                JobEvent::Failed { message, .. } => {
                    self.job_progress = None;
                    self.job_config = None;
                    self.job_source_revision = None;
                    self.job_started = None;
                    tracing::warn!("conversion failed: {message}");
                    self.status = StatusMessage::error(format!("转换失败：{message}"));
                }
                JobEvent::Cancelled { .. } => {
                    self.job_progress = None;
                    self.job_config = None;
                    self.job_source_revision = None;
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
        const MIN_CENTER: f32 = 320.0;
        let max_left = (total_width - self.right_panel_width - MIN_CENTER).clamp(220.0, 520.0);
        self.left_panel_width = self.left_panel_width.clamp(220.0, max_left);
        let max_right = (total_width - self.left_panel_width - MIN_CENTER).clamp(300.0, 900.0);
        self.right_panel_width = self.right_panel_width.clamp(300.0, max_right);
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

        let mut toolbar_action = ui::toolbar::Action::default();
        egui::Panel::top("toolbar")
            .show_separator_line(false)
            .frame(theme::panel_frame(ui, theme::PanelKind::Top))
            .show(ui, |ui| {
                toolbar_action =
                    ui::toolbar::show(ui, self.preview_is_current(), self.jobs.is_running());
            });

        // Reserve the status bar before the central panel. It intentionally
        // shows the previous frame's zoom/hover values; the status text itself
        // is updated by actions below and repainted immediately.
        let status_hover_source = self.source_hover;
        let status_hover_preview = self.preview_hover;
        egui::Panel::bottom("status")
            .show_separator_line(false)
            .frame(theme::panel_frame(ui, theme::PanelKind::Bottom))
            .show(ui, |ui| {
                ui::status_bar::show(
                    ui,
                    &self.status,
                    status_hover_source,
                    status_hover_preview,
                    self.image_view.relative_zoom(),
                    self.preview_view.relative_zoom(),
                );
            });

        let mut controls_action = ui::controls::Action::default();
        let can_export = self.preview_is_current();
        let left_panel = egui::Panel::left("controls")
            .exact_size(self.left_panel_width)
            .resizable(false)
            .show_separator_line(false)
            .frame(theme::panel_frame(ui, theme::PanelKind::Side))
            .show(ui, |ui| {
                controls_action = ui::controls::show(
                    ui,
                    &mut self.settings,
                    self.source.as_ref(),
                    self.preview.as_ref(),
                    self.jobs.is_running(),
                    self.job_progress.as_ref(),
                    can_export,
                );
            });
        let left_rect = left_panel.response.rect;

        let right_panel = egui::Panel::right("preview")
            .exact_size(self.right_panel_width)
            .resizable(false)
            .show_separator_line(false)
            .frame(theme::panel_frame(ui, theme::PanelKind::Side))
            .show(ui, |ui| {
                ui::preview_panel::show(
                    ui,
                    self.preview.as_ref(),
                    updating,
                    self.job_progress.as_ref(),
                    &mut self.preview_view,
                    &mut self.preview_hover,
                );
            });
        let right_rect = right_panel.response.rect;

        let mut source_action = ui::source_panel::Action::default();
        egui::CentralPanel::default()
            .frame(theme::panel_frame(ui, theme::PanelKind::Central))
            .show(ui, |ui| {
                source_action = ui::source_panel::show(
                    ui,
                    self.source.as_ref(),
                    &mut self.image_view,
                    &mut self.source_hover,
                );
            });

        let mut acted = false;

        let split_left = split_handle::vertical(
            &ctx,
            "split.left",
            root_rect.left() + self.left_panel_width,
            egui::Rangef::new(left_rect.top(), left_rect.bottom()),
        );
        if split_left.dragged() {
            let max_left = (total_width - self.right_panel_width - 320.0).clamp(220.0, 520.0);
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

        let split_right = split_handle::vertical(
            &ctx,
            "split.right",
            root_rect.right() - self.right_panel_width,
            egui::Rangef::new(right_rect.top(), right_rect.bottom()),
        );
        if split_right.dragged() {
            let max_right = (total_width - self.left_panel_width - 320.0).clamp(300.0, 900.0);
            self.right_panel_width =
                (self.right_panel_width - split_right.drag_delta().x).clamp(300.0, max_right);
            self.clamp_panel_widths(total_width);
            acted = true;
        }
        if split_right.double_clicked() {
            self.right_panel_width = 560.0;
            self.clamp_panel_widths(total_width);
            acted = true;
        }

        if toolbar_action.open || controls_action.open || source_action.open {
            self.open_dialog(&ctx);
            acted = true;
        }
        if let Some(frame) = controls_action.frame {
            if let Some(source) = self.source.as_mut() {
                match source.select_frame(&ctx, frame) {
                    Ok(true) => {
                        self.source_revision = self.source_revision.wrapping_add(1);
                        self.source_hover = None;
                        self.preview_hover = None;
                        self.status = StatusMessage::info("正在更新预览…");
                        self.schedule_auto_conversion(&ctx, AUTO_CONVERT_DEBOUNCE);
                        acted = true;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        self.status = StatusMessage::error(format!("切换帧失败：{error}"));
                    }
                }
            }
        }
        if toolbar_action.export || controls_action.export {
            self.export_dialog();
            acted = true;
        }
        if toolbar_action.cancel || controls_action.cancel {
            self.cancel_conversion();
            acted = true;
        }
        if toolbar_action.fit {
            self.image_view.reset_fit();
            self.preview_view.reset_fit();
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
