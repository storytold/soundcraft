//! More View menu commands: Edit-window column views, extra rulers, marker displays, clip
//! display options, waveform styles, expanded sends, transport views and other view toggles.
//! Every setting is saved with the session in `edit.flags` (or `edit.rulers` for rulers).
use super::{CommandSpec, always, str_param};

use super::more_util::{exclusive_flag, set_flags, toggle_flag};
use crate::{Engine, Result, cmd};
use serde_json::{Value, json};

/// A view toggle: `{value?: bool}` sets it, otherwise it flips.
macro_rules! flag {
    ($id:literal, $label:literal, [$($m:literal),*], $flag:literal) => {
        cmd!(noundo $id, $label, [$($m),*], None, "{value?: bool} (toggles when omitted)", always, |e, p| toggle_flag(e, p, $flag))
    };
}

/// A radio item: turns its flag on and the rest of its group off.
macro_rules! radio {
    ($id:literal, $label:literal, [$($m:literal),*], $flag:literal, $group:expr) => {
        cmd!(noundo $id, $label, [$($m),*], None, "{}", always, |e, _| exclusive_flag(e, $flag, $group))
    };
}

/// Edit window column views (View › Edit Window Views).
pub const EDIT_VIEWS: [&str; 14] = [
    "edit_view.comments",
    "edit_view.mic_preamps",
    "edit_view.instruments",
    "edit_view.inserts_ae",
    "edit_view.inserts_fj",
    "edit_view.sends_ae",
    "edit_view.sends_fj",
    "edit_view.io",
    "edit_view.object",
    "edit_view.realtime_properties",
    "edit_view.track_color",
    "edit_view.marker_controls",
    "edit_view.pinned_tracks",
    "edit_view.track_number",
];

const WAVEFORM_STYLE: &[&str] = &["waveform.peak", "waveform.power"];
const CLIP_TIME: &[&str] = &["view.clip.time.current", "view.clip.time.original", "view.clip.time.user", "view.clip.time.none"];
const SENDS: [&str; 10] = [
    "expanded_send.a",
    "expanded_send.b",
    "expanded_send.c",
    "expanded_send.d",
    "expanded_send.e",
    "expanded_send.f",
    "expanded_send.g",
    "expanded_send.h",
    "expanded_send.i",
    "expanded_send.j",
];

/// Every ruler id, in display order (the first ten are also served by `view.ruler`).
pub const RULERS: [&str; 15] = [
    "bars_beats",
    "min_secs",
    "timecode",
    "timecode2",
    "feet_frames",
    "samples",
    "tempo",
    "meter",
    "key",
    "chords",
    "markers",
    "markers2",
    "markers3",
    "markers4",
    "markers5",
];
const MARKER_RULERS: [&str; 5] = ["markers", "markers2", "markers3", "markers4", "markers5"];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        // Edit Window Views.
        flag!("view.edit_comments", "Comments", ["View", "Edit Window Views"], "edit_view.comments"),
        flag!("view.edit_mic_preamps", "Mic Preamps", ["View", "Edit Window Views"], "edit_view.mic_preamps"),
        flag!("view.edit_instruments", "Instruments", ["View", "Edit Window Views"], "edit_view.instruments"),
        flag!("view.edit_inserts_ae", "Inserts A-E", ["View", "Edit Window Views"], "edit_view.inserts_ae"),
        flag!("view.edit_inserts_fj", "Inserts F-J", ["View", "Edit Window Views"], "edit_view.inserts_fj"),
        flag!("view.edit_sends_ae", "Sends A-E", ["View", "Edit Window Views"], "edit_view.sends_ae"),
        flag!("view.edit_sends_fj", "Sends F-J", ["View", "Edit Window Views"], "edit_view.sends_fj"),
        flag!("view.edit_io", "I/O", ["View", "Edit Window Views"], "edit_view.io"),
        flag!("view.edit_object", "Object", ["View", "Edit Window Views"], "edit_view.object"),
        flag!("view.edit_realtime_properties", "Real-Time Properties", ["View", "Edit Window Views"], "edit_view.realtime_properties"),
        flag!("view.edit_track_color", "Track Color", ["View", "Edit Window Views"], "edit_view.track_color"),
        flag!("view.edit_marker_controls", "Marker Controls", ["View", "Edit Window Views"], "edit_view.marker_controls"),
        flag!("view.edit_pinned_tracks", "Pinned Tracks", ["View", "Edit Window Views"], "edit_view.pinned_tracks"),
        cmd!(noundo "view.edit_all", "All", ["View", "Edit Window Views"], None, "{} — shows every Edit window column", always, |e, _| Ok(set_flags(e, &EDIT_VIEWS, true))),
        cmd!(noundo "view.edit_minimal", "Minimal", ["View", "Edit Window Views"], None, "{} — hides every optional Edit window column", always, |e, _| Ok(set_flags(e, &EDIT_VIEWS, false))),
        // Rulers.
        cmd!(noundo "view.ruler_timecode2", "Timecode 2", ["View", "Rulers"], None, "{visible?: bool}", always, |e, p| toggle_ruler(e, p, "timecode2")),
        cmd!(noundo "view.ruler_markers2", "Markers 2", ["View", "Rulers"], None, "{visible?: bool}", always, |e, p| toggle_ruler(e, p, "markers2")),
        cmd!(noundo "view.ruler_markers3", "Markers 3", ["View", "Rulers"], None, "{visible?: bool}", always, |e, p| toggle_ruler(e, p, "markers3")),
        cmd!(noundo "view.ruler_markers4", "Markers 4", ["View", "Rulers"], None, "{visible?: bool}", always, |e, p| toggle_ruler(e, p, "markers4")),
        cmd!(noundo "view.ruler_markers5", "Markers 5", ["View", "Rulers"], None, "{visible?: bool}", always, |e, p| toggle_ruler(e, p, "markers5")),
        cmd!(noundo "view.ruler_all_markers", "All Marker Rulers", ["View", "Rulers"], None, "{visible?: bool} — shows (or hides) all five marker rulers", always, |e, p| {
            let s = e.session_mut();
            let vis = p.get("visible").and_then(Value::as_bool).unwrap_or(!MARKER_RULERS.iter().all(|r| s.edit.rulers.iter().any(|x| x == r)));
            for r in MARKER_RULERS {
                set_ruler(&mut s.edit.rulers, r, vis);
            }
            Ok(json!({"rulers": s.edit.rulers}))
        }),
        cmd!(noundo "view.ruler_all", "All", ["View", "Rulers"], None, "{} — shows every ruler", always, |e, _| {
            let s = e.session_mut();
            s.edit.rulers = RULERS.iter().map(|r| r.to_string()).collect();
            Ok(json!({"rulers": s.edit.rulers}))
        }),
        cmd!(noundo "view.ruler_minimal", "Minimal", ["View", "Rulers"], None, "{} — only the main time ruler and markers", always, |e, _| {
            let s = e.session_mut();
            let main = s.edit.main_counter.id().to_string();
            s.edit.rulers = vec![main, "markers".into()];
            sort_rulers(&mut s.edit.rulers);
            Ok(json!({"rulers": s.edit.rulers}))
        }),
        flag!("view.ruler_tempo_editor", "Tempo Editor", ["View", "Rulers", "Tempo"], "view.ruler.tempo_editor"),
        flag!("view.ruler_key_staff", "Key Signature Staff", ["View", "Rulers", "Key Signature"], "view.ruler.key_staff"),
        // Marker displays.
        flag!("view.marker_track_lane", "Track Lane", ["View", "Marker Displays"], "view.marker.track_lane"),
        flag!("view.marker_ruler_lines", "Ruler Lines", ["View", "Marker Displays"], "view.marker.ruler_lines"),
        flag!("view.marker_folder_lines", "Folder Lines", ["View", "Marker Displays"], "view.marker.folder_lines"),
        flag!("view.marker_ruler_lane_color", "Ruler Lane Color", ["View", "Marker Displays"], "view.marker.ruler_lane_color"),
        flag!("view.marker_track_lane_color", "Track Lane Color", ["View", "Marker Displays"], "view.marker.track_lane_color"),
        // Other displays.
        flag!("view.midi_input_display", "MIDI Input Display", ["View", "Other Displays"], "view.midi_input_display"),
        flag!(
            "view.midi_editor_clip_effects",
            "Clip Effects",
            ["View", "Other Displays", "Lower Dock", "MIDI Editor"],
            "view.lower_dock.midi_editor.clip_effects"
        ),
        // Clip display.
        flag!("view.clip_sync_point", "Sync Point", ["View", "Clip"], "view.clip.sync_point"),
        flag!("view.clip_processing_state", "Processing State", ["View", "Clip"], "view.clip.processing_state"),
        flag!("view.clip_name", "Name", ["View", "Clip"], "view.clip.name"),
        flag!("view.clip_channel_name", "Channel Name", ["View", "Clip"], "view.clip.channel_name"),
        flag!("view.clip_scene_take", "Scene And Take", ["View", "Clip"], "view.clip.scene_take"),
        flag!("view.clip_rating", "Rating", ["View", "Clip"], "view.clip.rating"),
        flag!("view.clip_overlap_shadows", "Overlap Shadows", ["View", "Clip"], "view.clip.overlap_shadows"),
        flag!("view.clip_transparency", "Transparency", ["View", "Clip"], "view.clip.transparency"),
        flag!("view.clip_overwrite_indicator", "Overwrite Indicator", ["View", "Clip"], "view.clip.overwrite_indicator"),
        flag!("view.clip_copy_move_indicators", "Copy/Move Indicators", ["View", "Clip"], "view.clip.copy_move_indicators"),
        flag!("view.clip_gain_line", "Clip Gain Line", ["View", "Clip"], "view.clip.gain_line"),
        flag!("view.clip_gain_info", "Clip Gain Info", ["View", "Clip"], "view.clip.gain_info"),
        flag!("view.clip_effects_status", "Clip Effects Status", ["View", "Clip"], "view.clip.effects_status"),
        flag!("view.clip_ara_note_overlay", "ARA Note Overlay", ["View", "Clip"], "view.clip.ara_note_overlay"),
        radio!("view.clip_time_current", "Current Time", ["View", "Clip"], "view.clip.time.current", CLIP_TIME),
        radio!("view.clip_time_original", "Original Time Stamp", ["View", "Clip"], "view.clip.time.original", CLIP_TIME),
        radio!("view.clip_time_user", "User Time Stamp", ["View", "Clip"], "view.clip.time.user", CLIP_TIME),
        radio!("view.clip_time_none", "No Time", ["View", "Clip"], "view.clip.time.none", CLIP_TIME),
        flag!("view.clip_all_channels", "Display on All Channels", ["View", "Clip"], "view.clip.all_channels"),
        // Waveforms.
        radio!("view.waveform_peak", "Peak", ["View", "Waveforms"], "waveform.peak", WAVEFORM_STYLE),
        radio!("view.waveform_power", "Power", ["View", "Waveforms"], "waveform.power", WAVEFORM_STYLE),
        flag!("view.waveform_rectified", "Rectified", ["View", "Waveforms"], "waveform.rectified"),
        flag!("view.waveform_outlines", "Outlines", ["View", "Waveforms"], "waveform.outlines"),
        flag!("view.waveform_overlapped_crossfades", "Overlapped Crossfades", ["View", "Waveforms"], "waveform.overlapped_crossfades"),
        // Automation playlists.
        flag!("view.automation_trim_playlist", "Trim Playlist", ["View", "Automation"], "view.automation.trim_playlist"),
        flag!("view.automation_composite_playlist", "Composite Playlist", ["View", "Automation"], "view.automation.composite_playlist"),
        // Expanded sends.
        cmd!(noundo "view.expanded_sends_all", "All", ["View", "Expanded Sends"], None, "{} — expands every send in the sends view", always, |e, _| Ok(set_flags(e, &SENDS, true))),
        cmd!(noundo "view.expanded_sends_none", "None", ["View", "Expanded Sends"], None, "{}", always, |e, _| Ok(set_flags(e, &SENDS, false))),
        flag!("view.expanded_send_a", "Send A", ["View", "Expanded Sends"], "expanded_send.a"),
        flag!("view.expanded_send_b", "Send B", ["View", "Expanded Sends"], "expanded_send.b"),
        flag!("view.expanded_send_c", "Send C", ["View", "Expanded Sends"], "expanded_send.c"),
        flag!("view.expanded_send_d", "Send D", ["View", "Expanded Sends"], "expanded_send.d"),
        flag!("view.expanded_send_e", "Send E", ["View", "Expanded Sends"], "expanded_send.e"),
        flag!("view.expanded_send_f", "Send F", ["View", "Expanded Sends"], "expanded_send.f"),
        flag!("view.expanded_send_g", "Send G", ["View", "Expanded Sends"], "expanded_send.g"),
        flag!("view.expanded_send_h", "Send H", ["View", "Expanded Sends"], "expanded_send.h"),
        flag!("view.expanded_send_i", "Send I", ["View", "Expanded Sends"], "expanded_send.i"),
        flag!("view.expanded_send_j", "Send J", ["View", "Expanded Sends"], "expanded_send.j"),
        // Misc.
        flag!("view.track_number", "Track Number", ["View"], "edit_view.track_number"),
        flag!("view.track_transcription_lane", "Track Transcription Lane", ["View"], "view.track_transcription_lane"),
        flag!("view.dropped_video_frame_indicator", "Dropped Video Frame Indicator", ["View"], "view.dropped_video_frame_indicator"),
        // Transport window views.
        flag!("view.transport_counters", "Counters", ["View", "Transport"], "view.transport.counters"),
        flag!("view.transport_midi_controls", "MIDI Controls", ["View", "Transport"], "view.transport.midi_controls"),
        flag!("view.transport_synchronization", "Synchronization", ["View", "Transport"], "view.transport.synchronization"),
        flag!("view.transport_ableton_link", "Ableton Link", ["View", "Transport"], "view.transport.ableton_link"),
        flag!("view.transport_output_meters", "Output Meters", ["View", "Transport"], "view.transport.output_meters"),
        flag!("view.transport_expanded", "Expanded", ["View", "Transport"], "view.transport.expanded"),
        cmd!(query "view.flags", "View Flags", [], None, "{prefix?} — lists the view/option flags that are on", always, |e, p| {
            let pre = str_param(p, "prefix").unwrap_or("");
            let f: Vec<&String> = e.session().edit.flags.iter().filter(|f| f.starts_with(pre)).collect();
            Ok(json!({"flags": f, "rulers": e.session().edit.rulers}))
        }),
    ]
}

fn sort_rulers(r: &mut Vec<String>) {
    r.sort_by_key(|x| RULERS.iter().position(|a| a == x).unwrap_or(usize::MAX));
    r.dedup();
}

fn set_ruler(rulers: &mut Vec<String>, id: &str, vis: bool) {
    rulers.retain(|x| x != id);
    if vis {
        rulers.push(id.to_string());
        sort_rulers(rulers);
    }
}

fn toggle_ruler(e: &mut Engine, p: &Value, id: &str) -> Result<Value> {
    let s = e.session_mut();
    let shown = s.edit.rulers.iter().any(|x| x == id);
    let vis = p.get("visible").and_then(Value::as_bool).unwrap_or(!shown);
    set_ruler(&mut s.edit.rulers, id, vis);
    Ok(json!({"ruler": id, "visible": vis}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flags_toggle_and_persist() {
        let mut e = Engine::default();
        let r = e.execute("view.clip_rating", &json!({})).unwrap();
        assert_eq!(r["value"], true);
        assert!(e.session().edit.flag("view.clip.rating"));
        e.execute("view.clip_rating", &json!({})).unwrap();
        assert!(!e.session().edit.flag("view.clip.rating"));
        e.execute("view.edit_inserts_ae", &json!({"value": true})).unwrap();
        let text = e.session().to_json().unwrap();
        let back = soundcraft_model::Session::from_json(&text).unwrap();
        assert!(back.edit.flag("edit_view.inserts_ae"));
        assert!(!e.can_undo(), "view toggles are not undo steps");
    }

    #[test]
    fn radio_groups_are_exclusive() {
        let mut e = Engine::default();
        assert!(e.session().edit.flag("waveform.peak"));
        e.execute("view.waveform_power", &json!({})).unwrap();
        assert!(e.session().edit.flag("waveform.power"));
        assert!(!e.session().edit.flag("waveform.peak"));
        e.execute("view.clip_time_user", &json!({})).unwrap();
        e.execute("view.clip_time_none", &json!({})).unwrap();
        assert!(!e.session().edit.flag("view.clip.time.user"));
        assert!(e.session().edit.flag("view.clip.time.none"));
    }

    #[test]
    fn all_and_minimal() {
        let mut e = Engine::default();
        e.execute("view.edit_all", &json!({})).unwrap();
        assert!(EDIT_VIEWS.iter().all(|f| e.session().edit.flag(f)));
        e.execute("view.edit_minimal", &json!({})).unwrap();
        assert!(EDIT_VIEWS.iter().all(|f| !e.session().edit.flag(f)));
        e.execute("view.expanded_sends_all", &json!({})).unwrap();
        assert!(e.session().edit.flag("expanded_send.j"));
        e.execute("view.expanded_sends_none", &json!({})).unwrap();
        assert!(!e.session().edit.flag("expanded_send.a"));
    }

    #[test]
    fn rulers() {
        let mut e = Engine::default();
        e.execute("view.ruler_markers3", &json!({})).unwrap();
        assert!(e.session().edit.rulers.iter().any(|r| r == "markers3"));
        e.execute("view.ruler_all_markers", &json!({})).unwrap();
        assert!(MARKER_RULERS.iter().all(|m| e.session().edit.rulers.iter().any(|r| r == m)));
        e.execute("view.ruler_all", &json!({})).unwrap();
        assert_eq!(e.session().edit.rulers.len(), RULERS.len());
        e.execute("view.ruler_minimal", &json!({})).unwrap();
        assert_eq!(e.session().edit.rulers, vec!["min_secs".to_string(), "markers".to_string()]);
    }
}
