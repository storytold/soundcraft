//! Borrowed document equality for the saved-state indicator.

use soundcraft_model::{EditState, Session};

/// Equal apart from the selection, playhead and zoom, without copying document data.
pub(super) fn same_document(saved: &Session, doc: &Session) -> bool {
    document_values(saved, &saved.edit) == document_values(doc, &saved.edit)
}

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
        tracks,
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
        pool,
        folder,
    } = s;
    // Tuples implement PartialEq up to twelve elements. These contain only borrowed fields.
    (
        (format_version, name, sample_rate, bit_depth, frame_rate, timecode_start, tempo, key_signatures, tracks, sources, videos, busses),
        (outputs, markers, groups, auto_names, edit_values(edit, navigation), next_id, comments, midi_originals, pool, folder),
    )
}

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
        (pre_fader_metering, delay_compensation, keyboard_focus, &navigation.timeline_selection, flags, values),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, Clip, ClipId, SourceId, TrackId, TrackKind, ZoomToggle};
    use soundcraft_time::{Range, TimeFormat};
    use std::sync::Arc;

    // The previous comparison is an oracle for preserving the saved-state policy.
    fn previous_same_document(saved: &Session, doc: &Session) -> bool {
        let mut doc = doc.clone();
        let (from, to) = (&saved.edit, &mut doc.edit);
        to.selection = from.selection;
        to.timeline_selection = from.timeline_selection;
        to.selected_tracks.clone_from(&from.selected_tracks);
        to.selected_clips.clone_from(&from.selected_clips);
        to.playhead = from.playhead;
        to.zoom = from.zoom;
        *saved == doc
    }

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
        let audio =
            Arc::new(soundcraft_model::SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 100]] }));
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
