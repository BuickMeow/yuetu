use egui::{RichText, Ui};

use crate::theme;

#[derive(Clone, Copy, Debug, Default)]
pub struct Action {
    pub open: bool,
    pub cancel: bool,
    pub export: bool,
    pub fit: bool,
}

pub fn show(ui: &mut Ui, can_export: bool, is_running: bool) -> Action {
    let mut action = Action::default();

    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(
            RichText::new("YUETU")
                .size(18.0)
                .strong()
                .color(theme::accent(ui)),
        );
        ui.add_space(14.0);

        if ui.button("打开图片").clicked() {
            action.open = true;
        }
        if is_running {
            ui.add_enabled(false, egui::Button::new(RichText::new("生成中…").strong()));
            if ui
                .add(egui::Button::new(
                    RichText::new("取消").color(theme::danger(ui)),
                ))
                .clicked()
            {
                action.cancel = true;
            }
        }
        if ui
            .add_enabled(can_export, egui::Button::new("导出 MIDI"))
            .clicked()
        {
            action.export = true;
        }
        if ui.button("适配窗口").clicked() {
            action.fit = true;
        }
    });

    action
}
