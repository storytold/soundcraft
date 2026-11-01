use super::{Align2, ClipId, Rect, Sense, SoundApp, SourceId, Tokens, Ui, bold, json, panel_header, pos2, regular, rgb, vec2, with_pending_offset};

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
