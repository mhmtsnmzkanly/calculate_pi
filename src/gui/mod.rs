//! Desktop graphical user interface (GUI) entry point.
mod app;
mod charts;
mod explorer;
mod panels;

use crate::app_state::Shared;

pub fn run_gui(state: Shared) -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Pi Calculator — Certified MPFR Engine")
            .with_inner_size([1320.0, 840.0])
            .with_min_inner_size([1120.0, 720.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Pi Calculator",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Box::new(app::GuiApp::new(state))
        }),
    )
}
