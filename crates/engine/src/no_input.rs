//! "no input" on an audio track (`Route::None`): the track records and monitors nothing, as
//! Pro Tools' No Input does. A new audio track gets its first input path explicitly instead.
//!
//! Sessions saved before this rule used `Route::None` for an audio track that recorded and
//! monitored hardware input 1 (the selector showed "In 1"). [`upgrade_legacy_inputs`] turns
//! those into the explicit first input when such a session is opened, so it keeps sounding and
//! recording as before. Sessions held by the engine carry [`NO_INPUT_MARKER`], so a "no input"
//! saved by this build stays "no input" when it is opened again.

use soundcraft_model::{ChannelFormat, Route, Session, TrackKind};

/// Set in `edit.flags` of every session the engine holds: its `Route::None` audio inputs mean
/// "no input". A session without it was saved before that, when they meant hardware input 1.
pub const NO_INPUT_MARKER: &str = "io.none_is_no_input";

/// The input channel an audio track with no input records: no device has it, so the take is
/// digital silence.
pub const NO_INPUT_CHANNEL: usize = usize::MAX;

/// The first hardware input path of a track of `format`: "In 1", "In 1-2", "In 1-6"…
pub fn first_input(format: ChannelFormat) -> Route {
    match format.channels() {
        0 | 1 => Route::Hardware("In 1".into()),
        n => Route::Hardware(format!("In 1-{n}")),
    }
}

/// Give the new track `id`, if it is an audio track, its first input path (Pro Tools assigns
/// new tracks an input path), and return `id`.
pub(crate) fn assign_first_input(s: &mut Session, id: soundcraft_model::TrackId) -> soundcraft_model::TrackId {
    if let Some(t) = s.track_mut(id)
        && t.kind == TrackKind::Audio
        && t.mixer.input == Route::None
    {
        t.mixer.input = first_input(t.format);
    }
    id
}

/// A session saved before [`NO_INPUT_MARKER`]: each audio track whose input is `Route::None`
/// recorded and monitored hardware input 1, so it gets that input explicitly. Then the session
/// is marked, and running this again changes nothing.
pub fn upgrade_legacy_inputs(s: &mut Session) {
    if s.edit.flag(NO_INPUT_MARKER) {
        return;
    }
    for t in &mut s.tracks {
        if t.kind == TrackKind::Audio && t.mixer.input == Route::None {
            t.mixer.input = first_input(t.format);
        }
    }
    s.edit.set_flag(NO_INPUT_MARKER, true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::TrackId;

    fn new_track(e: &mut Engine, format: &str) -> TrackId {
        TrackId(e.execute("track.new", &json!({"format": format})).unwrap()["tracks"][0].as_u64().unwrap())
    }

    fn input(e: &Engine, t: TrackId) -> Route {
        e.session().track(t).unwrap().mixer.input.clone()
    }

    #[test]
    fn a_new_audio_track_gets_its_first_input_path() {
        let mut e = Engine::default();
        let mono = new_track(&mut e, "mono");
        let stereo = new_track(&mut e, "stereo");
        assert_eq!(input(&e, mono), Route::Hardware("In 1".into()));
        assert_eq!(input(&e, stereo), Route::Hardware("In 1-2".into()));
        let aux = TrackId(e.execute("track.new", &json!({"kind": "aux"})).unwrap()["tracks"][0].as_u64().unwrap());
        assert_eq!(input(&e, aux), Route::None, "only audio tracks take a hardware input by default");
    }

    #[test]
    fn a_track_with_no_input_records_no_device_channel() {
        let dir = std::env::temp_dir().join(format!("soundcraft-no-input-{}", std::process::id()));
        let mut e = Engine::default();
        let vox = new_track(&mut e, "mono");
        let gtr = new_track(&mut e, "stereo");
        e.execute("track.input", &json!({"tracks": [vox.0, gtr.0], "input": "none"})).unwrap();
        e.execute("mix.record_arm", &json!({"tracks": [vox.0, gtr.0], "value": true})).unwrap();
        let plan = crate::io::plan_takes(&e, &dir, 30);
        assert_eq!(
            plan.iter().map(|(t, inputs, _)| (*t, inputs.clone())).collect::<Vec<_>>(),
            vec![(vox, vec![NO_INPUT_CHANNEL]), (gtr, vec![NO_INPUT_CHANNEL; 2])]
        );
    }

    #[test]
    fn a_session_saved_before_no_input_existed_keeps_recording_input_1() {
        let mut old = Session::default();
        let vox = old.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
        let gtr = old.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Gtr"));
        let keys = old.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let text = old.to_json().unwrap();
        assert!(!text.contains(NO_INPUT_MARKER));
        let mut e = Engine::default();
        e.replace_session(Session::from_json(&text).unwrap());
        assert_eq!(input(&e, vox), Route::Hardware("In 1".into()));
        assert_eq!(input(&e, gtr), Route::Hardware("In 1-2".into()));
        assert_eq!(input(&e, keys), Route::None, "a MIDI input is left alone");
        assert!(!e.is_dirty(), "opening an old session does not modify it");
    }

    #[test]
    fn no_input_survives_save_and_reopen() {
        let mut e = Engine::default();
        let vox = new_track(&mut e, "mono");
        e.execute("track.input", &json!({"tracks": [vox.0], "input": "no input"})).unwrap();
        let text = e.session().to_json().unwrap();
        let mut reopened = Engine::default();
        reopened.replace_session(Session::from_json(&text).unwrap());
        assert_eq!(input(&reopened, vox), Route::None);
    }
}
