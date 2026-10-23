//! Theme configuration.
//!
//! `CUSTOM_THEME` is intentionally off while we evaluate egui 0.37's built-in
//! dark visuals. All UI code asks this module for colors/frames, so flipping
//! the flag back on restores the graphite/mint look without touching panels.

use egui::{Color32, CornerRadius, Frame, Margin, Stroke, Ui, Visuals};

/// Set to `true` to restore Yuetu's custom graphite/mint visuals.
pub const CUSTOM_THEME: bool = false;

/// Built-in egui theme used while `CUSTOM_THEME` is off.
pub const THEME_PREFERENCE: egui::ThemePreference = egui::ThemePreference::Dark;

pub const BG_APP: Color32 = Color32::from_rgb(16, 18, 22);
pub const BG_PANEL: Color32 = Color32::from_rgb(22, 25, 31);
pub const BG_CARD: Color32 = Color32::from_rgb(29, 33, 41);
pub const BG_INPUT: Color32 = Color32::from_rgb(14, 16, 20);
pub const BG_VIEW: Color32 = Color32::from_rgb(11, 13, 17);
pub const BG_LANE_WHITE: Color32 = Color32::from_rgb(31, 36, 45);
pub const BG_LANE_BLACK: Color32 = Color32::from_rgb(22, 26, 33);
pub const BG_LANE_OFF: Color32 = Color32::from_rgb(18, 20, 25);

pub const TEXT: Color32 = Color32::from_rgb(232, 235, 240);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(148, 157, 172);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(104, 112, 126);

pub const ACCENT: Color32 = Color32::from_rgb(94, 234, 212);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(24, 78, 74);

pub const SUCCESS: Color32 = Color32::from_rgb(74, 222, 128);
pub const WARNING: Color32 = Color32::from_rgb(251, 191, 36);
pub const DANGER: Color32 = Color32::from_rgb(248, 113, 113);

pub const RADIUS_SM: CornerRadius = CornerRadius::same(6);
pub const RADIUS_MD: CornerRadius = CornerRadius::same(10);

#[derive(Clone, Copy)]
pub enum PanelKind {
    Bottom,
    Side,
    Central,
}

pub fn apply(ctx: &egui::Context) {
    ctx.add_font(egui_material_icons::font_insert());
    // Use egui's built-in theme while CUSTOM_THEME is off.
    ctx.set_theme(THEME_PREFERENCE);
    if !CUSTOM_THEME {
        return;
    }

    let mut visuals = Visuals::dark();
    visuals.override_text_color = Some(TEXT);
    visuals.panel_fill = BG_PANEL;
    visuals.window_fill = BG_CARD;
    visuals.faint_bg_color = BG_CARD;
    visuals.extreme_bg_color = BG_INPUT;
    visuals.code_bg_color = BG_INPUT;
    visuals.hyperlink_color = ACCENT;
    visuals.warn_fg_color = WARNING;
    visuals.error_fg_color = DANGER;
    visuals.window_stroke = Stroke::NONE;
    visuals.window_corner_radius = RADIUS_MD;

    visuals.widgets.noninteractive.bg_fill = BG_PANEL;
    visuals.widgets.noninteractive.weak_bg_fill = BG_PANEL;
    visuals.widgets.noninteractive.bg_stroke = Stroke::NONE;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);

    visuals.widgets.inactive.bg_fill = BG_CARD;
    visuals.widgets.inactive.weak_bg_fill = BG_CARD;
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.inactive.corner_radius = RADIUS_SM;

    visuals.widgets.hovered.bg_fill = Color32::from_rgb(38, 44, 54);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(38, 44, 54);
    visuals.widgets.hovered.bg_stroke = Stroke::NONE;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.hovered.corner_radius = RADIUS_SM;

    visuals.widgets.active.bg_fill = Color32::from_rgb(44, 51, 62);
    visuals.widgets.active.weak_bg_fill = Color32::from_rgb(44, 51, 62);
    visuals.widgets.active.bg_stroke = Stroke::NONE;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.active.corner_radius = RADIUS_SM;

    visuals.widgets.open.bg_fill = BG_INPUT;
    visuals.widgets.open.weak_bg_fill = BG_INPUT;
    visuals.widgets.open.bg_stroke = Stroke::NONE;
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.open.corner_radius = RADIUS_SM;

    visuals.selection.bg_fill = ACCENT_DIM;
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.slider_trailing_fill = true;
    visuals.striped = false;

    ctx.set_visuals(visuals);
    ctx.global_style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.interact_size.y = 26.0;
        style.spacing.slider_width = 150.0;
        style.spacing.combo_width = 150.0;
        style.spacing.scroll = egui::style::ScrollStyle::solid();
    });
}

pub fn panel_frame(ui: &Ui, kind: PanelKind) -> Frame {
    if !CUSTOM_THEME {
        return match kind {
            PanelKind::Central => Frame::central_panel(ui.style()),
            PanelKind::Bottom | PanelKind::Side => Frame::side_top_panel(ui.style()),
        };
    }

    match kind {
        PanelKind::Bottom => Frame::new()
            .fill(BG_APP)
            .inner_margin(Margin::symmetric(12, 5)),
        PanelKind::Side => Frame::new().fill(BG_PANEL).inner_margin(Margin::same(12)),
        PanelKind::Central => Frame::new().fill(BG_PANEL).inner_margin(Margin::same(10)),
    }
}

pub fn card_frame(ui: &Ui) -> Frame {
    if CUSTOM_THEME {
        Frame::new()
            .fill(BG_CARD)
            .corner_radius(RADIUS_MD)
            .inner_margin(Margin::same(10))
    } else {
        Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .corner_radius(ui.visuals().widgets.noninteractive.corner_radius)
            .inner_margin(Margin::same(10))
    }
}

pub fn view_frame(ui: &Ui) -> Frame {
    if CUSTOM_THEME {
        Frame::new()
            .fill(BG_VIEW)
            .corner_radius(RADIUS_MD)
            .inner_margin(Margin::same(12))
    } else {
        Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .inner_margin(Margin::same(12))
    }
}

pub fn muted(ui: &Ui) -> Color32 {
    if CUSTOM_THEME {
        TEXT_MUTED
    } else {
        ui.visuals().weak_text_color()
    }
}

pub fn faint(ui: &Ui) -> Color32 {
    if CUSTOM_THEME {
        TEXT_FAINT
    } else {
        ui.visuals().weak_text_color().gamma_multiply(0.7)
    }
}

pub fn accent(ui: &Ui) -> Color32 {
    if CUSTOM_THEME {
        ACCENT
    } else {
        ui.visuals().hyperlink_color
    }
}

pub fn primary_fill(ui: &Ui) -> Color32 {
    if CUSTOM_THEME {
        ACCENT_DIM
    } else {
        ui.visuals().selection.bg_fill
    }
}

pub fn success(ui: &Ui) -> Color32 {
    if CUSTOM_THEME {
        SUCCESS
    } else {
        ui.visuals().text_color()
    }
}

pub fn warning(ui: &Ui) -> Color32 {
    if CUSTOM_THEME {
        WARNING
    } else {
        ui.visuals().warn_fg_color
    }
}

pub fn danger(ui: &Ui) -> Color32 {
    if CUSTOM_THEME {
        DANGER
    } else {
        ui.visuals().error_fg_color
    }
}

pub fn view_bg(ui: &Ui) -> Color32 {
    if CUSTOM_THEME {
        BG_VIEW
    } else {
        ui.visuals().extreme_bg_color
    }
}

pub fn lane_white(ui: &Ui) -> Color32 {
    if CUSTOM_THEME {
        BG_LANE_WHITE
    } else {
        ui.visuals().faint_bg_color
    }
}

pub fn lane_black(ui: &Ui) -> Color32 {
    if CUSTOM_THEME {
        BG_LANE_BLACK
    } else {
        ui.visuals().extreme_bg_color
    }
}

pub fn lane_off(ui: &Ui) -> Color32 {
    if CUSTOM_THEME {
        BG_LANE_OFF
    } else {
        ui.visuals().panel_fill
    }
}
