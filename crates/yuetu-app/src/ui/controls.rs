use crate::jobs::ProgressView;
use crate::settings::{LengthMode, Settings};
use crate::state::{PipelineStage, Preview, SourceImage, StageStop};
use crate::theme;
use crate::ui::{card_with_rect, section_title, setting_row};
use egui::{RichText, Ui};
use yuetu_core::{KeyMap, TimeAxis};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveSetting {
    KeyMap,
    TimeGrid,
    Palette,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Action {
    pub open: bool,
    pub cancel: bool,
    pub export: bool,
    pub frame: Option<usize>,
    pub rotation: Option<f32>,
    pub rotation_dragging: bool,
    /// Conversion setting currently being dragged. The app suspends jobs for
    /// the gesture; key-map/time-grid changes also refresh the interaction
    /// preview without rebuilding the palette.
    pub live_setting: Option<LiveSetting>,
    pub reset: bool,
}

pub struct Output {
    pub action: Action,
    pub stops: Vec<StageStop>,
}

#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    settings: &mut Settings,
    source: Option<&SourceImage>,
    preview: Option<&Preview>,
    is_running: bool,
    progress: Option<&ProgressView>,
    can_export: bool,
) -> Output {
    let mut action = Action::default();
    let mut stops = Vec::with_capacity(4);
    let source_size = source.map(|source| source.display_size).unwrap_or((1, 1));

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (_, rect) = card_with_rect(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("源图").strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("打开图片…").clicked() {
                            action.open = true;
                        }
                    });
                });
                ui.add_space(2.0);

                if let Some(source) = source {
                    ui.label(RichText::new(source.name.as_str()).strong());
                    if source.frame_count() > 1 {
                        ui.label(format!(
                            "{} × {} px · {} 帧",
                            source.display_size.0,
                            source.display_size.1,
                            source.frame_count()
                        ));
                        setting_row(ui, "帧", |ui| {
                            let count = source.frame_count() as u32;
                            let mut frame = source.frame_index() as u32 + 1;
                            let response = ui.add(
                                egui::DragValue::new(&mut frame)
                                    .range(1..=count)
                                    .speed(1.0)
                                    .suffix(format!(" / {count}")),
                            );
                            if response.changed() {
                                action.frame = Some(frame.saturating_sub(1) as usize);
                            }
                        });
                    } else {
                        ui.label(format!(
                            "{} × {} px",
                            source.display_size.0, source.display_size.1
                        ));
                    }

                    setting_row(ui, "旋转", |ui| {
                        let response = ui.add(
                            egui::DragValue::new(&mut settings.rotation_deg)
                                .speed(1.0)
                                .max_decimals(1)
                                .custom_formatter(|value, _| {
                                    format!("{:.1}", value.rem_euclid(360.0))
                                })
                                .suffix("°"),
                        );
                        action.rotation_dragging |= response.dragged();
                        if response.changed() {
                            action.rotation = Some(settings.rotation_deg);
                        }
                    });
                } else {
                    ui.label("尚未打开图片");
                }
            });
            stops.push(StageStop {
                stage: PipelineStage::Source,
                center_y: rect.center().y,
            });

            ui.add_space(10.0);
            let (_, rect) = card_with_rect(ui, |ui| {
                section_title(ui, "键位映射");
                setting_row(ui, "起始键", |ui| {
                    let response = ui.add(
                        egui::DragValue::new(&mut settings.key_start)
                            .range(0..=127)
                            .speed(1.0),
                    );
                    if response.dragged() {
                        action.live_setting = Some(LiveSetting::KeyMap);
                    }
                });
                setting_row(ui, "结束键", |ui| {
                    let response = ui.add(
                        egui::DragValue::new(&mut settings.key_end)
                            .range(0..=127)
                            .speed(1.0),
                    );
                    if response.dragged() {
                        action.live_setting = Some(LiveSetting::KeyMap);
                    }
                });
                if settings.key_start > settings.key_end {
                    settings.key_end = settings.key_start;
                }

                setting_row(ui, "键过滤", |ui| {
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

                setting_row(ui, "键位填充", |ui| {
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

                setting_row(ui, "时间方向", |ui| {
                    egui::ComboBox::from_id_salt("time_axis")
                        .selected_text(settings.time_axis.label())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut settings.time_axis,
                                TimeAxis::Vertical,
                                TimeAxis::Vertical.label(),
                            );
                            ui.selectable_value(
                                &mut settings.time_axis,
                                TimeAxis::Horizontal,
                                TimeAxis::Horizontal.label(),
                            );
                        });
                });

                let key_count = KeyMap::build(
                    settings.key_start.min(settings.key_end),
                    settings.key_end.max(settings.key_start),
                    settings.key_filter,
                    settings.column_mapping,
                )
                .map(|keymap| keymap.width())
                .unwrap_or(0);
                let current_rows = settings.time_rows(source_size);

                setting_row(ui, "长度", |ui| {
                    let previous = settings.length_mode;
                    egui::ComboBox::from_id_salt("time_length_mode")
                        .selected_text(settings.length_mode.label())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut settings.length_mode,
                                LengthMode::Source,
                                LengthMode::Source.label(),
                            );
                            ui.selectable_value(
                                &mut settings.length_mode,
                                LengthMode::Square,
                                LengthMode::Square.label(),
                            );
                            ui.selectable_value(
                                &mut settings.length_mode,
                                LengthMode::Fixed,
                                LengthMode::Fixed.label(),
                            );
                        });
                    if settings.length_mode == LengthMode::Fixed && previous != LengthMode::Fixed {
                        settings.fixed_rows = current_rows.max(1);
                    }
                });
                if settings.length_mode == LengthMode::Fixed {
                    setting_row(ui, "固定高度", |ui| {
                        let response = ui.add(
                            egui::DragValue::new(&mut settings.fixed_rows)
                                .range(1..=65_535)
                                .speed(1.0)
                                .suffix(" 行"),
                        );
                        if response.dragged() {
                            action.live_setting = Some(LiveSetting::TimeGrid);
                        }
                    });
                }

                setting_row(ui, "PPQ", |ui| {
                    ui.add(egui::DragValue::new(&mut settings.ppq).range(1..=0x7FFF));
                });
                setting_row(ui, "每行 Ticks", |ui| {
                    ui.add(egui::DragValue::new(&mut settings.ticks_per_pixel).range(1..=0xFFFF));
                });
                setting_row(ui, "拍号", |ui| {
                    ui.horizontal(|ui| {
                        // The row layout is right-to-left, so add the rightmost
                        // widget first to keep the visual order "numerator / denominator".
                        egui::ComboBox::from_id_salt("time_signature_denominator")
                            .selected_text(settings.time_signature.denominator.to_string())
                            .show_ui(ui, |ui| {
                                for denominator in [1, 2, 4, 8, 16, 32] {
                                    ui.selectable_value(
                                        &mut settings.time_signature.denominator,
                                        denominator,
                                        denominator.to_string(),
                                    );
                                }
                            });
                        ui.label("/");
                        ui.add(
                            egui::DragValue::new(&mut settings.time_signature.numerator)
                                .range(1..=32)
                                .speed(1.0),
                        );
                    });
                });

                settings.time_signature = settings.valid_time_signature();
                let rows = settings.time_rows(source_size);
                let total_ticks = settings.total_ticks(source_size);
                let bars = settings.bar_count(source_size);
                ui.label(
                    RichText::new(format!(
                        "{key_count} 键 × {rows} 行 · {total_ticks} ticks · {bars:.2} 小节"
                    ))
                    .color(theme::muted(ui)),
                );

                setting_row(ui, "缩放算法", |ui| {
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
            });
            stops.push(StageStop {
                stage: PipelineStage::KeyMap,
                center_y: rect.center().y,
            });

            ui.add_space(10.0);
            let (_, rect) = card_with_rect(ui, |ui| {
                section_title(ui, "调色板");
                setting_row(ui, "颜色数", |ui| {
                    let response = ui.add(
                        egui::DragValue::new(&mut settings.palette_colors)
                            .range(1..=256)
                            .speed(1.0),
                    );
                    if response.dragged() {
                        action.live_setting = Some(LiveSetting::Palette);
                    }
                });
                if let Some(preview) = preview {
                    ui.add_space(4.0);
                    palette_swatches(ui, preview);
                }
            });
            stops.push(StageStop {
                stage: PipelineStage::Palette,
                center_y: rect.center().y,
            });

            ui.add_space(10.0);
            let (_, rect) = card_with_rect(ui, |ui| {
                section_title(ui, "MIDI 生成");

                setting_row(ui, "限制最大音符长度", |ui| {
                    ui.add(egui::Checkbox::without_text(
                        &mut settings.limit_note_length,
                    ));
                });
                if settings.limit_note_length {
                    setting_row(ui, "最大长度", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut settings.max_note_length)
                                .range(1..=65_535)
                                .suffix(" 行"),
                        );
                    });
                    setting_row(ui, "长度测量", |ui| {
                        let selected = if settings.measure_from_image_start {
                            "按图像起始网格"
                        } else {
                            "按持有时长"
                        };
                        egui::ComboBox::from_id_salt("note_length_mode")
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut settings.measure_from_image_start,
                                    false,
                                    "按持有时长重触发",
                                );
                                ui.selectable_value(
                                    &mut settings.measure_from_image_start,
                                    true,
                                    "按图像起始网格切分",
                                );
                            });
                    });
                }

                setting_row(ui, "BPM", |ui| {
                    ui.add(egui::DragValue::new(&mut settings.bpm).range(1..=1000));
                });
                setting_row(ui, "起始偏移", |ui| {
                    ui.add(egui::DragValue::new(&mut settings.start_offset).speed(10.0));
                });
                setting_row(ui, "写入颜色 Meta 事件", |ui| {
                    ui.add(egui::Checkbox::without_text(
                        &mut settings.emit_color_events,
                    ));
                });
            });
            stops.push(StageStop {
                stage: PipelineStage::Midi,
                center_y: rect.center().y,
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
                }

                let export = egui::Button::new("导出 MIDI…")
                    .min_size(egui::vec2(ui.available_width(), 30.0));
                if ui.add_enabled(can_export, export).clicked() {
                    action.export = true;
                }
            }

            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("重置全部参数").clicked() {
                    *settings = Settings::default();
                    action.reset = true;
                }
            });
        });

    Output { action, stops }
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
