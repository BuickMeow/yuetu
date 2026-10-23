use egui::{RichText, Ui};
use yuetu_core::note_name;

use crate::state::{PreviewHover, SourceHover, StatusKind, StatusMessage};
use crate::theme;

pub fn show(
    ui: &mut Ui,
    status: &StatusMessage,
    source_hover: Option<SourceHover>,
    preview_hover: Option<PreviewHover>,
    image_zoom: f32,
    preview_zoom: f32,
) {
    let status_color = match status.kind {
        StatusKind::Info => theme::muted(ui),
        StatusKind::Success => theme::success(ui),
        StatusKind::Warning => theme::warning(ui),
        StatusKind::Error => theme::danger(ui),
    };

    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.5, status_color);
        ui.label(RichText::new(status.text.as_str()).color(status_color));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(format!("预览 {}", zoom_label(preview_zoom)));
            ui.add_space(10.0);
            ui.label(format!("原图 {}", zoom_label(image_zoom)));

            if let Some(hover) = source_hover {
                ui.add_space(10.0);
                let color = egui::Color32::from_rgba_unmultiplied(
                    hover.color.r,
                    hover.color.g,
                    hover.color.b,
                    hover.color.a,
                );
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 2.0, color);
                ui.label(format!(
                    "({}, {}) {}",
                    hover.source_pos.0,
                    hover.source_pos.1,
                    hover.color.hex()
                ));
            } else if let Some(hover) = preview_hover {
                ui.add_space(10.0);
                ui.label(format!("{} · tick {}", note_name(hover.key), hover.tick));
            }
        });
    });
}

fn zoom_label(zoom: f32) -> String {
    if (zoom - 1.0).abs() < 0.01 {
        "适应".to_owned()
    } else {
        format!("{:.0}%", zoom * 100.0)
    }
}
