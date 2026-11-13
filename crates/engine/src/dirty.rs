//! Borrowed document equality for the saved-state indicator.

use soundcraft_model::{EditState, Mixer, Session, Track};

/// Equal apart from navigation and live input flags, without copying document data.
pub(super) fn same_document(saved: &Session, doc: &Session) -> bool {
    document_values(saved, &saved.edit) == document_values(doc, &saved.edit)
        && saved.tracks.len() == doc.tracks.len()
        && saved.tracks.iter().zip(&doc.tracks).all(|(saved, doc)| track_values(saved) == track_values(doc))
}

/// Borrow persistent session fields; track fields are compared separately.
fn document_values<'a>(s: &'a Session, navigation: &'a EditState) -> impl PartialEq + 'a {
    // No `..`: adding a document field must also decide how it affects saved-state equality.
    let Session {
        format_version,
        name,
        sample_rate,
        bit_depth,
        frame_rate,
        timecode_start,
        tempo,
        key_signatures,
        tracks: _,
        sources,
        videos,
        busses,
        outputs,
        markers,
        groups,
        auto_names,
        edit,
        next_id,
        comments,
        midi_originals,
        room_tone,
        pool,
        folder,
    } = s;
    // Tuples implement PartialEq up to twelve elements. These contain only borrowed fields.
    (
        (format_version, name, sample_rate, bit_depth, frame_rate, timecode_start, tempo, key_signatures, sources, videos, busses),
        (outputs, markers, groups, auto_names, edit_values(edit, navigation), next_id, comments, midi_originals, room_tone, pool, folder),
    )
}

/// Borrow persistent track fields without allocating or copying media.
fn track_values(track: &Track) -> impl PartialEq + '_ {
    let Track {
        id,
        name,
        kind,
        format,
        color,
        height,
        playlists,
        active_playlist,
        mixer,
        automation,
        view,
        hidden,
        inactive,
        frozen,
        comments,
        folder,
        folder_open,
        instrument,
        ticks_timebase,
        elastic,
        pinned,
    } = track;
    (
        (id, name, kind, format, color, height, playlists, active_playlist, mixer_values(mixer), automation, view),
        (hidden, inactive, frozen, comments, folder, folder_open, instrument, ticks_timebase, elastic, pinned),
    )
}

/// Borrow persistent mixer fields without allocating or copying media.
fn mixer_values(mixer: &Mixer) -> impl PartialEq + '_ {
    let Mixer {
        volume_db,
        pan,
        mute,
        solo,
        solo_safe,
        record_arm: _,
        input_monitor: _,
        phase_invert,
        input,
        output,
        inserts,
        sends,
        automation_mode,
        trim_db,
        vca,
        surround,
    } = mixer;
    ((volume_db, pan, mute, solo, solo_safe, phase_invert, input, output, inserts, sends, automation_mode), (trim_db, vca, surround))
}

/// Borrow edit settings while normalizing transient navigation to the saved values.
fn edit_values<'a>(edit: &'a EditState, navigation: &'a EditState) -> impl PartialEq + 'a {
    let EditState {
        tool,
        edit_mode,
        selection: _,
        selected_tracks: _,
        selected_clips: _,
        selected_sources,
        playhead: _,
        grid,
        nudge,
        grid_lines,
        grid_follow_main,
        nudge_follow_main,
        grid_clips_markers,
        main_counter,
        sub_counter,
        zoom: _,
        zoom_toggle,
        rulers,
        link_timeline_edit,
        link_track_edit,
        insertion_follows_playback,
        loop_playback,
        pre_post_roll,
        pre_roll,
        post_roll,
        tab_to_transient,
        automation_follows_edit,
        markers_follow_edit,
        layered_editing,
        mirrored_midi,
        click,
        countoff,
        countoff_bars,
        midi_merge,
        record_mode,
        loop_record,
        quickpunch,
        scrolling,
        solo_mode,
        pre_fader_metering,
        delay_compensation,
        keyboard_focus,
        timeline_selection: _,
        flags,
        values,
    } = edit;
    // Borrow the saved navigation on both sides instead of overwriting a document clone.
    // Keeping these values in the comparison also preserves PartialEq for non-finite zooms.
    (
        (
            tool,
            edit_mode,
            &navigation.selection,
            &navigation.selected_tracks,
            &navigation.selected_clips,
            selected_sources,
            &navigation.playhead,
            grid,
            nudge,
            grid_lines,
            main_counter,
            sub_counter,
        ),
        (
            &navigation.zoom,
            zoom_toggle,
            rulers,
            link_timeline_edit,
            link_track_edit,
            insertion_follows_playback,
            loop_playback,
            pre_post_roll,
            pre_roll,
            post_roll,
            tab_to_transient,
            automation_follows_edit,
        ),
        (
            markers_follow_edit,
            layered_editing,
            mirrored_midi,
            click,
            countoff,
            countoff_bars,
            midi_merge,
            record_mode,
            loop_record,
            quickpunch,
            scrolling,
            solo_mode,
        ),
        (
            grid_follow_main,
            nudge_follow_main,
            grid_clips_markers,
            pre_fader_metering,
            delay_compensation,
            keyboard_focus,
            &navigation.timeline_selection,
            flags,
            values,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, Clip, ClipId, SourceId, TrackId, TrackKind, ZoomToggle};
    use soundcraft_time::{Range, TimeFormat};
    use std::sync::Arc;

    /// The clone-based policy, extended to ignore live input flags, serves as a test oracle.
    fn previous_same_document(saved: &Session, doc: &Session) -> bool {
        let mut doc = doc.clone();
        let (from, to) = (&saved.edit, &mut doc.edit);
        to.selection = from.selection;
        to.timeline_selection = from.timeline_selection;
        to.selected_tracks.clone_from(&from.selected_tracks);
        to.selected_clips.clone_from(&from.selected_clips);
        to.playhead = from.playhead;
        to.zoom = from.zoom;
        for (saved, doc) in saved.tracks.iter().zip(&mut doc.tracks) {
            doc.mixer.record_arm = saved.mixer.record_arm;
            doc.mixer.input_monitor = saved.mixer.input_monitor;
        }
        *saved == doc
    }

    /// Check both directions and compare against the normalized clone oracle.
    fn check(saved: &Session, doc: &Session, expected: bool, field: &str) {
        assert_eq!(same_document(saved, doc), expected, "{field}");
        assert_eq!(same_document(doc, saved), expected, "{field}, reversed");
        assert_eq!(same_document(saved, doc), previous_same_document(saved, doc), "{field}, previous policy");
    }

    #[test]
    fn navigation_fields_are_ignored() {
        let saved = Session::default();
        let mut doc = saved.clone();
        doc.edit.selection = Range::new(100, 200);
        doc.edit.timeline_selection = Range::new(300, 400);
        doc.edit.selected_tracks = vec![TrackId(50)];
        doc.edit.selected_clips = vec![ClipId(60)];
        doc.edit.playhead = 500;
        doc.edit.zoom.samples_per_px = 64.0;
        doc.edit.zoom.scroll = 600;
        doc.edit.zoom.waveform_zoom = 2.0;
        doc.edit.zoom.presets = [1.0; 5];
        check(&saved, &doc, true, "navigation");
    }

    #[test]
    fn persisted_edit_settings_still_change_document_equality() {
        let saved = Session::default();
        let edits: &[(&str, fn(&mut EditState))] = &[
            ("tool", |e| e.tool = soundcraft_model::Tool::Grabber),
            ("edit_mode", |e| e.edit_mode = soundcraft_model::EditMode::Grid),
            ("selected_sources", |e| e.selected_sources.push(SourceId(1))),
            ("grid", |e| e.grid = soundcraft_time::GridValue::Seconds(2.0)),
            ("nudge", |e| e.nudge = soundcraft_time::GridValue::Seconds(2.0)),
            ("grid_lines", |e| e.grid_lines = !e.grid_lines),
            ("grid_follow_main", |e| e.grid_follow_main = !e.grid_follow_main),
            ("nudge_follow_main", |e| e.nudge_follow_main = !e.nudge_follow_main),
            ("grid_clips_markers", |e| e.grid_clips_markers = !e.grid_clips_markers),
            ("main_counter", |e| e.main_counter = TimeFormat::Samples),
            ("sub_counter", |e| e.sub_counter = None),
            ("zoom_toggle", |e| e.zoom_toggle = Some(ZoomToggle { samples_per_px: 64.0, scroll: 0, heights: Vec::new() })),
            ("rulers", |e| e.rulers.clear()),
            ("link_timeline_edit", |e| e.link_timeline_edit = !e.link_timeline_edit),
            ("link_track_edit", |e| e.link_track_edit = !e.link_track_edit),
            ("insertion_follows_playback", |e| e.insertion_follows_playback = !e.insertion_follows_playback),
            ("loop_playback", |e| e.loop_playback = !e.loop_playback),
            ("pre_post_roll", |e| e.pre_post_roll = !e.pre_post_roll),
            ("pre_roll", |e| e.pre_roll += 1),
            ("post_roll", |e| e.post_roll += 1),
            ("tab_to_transient", |e| e.tab_to_transient = !e.tab_to_transient),
            ("automation_follows_edit", |e| e.automation_follows_edit = !e.automation_follows_edit),
            ("markers_follow_edit", |e| e.markers_follow_edit = !e.markers_follow_edit),
            ("layered_editing", |e| e.layered_editing = !e.layered_editing),
            ("mirrored_midi", |e| e.mirrored_midi = !e.mirrored_midi),
            ("click", |e| e.click = !e.click),
            ("countoff", |e| e.countoff = !e.countoff),
            ("countoff_bars", |e| e.countoff_bars += 1),
            ("midi_merge", |e| e.midi_merge = !e.midi_merge),
            ("record_mode", |e| e.record_mode = "destructive".into()),
            ("loop_record", |e| e.loop_record = !e.loop_record),
            ("quickpunch", |e| e.quickpunch = !e.quickpunch),
            ("scrolling", |e| e.scrolling = "continuous".into()),
            ("solo_mode", |e| e.solo_mode = "afl".into()),
            ("pre_fader_metering", |e| e.pre_fader_metering = !e.pre_fader_metering),
            ("delay_compensation", |e| e.delay_compensation = !e.delay_compensation),
            ("keyboard_focus", |e| e.keyboard_focus = "midi".into()),
            ("flags", |e| {
                e.flags.insert("test.flag".into());
            }),
            ("values", |e| {
                e.values.insert("test.value".into(), 0.5);
            }),
        ];
        for &(field, edit) in edits {
            let mut doc = saved.clone();
            edit(&mut doc.edit);
            check(&saved, &doc, false, field);
        }
    }

    #[test]
    fn track_and_media_changes_still_change_document_equality() {
        let mut saved = Session::default();
        let track = saved.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let clip = Clip::audio(saved.new_clip_id(), "Clip", SourceId(900), 0, 0, 100);
        saved.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(clip);
        let audio = Arc::new(soundcraft_model::SourceAudio::new(craft_audio::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 100]] }));
        saved.pool.insert(SourceId(900), audio);
        check(&saved, &saved.clone(), true, "shared decoded audio");
        let mut doc = saved.clone();
        doc.track_mut(track).unwrap().playlist_mut().unwrap().clips[0].gain_db = -6.0;
        check(&saved, &doc, false, "clip gain");
        let mut doc = saved.clone();
        doc.pool.remove(SourceId(900));
        check(&saved, &doc, false, "decoded audio removed");
        let mut doc = saved.clone();
        doc.folder = Some(std::path::PathBuf::from("session"));
        check(&saved, &doc, false, "session folder");
        let mut doc = saved.clone();
        doc.edit.values.insert("test.nan".into(), f64::NAN);
        assert_eq!(same_document(&doc, &doc), previous_same_document(&doc, &doc), "NaN keeps the previous PartialEq behavior");
    }

    /// Only arm and monitoring are transient; every other mixer field still contributes to dirtiness.
    #[test]
    fn live_flags_do_not_hide_persistent_mixer_changes() {
        use soundcraft_model::{AutomationMode, BusId, Insert, Route, SendSlot, SurroundPan};
        let mut saved = Session::default();
        let track = saved.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let mut live = saved.clone();
        live.track_mut(track).unwrap().mixer.record_arm = true;
        live.track_mut(track).unwrap().mixer.input_monitor = true;
        check(&saved, &live, true, "live input flags");
        let edits: &[(&str, fn(&mut Mixer))] = &[
            ("volume", |m| m.volume_db = -6.0),
            ("pan", |m| m.pan[0] = 0.5),
            ("mute", |m| m.mute = true),
            ("solo", |m| m.solo = true),
            ("solo_safe", |m| m.solo_safe = true),
            ("phase_invert", |m| m.phase_invert = true),
            ("input", |m| m.input = Route::Hardware("Mic".into())),
            ("output", |m| m.output = Route::Bus(BusId(900))),
            ("inserts", |m| m.inserts[0] = Some(Insert::new("eq7"))),
            ("sends", |m| m.sends[0] = Some(SendSlot::new(Route::Main))),
            ("automation_mode", |m| m.automation_mode = AutomationMode::Touch),
            ("trim_db", |m| m.trim_db = -3.0),
            ("vca", |m| m.vca = Some(900)),
            ("surround", |m| m.surround = Some(SurroundPan::default())),
        ];
        for &(field, edit) in edits {
            let mut doc = live.clone();
            edit(&mut doc.track_mut(track).unwrap().mixer);
            check(&saved, &doc, false, field);
        }
        let mut doc = live.clone();
        doc.tracks.push(doc.tracks[0].clone());
        check(&saved, &doc, false, "track count");
        let mut doc = live.clone();
        doc.tracks[0].id = TrackId(900);
        check(&saved, &doc, false, "track identity");
        let mut doc = live.clone();
        doc.tracks[0].mixer.volume_db = f32::NAN;
        assert!(!same_document(&doc, &doc), "persistent NaN keeps PartialEq semantics");
    }

    #[test]
    fn non_finite_saved_zoom_keeps_the_previous_partial_eq_behavior() {
        let mut saved = Session::default();
        let doc = saved.clone();
        saved.edit.zoom.waveform_zoom = f32::NAN;
        assert!(!same_document(&saved, &doc));
        assert!(same_document(&doc, &saved));
        assert_eq!(same_document(&saved, &doc), previous_same_document(&saved, &doc));
        assert_eq!(same_document(&doc, &saved), previous_same_document(&doc, &saved));
    }
}
