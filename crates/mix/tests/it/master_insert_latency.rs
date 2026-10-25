//! Delay compensation covers the inserts of the master faders: a bounce of the main mix lines up
//! with the same mix without them.

use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::{MixEngine, render_range};
use soundcraft_model::{ChannelFormat, Clip, Insert, Route, Session, SourceAudio, SourceId, TrackKind};
use soundcraft_time::Range;
use std::sync::Arc;

const AT: usize = 2000;

/// One track with an impulse at `AT` (and a loud steady tone after it, so a limiter has work),
/// routed to the main mix, plus a master fader carrying `insert` when given.
fn session(insert: Option<Insert>) -> Session {
    let mut s = Session::default();
    s.edit.delay_compensation = true;
    let mut signal = vec![0.0f32; 12_000];
    signal[AT] = 0.9;
    s.pool.insert(SourceId(902), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![signal.clone(), signal] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Voice"));
    let clip = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "impulse", SourceId(902), 0, 0, 12_000));
    s.track_mut(t).unwrap().mixer.pan = vec![-1.0, 1.0];
    s.track_mut(t).unwrap().mixer.output = Route::Main;
    if let Some(insert) = insert {
        let m = s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Master"));
        s.track_mut(m).unwrap().mixer.inserts[0] = Some(insert);
    }
    s
}

fn peak_at(ch: &[f32]) -> usize {
    ch.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).map(|(i, _)| i).unwrap()
}

#[test]
fn a_master_insert_with_lookahead_does_not_delay_the_bounce() {
    let control = render_range(&session(None), Range::new(0, 12_000), 512);
    assert_eq!(peak_at(&control[0]), AT);
    let mut maximizer = Insert::new("maximizer");
    maximizer.params.insert("threshold".into(), -7.0);
    maximizer.params.insert("ceiling".into(), -1.0);
    let s = session(Some(maximizer));
    let out = render_range(&s, Range::new(0, 12_000), 512);
    assert_eq!(out[0].len(), 12_000);
    assert_eq!(peak_at(&out[0]), AT, "the limited impulse lines up with the mix without the master insert");
    assert_eq!(peak_at(&out[1]), AT);
    let mut probe = MixEngine::new(48_000.0, 512);
    let mut tmp = vec![vec![0.0f32; 512]; 2];
    probe.render(&s, 0, 512, &mut tmp);
    assert!(probe.latency > 0, "the Maximizer reports a lookahead");
    // With Delay Compensation off nothing is compensated (and nothing is dropped).
    let mut off = s.clone();
    off.edit.delay_compensation = false;
    let late = render_range(&off, Range::new(0, 12_000), 512);
    assert_eq!(peak_at(&late[0]), AT + probe.latency);
}

#[test]
fn master_inserts_without_latency_change_nothing() {
    let control = render_range(&session(None), Range::new(0, 12_000), 512);
    let gain = render_range(&session(Some(Insert::new("gain"))), Range::new(0, 12_000), 512);
    assert_eq!(peak_at(&gain[0]), AT);
    assert_eq!(gain[0].len(), control[0].len());
}
