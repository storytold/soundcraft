use super::sampler_song::sampler_song;
use super::*;
use soundcraft_model::{ChannelFormat, ClipContent, Session, SourceAudio, SourceId, TrackKind};
use std::sync::{Arc, OnceLock};

const TAGS: usize = 16;
const HAT: usize = 96;
const KICK: usize = 240;
const SCORE_WINDOW: usize = 64;
const ORIGIN: i64 = 20_000;

#[derive(Clone, Copy, Debug)]
pub(super) enum Ending {
    Release,
    Stop,
    Loss { coast: bool },
}

pub(super) struct Rendered {
    /// Every onset recovered from the PCM, grouped by scored eighth; errors in 48 kHz samples.
    pub attacks: Vec<Vec<i64>>,
    pub damaged: usize,
    pub extra: usize,
    pub tail_open: bool,
    pub snapshots: Vec<Snapshot>,
    pub transitions: Vec<(usize, i64, State)>,
    pub final_state: State,
    pub tempo_events: usize,
}

fn band_song(start_bpm: f64, capacity: usize, end: i64) -> Session {
    let sr = SampleRate::HZ_48000;
    let mut song = sampler_song(TempoMap::new(start_bpm, 4, 4).unwrap(), sr, capacity, end);
    let id = song.new_clip_id();
    let mut clip = song.tracks[0].playlist_mut().unwrap().clips[0].clone();
    clip.id = id;
    let mut instrument = song.tracks[0].instrument.clone().unwrap();
    let source = SourceId(201);
    // Short kick with a decaying low-frequency body, distinguishable from the brighter
    // 96-sample hi-hat even when both attacks fall on the same PCM sample.
    let kick = (0..KICK)
        .map(|i| {
            let phase = i as f32 / KICK as f32;
            0.35 * (-4. * phase).exp() * (1. + 0.6 * (phase * 3.8).cos())
        })
        .collect();
    song.pool.insert(source, Arc::new(SourceAudio::new(craft_audio::AudioBuffer { sample_rate: sr.hz(), channels: vec![kick] })));
    instrument.sample = Some(source);
    let track = song.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let kick = song.track_mut(track).unwrap();
    kick.instrument = Some(instrument);
    kick.playlist_mut().unwrap().clips.push(clip);
    song
}

fn score_window(song: &mut Session, first: usize, total: usize) {
    for (track_index, stride) in [1, 2].into_iter().enumerate() {
        let clip = &mut song.tracks[track_index].playlist_mut().unwrap().clips[0];
        let ClipContent::Midi { sequence } = &mut clip.content else { unreachable!() };
        // first moves forward only. The window starts full and only shrinks for the final section.
        sequence.notes.truncate(total.saturating_sub(first).div_ceil(stride));
        for (i, note) in sequence.notes.iter_mut().enumerate() {
            let eighth = first + i * stride;
            note.start = eighth as i64 * 480;
            note.velocity = 32 + (eighth % TAGS) as u8 * 6;
        }
    }
}

/// Each tag has a distinct velocity and an independently rendered native-pitch waveform.
/// Comparing the entire voice checks its release as well as its attack.
fn templates() -> &'static [Vec<f32>] {
    static REFERENCE: OnceLock<Vec<Vec<f32>>> = OnceLock::new();
    REFERENCE.get_or_init(|| {
        let mut song = band_song(120., TAGS, 192_000);
        score_window(&mut song, 0, TAGS);
        let mut mix = soundcraft_mix::MixEngine::new(48_000., 128);
        let mut output = vec![vec![0.; 128]; 2];
        let mut waves = Vec::<Vec<f32>>::new();
        let mut sounding = false;
        for pos in (0..192_000).step_by(128) {
            mix.render_with_clock_limit(&song, pos, 128, &mut output, None);
            for &value in &output[0] {
                if value.abs() > 0.000001 {
                    if !sounding {
                        waves.push(Vec::with_capacity(KICK));
                    }
                    waves.last_mut().unwrap().push(value);
                    sounding = true;
                } else {
                    sounding = false;
                }
            }
        }
        assert_eq!(waves.len(), TAGS);
        assert!(waves.iter().enumerate().all(|(i, v)| v.len() == if i % 2 == 0 { KICK } else { HAT }));
        assert!(waves.windows(2).all(|pair| pair[0] != pair[1]));
        waves
    })
}

fn put_voice(result: &mut Rendered, expected: &[i64], start: i64, audio: &[f32]) {
    let references = templates();
    let tag = references.iter().position(|v| v == audio).unwrap_or_else(|| {
        result.damaged += 1;
        references.iter().enumerate().min_by(|(_, a), (_, b)| (a[0] - audio[0]).abs().total_cmp(&(b[0] - audio[0]).abs())).unwrap().0
    });
    // The same velocity repeats only after eight beats. Check the neighbouring repeats
    // without scanning an entire long song for every note.
    let nearest = expected.partition_point(|at| *at < start);
    let group = nearest / TAGS;
    let candidates = group.saturating_sub(1)..=group + 1;
    if let Some(i) = candidates.map(|g| g * TAGS + tag).filter(|i| *i < expected.len()).min_by_key(|i| start.abs_diff(expected[*i])) {
        result.attacks[i].push(start - expected[i]);
    } else {
        result.extra += 1;
    }
}

pub(super) fn render_dj(start_bpm: f64, ideal_us: &[i64], arrivals_us: &[i64], ending: Ending) -> Rendered {
    assert_eq!(ideal_us.len(), arrivals_us.len());
    assert!(ideal_us.len() >= 2 && arrivals_us.windows(2).all(|w| w[1] >= w[0]));
    let sr = SampleRate::HZ_48000;
    let sample = |us: i64| sr.samples(us as f64 / 1e6);
    let expected: Vec<_> = ideal_us.iter().step_by(12).map(|us| sample(*us)).collect();
    let mut result = Rendered {
        attacks: vec![vec![]; expected.len()],
        damaged: 0,
        extra: 0,
        tail_open: false,
        snapshots: Vec::with_capacity(ideal_us.len()),
        transitions: vec![],
        final_state: State::Waiting,
        tempo_events: 0,
    };
    let last = *arrivals_us.last().unwrap();
    let stop_at = matches!(ending, Ending::Stop).then_some(last + 20_000);
    let end = last + if matches!(ending, Ending::Loss { .. }) { 2_010_000 } else { 100_000 };
    let inputs = MidiInputs::with_port_names(vec![], vec!["DJ".into()]);
    let feed = inputs.feed();
    let port = feed.ports().assign("DJ");
    feed.set_clock_follow(Some(port), !matches!(ending, Ending::Loss { coast: false }), 0, false);
    feed.set_clock(ORIGIN);
    feed.receive(port, &[0xfa]);
    let mut song = band_song(start_bpm, expected.len().min(SCORE_WINDOW), sample(end));
    let mut mix = soundcraft_mix::MixEngine::new(48_000., 128);
    let mut output = vec![vec![0.; 128]; 2];
    let mut voice = Vec::with_capacity(KICK);
    let mut onset = 0;
    let mut pos = 0;
    let mut now = 0;
    let mut p = 0;
    let mut running = false;
    let mut actions = Vec::new();
    loop {
        feed.set_clock(ORIGIN + now);
        let pulse = arrivals_us.get(p) == Some(&now);
        if pulse {
            // Keep a bounded window of notes in the score, with overlap in both directions.
            // The mix graph, tempo map and follower run uninterrupted for a multi-hour case.
            if p.is_multiple_of(384) {
                score_window(&mut song, (p / 12).saturating_sub(16), expected.len());
            }
            feed.receive(port, &[0xf8]);
        } else if stop_at == Some(now) {
            feed.receive(port, &[0xfc]);
        }
        let snap = feed.clock_follow_status(&mut actions);
        for action in actions.drain(..) {
            match action {
                Action::Stop => {
                    running = false;
                    mix.reset();
                }
                Action::Play { .. } | Action::Started { .. } => running = true,
                Action::Locate(_) => {}
            }
        }
        if snap.state != result.final_state {
            result.transitions.push((p, now, snap.state));
            result.final_state = snap.state;
        }
        if pulse {
            result.snapshots.push(snap);
            update_grid(&mut song.tempo, Some((0, ORIGIN + arrivals_us[0])), 0, snap, sr, pos + 1440);
            p += 1;
        }
        if now >= end {
            break;
        }
        let next_wire = arrivals_us.get(p).copied().unwrap_or(end);
        // A loss must be observable in the silence before the next packet. At ordinary
        // tempos the next F8 arrives well inside the minimum loss timeout.
        let previous = arrivals_us.get(p.saturating_sub(1)).copied().unwrap_or(0);
        let mut next = if next_wire - previous >= 75_000 { next_wire.min(now + 1_000) } else { next_wire };
        if let Some(stop) = stop_at.filter(|s| *s > now) {
            next = next.min(stop);
        }
        let until = sample(next);
        while pos < until {
            let count = (until - pos).min(128) as usize;
            if running {
                mix.render_with_clock_limit(&song, pos, count, &mut output, feed.clock_render_tick());
            }
            for i in 0..count {
                let value = if running { output[0][i] } else { 0. };
                if value.abs() > 0.000001 {
                    if voice.is_empty() {
                        onset = pos + i as i64;
                    }
                    voice.push(value);
                } else if !voice.is_empty() {
                    put_voice(&mut result, &expected, onset, &voice);
                    voice.clear();
                }
            }
            pos += count as i64;
        }
        now = next;
    }
    result.tail_open = !voice.is_empty();
    result.tempo_events = song.tempo.tempos().len();
    result
}
