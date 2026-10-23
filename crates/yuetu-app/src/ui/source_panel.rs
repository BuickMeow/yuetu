use egui::{RichText, Ui};

use crate::state::{SourceHover, SourceImage};
use crate::theme;
use crate::widgets::{View2D, image_view};

#[derive(Clone, Copy, Debug, Default)]
pub struct Action {
    pub open: bool,
}

pub fn show(
    ui: &mut Ui,
    source: Option<&SourceImage>,
    view: &mut View2D,
    hover: &mut Option<SourceHover>,
) -> Action {
    let mut action = Action::default();

    let Some(source) = source else {
        *hover = None;
        let rect = ui.available_rect_before_wrap();
        theme::view_frame(ui).show(ui, |ui| {
            let min = (rect.size() - egui::vec2(24.0, 24.0)).max(egui::vec2(1.0, 1.0));
            ui.set_min_size(min);
            ui.vertical_centered(|ui| {
                ui.add_space((rect.height() * 0.32).max(24.0));
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
        return action;
    };

    let output = image_view::show(ui, source, view);
    *hover = output.hovered;

    action
}
