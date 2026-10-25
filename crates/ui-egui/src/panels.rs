//! Side panels (Tracks, Groups, Clips) and floating windows.

use crate::SoundApp;
use crate::theme::{Tokens, bold, mono, regular, rgb};
use egui::{Align2, Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::{ClipId, SourceId};

/// Drag a Tracks list row to reorder the tracks: a line marks the gap it will land in.
fn drag_track(app: &mut SoundApp, ui: &Ui, row: Rect, id: u64, dropped: bool) {
    let s = app.engine.session();
    let (Some(from), Some(p)) = (s.track_index(soundcraft_model::TrackId(id)), ui.ctx().pointer_latest_pos()) else { return };
    let spacing = ui.spacing().item_spacing.y;
    let pitch = row.height() + spacing;
    let gap = (from as f32 + ((p.y - row.center().y) / pitch).floor() + 1.0).clamp(0.0, s.tracks.len() as f32);
    if dropped {
        let to = gap as usize;
        let to = if to > from { to - 1 } else { to };
        if to != from {
            let _ = app.run("track.move", json!({"track": id, "to": to}));
        }
    } else {
        let y = row.min.y + (gap - from as f32) * pitch - spacing / 2.0;
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        ui.ctx()
            .layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("track_list_reorder")))
            .line_segment([pos2(row.min.x, y), pos2(row.max.x, y)], Stroke::new(2.0, Tokens::current().accent));
    }
}

fn panel_header(ui: &mut Ui, title: &str) {
    let t = Tokens::current();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, t.panel_bg2);
    ui.painter().text(pos2(r.min.x + 8.0, r.center().y), Align2::LEFT_CENTER, title, bold(11.5), t.header_text);
    ui.painter().line_segment([pos2(r.min.x, r.max.y), pos2(r.max.x, r.max.y)], Stroke::new(1.0, t.border));
}

/// The strip down the Tracks panel's right edge: the lists' scroll bars, and a clear line between
/// the panel and the window beside it.
const GUTTER_W: f32 = 10.0;

/// Tracks and Groups panels: headers, track and group lists, and their scrollbars in the panel gutter.
pub fn tracks_and_groups(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let full = ui.max_rect();
    let gutter = full.with_min_x(full.max.x - GUTTER_W);
    ui.painter().rect_filled(gutter, 0.0, t.panel_bg.lerp_to_gamma(t.text_dim, 0.15));
    ui.painter().vline(gutter.min.x + 0.5, full.y_range(), Stroke::new(1.0, t.border));
    ui.painter().vline(gutter.max.x - 0.5, full.y_range(), Stroke::new(1.0, t.border));
    let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(full.with_max_x(gutter.min.x)));
    let total = ui.available_height();
    let header = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), 22.0));
    panel_header(ui, "TRACKS");
    let plus = Rect::from_center_size(pos2(header.max.x - 13.0, header.center().y), vec2(18.0, 18.0));
    let tip = format!("New Tracks… ({})", crate::shortcuts::shortcut_label("Cmd+Shift+N", ui.ctx().os().is_mac()));
    if crate::widgets::icon_button(&mut ui.new_child(egui::UiBuilder::new().max_rect(plus)), plus.size(), "plus", false, &tip).clicked() {
        let _ = app.dialogs.open_for_command(&app.engine, "track.new");
    }
    let tracks: Vec<(u64, String, bool, [u8; 3], bool)> = {
        let s = app.engine.session();
        s.tracks.iter().map(|x| (x.id.0, x.name.clone(), !x.hidden, x.color, s.edit.selected_tracks.contains(&x.id))).collect()
    };
    let hidden = egui::scroll_area::ScrollBarVisibility::AlwaysHidden;
    let lists = with_pending_offset(egui::ScrollArea::vertical().id_salt("tracks_scroll"), ui, "tracks_scroll")
        .max_height(total * 0.62)
        .auto_shrink([false, false])
        .scroll_bar_visibility(hidden);
    let out = lists.show(ui, |ui| {
        for (id, name, shown, color, sel) in tracks {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 17.0), Sense::click_and_drag());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, sel, &name));
            if sel {
                ui.painter().rect_filled(r, 0.0, t.row_selected);
            }
            let dot = Rect::from_center_size(pos2(r.min.x + 10.0, r.center().y), vec2(8.0, 8.0));
            let dresp = ui.interact(dot.expand(3.0), ui.scope_id().with(("vis", id)), Sense::click());
            ui.painter().circle(dot.center(), 3.5, if shown { t.visible_dot } else { Color32::TRANSPARENT }, Stroke::new(1.0, t.text_dim));
            if dresp.clicked() {
                let _ = app.run("track.hide", json!({"tracks": [id], "hidden": shown}));
            }
            ui.painter().rect_filled(Rect::from_min_size(pos2(r.min.x + 20.0, r.min.y + 3.0), vec2(4.0, 11.0)), 0.0, rgb(color));
            ui.painter().with_clip_rect(r).text(
                pos2(r.min.x + 30.0, r.center().y),
                Align2::LEFT_CENTER,
                &name,
                regular(11.5),
                if shown { t.text } else { t.text_dim },
            );
            if resp.clicked() {
                crate::edit_window::click_track(app, soundcraft_model::TrackId(id), ui.input(|i| i.modifiers), |_, _| true);
            }
            if resp.dragged() || resp.drag_stopped() {
                drag_track(app, ui, r, id, resp.drag_stopped());
            }
        }
    });
    gutter_scroll(ui, gutter, &out);
    ui.add_space(4.0);
    panel_header(ui, "GROUPS");
    let groups: Vec<(u64, char, String, bool, [u8; 3])> =
        app.engine.session().groups.iter().map(|g| (g.id.0, g.letter, g.name.clone(), g.active, g.color)).collect();
    let groups_area = with_pending_offset(egui::ScrollArea::vertical().id_salt("groups_scroll"), ui, "groups_scroll");
    let out = groups_area.auto_shrink([false, false]).scroll_bar_visibility(hidden).show(ui, |ui| {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 17.0), Sense::hover());
        ui.painter().text(pos2(r.min.x + 30.0, r.center().y), Align2::LEFT_CENTER, "<ALL>", regular(11.5).clone(), t.text);
        let all = ui.interact(r, ui.scope_id().with("group_all"), Sense::click());
        all.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "<ALL>"));
        if all.double_clicked() {
            let _ = app.run("edit.select_all", json!({}));
        }
        all.context_menu(|ui| {
            if ui.button("New Group…").clicked() {
                let _ = app.dialogs.open_for_command(&app.engine, "track.group");
                ui.close();
            }
        });
        for (id, letter, name, active, color) in groups {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 17.0), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, active, &name));
            if active {
                ui.painter().rect_filled(Rect::from_min_size(pos2(r.min.x + 4.0, r.min.y + 2.0), vec2(14.0, 13.0)), 1.0, rgb(color));
            }
            ui.painter().text(
                pos2(r.min.x + 11.0, r.center().y),
                Align2::CENTER_CENTER,
                letter.to_string(),
                bold(10.0),
                if active { t.text_dark } else { t.text_dim },
            );
            ui.painter().with_clip_rect(r).text(pos2(r.min.x + 30.0, r.center().y), Align2::LEFT_CENTER, &name, regular(11.5), t.text);
            if resp.clicked() {
                let _ = app.run("track.group_toggle", json!({"group": id}));
            }
            let members: Vec<u64> =
                app.engine.session().groups.iter().find(|g| g.id.0 == id).map(|g| g.members.iter().map(|m| m.0).collect()).unwrap_or_default();
            if resp.double_clicked() {
                let _ = app.run("edit.select", json!({"tracks": members, "exact": true}));
            }
            resp.context_menu(|ui| {
                if ui.button("Select Tracks in Group").clicked() {
                    let _ = app.run("edit.select", json!({"tracks": members, "exact": true}));
                    ui.close();
                }
                if ui.button(if active { "Disable" } else { "Enable" }).clicked() {
                    let _ = app.run("track.group_toggle", json!({"group": id}));
                    ui.close();
                }
                if ui.button("Modify Group…").clicked() {
                    let _ = app.dialogs.open_modify_group(&app.engine, soundcraft_model::GroupId(id));
                    ui.close();
                }
                if ui.button("Delete Group").clicked() {
                    let _ = app.run("track.ungroup", json!({"group": id}));
                    ui.close();
                }
            });
        }
    });
    gutter_scroll(ui, gutter, &out);
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

/// One row of the clip list: a whole file, or a clip on a track (positions in the session).
#[derive(Clone, Copy)]
enum ClipRow {
    Source(usize),
    Clip { track: usize, clip: usize },
}

/// The clip list's rows for one document revision and filter. A drum edit holds tens of thousands
/// of clips: the rows are listed (and filtered) when either changes, not every frame.
#[derive(Clone)]
struct ClipRows {
    revision: u64,
    filter: String,
    rows: std::sync::Arc<Vec<ClipRow>>,
}

fn clip_rows(s: &soundcraft_model::Session, needle: &str) -> Vec<ClipRow> {
    let shown = |name: &str| needle.is_empty() || name.to_lowercase().contains(needle);
    // Whole files first (bold), then clips on tracks.
    let sources =
        s.sources.iter().enumerate().filter(|(_, src)| shown(&format!("{} ({}ch)", src.name, src.channels))).map(|(i, _)| ClipRow::Source(i));
    let clips =
        s.tracks.iter().enumerate().flat_map(|(track, tr)| {
            tr.clips().iter().enumerate().filter(|(_, c)| shown(&c.name)).map(move |(clip, _)| ClipRow::Clip { track, clip })
        });
    sources.chain(clips).collect()
}

/// Clips panel with a filter and a revision-cached list of audio sources and clips.
pub fn clip_list(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    panel_header(ui, "CLIPS");
    let fkey = egui::Id::unique("clip_filter");
    let mut filter: String = ui.ctx().memory(|m| m.data.get_temp(fkey)).unwrap_or_default();
    ui.add(egui::TextEdit::singleline(&mut filter).hint_text("🔍 Find clips").desired_width(f32::INFINITY));
    ui.ctx().memory_mut(|m| m.data.insert_temp(fkey, filter.clone()));
    let needle = filter.to_lowercase();
    let rkey = egui::Id::unique("clip_rows");
    let revision = app.engine.revision;
    let cached = ui.ctx().memory(|m| m.data.get_temp::<ClipRows>(rkey)).filter(|c| c.revision == revision && c.filter == needle);
    let rows = match cached {
        Some(c) => c.rows,
        None => {
            let rows = std::sync::Arc::new(clip_rows(app.engine.session(), &needle));
            ui.ctx().memory_mut(|m| m.data.insert_temp(rkey, ClipRows { revision, filter: needle, rows: rows.clone() }));
            rows
        }
    };
    let row_h = 17.0;
    let clips_area = with_pending_offset(egui::ScrollArea::vertical().id_salt("clips_scroll"), ui, "clips_scroll");
    clips_area.auto_shrink([false, false]).show_rows(ui, row_h, rows.len(), |ui, shown| {
        // Only the rows in view: what they show is read now, before any of them acts on the session.
        let items: Vec<(u64, String, bool, [u8; 3], bool)> = {
            let s = app.engine.session();
            rows.get(shown)
                .unwrap_or_default()
                .iter()
                .filter_map(|row| match *row {
                    ClipRow::Source(i) => s.sources.get(i).map(|src| {
                        (src.id.0, format!("{} ({}ch)", src.name, src.channels), true, [150, 150, 150], s.edit.selected_sources.contains(&src.id))
                    }),
                    ClipRow::Clip { track, clip } => {
                        let tr = s.tracks.get(track)?;
                        let c = tr.clips().get(clip)?;
                        Some((c.id.0, c.name.clone(), false, c.color.unwrap_or(tr.color), s.edit.selected_clips.contains(&c.id)))
                    }
                })
                .collect()
        };
        for (id, name, whole, color, selected) in items {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), if whole { Sense::click_and_drag() } else { Sense::click() });
            resp.context_menu(|ui| {
                if whole {
                    if ui.button("Place on New Track").clicked() {
                        let _ = app.run("clip.place_source", json!({"source": id}));
                        ui.close();
                    }
                    let sel_track = app.engine.session().edit.selected_tracks.first().map(|t| t.0);
                    if let Some(t) = sel_track
                        && ui.button("Place on Selected Track").clicked()
                    {
                        let _ = app.run("clip.place_source", json!({"source": id, "track": t}));
                        ui.close();
                    }
                    ui.separator();
                }
                if !whole && ui.button("Auto Rename…").clicked() {
                    // The selected clips when the row is one of them, otherwise the row's clip.
                    let clips = if selected { app.engine.session().edit.selected_clips.clone() } else { vec![ClipId(id)] };
                    app.dialogs.open_auto_rename(&app.engine, clips);
                    ui.close();
                }
                if ui.button("Clear…").clicked() {
                    let (clips, sources) = if whole { (vec![], vec![SourceId(id)]) } else { (vec![ClipId(id)], vec![]) };
                    app.dialogs.open_clear(&app.engine, clips, sources);
                    ui.close();
                }
            });
            if whole {
                // Whole files drag onto tracks.
                resp.dnd_set_drag_payload(crate::DragSource(id));
                if resp.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                }
            }
            if selected {
                ui.painter().rect_filled(r, 0.0, t.row_selected);
            }
            ui.painter().rect_filled(Rect::from_min_size(pos2(r.min.x + 6.0, r.min.y + 4.0), vec2(9.0, 9.0)), 1.0, rgb(color));
            ui.painter().with_clip_rect(r).text(
                pos2(r.min.x + 22.0, r.center().y),
                Align2::LEFT_CENTER,
                &name,
                if whole { bold(11.0) } else { regular(11.0) },
                t.text,
            );
            if resp.clicked() {
                let _ = app.run("edit.select", if whole { json!({"sources": [id]}) } else { json!({"clips": [id]}) });
            }
        }
    });
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

fn transport_window(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_transport;
    if !open {
        return;
    }
    let t = Tokens::current();
    egui::Window::new("Transport").open(&mut open).resizable(false).default_pos(pos2(400.0, 500.0)).show(ctx, |ui| {
        let flags = app.engine.session().edit.flags.clone();
        let expanded = flags.contains("view.transport.expanded");
        ui.horizontal(|ui| {
            for (icon, cmd) in [
                ("rtz", "transport.rtz"),
                ("rewind", "transport.rewind"),
                ("stop", "transport.stop"),
                ("play", "transport.play"),
                ("ffwd", "transport.fast_forward"),
                ("end", "transport.go_to_end"),
                ("record", "transport.record"),
            ] {
                let resp = crate::widgets::icon_button(
                    ui,
                    vec2(34.0, 26.0),
                    icon,
                    (cmd == "transport.play" && app.is_playing()) || (cmd == "transport.record" && app.engine.transport.recording),
                    cmd,
                );
                if cmd == "transport.record"
                    && let Some(why) = app.record_hint()
                {
                    egui::Tooltip::for_widget(&resp).show(|ui| ui.label(why));
                }
                if resp.clicked() {
                    let _ = app.run(cmd, json!({}));
                }
            }
            if flags.contains("view.transport.output_meters") {
                let (r, _) = ui.allocate_exact_size(vec2(22.0, 26.0), Sense::hover());
                let m = app.main_meter;
                crate::widgets::meter(ui, Rect::from_min_size(r.min, vec2(10.0, 26.0)), m.level[0], m.hold[0], m.clip);
                crate::widgets::meter(ui, Rect::from_min_size(pos2(r.min.x + 12.0, r.min.y), vec2(10.0, 26.0)), m.level[1], m.hold[1], m.clip);
            }
        });
        let s = app.engine.session();
        let fmt = |x: i64| s.format_position(x, s.edit.main_counter);
        if flags.contains("view.transport.counters") || expanded || flags.is_empty() {
            ui.label(egui::RichText::new(fmt(app.position())).font(mono(20.0)).color(t.counter_text));
        }
        if expanded {
            let e = s.edit.clone();
            egui::Grid::new("tx_exp").num_columns(4).show(ui, |ui| {
                ui.label("Start");
                ui.label(egui::RichText::new(fmt(e.selection.start)).font(mono(11.0)));
                ui.label("Pre-roll");
                ui.label(egui::RichText::new(format!("{:.2} s", s.sample_rate.seconds(e.pre_roll))).font(mono(11.0)));
                ui.end_row();
                ui.label("End");
                ui.label(egui::RichText::new(fmt(e.selection.end)).font(mono(11.0)));
                ui.label("Post-roll");
                ui.label(egui::RichText::new(format!("{:.2} s", s.sample_rate.seconds(e.post_roll))).font(mono(11.0)));
                ui.end_row();
            });
        }
        if flags.contains("view.transport.midi_controls") || expanded {
            let (click, countoff, merge, loop_rec, prepost) = {
                let e = &app.engine.session().edit;
                (e.click, e.countoff, e.midi_merge, e.loop_record, e.pre_post_roll)
            };
            ui.horizontal(|ui| {
                for (label, on, cmd) in [
                    ("Click", click, "options.click"),
                    ("Count Off", countoff, "options.countoff"),
                    ("MIDI Merge", merge, "options.midi_merge"),
                    ("Loop Rec", loop_rec, "options.loop_record"),
                    ("Pre/Post", prepost, "options.pre_post_roll"),
                ] {
                    if ui.selectable_label(on, label).clicked() {
                        let _ = app.run(cmd, json!({}));
                    }
                }
            });
            let tick = app.engine.session().tempo.samples_to_ticks(app.position(), app.engine.session().sample_rate);
            let bpm = app.engine.session().tempo.tempo_at_tick(tick);
            let m = app.engine.session().tempo.meter_at_tick(tick);
            ui.label(egui::RichText::new(format!("♩ = {bpm:.2}   {}/{}", m.numerator, m.denominator)).font(mono(12.0)).color(t.counter_text));
        }
    });
    app.ui.show_transport = open;
}

fn big_counter(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_big_counter;
    if !open {
        return;
    }
    egui::Window::new("Big Counter").open(&mut open).default_size(vec2(520.0, 110.0)).show(ctx, |ui| {
        let s = app.engine.session();
        let txt = s.format_position(app.position(), s.edit.main_counter);
        let r = ui.available_rect_before_wrap();
        let t = Tokens::current();
        ui.painter().rect_filled(r, 4.0, t.big_counter_bg);
        ui.painter().text(r.center(), Align2::CENTER_CENTER, txt, mono((r.height() * 0.6).clamp(20.0, 120.0)), t.counter_text);
    });
    app.ui.show_big_counter = open;
}

/// One recallable memory-location row: number button, editable name, time, length, kind and delete.
fn memory_location_row(app: &mut SoundApp, ui: &mut Ui, n: u32, name: &str, loc: &str, len: &str, kind: &str) {
    if ui.button(n.to_string()).on_hover_text("Recall").clicked() {
        let _ = app.run("markers.recall", json!({"number": n}));
    }
    let key = egui::Id::unique(("mem_name", n));
    let mut edit: String = ui.ctx().memory(|m| m.data.get_temp(key)).unwrap_or_else(|| name.to_string());
    let r = crate::widgets::named_text_field(ui.add(egui::TextEdit::singleline(&mut edit).desired_width(140.0)), "Name");
    if r.changed() {
        ui.ctx().memory_mut(|m| m.data.insert_temp(key, edit.clone()));
    }
    if r.lost_focus() && edit != name {
        let _ = app.run("markers.edit", json!({"number": n, "rename": edit}));
        ui.ctx().memory_mut(|m| m.data.remove::<String>(key));
    }
    ui.label(egui::RichText::new(loc).font(mono(11.0)));
    ui.label(egui::RichText::new(len).font(mono(11.0)));
    ui.label(kind);
    if crate::widgets::close_button(ui, "Delete marker").clicked() {
        let _ = app.run("markers.delete", json!({"number": n}));
    }
    ui.end_row();
}

pub(crate) fn memory_locations(app: &mut SoundApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        if ui.button("+ Marker").clicked() {
            let _ = app.run("markers.add", json!({}));
        }
        if ui.button("+ Selection").clicked() {
            let _ = app.run("markers.add", json!({"kind": "selection"}));
        }
    });
    let rows: Vec<(u32, String, String, String, String)> = {
        let s = app.engine.session();
        let f = |x: i64| s.format_position(x, s.edit.main_counter);
        s.markers
            .iter()
            .map(|m| {
                let kind = match m.kind {
                    soundcraft_model::MarkerKind::Marker => "marker",
                    soundcraft_model::MarkerKind::Selection => "selection",
                    soundcraft_model::MarkerKind::None => "none",
                };
                let len = if m.kind == soundcraft_model::MarkerKind::Selection { f(m.end - m.start) } else { String::new() };
                (m.number, m.name.clone(), f(m.start), len, kind.to_string())
            })
            .collect()
    };
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("mem_grid").striped(true).num_columns(6).show(ui, |ui| {
            for h in ["#", "Name", "Location", "Length", "Type", ""] {
                ui.label(egui::RichText::new(h).strong());
            }
            ui.end_row();
            for (n, name, loc, len, kind) in &rows {
                memory_location_row(app, ui, *n, name, loc, len, kind);
            }
        });
    });
}

pub(crate) fn undo_history(app: &mut SoundApp, ui: &mut Ui) {
    let hist = app.engine.undo_history();
    let n = hist.len();
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (i, h) in hist.iter().enumerate() {
            if ui.selectable_label(i + 1 == n, format!("{}  {h}", i + 1)).clicked() {
                for _ in 0..(n - i - 1) {
                    let _ = app.run("edit.undo", json!({}));
                }
            }
        }
        if let Some(r) = app.engine.redo_label() {
            ui.label(egui::RichText::new(format!("(redo) {r}")).italics().color(Tokens::current().text_dim));
        }
    });
}

/// Draw plugin windows for instrument and insert slots that still exist, removing closed ones from the list.
fn plugin_windows(app: &mut SoundApp, ctx: &egui::Context) {
    let wins = app.ui.plugin_windows.clone();
    let mut keep = Vec::new();
    for (tid, slot) in wins {
        // The instrument slot (an instrument track's instrument) or an insert slot.
        let instrument = slot == soundcraft_mix::INSTRUMENT_SLOT;
        let Some((tname, ins)) = app.engine.session().track(tid).and_then(|t| {
            let ins = if instrument { t.instrument.clone() } else { t.mixer.inserts.get(slot).cloned().flatten() };
            ins.map(|i| (t.name.clone(), i))
        }) else {
            continue;
        };
        let Some(info) = crate::mix_window::plugin_info(&ins.plugin) else { continue };
        let place = if instrument { "instrument".to_string() } else { char::from(b'a'.saturating_add(slot as u8)).to_string() };
        let mut open = true;
        let shown = egui::Window::new(format!("{tname} · {place} · {}", info.name))
            .id(egui::Id::unique(("plugin", tid.0, slot)))
            .open(&mut open)
            .default_width(340.0)
            .default_pos(egui::pos2(ctx.content_rect().width() - 380.0, 120.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // Bypass and presets act on insert slots.
                    if !instrument {
                        ui.label(egui::RichText::new(&ins.preset).color(Tokens::current().text_dim));
                        let label = if ins.bypass { "BYPASSED" } else { "Bypass" };
                        if ui.button(label).clicked() {
                            let _ = app.run("mix.insert_bypass", json!({"track": tid.0, "slot": slot}));
                        }
                        ui.menu_button("Presets v", |ui| preset_menu(app, ui, tid, slot, info, &ins));
                    }
                    if soundcraft_mix::is_third_party(&ins.plugin) {
                        let open = app.player.as_ref().is_some_and(|p| p.editor_open(tid, slot));
                        let available = app.player.as_ref().is_some_and(|p| p.has_editor(tid, slot));
                        let label = if open { "Close Plugin Editor" } else { "Open Plugin Editor" };
                        let resp = ui.add_enabled(available, egui::Button::new(label));
                        let resp =
                            if available { resp } else { resp.on_disabled_hover_text("This plugin has no editor of its own, or it is not loaded") };
                        if resp.clicked()
                            && let Some(p) = &app.player
                        {
                            if open {
                                p.close_editor(tid, slot);
                            } else if let Err(e) = p.open_editor(tid, slot) {
                                app.ui.status = format!("{}: {e}", info.name);
                            }
                        }
                    }
                });
                if instrument && soundcraft_mix::is_third_party(&ins.plugin) {
                    // A hosted instrument keeps its sounds and settings in its own editor.
                    ui.label(egui::RichText::new("Load and edit sounds in the plugin's own editor.").color(Tokens::current().text_dim));
                    return;
                }
                if instrument && ins.plugin == crate::sampler_window::SAMPLER {
                    crate::sampler_window::sample_strip(app, ui, tid, &ins);
                    ui.add_space(4.0);
                }
                if info.id == "eq_7band" || info.id == "eq_1band" {
                    eq_curve(ui, info.id, &ins);
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::Grid::new(("pg", tid.0, slot)).num_columns(3).show(ui, |ui| {
                        for p in info.params {
                            let v = ins.params.get(p.id).copied().unwrap_or(p.default);
                            let label = ui.label(p.name);
                            let mut x = v;
                            let resp = if !p.choices.is_empty() {
                                let mut idx = x.round().max(0.0) as usize;
                                let r = egui::ComboBox::from_id_salt(("pc", tid.0, slot, p.id))
                                    .selected_text(p.choices.get(idx).copied().unwrap_or("?"))
                                    .show_ui(ui, |ui| {
                                        for (i, c) in p.choices.iter().enumerate() {
                                            ui.selectable_value(&mut idx, i, *c);
                                        }
                                    });
                                x = idx as f32;
                                r.response
                            } else {
                                let mut sl = egui::Slider::new(&mut x, p.min..=p.max).show_value(false);
                                if instrument && crate::sampler_window::whole_number(&ins.plugin, p.id) {
                                    sl = sl.integer();
                                }
                                if p.taper == soundcraft_dsp::Taper::Log && p.min > 0.0 {
                                    sl = sl.logarithmic(true);
                                }
                                ui.add(sl)
                            };
                            // The grid cell's painted label is not an accesskit name; tie the
                            // combo box or slider to it so screen readers and label queries find it.
                            let resp = resp.labelled_by(label.id);
                            if (x - v).abs() > f32::EPSILON && instrument {
                                let _ = app.engine.execute_merged(
                                    "mix.instrument_param",
                                    &json!({"track": tid.0, "param": p.id, "value": x}),
                                    &format!("instrument_param:{}:{}", tid.0, p.id),
                                );
                            } else if (x - v).abs() > f32::EPSILON {
                                let _ = app.engine.execute_merged(
                                    "mix.insert_param",
                                    &json!({"track": tid.0, "slot": slot, "param": p.id, "value": x}),
                                    &format!("param:{}:{slot}:{}", tid.0, p.id),
                                );
                                if let Some(pl) = &app.player {
                                    // Keep the plugin's own editor in step.
                                    pl.editor_set_param(tid, slot, p.id, x);
                                }
                            }
                            let _ = resp;
                            let shown = if instrument {
                                crate::sampler_window::format_param(&ins.plugin, p, x)
                            } else {
                                app.player.as_ref().and_then(|pl| pl.editor_value_text(tid, slot, p.id, x)).unwrap_or_else(|| p.format(x))
                            };
                            ui.label(egui::RichText::new(shown).font(mono(11.0)));
                            ui.end_row();
                        }
                    });
                });
            });
        if let Some(w) = shown
            && instrument
            && ins.plugin == crate::sampler_window::SAMPLER
        {
            crate::sampler_window::drop_target(app, ctx, w.response.rect, tid);
        }
        if open {
            keep.push((tid, slot));
        }
    }
    app.ui.plugin_windows = keep;
}

fn eq_curve(ui: &mut Ui, id: &str, ins: &soundcraft_model::Insert) {
    let t = Tokens::current();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width().max(300.0), 110.0), Sense::hover());
    ui.painter().rect_filled(r, 4.0, Color32::from_rgb(14, 18, 22));
    let n = 160;
    let freqs: Vec<f32> = (0..n).map(|i| 20.0 * 1000f32.powf(i as f32 / (n - 1) as f32)).collect();
    let params: Vec<(&str, f32)> = ins.params.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let resp = if id == "eq_7band" {
        soundcraft_dsp::eq7_response(&params, &freqs, 48_000.0)
    } else {
        soundcraft_dsp::eq1_response(&params, &freqs, 48_000.0)
    };
    for db in [-12.0f32, 0.0, 12.0] {
        let y = r.center().y - db / 18.0 * r.height() * 0.5;
        ui.painter().line_segment([pos2(r.min.x, y), pos2(r.max.x, y)], Stroke::new(1.0, Color32::from_rgb(40, 50, 60)));
    }
    let pts: Vec<egui::Pos2> = resp
        .iter()
        .enumerate()
        .map(|(i, db)| pos2(r.min.x + r.width() * i as f32 / (n - 1) as f32, r.center().y - db.clamp(-18.0, 18.0) / 18.0 * r.height() * 0.5))
        .collect();
    ui.painter().add(egui::Shape::line(pts, Stroke::new(2.0, t.counter_text)));
}

/// About window tabs for version information, contributors and models.
fn about(app: &mut SoundApp, ctx: &egui::Context) {
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

fn session_info(app: &mut SoundApp, ctx: &egui::Context) {
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

/// Folder for a plugin's user presets.
fn preset_dir(app: &SoundApp, plugin: &str) -> Option<std::path::PathBuf> {
    app.preset_dir.as_ref().map(|d| d.join(crate::preset_folder_name(plugin)))
}

/// Factory default plus user presets for a plugin insert; user presets need a desktop preset directory.
fn preset_menu(
    app: &mut SoundApp,
    ui: &mut Ui,
    tid: soundcraft_model::TrackId,
    slot: usize,
    info: &soundcraft_dsp::PluginInfo,
    ins: &soundcraft_model::Insert,
) {
    if ui.button("<factory default>").clicked() {
        let defaults: serde_json::Map<String, serde_json::Value> = info.params.iter().map(|p| (p.id.to_string(), json!(p.default))).collect();
        let _ = app.run("mix.insert_params", json!({"track": tid.0, "slot": slot, "params": defaults, "preset": "<factory default>"}));
        ui.close();
    }
    let Some(dir) = preset_dir(app, info.id) else {
        ui.label("User presets need the desktop app.");
        return;
    };
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map(|rd| rd.flatten().filter_map(|e| e.path().file_stem().and_then(std::ffi::OsStr::to_str).map(str::to_string)).collect())
        .unwrap_or_default();
    names.sort();
    for n in &names {
        if ui.button(n).clicked() {
            if let Ok(text) = std::fs::read_to_string(dir.join(format!("{n}.json")))
                && let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
            {
                // Old presets may carry parameters the plugin no longer exposes; drop them so recall succeeds.
                let params: serde_json::Map<String, serde_json::Value> = v
                    .get("params")
                    .and_then(serde_json::Value::as_object)
                    .into_iter()
                    .flatten()
                    .filter(|(k, val)| info.params.iter().any(|pi| pi.id == k.as_str()) && val.as_f64().is_some_and(f64::is_finite))
                    .map(|(k, val)| (k.clone(), val.clone()))
                    .collect();
                let _ = app.run("mix.insert_params", json!({"track": tid.0, "slot": slot, "params": params, "preset": n}));
            }
            ui.close();
        }
    }
    ui.separator();
    let key = egui::Id::unique(("preset_name", tid.0, slot));
    let mut name: String = ui.ctx().memory(|m| m.data.get_temp(key)).unwrap_or_else(|| format!("{} preset", info.name));
    ui.horizontal(|ui| {
        crate::widgets::named_text_field(ui.add(egui::TextEdit::singleline(&mut name).desired_width(140.0)), "Preset name");
        if ui.button("Save").clicked() {
            let clean = soundcraft_engine::io::sanitize_name(&name);
            let body = json!({"format": "soundcraft-plugin-preset", "plugin": info.id, "params": ins.params});
            let ok = std::fs::create_dir_all(&dir).is_ok() && std::fs::write(dir.join(format!("{clean}.json")), body.to_string()).is_ok();
            app.ui.status = if ok { format!("Saved preset {clean}") } else { "Could not save the preset".into() };
            if ok {
                let _ = app.run("mix.insert_params", json!({"track": tid.0, "slot": slot, "params": {}, "preset": clean}));
            }
        }
    });
    ui.ctx().memory_mut(|m| m.data.insert_temp(key, name));
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, Clip, Session, TrackKind};

    #[test]
    fn clip_rows_list_every_clip_in_track_order_and_filter_by_name() {
        let mut s = Session::default();
        for names in [["Kick 1", "Snare 1"], ["Kick 2", "Tom"]] {
            let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
            for (i, n) in names.into_iter().enumerate() {
                let c = Clip::audio(s.new_clip_id(), n, SourceId(1), 0, i as i64 * 100, 100);
                s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(c);
            }
        }
        let names = |rows: Vec<ClipRow>| -> Vec<String> {
            rows.into_iter()
                .filter_map(|r| match r {
                    ClipRow::Clip { track, clip } => Some(s.tracks[track].clips()[clip].name.clone()),
                    ClipRow::Source(_) => None,
                })
                .collect()
        };
        assert_eq!(names(clip_rows(&s, "")), ["Kick 1", "Snare 1", "Kick 2", "Tom"]);
        assert_eq!(names(clip_rows(&s, "kick")), ["Kick 1", "Kick 2"]);
        assert!(clip_rows(&s, "cowbell").is_empty());
    }
}
