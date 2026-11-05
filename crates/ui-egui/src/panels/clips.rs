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

/// How many samples of the source file a clip plays: its timeline length divided by the
/// stretch ratio (the mixer plays `offset + rel / stretch`), rounded to the nearest sample.
/// This is the audio span; video playback ignores the stretch (see `clip_is_subset`).
fn clip_source_span(c: &soundcraft_model::Clip) -> i64 {
    let stretch = if c.stretch.is_finite() && c.stretch > 0.0 { c.stretch } else { 1.0 };
    (c.length as f64 / stretch).round() as i64
}

/// Whether a clip plays only part of a source file `source_len` samples long (#1264): it
/// starts into the file, or it ends before the file does. MIDI clips play their own sequence
/// and are never subsets. (The session model now lives in `craft-scraft-format`, which has
/// no subset predicate, so the check lives with its only caller, the clip list.)
fn clip_is_subset(c: &soundcraft_model::Clip, source_len: i64) -> bool {
    match c.content {
        // Audio plays `length / stretch` samples of the source; video ignores the stretch and
        // plays its timeline length (edit.rs takes the source length as duration × sample rate).
        soundcraft_model::ClipContent::Audio { .. } => c.source_offset() > 0 || clip_source_span(c) < source_len,
        soundcraft_model::ClipContent::Video { .. } => c.source_offset() > 0 || c.length < source_len,
        soundcraft_model::ClipContent::Midi { .. } => false,
    }
}

/// Whether a clip on a track plays only part of its source file (#1264). A clip whose file is
/// not in the session has nothing to be a subset of.
fn clip_subset(s: &soundcraft_model::Session, c: &soundcraft_model::Clip) -> bool {
    match c.content {
        soundcraft_model::ClipContent::Audio { source, .. } => {
            s.source(source).is_some_and(|src| clip_is_subset(c, i64::try_from(src.frames).unwrap_or(i64::MAX)))
        }
        soundcraft_model::ClipContent::Video { source, .. } => s.video(source).is_some_and(|v| clip_is_subset(c, s.sample_rate.samples(v.duration))),
        soundcraft_model::ClipContent::Midi { .. } => false,
    }
}

/// The name a clip row shows in the clip list: subset clips carry a suffix, so they do not read
/// the same as whole-file clips (#1264). The clip's own name is never changed.
fn clip_display_name(s: &soundcraft_model::Session, c: &soundcraft_model::Clip) -> String {
    if clip_subset(s, c) { format!("{} (subset)", c.name) } else { c.name.clone() }
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
        let items: Vec<(u64, String, bool, [u8; 3], bool, bool)> = {
            let s = app.engine.session();
            rows.get(shown)
                .unwrap_or_default()
                .iter()
                .filter_map(|row| match *row {
                    ClipRow::Source(i) => s.sources.get(i).map(|src| {
                        (
                            src.id.0,
                            format!("{} ({}ch)", src.name, src.channels),
                            true,
                            [150, 150, 150],
                            s.edit.selected_sources.contains(&src.id),
                            false,
                        )
                    }),
                    ClipRow::Clip { track, clip } => {
                        let tr = s.tracks.get(track)?;
                        let c = tr.clips().get(clip)?;
                        Some((
                            c.id.0,
                            clip_display_name(s, c),
                            false,
                            c.color.unwrap_or(tr.color),
                            s.edit.selected_clips.contains(&c.id),
                            clip_subset(s, c),
                        ))
                    }
                })
                .collect()
        };
        for (id, name, whole, color, selected, subset) in items {
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
                // Subset clips are grayed out (#1264); whole files and whole-file clips are not.
                if subset { t.text_dim } else { t.text },
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

    #[test]
    fn subset_detection_covers_only_part_of_the_source() {
        // #1264: a clip spanning the whole file is a whole-file clip; one that starts into the
        // file or ends before the file does is a subset.
        let whole = Clip::audio(ClipId(1), "a", SourceId(1), 0, 0, 1000);
        assert!(!clip_is_subset(&whole, 1000));
        assert!(clip_is_subset(&Clip::audio(ClipId(2), "a", SourceId(1), 0, 0, 999), 1000));
        assert!(clip_is_subset(&Clip::audio(ClipId(3), "a", SourceId(1), 1, 0, 999), 1000));
        assert!(clip_is_subset(&Clip::audio(ClipId(4), "a", SourceId(1), 500, 0, 500), 1000));
        // A clip running past the end of the file still covers it whole when it starts at 0.
        assert!(!clip_is_subset(&Clip::audio(ClipId(5), "a", SourceId(1), 0, 0, 2000), 1000));
        // Stretch: a timeline length of 2000 at 2x plays 1000 source samples, the whole file.
        let mut stretched = Clip::audio(ClipId(6), "a", SourceId(1), 0, 0, 2000);
        stretched.stretch = 2.0;
        assert_eq!(clip_source_span(&stretched), 1000);
        assert!(!clip_is_subset(&stretched, 1000));
        // MIDI clips play their own sequence, never part of a file.
        let midi = Clip::midi(ClipId(7), "m", 0, 100, soundcraft_midi::Sequence::default());
        assert!(!clip_is_subset(&midi, 0));
    }

    #[test]
    fn video_subsets_ignore_audio_stretch() {
        // Video playback uses timeline samples directly, even when audio stretch is set.
        for (offset, length, subset) in [(0, 1000, false), (0, 999, true), (1, 1000, true)] {
            let mut video = Clip::video(ClipId(8), "v", SourceId(1), offset, 0, length);
            video.stretch = 2.0;
            assert_eq!(clip_is_subset(&video, 1000), subset, "offset {offset}, length {length}");
        }
    }

    #[test]
    fn subset_clips_get_a_distinct_name_in_the_clip_list() {
        // #1264: a subset clip (part of a file) must not read the same as a whole-file clip.
        let mut s = Session::default();
        s.sources.push(soundcraft_model::Source {
            id: SourceId(1),
            name: "Vox.wav".into(),
            path: "Audio Files/Vox.wav".into(),
            channels: 1,
            frames: 1000,
            sample_rate: 48_000,
            format: soundcraft_model::FileFormat::Wav,
            time_reference: 0,
            unsaved: false,
        });
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let whole = Clip::audio(s.new_clip_id(), "Vox", SourceId(1), 0, 0, 1000);
        let subset = Clip::audio(s.new_clip_id(), "Vox", SourceId(1), 250, 1000, 500);
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(whole.clone());
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(subset.clone());
        assert!(!clip_subset(&s, &whole));
        assert!(clip_subset(&s, &subset));
        assert_eq!(clip_display_name(&s, &whole), "Vox");
        assert_eq!(clip_display_name(&s, &subset), "Vox (subset)");
        // A clip whose file is not in the session has nothing to be a subset of.
        let orphan = Clip::audio(s.new_clip_id(), "Ghost", SourceId(99), 10, 0, 10);
        assert!(!clip_subset(&s, &orphan));
        assert_eq!(clip_display_name(&s, &orphan), "Ghost");
    }
}
