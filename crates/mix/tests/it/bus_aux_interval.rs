//! A bounce of a bus keeps the requested timeline interval when an aux returns onto the bus
//! (its insert's lookahead is compensated like the main mix's).

use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::{render_bus, render_range};
use soundcraft_model::{BusId, ChannelFormat, Clip, Insert, Route, Session, SourceAudio, SourceId, TrackKind};
use soundcraft_time::Range;
use std::sync::Arc;

const AT: [usize; 3] = [0, 2048, 8000];

/// Impulses at 0, 2048 and 8000 on a track routed to bus A, and an aux reading bus A that returns
/// onto bus B (or the main mix) with a unity-ratio compressor of 5 ms lookahead on it.
fn fixture(insert: bool, aux_to_main: bool) -> (Session, BusId, BusId) {
    let mut s = Session::default();
    s.edit.delay_compensation = true;
    let mut signal = vec![0.0; 8192];
    for at in AT {
        signal[at] = 0.25;
    }
    s.pool.insert(SourceId(901), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48000, channels: vec![signal.clone(), signal] })));
    let a = s.add_bus("A", ChannelFormat::Stereo);
    let b = s.add_bus("B", ChannelFormat::Stereo);
    let source = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Source"));
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Return"));
    let clip = s.new_clip_id();
    s.track_mut(source).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "impulses", SourceId(901), 0, 0, 8192));
    s.track_mut(source).unwrap().mixer.pan = vec![-1.0, 1.0];
    s.track_mut(source).unwrap().mixer.output = Route::Bus(a);
    s.track_mut(aux).unwrap().mixer.pan = vec![-1.0, 1.0];
    s.track_mut(aux).unwrap().mixer.input = Route::Bus(a);
    s.track_mut(aux).unwrap().mixer.output = if aux_to_main { Route::Main } else { Route::Bus(b) };
    if insert {
        let mut compressor = Insert::new("compressor");
        compressor.params.insert("lookahead".into(), 5.0);
        compressor.params.insert("ratio".into(), 1.0);
        s.track_mut(aux).unwrap().mixer.inserts[0] = Some(compressor);
    }
    (s, a, b)
}

fn positions(ch: &[f32]) -> Vec<usize> {
    ch.iter().enumerate().filter(|(_, x)| x.abs() > 1e-4).map(|(i, _)| i).collect()
}

#[test]
fn named_bus_after_aux_preserves_requested_timeline() {
    let (s, a, b) = fixture(true, false);
    let bus = render_bus(&s, b, Range::new(0, 8192), 1024);
    let upstream = render_bus(&s, a, Range::new(0, 8192), 1024);
    let (plain, _, plain_b) = fixture(false, false);
    let control = render_bus(&plain, plain_b, Range::new(0, 8192), 1024);
    let (main, _, _) = fixture(true, true);
    let main = render_range(&main, Range::new(0, 8192), 1024);
    for c in 0..2 {
        // Controls: the bus before the aux, the same route without the insert, and the main mix.
        assert_eq!(positions(&control[c]), AT);
        assert_eq!(upstream[c], control[c]);
        assert_eq!(main[c], control[c]);
        // The bus after the aux: the whole interval, nothing late, nothing cut off.
        assert_eq!(bus[c].len(), 8192);
        assert_eq!(positions(&bus[c]), AT, "the named bus must preserve the requested interval");
        assert!(bus[c] == control[c], "the aligned samples must match the control");
    }
    // A partial range lines up too.
    let part = render_bus(&s, b, Range::new(2048, 4096), 1024);
    assert_eq!(positions(&part[0]), vec![0]);
}

#[test]
fn a_track_routed_to_the_same_bus_stays_aligned_with_the_aux_return() {
    // A second track goes straight to bus B, where the aux (with its lookahead) also returns.
    let (mut s, _, b) = fixture(true, false);
    let direct = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Direct"));
    let clip = s.new_clip_id();
    s.track_mut(direct).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "impulses", SourceId(901), 0, 0, 8192));
    s.track_mut(direct).unwrap().mixer.pan = vec![-1.0, 1.0];
    s.track_mut(direct).unwrap().mixer.output = Route::Bus(b);
    let bus = render_bus(&s, b, Range::new(0, 8192), 1024);
    for (c, ch) in bus.iter().enumerate() {
        assert_eq!(positions(ch), AT, "direct and aux signals arrive together");
        for at in AT {
            assert!((ch[at] - 0.5).abs() < 1e-3, "channel {c} at {at}: {}", ch[at]);
        }
    }
}
