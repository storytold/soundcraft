use super::*;
use soundcraft_model::{ChannelFormat, Source, SourceId, TrackKind};

fn session_with_clip() -> (Session, TrackId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    s.sources.push(Source {
        id: SourceId(500),
        name: "src".into(),
        path: "src.wav".into(),
        channels: 1,
        frames: 10_000,
        sample_rate: 48_000,
        format: soundcraft_audio_io::FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
    });
    let id = s.new_clip_id();
    if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
        pl.clips.push(Clip::audio(id, "a", SourceId(500), 1000, 1000, 4000));
    }
    (s, t)
}

fn clips(s: &Session, t: TrackId) -> Vec<(Samples, Samples, Samples)> {
    s.track(t).unwrap().clips().iter().map(|c| (c.start, c.length, c.source_offset())).collect()
}

#[test]
fn copy_range_resets_fades_at_cut_edges() {
    let (mut s, t) = session_with_clip();
    if let Some(c) = s.track_mut(t).and_then(|t| t.playlist_mut()).and_then(|p| p.clips.first_mut()) {
        c.fade_in = Fade { len: 100, shape: FadeShape::Linear, curvature: 0.0 };
        c.fade_out = Fade { len: 100, shape: FadeShape::Linear, curvature: 0.0 };
    }
    let fades = |r: Range| -> Vec<(Samples, Samples)> { copy_range(&s, t, r).iter().map(|c| (c.fade_in.len, c.fade_out.len)).collect() };
    assert_eq!(fades(Range::new(2000, 3000)), vec![(0, 0)]);
    assert_eq!(fades(Range::new(0, 3000)), vec![(100, 0)]);
    assert_eq!(fades(Range::new(2000, 6000)), vec![(0, 100)]);
    assert_eq!(fades(Range::new(0, 6000)), vec![(100, 100)]);
}

#[test]
fn separate_splits_and_heal_rejoins() {
    let (mut s, t) = session_with_clip();
    assert_eq!(separate_at(&mut s, t, 3000), 1);
    assert_eq!(clips(&s, t), vec![(1000, 2000, 1000), (3000, 2000, 3000)]);
    assert_eq!(separate_at(&mut s, t, 1000), 0);
    assert_eq!(heal(&mut s, t, Range::new(0, 10_000)), 1);
    assert_eq!(clips(&s, t), vec![(1000, 4000, 1000)]);
}

#[test]
fn heal_rejoins_cut_separation() {
    let (mut s, t) = session_with_clip();
    clear_range(&mut s, t, Range::new(2000, 3000), false);
    assert_eq!(heal(&mut s, t, Range::new(1500, 3500)), 1);
    assert_eq!(clips(&s, t), vec![(1000, 4000, 1000)]);
    // A selection that does not span the whole gap leaves it alone.
    let (mut s, t) = session_with_clip();
    clear_range(&mut s, t, Range::new(2000, 3000), false);
    assert_eq!(heal(&mut s, t, Range::new(2500, 3500)), 0);
    // Shuffled pieces no longer line up with their source and stay apart.
    let (mut s, t) = session_with_clip();
    clear_range(&mut s, t, Range::new(2000, 3000), true);
    assert_eq!(heal(&mut s, t, Range::new(0, 10_000)), 0);
    // Moved apart: the timeline gap no longer matches the source gap.
    let (mut s, t) = session_with_clip();
    clear_range(&mut s, t, Range::new(2000, 3000), false);
    if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
        pl.clips[1].start = 3500;
    }
    assert_eq!(heal(&mut s, t, Range::new(0, 10_000)), 0);
    // Overlapping neighbours whose offsets line up are not a separation.
    let (mut s, t) = session_with_clip();
    assert_eq!(separate_at(&mut s, t, 3000), 1);
    if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
        pl.clips[1].start = 2500;
    }
    assert_eq!(heal(&mut s, t, Range::new(0, 10_000)), 0);
    assert_eq!(clips(&s, t).len(), 2);
}

#[test]
fn heal_skips_clips_with_different_playback() {
    for tweak in [(|c: &mut Clip| c.muted = true) as fn(&mut Clip), |c: &mut Clip| c.stretch = 1.5] {
        let (mut s, t) = session_with_clip();
        separate_at(&mut s, t, 3000);
        if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
            tweak(&mut pl.clips[1]);
        }
        assert_eq!(heal(&mut s, t, Range::new(0, 10_000)), 0);
        assert_eq!(s.track(t).unwrap().clips().len(), 2);
    }
}

#[test]
fn heal_keeps_each_side_gain_envelope() {
    let cases: [(Vec<(Samples, f32)>, Vec<(Samples, f32)>); 4] = [
        (vec![(500, -3.0), (2500, -12.0)], vec![]),
        (vec![], vec![(0, -6.0), (1000, 0.0)]),
        (vec![(0, 0.0), (1999, -20.0)], vec![(-100, 4.0), (300, -9.0)]),
        (vec![(500, -3.0), (2500, -12.0)], vec![(0, -9.75), (500, -12.0)]),
    ];
    for (left_env, right_env) in cases {
        let (mut s, t) = session_with_clip();
        separate_at(&mut s, t, 3000);
        let pl = s.track_mut(t).and_then(|t| t.playlist_mut()).unwrap();
        pl.clips[0].gain_env = left_env;
        pl.clips[1].gain_env = right_env;
        let (l, r) = (pl.clips[0].clone(), pl.clips[1].clone());
        assert_eq!(heal(&mut s, t, Range::new(0, 10_000)), 1);
        let healed = &s.track(t).unwrap().clips()[0];
        for rel in 0..4000 {
            let want = if rel < 2000 { l.gain_at(rel) } else { r.gain_at(rel - 2000) };
            assert!((healed.gain_at(rel) - want).abs() < 1e-4, "sample {rel}: {} != {want}", healed.gain_at(rel));
        }
    }
}

#[test]
fn heal_keeps_gain_envelopes_across_a_gap() {
    let (mut s, t) = session_with_clip();
    clear_range(&mut s, t, Range::new(2000, 3000), false);
    let pl = s.track_mut(t).and_then(|t| t.playlist_mut()).unwrap();
    pl.clips[0].gain_env = vec![(0, 0.0), (999, -6.0)];
    pl.clips[1].gain_env = vec![(0, -12.0), (1999, -3.0)];
    let (l, r) = (pl.clips[0].clone(), pl.clips[1].clone());
    assert_eq!(heal(&mut s, t, Range::new(1500, 3500)), 1);
    let healed = &s.track(t).unwrap().clips()[0];
    assert_eq!(healed.length, 4000);
    for rel in (0..1000).chain(2000..4000) {
        let want = if rel < 1000 { l.gain_at(rel) } else { r.gain_at(rel - 2000) };
        assert!((healed.gain_at(rel) - want).abs() < 1e-4, "sample {rel}: {} != {want}", healed.gain_at(rel));
    }
}

#[test]
fn split_then_heal_rejoins_stretched_clip() {
    let (mut s, t) = session_with_clip();
    if let Some(c) = s.track_mut(t).and_then(|t| t.playlist_mut()).and_then(|p| p.clips.first_mut()) {
        c.stretch = 2.0;
    }
    assert_eq!(separate_at(&mut s, t, 3000), 1);
    assert_eq!(clips(&s, t), vec![(1000, 2000, 1000), (3000, 2000, 2000)]);
    assert_eq!(heal(&mut s, t, Range::new(0, 10_000)), 1);
    assert_eq!(clips(&s, t), vec![(1000, 4000, 1000)]);
}

#[test]
fn clear_slip_and_shuffle() {
    let (mut s, t) = session_with_clip();
    let removed = clear_range(&mut s, t, Range::new(2000, 3000), false);
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].start, 0);
    assert_eq!(clips(&s, t), vec![(1000, 1000, 1000), (3000, 2000, 3000)]);
    let (mut s, t) = session_with_clip();
    clear_range(&mut s, t, Range::new(2000, 3000), true);
    assert_eq!(clips(&s, t), vec![(1000, 1000, 1000), (2000, 2000, 3000)]);
}

#[test]
fn copy_paste_overwrites() {
    let (mut s, t) = session_with_clip();
    let copied = copy_range(&s, t, Range::new(1500, 2500));
    assert_eq!(copied.len(), 1);
    assert_eq!((copied[0].start, copied[0].length, copied[0].source_offset()), (0, 1000, 1500));
    paste(&mut s, t, 4000, &copied, 1000, false).unwrap();
    assert_eq!(clips(&s, t), vec![(1000, 3000, 1000), (4000, 1000, 1500)]);
}

#[test]
fn paste_shuffle_ripples() {
    let (mut s, t) = session_with_clip();
    let copied = copy_range(&s, t, Range::new(1000, 2000));
    paste(&mut s, t, 1000, &copied, 1000, true).unwrap();
    assert_eq!(clips(&s, t), vec![(1000, 1000, 1000), (2000, 4000, 1000)]);
}

#[test]
fn insert_silence_and_trim() {
    let (mut s, t) = session_with_clip();
    insert_silence(&mut s, t, Range::new(2000, 2500));
    assert_eq!(clips(&s, t), vec![(1000, 1000, 1000), (2500, 3000, 2000)]);
    let (mut s, t) = session_with_clip();
    trim_to(&mut s, t, Range::new(1500, 2000));
    assert_eq!(clips(&s, t), vec![(1500, 500, 1500)]);
}

#[test]
fn fades_and_crossfades() {
    let (mut s, t) = session_with_clip();
    assert_eq!(create_fades(&mut s, t, Range::new(500, 1500), FadeShape::Linear), 1);
    assert_eq!(s.track(t).unwrap().clips()[0].fade_in.len, 500);
    let (mut s, t) = session_with_clip();
    separate_at(&mut s, t, 3000);
    assert_eq!(create_fades(&mut s, t, Range::new(2900, 3100), FadeShape::EqualPower), 1);
    let c = s.track(t).unwrap().clips();
    assert_eq!(c[0].end(), 3100);
    assert_eq!(c[1].start, 2900);
    assert_eq!(c[0].fade_out.len, 200);
    assert_eq!(c[1].fade_in.len, 200);
    delete_fades(&mut s, t, Range::new(2900, 3100));
    let c = s.track(t).unwrap().clips();
    assert_eq!((c[0].end(), c[1].start), (3000, 3000));
}

fn three_clips() -> (Session, TrackId, [ClipId; 3]) {
    let (mut s, t) = session_with_clip();
    let ids = [s.new_clip_id(), s.new_clip_id(), s.new_clip_id()];
    if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
        pl.clips = vec![
            Clip::audio(ids[0], "a", SourceId(500), 0, 0, 1000),
            Clip::audio(ids[1], "b", SourceId(500), 1000, 1000, 2000),
            Clip::audio(ids[2], "c", SourceId(500), 3000, 5000, 1000),
        ];
    }
    (s, t, ids)
}

#[test]
fn shuffle_moves_reorder_clips_and_keep_the_silence() {
    let (mut s, t, [_, b, _]) = three_clips();
    assert_eq!(shuffle_clips(&mut s, &[b], -900, &[]), Ok(1));
    assert_eq!(clips(&s, t), vec![(0, 2000, 1000), (2000, 1000, 0), (5000, 1000, 3000)]);
    let (mut s, t, [a, ..]) = three_clips();
    assert_eq!(shuffle_slot(&s, t, 900, &[a]), 0);
    assert_eq!(shuffle_slot(&s, t, 1100, &[a]), 2000);
    shuffle_clips(&mut s, &[a], 2500, &[]).unwrap();
    assert_eq!(clips(&s, t), vec![(0, 2000, 1000), (2000, 1000, 0), (5000, 1000, 3000)]);
}

#[test]
fn shuffling_onto_an_empty_track_starts_it_at_zero() {
    let (mut s, t, [_, _, c]) = three_clips();
    let other = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    shuffle_clips(&mut s, &[c], 40_000, &[(t, other)]).unwrap();
    assert_eq!(clips(&s, other), vec![(0, 1000, 3000)]);
    assert_eq!(clips(&s, t), vec![(0, 1000, 0), (1000, 2000, 1000)]);
}

#[test]
fn shuffle_trims_move_later_clips_and_slip_trims_stop_at_neighbours() {
    let (mut s, t, [a, b, c]) = three_clips();
    trim_clip(&mut s, b, None, Some(2500), true);
    trim_clip(&mut s, b, Some(1500), None, true);
    assert_eq!(clips(&s, t), vec![(0, 1000, 0), (1000, 1000, 1500), (4000, 1000, 3000)]);
    let (mut s, t, _) = three_clips();
    trim_clip(&mut s, a, None, Some(2500), false);
    trim_clip(&mut s, c, Some(2000), None, false);
    assert_eq!(clips(&s, t), vec![(0, 1000, 0), (1000, 2000, 1000), (3000, 3000, 1000)]);
    trim_clip(&mut s, c, None, Some(99_999), false);
    assert_eq!(clips(&s, t)[2], (3000, 9000, 1000));
}

#[test]
fn shuffle_leaves_locked_clips_and_what_follows_them_in_place() {
    let (mut s, t, [a, b, c]) = three_clips();
    s.find_clip_mut(c).unwrap().time_locked = true;
    clear_range(&mut s, t, Range::new(1000, 3000), true);
    assert_eq!(clips(&s, t), vec![(0, 1000, 0), (5000, 1000, 3000)]);
    let (mut s, t, [a2, ..]) = three_clips();
    s.find_clip_mut(c).unwrap().edit_locked = true;
    assert_eq!(shuffle_room(&s, t, 0), 2000);
    assert_eq!(make_room(&mut s, t, 0, 2500), Err(NO_ROOM));
    assert_eq!(make_room(&mut s, t, 0, 2000), Ok(()));
    assert_eq!(clips(&s, t), vec![(2000, 1000, 0), (3000, 2000, 1000), (5000, 1000, 3000)]);
    assert_eq!(shuffle_clips(&mut s, &[a2], 2500, &[]), Ok(1));
    assert_eq!(clips(&s, t)[2], (5000, 1000, 3000));
    let (mut s, t, _) = three_clips();
    s.find_clip_mut(c).unwrap().time_locked = true;
    trim_clip(&mut s, b, None, Some(9000), true);
    assert_eq!(clips(&s, t), vec![(0, 1000, 0), (1000, 4000, 1000), (5000, 1000, 3000)]);
    assert_eq!(shuffle_slot(&s, t, 4000, &[a]), 4000);
}

#[test]
fn multi_track_moves_keep_the_tracks_apart() {
    let (mut s, t1, [a, ..]) = three_clips();
    let t2 = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let t3 = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let m = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let d = s.new_clip_id();
    s.track_mut(t2).and_then(|t| t.playlist_mut()).unwrap().clips.push(Clip::audio(d, "d", SourceId(500), 0, 0, 500));
    let dest = destinations(&s, &[a, d], t1, t2);
    assert_eq!(dest, Some(vec![(t1, t2), (t2, t3)]));
    assert_eq!(destinations(&s, &[a, d], t1, m), None);
    assert_eq!(destinations(&s, &[a], t1, m), None);
    assert_eq!(move_clips(&mut s, &[a, d], 0, &dest.unwrap()), 2);
    assert_eq!(clips(&s, t2), vec![(0, 1000, 0)]);
    assert_eq!(clips(&s, t3), vec![(0, 500, 0)]);
}

#[test]
fn grid_moves_snap_the_clip_start_or_keep_its_offset() {
    let (s, t) = session_with_clip();
    let step = s.edit.grid.step_samples(0, s.sample_rate, &s.tempo, s.frame_rate);
    let c = s.track(t).unwrap().clips()[0].clone();
    assert_eq!(c.start + grid_delta(&s, &c, step + 200, false), step);
    assert_eq!(c.start + grid_delta(&s, &c, step + 200, true), step + 1000);
    assert_eq!(grid_delta(&s, &c, step / 3, true), 0);
}

#[test]
fn moves_overwrite_and_respect_locks() {
    let (mut s, t) = session_with_clip();
    let id = s.track(t).unwrap().clips()[0].id;
    assert_eq!(move_clips(&mut s, &[id], 500, &[]), 1);
    assert_eq!(clips(&s, t), vec![(1500, 4000, 1000)]);
    s.find_clip_mut(id).unwrap().time_locked = true;
    assert_eq!(move_clips(&mut s, &[id], 500, &[]), 0);
}

#[test]
fn delete_fades_keeps_playlist_sorted_when_a_clip_overlaps_several() {
    let (mut s, t) = session_with_clip();
    let pl = s.track_mut(t).and_then(|t| t.playlist_mut()).expect("track has a playlist");
    pl.clips.clear();
    pl.clips.push(Clip::audio(ClipId(901), "a", SourceId(500), 0, 0, 1000));
    pl.clips.push(Clip::audio(ClipId(902), "b", SourceId(500), 100, 100, 100));
    pl.clips.push(Clip::audio(ClipId(903), "c", SourceId(500), 150, 150, 750));
    delete_fades(&mut s, t, Range::new(0, 10_000));
    let got = clips(&s, t);
    assert!(got.windows(2).all(|w| w[0].0 <= w[1].0), "playlist not sorted: {got:?}");
    assert!(got.iter().all(|c| c.1 > 0), "empty clip: {got:?}");
}

#[test]
fn delete_fades_leaves_unrelated_overlaps_alone() {
    let (mut s, t) = session_with_clip();
    let ids: Vec<ClipId> = (0..4).map(|_| s.new_clip_id()).collect();
    if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
        pl.clips.clear();
        for (id, start, len) in [(ids[0], 0, 1100), (ids[1], 1000, 1000), (ids[2], 5000, 1100), (ids[3], 6000, 1000)] {
            let mut c = Clip::audio(id, "c", SourceId(500), start, start, len);
            c.fade_in = Fade { len: 100, shape: FadeShape::Linear, curvature: 0.0 };
            c.fade_out = Fade { len: 100, shape: FadeShape::Linear, curvature: 0.0 };
            pl.clips.push(c);
        }
    }
    assert_eq!(delete_fades(&mut s, t, Range::new(900, 1200)), 2);
    assert_eq!(clips(&s, t), vec![(0, 1050, 0), (1050, 950, 1050), (5000, 1100, 5000), (6000, 1000, 6000)]);
    let c = s.track(t).unwrap().clips();
    assert_eq!((c[1].fade_in.len, c[2].fade_out.len, c[3].fade_in.len), (0, 100, 100));
}
