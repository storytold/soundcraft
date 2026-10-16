//! Key input (sidechain): a dynamics insert detects on a bus instead of its own track.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::{key_feeds_back, key_loop, render_range, render_track_pre_fader};
use soundcraft_model::{BusId, ChannelFormat, Clip, Insert, Route, SendSlot, Session, SourceAudio, SourceId, TrackId, TrackKind};
use soundcraft_time::Range;
use std::sync::Arc;

const LEN: usize = 48_000;

fn add_audio(s: &mut Session, name: &str, source: u64, samples: Vec<f32>) -> TrackId {
    let len = samples.len() as i64;
    s.pool.insert(SourceId(source), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![samples] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
    let clip = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, name, SourceId(source), 0, 0, len));
    t
}

fn send_to(s: &mut Session, t: TrackId, bus: BusId) {
    let mut snd = SendSlot::new(Route::Bus(bus));
    snd.level_db = 0.0;
    s.track_mut(t).unwrap().mixer.sends[0] = Some(snd);
}

/// A quiet bass (-26 dBFS, under the compressor's threshold) with a compressor on it, and a
/// kick (loud bursts) that only feeds the "Kick Key" bus: the main mix is the bass alone.
fn kick_and_bass(key: bool, ext_key: bool) -> Session {
    keyed_bass("compressor", key, ext_key)
}

fn keyed_bass(plugin: &str, key: bool, ext_key: bool) -> Session {
    let mut s = Session::default();
    let bus = s.add_bus("Kick Key", ChannelFormat::Mono);
    let bass = add_audio(&mut s, "Bass", 901, vec![0.05; LEN]);
    let kick = add_audio(&mut s, "Kick", 902, (0..LEN).map(|i| if i % 9600 < 2400 { 0.9 } else { 0.0 }).collect());
    s.track_mut(kick).unwrap().mixer.output = Route::None;
    send_to(&mut s, kick, bus);
    let mut comp = Insert::new(plugin);
    comp.params.insert("threshold".into(), -20.0);
    comp.params.insert("ratio".into(), 10.0);
    comp.params.insert("attack".into(), 1.0);
    comp.params.insert("ext_key".into(), if ext_key { 1.0 } else { 0.0 });
    comp.key = key.then_some(bus);
    s.track_mut(bass).unwrap().mixer.inserts[0] = Some(comp);
    s
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

#[test]
fn a_kick_keyed_into_a_compressor_ducks_the_bass_only_when_keyed() {
    let main = |key, ext| render_range(&kick_and_bass(key, ext), Range::new(0, LEN as i64), 512);
    let plain = rms(&main(false, true)[0]);
    let keyed = rms(&main(true, true)[0]);
    let key_off = rms(&main(true, false)[0]);
    assert!(plain > 0.03, "the bass should sound: {plain}");
    assert!(keyed < 0.6 * plain, "the kick should duck the bass: {keyed} vs {plain}");
    assert!((key_off - plain).abs() < 1e-4, "a key bus without External Key changes nothing: {key_off} vs {plain}");
}

#[test]
fn an_aux_hears_a_keyed_track_routed_to_its_bus() {
    // The bass is routed through an aux; the aux comes first in the session, so only the routing
    // puts the bass before it.
    let via_aux = |key: bool| {
        let mut s = kick_and_bass(key, true);
        let sub = s.add_bus("Bass Sub", ChannelFormat::Mono);
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Mono, Some("Bass Sub"));
        s.track_mut(aux).unwrap().mixer.input = Route::Bus(sub);
        s.tracks.rotate_right(1);
        assert_eq!(s.tracks[0].id, aux);
        let bass = s.tracks.iter().find(|t| t.name == "Bass").unwrap().id;
        s.track_mut(bass).unwrap().mixer.output = Route::Bus(sub);
        rms(&render_range(&s, Range::new(0, LEN as i64), 512)[0])
    };
    let direct = |key: bool| rms(&render_range(&kick_and_bass(key, true), Range::new(0, LEN as i64), 512)[0]);
    let (plain, keyed) = (via_aux(false), via_aux(true));
    assert!(plain > 0.01, "the aux should carry the bass: {plain}");
    assert!(keyed < 0.6 * plain, "the bass should be ducked through the aux too: {keyed} vs {plain}");
    // The aux reads the bass in the same block: the same ducking as straight to the main mix.
    let (a, b) = (keyed / plain, direct(true) / direct(false));
    assert!((a - b).abs() < 1e-3, "ducking through the aux {a} vs direct {b}");
}

#[test]
fn a_key_from_a_bus_the_track_feeds_is_a_feedback_loop() {
    let mut s = kick_and_bass(false, true);
    let key = s.bus_by_name("Kick Key").unwrap().id;
    let other = s.add_bus("Other", ChannelFormat::Mono);
    let [bass, kick] = ["Bass", "Kick"].map(|n| s.tracks.iter().find(|t| t.name == n).unwrap().id);
    assert!(!key_feeds_back(&s, bass, key), "the kick feeds the key, not the bass");
    assert!(key_feeds_back(&s, kick, key), "a send from the track itself");
    // Through an aux: bass → Other → aux → Kick Key.
    s.track_mut(bass).unwrap().mixer.output = Route::Bus(other);
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Mono, Some("Return"));
    s.track_mut(aux).unwrap().mixer.input = Route::Bus(other);
    s.track_mut(aux).unwrap().mixer.output = Route::Bus(key);
    assert!(key_feeds_back(&s, bass, key), "through an aux");
    // Through another keyed insert: bass → Other keys a track that sends to Kick Key.
    s.track_mut(aux).unwrap().mixer.output = Route::Main;
    assert!(!key_feeds_back(&s, bass, key));
    let ducked = add_audio(&mut s, "Pad", 903, vec![0.1; LEN]);
    let mut gate = Insert::new("expander_gate");
    gate.key = Some(other);
    s.track_mut(ducked).unwrap().mixer.inserts[0] = Some(gate);
    s.track_mut(ducked).unwrap().mixer.output = Route::Bus(key);
    assert!(key_feeds_back(&s, bass, key), "through a keyed track");
}

/// Commit renders the track alone, but the tracks feeding its key keep playing into the key bus.
#[test]
fn commit_of_a_keyed_track_keeps_its_key() {
    let range = Range::new(0, LEN as i64);
    let commit = |s: &Session| {
        let bass = s.tracks.iter().find(|t| t.name == "Bass").unwrap().id;
        render_track_pre_fader(s, bass, range)
    };
    // The gate opens only on the kick: without the key it would commit silence.
    let gated = keyed_bass("expander_gate", true, true);
    let live = render_range(&gated, range, 1024);
    let committed = commit(&gated);
    assert!(rms(&committed[0]) > 0.01, "the committed gate should open on the kick: {}", rms(&committed[0]));
    let g = std::f32::consts::SQRT_2;
    let diff = committed[0].iter().zip(&live[0]).map(|(c, l)| (c - l * g).abs()).fold(0.0f32, f32::max);
    assert!(diff < 1e-4, "the commit should sound like the track in the mix: off by {diff}");
    // A keyed compressor commits ducked.
    let (plain, keyed) = (rms(&commit(&kick_and_bass(false, true))[0]), rms(&commit(&kick_and_bass(true, true))[0]));
    assert!(keyed < 0.6 * plain, "the committed bass should be ducked: {keyed} vs {plain}");
}

#[test]
fn a_route_into_a_key_bus_from_the_keyed_track_is_a_loop() {
    let mut s = kick_and_bass(true, true);
    assert_eq!(key_loop(&s), None);
    let key = s.bus_by_name("Kick Key").unwrap().id;
    let bass = s.tracks.iter().find(|t| t.name == "Bass").unwrap().id;
    s.track_mut(bass).unwrap().mixer.sends[1] = Some(SendSlot::new(Route::Bus(key)));
    assert_eq!(key_loop(&s), Some((bass, key)));
}
