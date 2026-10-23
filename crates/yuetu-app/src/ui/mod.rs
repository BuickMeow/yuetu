pub mod controls;
pub mod preview_panel;
pub mod source_panel;
pub mod status_bar;
pub mod toolbar;

use egui::{RichText, Ui};

use crate::theme;

pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let outer_width = ui.available_width();
    theme::card_frame(ui)
        .show(ui, |ui| {
            // Frame margins are 10 px on each side; force every sidebar card
            // to the same outer width regardless of its content.
            ui.set_min_width((outer_width - 20.0).max(1.0));
            add(ui)
        })
        .inner
}

pub fn section_title(ui: &mut Ui, title: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(title).strong());
    ui.add_space(1.0);
}

/// One setting row: label on the left, control right-aligned.
pub fn setting_row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.set_min_width(ui.available_width());
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add)
            .inner
    })
    .inner
}
