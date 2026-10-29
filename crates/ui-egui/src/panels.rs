//! Side panels (Tracks, Groups, Clips) and floating windows.

use crate::SoundApp;
use crate::theme::{Tokens, bold, mono, regular, rgb};
use egui::{Align2, Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::{ClipId, SourceId};

mod about;
mod clips;
mod history;
mod memory;
mod plugins;
mod tracks;
mod transport;

use about::{about, session_info};
pub use clips::clip_list;
pub(crate) use history::undo_history;
pub(crate) use memory::memory_locations;
use plugins::plugin_windows;
pub use tracks::tracks_and_groups;
use transport::{big_counter, transport_window};

pub mod extra_windows;
pub mod midi_editor;
pub mod sampler_window;
pub mod score_editor;
pub mod video_window;
pub mod windows;

fn panel_header(ui: &mut Ui, title: &str) {
    let t = Tokens::current();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, t.panel_bg2);
    ui.painter().text(pos2(r.min.x + 8.0, r.center().y), Align2::LEFT_CENTER, title, bold(11.5), t.header_text);
    ui.painter().line_segment([pos2(r.min.x, r.max.y), pos2(r.max.x, r.max.y)], Stroke::new(1.0, t.border));
}

/// Apply a pending gutter page/drag target to the next frame's `ScrollArea`. `State::offset` is
/// private in egui 0.37, so the target cannot be written directly; it is applied here before layout.
fn with_pending_offset(area: egui::ScrollArea, ui: &Ui, salt: &str) -> egui::ScrollArea {
    // Match `ScrollArea` exactly: it hashes an `IdSalt` into the persistent id.
    let id = ui.make_persistent_id(egui::IdSalt::new(salt));
    let pending = ui.ctx().memory_mut(|m| m.data.remove_temp::<f32>(id.with("gutter_target")));
    match pending {
        Some(y) => area.vertical_scroll_offset(y),
        None => area,
    }
}

/// A list's scroll thumb in the gutter beside it, when the list is longer than its space: drag it,
/// or click above or below it to page. The target applies on the next frame, because the public
/// `ScrollArea` API has no way to write the current pass's offset.
fn gutter_scroll<R>(ui: &Ui, gutter: Rect, out: &egui::scroll_area::ScrollAreaOutput<R>) {
    let t = Tokens::current();
    let (view, content) = (out.inner_rect.height(), out.content_size.y);
    if content <= view + 0.5 {
        return;
    }
    let bar = Rect::from_x_y_ranges(gutter.x_range(), out.inner_rect.y_range());
    let (max, h) = (content - view, (bar.height() * view / content).max(20.0));
    let offset = out.state.clamped_offset().y;
    let thumb = Rect::from_min_size(pos2(bar.min.x + 2.0, bar.min.y + (bar.height() - h) * offset / max), vec2(bar.width() - 4.0, h));
    let resp = ui.interact(bar, out.id.with("gutter"), Sense::click_and_drag());
    ui.painter().rect_filled(thumb, 3.0, if resp.hovered() || resp.dragged() { t.button_hi.lerp_to_gamma(Color32::WHITE, 0.1) } else { t.button_hi });
    let to = if resp.dragged() {
        Some(offset + resp.drag_delta().y * max / (bar.height() - h).max(1.0))
    } else if let Some(p) = resp.interact_pointer_pos().filter(|_| resp.clicked()) {
        let page = if p.y < thumb.min.y {
            -view
        } else if p.y > thumb.max.y {
            view
        } else {
            0.0
        };
        Some(offset + page)
    } else {
        None
    };
    if let Some(y) = to {
        ui.ctx().memory_mut(|m| m.data.insert_temp(out.id.with("gutter_target"), y.clamp(0.0, max)));
        ui.ctx().request_repaint();
    }
}

/// Floating windows.
pub fn floating(app: &mut SoundApp, ctx: &egui::Context) {
    transport_window(app, ctx);
    big_counter(app, ctx);
    plugin_windows(app, ctx);
    crate::mix_window::send_window(app, ctx);
    about(app, ctx);
    session_info(app, ctx);
}
