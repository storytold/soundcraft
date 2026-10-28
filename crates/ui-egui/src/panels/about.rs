use super::{SoundApp, bold, mono, vec2};

/// About window tabs for version information, contributors and models.
pub(super) fn about(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_about;
    if !open {
        return;
    }
    let tab_id = egui::Id::unique("about_tab");
    egui::Window::new("About SoundCraft").open(&mut open).default_size(vec2(640.0, 420.0)).collapsible(false).show(ctx, |ui| {
        let mut tab = ui.data_mut(|d| d.get_temp::<u8>(tab_id)).unwrap_or(0);
        ui.horizontal(|ui| {
            for (i, l) in (0u8..).zip(["About", "Contributors", "Models"]) {
                if ui.selectable_label(tab == i, l).clicked() {
                    tab = i;
                }
            }
        });
        ui.data_mut(|d| d.insert_temp(tab_id, tab));
        ui.separator();
        match tab {
            1 => crate::credits::contributors_ui(ui),
            2 => crate::credits::models_ui(ui),
            _ => {
                ui.label(egui::RichText::new("SoundCraft").font(bold(22.0)));
                ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                ui.label("An open-source digital audio workstation from the ArtCraft team.");
                ui.label("Dual-licensed MIT OR Apache-2.0. Made with Rust and egui.");
                ui.hyperlink_to("getartcraft.com/apps/soundcraft", "https://getartcraft.com/apps/soundcraft");
                ui.hyperlink_to("Join us on Discord", "https://discord.gg/artcraft");
            }
        }
    });
    app.ui.show_about = open;
}

pub(super) fn session_info(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_session_info;
    if !open {
        return;
    }
    egui::Window::new("Session Info").open(&mut open).default_size(vec2(480.0, 360.0)).show(ctx, |ui| {
        let text = soundcraft_engine::inspect::session_text(&app.engine);
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(egui::RichText::new(text).font(mono(11.0)));
        });
    });
    app.ui.show_session_info = open;
}
