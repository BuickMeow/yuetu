#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod jobs;
mod settings;
mod state;
mod theme;
mod ui;
mod widgets;

use std::path::PathBuf;

use tracing_subscriber::EnvFilter;

fn main() -> eframe::Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,wgpu=warn,naga=warn"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();

    let startup_image = std::env::args_os().nth(1).map(PathBuf::from);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([1040.0, 640.0])
            .with_title("月兔"),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native(
        "月兔",
        options,
        Box::new(move |cc| Ok(Box::new(app::YuetuApp::new(cc, startup_image.clone())))),
    )
}
