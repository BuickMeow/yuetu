use egui::{RichText, Ui};

use crate::jobs::ProgressView;
use crate::settings::Settings;
use crate::state::{Preview, SourceImage};
use crate::theme;
use crate::ui::{card, section_title};

#[derive(Clone, Copy, Debug, Default)]
pub struct Action {
    pub open: bool,
    pub convert: bool,
    pub cancel: bool,
    pub export: bool,
}

pub fn show(
    ui: &mut Ui,
    settings: &mut Settings,
    source: Option<&SourceImage>,
    preview: Option<&Preview>,
    is_running: bool,
    progress: Option<&ProgressView>,
) -> Action {
    let mut action = Action::default();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            card(ui, |ui| {
                section_title(ui, "源图");
                if let Some(source) = source {
                    ui.label(RichText::new(source.name.as_str()).strong());
                    ui.label(format!(
                        "{} × {} px",
                        source.image.width, source.image.height
                    ));
                } else {
                    ui.label("尚未打开图片");
                }
                if ui.button("打开图片…").clicked() {
                    action.open = true;
                }
            });

            ui.add_space(10.0);
            card(ui, |ui| {
                section_title(ui, "键位映射");
                ui.horizontal(|ui| {
                    ui.label("起始键");
                    ui.add(
                        egui::DragValue::new(&mut settings.key_start)
                            .range(0..=127)
                            .speed(1.0),
                    );
                    ui.label("结束键");
                    ui.add(
                        egui::DragValue::new(&mut settings.key_end)
                            .range(0..=127)
                            .speed(1.0),
                    );
                });
                if settings.key_start > settings.key_end {
                    settings.key_end = settings.key_start;
                }

                ui.horizontal(|ui| {
                    ui.label("键过滤");
                    egui::ComboBox::from_id_salt("key_filter")
                        .selected_text(settings.key_filter.label())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut settings.key_filter,
                                yuetu_core::KeyFilter::All,
                                yuetu_core::KeyFilter::All.label(),
                            );
                            ui.selectable_value(
                                &mut settings.key_filter,
                                yuetu_core::KeyFilter::White,
                                yuetu_core::KeyFilter::White.label(),
                            );
                            ui.selectable_value(
                                &mut settings.key_filter,
                                yuetu_core::KeyFilter::Black,
                                yuetu_core::KeyFilter::Black.label(),
                            );
                        });
                });

                ui.horizontal(|ui| {
                    ui.label("列映射");
                    egui::ComboBox::from_id_salt("column_mapping")
                        .selected_text(settings.column_mapping.label())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut settings.column_mapping,
                                yuetu_core::ColumnMapping::Filled,
                                yuetu_core::ColumnMapping::Filled.label(),
                            );
                            ui.selectable_value(
                                &mut settings.column_mapping,
                                yuetu_core::ColumnMapping::Clipped,
                                yuetu_core::ColumnMapping::Clipped.label(),
                            );
                        });
                });
            });

            ui.add_space(10.0);
            card(ui, |ui| {
                section_title(ui, "调色板与缩放");
                ui.horizontal(|ui| {
                    ui.label("颜色数");
                    ui.add(
                        egui::DragValue::new(&mut settings.palette_colors)
                            .range(1..=256)
                            .speed(1.0),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("缩放算法");
                    egui::ComboBox::from_id_salt("resize_algorithm")
                        .selected_text(settings.resize.label())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut settings.resize,
                                yuetu_core::ResizeAlgorithm::Area,
                                yuetu_core::ResizeAlgorithm::Area.label(),
                            );
                            ui.selectable_value(
                                &mut settings.resize,
                                yuetu_core::ResizeAlgorithm::Nearest,
                                yuetu_core::ResizeAlgorithm::Nearest.label(),
                            );
                        });
                });

                if let Some(preview) = preview {
                    ui.add_space(4.0);
                    palette_swatches(ui, preview);
                }
            });

            ui.add_space(10.0);
            card(ui, |ui| {
                section_title(ui, "音符长度");
                ui.checkbox(&mut settings.limit_note_length, "限制最大音符长度");
                if settings.limit_note_length {
                    ui.horizontal(|ui| {
                        ui.label("最长");
                        ui.add(
                            egui::DragValue::new(&mut settings.max_note_length)
                                .range(1..=65_535)
                                .suffix(" 行"),
                        );
                    });
                    ui.radio_value(
                        &mut settings.measure_from_image_start,
                        false,
                        "按持有时长重触发",
                    );
                    ui.radio_value(
                        &mut settings.measure_from_image_start,
                        true,
                        "按图像起始网格切分",
                    );
                }
            });

            ui.add_space(10.0);
            card(ui, |ui| {
                section_title(ui, "MIDI 输出");
                ui.horizontal(|ui| {
                    ui.label("PPQ");
                    ui.add(egui::DragValue::new(&mut settings.ppq).range(1..=0x7FFF));
                });
                ui.horizontal(|ui| {
                    ui.label("Ticks / px");
                    ui.add(egui::DragValue::new(&mut settings.ticks_per_pixel).range(1..=0xFFFF));
                });
                ui.horizontal(|ui| {
                    ui.label("BPM");
                    ui.add(egui::DragValue::new(&mut settings.bpm).range(1..=1000));
                });
                ui.horizontal(|ui| {
                    ui.label("起始偏移");
                    ui.add(egui::DragValue::new(&mut settings.start_offset).speed(10.0));
                });
                ui.checkbox(&mut settings.emit_color_events, "写入颜色 Meta 事件");
            });

            ui.add_space(12.0);
            if source.is_some() {
                if is_running {
                    let text = if let Some(progress) = progress {
                        format!(
                            "正在生成：{} {:.0}%",
                            progress.label(),
                            progress.overall * 100.0
                        )
                    } else {
                        "正在生成…".to_owned()
                    };
                    ui.label(RichText::new(text).color(theme::accent(ui)));
                    let cancel =
                        egui::Button::new(RichText::new("取消生成").color(theme::danger(ui)))
                            .min_size(egui::vec2(ui.available_width(), 32.0));
                    if ui.add(cancel).clicked() {
                        action.cancel = true;
                    }
                } else {
                    let enabled = settings.validate().is_ok();
                    let button = egui::Button::new(RichText::new("生成 MIDI 预览").strong())
                        .fill(theme::primary_fill(ui))
                        .min_size(egui::vec2(ui.available_width(), 34.0));
                    if ui.add_enabled(enabled, button).clicked() {
                        action.convert = true;
                    }
                    if !enabled {
                        ui.label(
                            RichText::new(settings.validate().unwrap_err())
                                .color(theme::danger(ui)),
                        );
                    }
                }

                let export = egui::Button::new("导出 MIDI…")
                    .min_size(egui::vec2(ui.available_width(), 30.0));
                if ui.add_enabled(preview.is_some(), export).clicked() {
                    action.export = true;
                }
            }

            ui.add_space(8.0);
            if ui.button("重置全部参数").clicked() {
                *settings = Settings::default();
            }
        });

    action
}

fn palette_swatches(ui: &mut Ui, preview: &Preview) {
    ui.horizontal_wrapped(|ui| {
        for (index, color) in preview.result.palette.colors.iter().enumerate() {
            let color32 = egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a);
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 3.0, color32);
            response.on_hover_text(format!("音轨 {index} · {}", color.hex()));
        }
    });
}
