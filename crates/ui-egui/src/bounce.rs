//! Bounce Mix renders a slice per frame behind a progress window, so the app keeps drawing and the
//! bounce can be cancelled.

use crate::SoundApp;
use crate::theme::{Tokens, bold};
use serde_json::Value;

/// Starts the bounce `file.bounce_mix` runs for `params`; [`step`] renders it.
pub fn start(app: &mut SoundApp, params: &Value) {
    if app.bounce.is_some() {
        app.ui.status = "A bounce is already running".into();
        return;
    }
    // Third-party plugins keep state the session can't see: store it first.
    app.capture_plugin_states();
    match soundcraft_engine::cmd::bounce_job(&app.engine, params) {
        Ok(job) => app.bounce = Some(job),
        Err(e) => app.ui.status = e.to_string(),
    }
}

/// Renders the running bounce for a moment. Called from `logic`, which eframe keeps calling while
/// the window is hidden as long as a repaint is requested, so a bounce doesn't pause then.
pub fn step(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(job) = &mut app.bounce else { return };
    app.ui.status = match job.step(slice()) {
        Ok(None) => {
            ctx.request_repaint();
            return;
        }
        Ok(Some(done)) => {
            format!("Bounced {} (peak {:.1} dBFS, {:.1} LUFS)", job.path().file_name().unwrap_or_default().to_string_lossy(), done.peak_db, done.lufs)
        }
        Err(e) => e.to_string(),
    };
    app.bounce = None;
}

/// Refuses to replace the session while a bounce holds its own copy: two large sessions could run
/// out of memory. True when refused; the status line says why.
pub fn refuses_session_switch(app: &mut SoundApp) -> bool {
    if app.bounce.is_some() {
        app.ui.status = "Finish or cancel the bounce first".into();
    }
    app.bounce.is_some()
}

/// Whether to render another block in this call: for 30 ms.
#[cfg(not(target_arch = "wasm32"))]
fn slice() -> impl FnMut() -> bool {
    let until = std::time::Instant::now() + std::time::Duration::from_millis(30);
    move || std::time::Instant::now() < until
}

/// The browser has no `Instant` (it panics there), so a call renders 32 blocks.
#[cfg(target_arch = "wasm32")]
fn slice() -> impl FnMut() -> bool {
    let mut blocks = 32;
    move || {
        blocks -= 1;
        blocks > 0
    }
}

/// Shows how far the running bounce got, with Cancel (or Esc).
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(job) = &app.bounce else { return };
    let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    egui::Modal::new(egui::Id::unique("bounce_progress")).show(ctx, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        ui.set_width(320.0);
        let name = job.path().file_name().unwrap_or_default().to_string_lossy();
        ui.label(egui::RichText::new(format!("Bouncing {name}")).font(bold(13.0)));
        ui.add(egui::ProgressBar::new(job.progress()));
        ui.horizontal(|ui| {
            cancel |= ui.button("Cancel").clicked();
            ui.label(egui::RichText::new(format!("{:.0}%", job.progress() * 100.0)).color(Tokens::current().text_dim));
        });
    });
    if cancel {
        app.bounce = None;
        app.ui.status = "Bounce cancelled".into();
    }
}
