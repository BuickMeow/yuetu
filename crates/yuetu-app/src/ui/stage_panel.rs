use egui::{RichText, Ui};
use egui_extras::{Size, StripBuilder};
use yuetu_core::{Color, MidiOutputConfig, TimeAxis};

use crate::jobs::ProgressView;
use crate::state::{PipelineStage, Preview, SourceImage, StageHover, StageSnapshot};
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
    source: Option<&SourceImage>,
    preview: Option<&Preview>,
    midi_config: &MidiOutputConfig,
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
            let hovered = if let Some(vector) = source.vector() {
                let ctx = ui.ctx().clone();
                let rotation = source.rotation_deg();
                stage_image::show_with_texture(
                    ui,
                    view,
                    source.display_size,
                    source.display_size,
                    true,
                    checker_texture,
                    |interaction| {
                        vector
                            .draw(
                                &ctx,
                                interaction.content_rect,
                                interaction.viewport,
                                source.display_size,
                                rotation,
                            )
                            .map(|draw| stage_image::TextureDraw {
                                texture: draw.texture,
                                rect: draw.rect,
                            })
                    },
                )
            } else {
                stage_image::show(
                    ui,
                    view,
                    source.texture.id(),
                    source.display_size,
                    source.display_size,
                    source.has_transparency,
                    checker_texture,
                )
            };
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
                    let (texture, size, time_axis, transparency, image, keymap) =
                        if let Some(StageSnapshot::KeyMap {
                            time_axis,
                            resized_image,
                            resized_texture,
                            resized_has_transparency,
                            resized_size,
                            keymap,
                        }) = preview.interaction.as_ref()
                        {
                            (
                                resized_texture.id(),
                                *resized_size,
                                *time_axis,
                                *resized_has_transparency,
                                resized_image,
                                keymap,
                            )
                        } else {
                            (
                                preview.resized_texture.id(),
                                preview.result.resized_size,
                                preview.result.time_axis,
                                preview.resized_has_transparency,
                                &preview.result.resized_image,
                                &preview.result.keymap,
                            )
                        };
                    let hovered = stage_image::show(
                        ui,
                        view,
                        texture,
                        source.display_size,
                        size,
                        transparency,
                        checker_texture,
                    );
                    *hover = hovered.map(|pixel| {
                        let key_position = match time_axis {
                            TimeAxis::Vertical => pixel.x,
                            TimeAxis::Horizontal => {
                                size.1.saturating_sub(1).saturating_sub(pixel.y)
                            }
                        };
                        StageHover::KeyMap {
                            pos: (pixel.x, pixel.y),
                            color: image.pixel(pixel.x, pixel.y).unwrap_or(Color::TRANSPARENT),
                            key: keymap.key_at(key_position),
                            height: size.1,
                            time_axis,
                        }
                    });
                }
                PipelineStage::Palette => {
                    let (texture, size, transparency, indexed, palette) =
                        if let Some(StageSnapshot::Palette {
                            indexed_image,
                            indexed_texture,
                            indexed_has_transparency,
                            palette,
                        }) = preview.interaction.as_ref()
                        {
                            (
                                indexed_texture.id(),
                                (indexed_image.width, indexed_image.height),
                                *indexed_has_transparency,
                                indexed_image,
                                palette,
                            )
                        } else {
                            (
                                preview.indexed_texture.id(),
                                (
                                    preview.result.indexed_image.width,
                                    preview.result.indexed_image.height,
                                ),
                                preview.indexed_has_transparency,
                                &preview.result.indexed_image,
                                &preview.result.palette,
                            )
                        };
                    let hovered = stage_image::show(
                        ui,
                        view,
                        texture,
                        source.display_size,
                        size,
                        transparency,
                        checker_texture,
                    );
                    *hover = hovered.map(|pixel| {
                        let index = indexed.at(pixel.x, pixel.y);
                        let color = if index >= 0 {
                            palette
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
                        midi_config,
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
