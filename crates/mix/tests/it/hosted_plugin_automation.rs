//! Automation reaches a hosted plug-in end to end: a session with the VST3 fixture's gain plug-in
//! on an insert and a lane for its Gain parameter renders at the level the lane asks for.
#[path = "../../../vst3-host/tests/common/mod.rs"]
mod common;

use craft_audio::AudioBuffer;
use soundcraft_mix::render_range;
use soundcraft_model::{ChannelFormat, Clip, Insert, Route, Session, SourceAudio, SourceId, TrackId, TrackKind, parse_auto_param};
use soundcraft_time::Range;
use std::sync::Arc;

const LEN: usize = 4096;
const HALF: usize = LEN / 2;
const DC: f32 = 0.25;

fn gain_id() -> String {
    let found = soundcraft_vst3_host::scan_paths(&[common::fixture_dir().to_path_buf()]);
    found.into_iter().find(|d| d.name == "Fixture Gain").expect("fixture").id
}

/// A mono track playing a constant `DC` through Fixture Gain on insert A, panned center.
fn session() -> (Session, TrackId) {
    let mut s = Session::default();
    s.pool.insert(SourceId(1), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![DC; LEN]] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Source"));
    let clip = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, "dc", SourceId(1), 0, 0, LEN as i64));
    tr.mixer.output = Route::Main;
    tr.mixer.inserts[0] = Some(Insert::new(gain_id()));
    (s, t)
}

fn level(out: &[Vec<f32>], at: usize) -> f32 {
    out[0][at].abs()
}

#[test]
fn a_gain_lane_drives_the_hosted_plugin() {
    let (s, _) = session();
    let flat = render_range(&s, Range::new(0, LEN as i64), 256);
    let unity = level(&flat, HALF);
    assert!(unity > 0.01, "the fixture passes audio: {unity}");

    // The engine resolves names to ids before they reach the model; the mix matches ids.
    let key = "plugin:a:7";
    let (mut s, t) = session();
    let param = parse_auto_param(key).unwrap_or_else(|| panic!("{key} parses"));
    let lane = s.track_mut(t).unwrap().lane_mut(&param);
    // Normalized 0.5 is the fixture's unity gain; 0.25 halves it.
    lane.set_point(0, 0.25);
    lane.set_point(HALF as i64 - 1, 0.25);
    lane.set_point(HALF as i64, 0.5);
    for block in [64, 256, 1000] {
        let out = render_range(&s, Range::new(0, LEN as i64), block);
        let (low, high) = (level(&out, HALF / 2), level(&out, HALF + HALF / 2));
        assert!((low - unity / 2.0).abs() < 1e-4, "block {block}: first half at half gain: {low} vs {unity}");
        assert!((high - unity).abs() < 1e-4, "block {block}: second half back at unity: {high} vs {unity}");
    }
}
