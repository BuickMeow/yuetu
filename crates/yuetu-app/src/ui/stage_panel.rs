use egui::{RichText, Ui};
use egui_extras::{Size, StripBuilder};
use yuetu_core::Color;

use crate::jobs::ProgressView;
use crate::state::{CoordinateOrigin, PipelineStage, Preview, SourceImage, StageHover};
use crate::theme;
use crate::widgets::{View2D, midi_preview, stage_image};

#[derive(Clone, Copy, Debug, Default)]
pub struct Action {
    pub open: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    stage: PipelineStage,
    coordinate_origin: CoordinateOrigin,
    source: Option<&SourceImage>,
    preview: Option<&Preview>,
    view: &mut View2D,
    hover: &mut Option<StageHover>,
    updating: bool,
    progress: Option<&ProgressView>,
    checker_texture: egui::TextureId,
    gpu_target_format: eframe::wgpu::TextureFormat,
) -> Action {
    let mut action = Action::default();

    let Some(source) = source else {
        *hover = None;
        empty_source(ui, &mut action);
        return action;
    };

    match stage {
        PipelineStage::Source => {
            let hovered = stage_image::show(
                ui,
                view,
                source.texture.id(),
                source.display_size,
                source.has_transparency,
                checker_texture,
                coordinate_origin,
            );
            *hover = hovered.map(|pixel| StageHover::Source {
                pos: (pixel.x, pixel.y),
                color: source.display_pixel(pixel.x, pixel.y),
                height: source.display_size.1,
            });
        }
        PipelineStage::KeyMap | PipelineStage::Palette | PipelineStage::Midi => {
            let Some(preview) = preview else {
                *hover = None;
                waiting(ui, updating, progress);
                return action;
            };

            match stage {
                PipelineStage::KeyMap => {
                    let hovered = stage_image::show(
                        ui,
                        view,
                        preview.resized_texture.id(),
                        preview.result.resized_size,
                        preview.resized_has_transparency,
                        checker_texture,
                        coordinate_origin,
                    );
                    *hover = hovered.map(|pixel| StageHover::KeyMap {
                        pos: (pixel.x, pixel.y),
                        color: preview
                            .result
                            .resized_image
                            .pixel(pixel.x, pixel.y)
                            .unwrap_or(Color::TRANSPARENT),
                        key: preview.result.keymap.key_at(pixel.x),
                        height: preview.result.resized_size.1,
                    });
                }
                PipelineStage::Palette => {
                    let hovered = stage_image::show(
                        ui,
                        view,
                        preview.indexed_texture.id(),
                        (
                            preview.result.indexed_image.width,
                            preview.result.indexed_image.height,
                        ),
                        preview.indexed_has_transparency,
                        checker_texture,
                        coordinate_origin,
                    );
                    *hover = hovered.map(|pixel| {
                        let index = preview.result.indexed_image.at(pixel.x, pixel.y);
                        let color = if index >= 0 {
                            preview
                                .result
                                .palette
                                .colors
                                .get(index as usize)
                                .copied()
                                .unwrap_or(Color::TRANSPARENT)
                        } else {
                            Color::TRANSPARENT
                        };
                        StageHover::Palette { index, color }
                    });
                }
                PipelineStage::Midi => {
                    let output = midi_preview::show(
                        ui,
                        &preview.result,
                        &preview.midi_gpu,
                        view,
                        gpu_target_format,
                    );
                    *hover = output.hovered;
                }
                PipelineStage::Source => unreachable!(),
            }
        }
    }

    action
}

fn empty_source(ui: &mut Ui, action: &mut Action) {
    let rect = ui.available_rect_before_wrap();
    theme::view_frame(ui).show(ui, |ui| {
        let min = (rect.size() - egui::vec2(24.0, 24.0)).max(egui::vec2(1.0, 1.0));
        ui.set_min_size(min);
        centered_content(ui, 78.0, |ui| {
            ui.label(RichText::new("把图片拖进来").size(22.0));
            ui.add_space(10.0);
            if ui
                .add(
                    egui::Button::new(RichText::new("选择图片…").strong())
                        .fill(theme::primary_fill(ui))
                        .min_size(egui::vec2(150.0, 36.0)),
                )
                .clicked()
            {
                action.open = true;
            }
        });
    });
}

fn centered_content(ui: &mut Ui, height: f32, add: impl FnOnce(&mut Ui)) {
    StripBuilder::new(ui)
        .size(Size::remainder())
        .size(Size::exact(height))
        .size(Size::remainder())
        .vertical(|mut strip| {
            strip.cell(|_| {});
            strip.cell(|ui| {
                ui.vertical_centered(add);
            });
            strip.cell(|_| {});
        });
}

fn waiting(ui: &mut Ui, updating: bool, progress: Option<&ProgressView>) {
    let rect = ui.available_rect_before_wrap();
    theme::view_frame(ui).show(ui, |ui| {
        let min = (rect.size() - egui::vec2(24.0, 24.0)).max(egui::vec2(1.0, 1.0));
        ui.set_min_size(min);
        let height = if progress.is_some() { 54.0 } else { 28.0 };
        centered_content(ui, height, |ui| {
            ui.label(
                RichText::new(if updating {
                    "正在生成…"
                } else {
                    "暂无预览"
                })
                .size(15.0),
            );
            if let Some(progress) = progress {
                ui.add_space(6.0);
                ui.add(
                    egui::ProgressBar::new(progress.overall)
                        .desired_width(260.0)
                        .text(format!(
                            "{} {:.0}%",
                            progress.label(),
                            progress.overall * 100.0
                        ))
                        .animate(true),
                );
            }
        });
    });
}
