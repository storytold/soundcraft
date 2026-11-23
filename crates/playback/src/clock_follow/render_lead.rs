use crate::callback::AudioState;
use crate::midi::MidiFeed;
use crate::sync_clock::DeviceTiming;
use crate::{Cmd, MeterSnapshot, Shared};
use soundcraft_model::soundcraft_midi::{Note, Sequence};
use soundcraft_model::{ChannelFormat, Clip, Insert, Session, SourceAudio, SourceId, TrackKind};
use soundcraft_time::{SampleRate, TempoMap};
use std::sync::{Arc, Mutex, mpsc::channel};
use std::time::{Duration, Instant};

const RATE: i64 = 48_000;
const EIGHTHS: i64 = 32;

fn song(bpm: i64, clicks: bool) -> Result<Session, &'static str> {
    let mut s = Session {
        tempo: TempoMap::new(bpm as f64, 4, 4).map_err(|_| "invalid fixture tempo")?,
        sample_rate: SampleRate::HZ_48000,
        ..Session::default()
    };
    s.edit.click = clicks;
    if !clicks {
        let id = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        let clip = s.new_clip_id();
        let sid = SourceId(200);
        let burst = (0..96).map(|n| 0.5 + 0.3 * (n as f32 * 0.31).sin()).collect();
        s.pool.insert(sid, Arc::new(SourceAudio::new(craft_audio::AudioBuffer { sample_rate: 48000, channels: vec![burst] })));
        let track = s.track_mut(id).ok_or("instrument track missing")?;
        let mut ins = Insert::new("sampler");
        ins.sample = Some(sid);
        ins.params.insert("root_note".into(), 60.);
        ins.params.insert("attack".into(), 0.);
        track.instrument = Some(ins);
        let mut seq = Sequence::default();
        for i in 1..=EIGHTHS {
            seq.notes.push(Note { start: i * 480, length: 80, pitch: 60, channel: 0, velocity: 110, release_velocity: 64 });
        }
        track.playlist_mut().ok_or("instrument playlist missing")?.clips.push(Clip::midi(clip, "eighths", 0, RATE * 60, seq));
    }
    Ok(s)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Policy {
    Reference,
    Hard,
    Predict,
    Expired,
    Clamped,
}

fn render(bpm: i64, buf: usize, lead_ms: i64, clicks: bool, policy: Policy) -> Result<Vec<f32>, &'static str> {
    let lead = lead_ms * RATE / 1000;
    let session = Arc::new(song(bpm, clicks)?);
    let shared = Arc::new(Shared { position: 0.into(), playing: false.into(), speed: 1000.into(), meters: Mutex::new(MeterSnapshot::default()) });
    let (tx, rx) = channel();
    let mut audio = AudioState::new(rx, shared, session, RATE as f64, channel().0);
    let feed = Arc::new(MidiFeed::default());
    feed.set_clock_follow(Some(1), true, 0, false);
    tx.send(Cmd::Midi(feed.clone())).map_err(|_| "audio command queue closed")?;
    tx.send(if policy != Policy::Reference {
        Cmd::PlayClock { origin: 0, render_from: lead, arrival: None }
    } else {
        Cmd::Play { from: lead, end: None, looped: None }
    })
    .map_err(|_| "playback command queue closed")?;
    audio.device_buffer_frames = buf;
    let start = Instant::now();
    let epoch = feed.timestamp_at(start);
    let mut stream_lead = crate::sync_clock::StreamLead::default();
    let measured = Duration::from_millis(lead_ms as u64);
    stream_lead.observe(start, Some(measured));
    let mut pulse = 0;
    feed.set_clock(epoch);
    feed.receive(1, &[0xfa]);
    let mut out = vec![0.; buf * 2];
    // Samples before the first audible device block cannot be changed at Start.
    let mut pcm = vec![0.; lead as usize];
    let last = EIGHTHS * RATE * 30 / bpm;
    // The final click has 30 ms of tail. Stop before the next beat.
    let end = last + RATE * 140 / 1000;
    while (pcm.len() as i64) < end {
        let at = pcm.len() as i64 - lead;
        // Send precisely the wire pulses already delivered at the device callback.
        // No pulse that arrives mid-buffer can affect this callback's finished PCM.
        while pulse * RATE * 60 <= at * bpm * 24 {
            feed.set_clock(epoch + (pulse * 60_000_000 + bpm * 12) / (bpm * 24));
            feed.receive(1, &[0xf8]);
            pulse += 1;
        }
        feed.set_clock(epoch + (at * 1_000_000 + RATE / 2) / RATE);
        let callback = start + Duration::from_secs_f64(at as f64 / RATE as f64);
        audio.device_timing = match policy {
            Policy::Hard => None,
            Policy::Expired => Some(DeviceTiming::new(callback - Duration::from_secs(2), Some(measured), 0)),
            Policy::Clamped => Some(stream_lead.observe(callback, None)),
            Policy::Reference | Policy::Predict => Some(DeviceTiming::new(callback, Some(measured), 0)),
        };
        audio.fill(&mut out, 2);
        pcm.extend(out.as_chunks::<2>().0.iter().map(|f| f[0]));
    }
    pcm.truncate(end as usize);
    Ok(pcm)
}

// Merge short zero crossings in a single click, preserving the actual first and
// last nonzero samples. A 30 ms click is separated from the next by at least 170 ms.
fn bursts(pcm: &[f32]) -> Vec<(usize, usize)> {
    let mut result: Vec<(usize, usize)> = Vec::new();
    for (i, _) in pcm.iter().enumerate().filter(|(_, v)| v.abs() > 1e-10) {
        if let Some(last) = result.last_mut()
            && i - last.1 <= 16
        {
            last.1 = i + 1;
        } else {
            result.push((i, i + 1));
        }
    }
    result
}

#[test]
fn fast_clock_notes_and_clicks_reach_the_device_on_the_reference_samples() -> Result<(), &'static str> {
    let mut all = true;
    for bpm in [200, 240, 300] {
        for buf in [128, 512, 2048] {
            for lead in [30, 50] {
                for clicks in [false, true] {
                    let reference = render(bpm, buf, lead, clicks, Policy::Reference)?;
                    let hard = render(bpm, buf, lead, clicks, Policy::Hard)?;
                    let predict = render(bpm, buf, lead, clicks, Policy::Predict)?;
                    let small = (buf as i64) * bpm * 24 < RATE * 60;
                    let expected = bursts(&reference);
                    assert_eq!(expected.len(), (if clicks { EIGHTHS / 2 } else { EIGHTHS }) as usize);
                    let starts: Vec<_> = expected.iter().map(|(start, _)| *start).collect();
                    let lengths: Vec<_> = expected.iter().map(|(start, end)| end - start).collect();
                    eprintln!("reference bpm={bpm} frames={buf} ms={lead} clicks={clicks} starts={starts:?} lengths={lengths:?}");
                    for (policy, pcm) in [(Policy::Hard, &hard), (Policy::Predict, &predict)] {
                        let actual = bursts(pcm);
                        let errors: Vec<_> = actual.iter().zip(&expected).map(|(&(on, _), &(at, _))| on as i64 - at as i64).collect();
                        let intact = actual.iter().zip(&expected).filter(|&(&(on, off), &(at, end))| pcm[on..off] == reference[at..end]).count();
                        eprintln!(
                            "lead bpm={bpm} frames={buf} ms={lead} clicks={clicks} policy={policy:?} count={}/{} intact={intact} samples={errors:?}",
                            actual.len(),
                            expected.len()
                        );
                        all &= actual.len() == expected.len() && intact == expected.len();
                        if !small && policy == Policy::Predict {
                            all &= errors.iter().all(|e| *e == 0);
                        }
                    }
                    if small {
                        assert_eq!(predict, hard, "short callbacks must retain the strict horizon");
                    }
                }
            }
        }
    }
    assert!(all, "externally clocked output differed from the presentation-aligned PCM reference");
    Ok(())
}

#[test]
fn prediction_stops_at_the_first_overdue_or_slow_pulse() {
    let feed = MidiFeed::default();
    feed.set_clock_follow(Some(1), true, 0, false);
    feed.set_clock(0);
    feed.receive(1, &[0xfa]);
    for pulse in 0..=48 {
        feed.set_clock(pulse * 10_000);
        feed.receive(1, &[0xf8]);
    }
    let hard = feed.clock_render_tick().unwrap();
    assert!(feed.clock_render_tick_for_output(489_999, 42_667, 550_000).unwrap() > hard);
    assert_eq!(feed.clock_render_tick_for_output(490_001, 42_667, 550_000), Some(hard));
    feed.set_clock(493_000);
    feed.receive(1, &[0xf8]);
    assert_eq!(feed.clock_render_tick_for_output(493_000, 42_667, 550_000), feed.clock_render_tick());
    feed.receive(1, &[0xfc]);
    assert_eq!(feed.clock_render_tick_for_output(493_000, 42_667, 550_000), feed.clock_render_tick());
}

#[test]
fn expired_and_clamped_device_timestamps_keep_the_hard_horizon() -> Result<(), &'static str> {
    for clicks in [true, false] {
        let hard = render(300, 1500, 50, clicks, Policy::Hard)?;
        assert_eq!(hard, render(300, 1500, 50, clicks, Policy::Expired)?);
        assert_eq!(hard, render(300, 1500, 50, clicks, Policy::Clamped)?);
    }
    Ok(())
}

#[test]
fn held_note_remains_a_complete_voice_when_device_prediction_reopens() -> Result<(), &'static str> {
    let s = song(300, false)?;
    let mut free = soundcraft_mix::MixEngine::new(RATE as f32, 2048);
    let mut follow = soundcraft_mix::MixEngine::new(RATE as f32, 2048);
    let mut buf = vec![vec![0.; 2048]; 2];
    free.render(&s, 4096, 2048, &mut buf);
    let reference = buf[0][704..800].to_owned();
    assert!(reference.iter().all(|v| v.abs() > 1e-10));
    // The attack at 4800 and release at 5600 were both held by the wire gate.
    follow.render_with_clock_limit(&s, 4096, 2048, &mut buf, Some(418));
    assert!(bursts(&buf[0]).is_empty());
    follow.render_with_clock_prediction(&s, 6144, 512, &mut buf, 666);
    let actual = bursts(&buf[0][..512]);
    assert_eq!(actual.len(), 1, "the overdue attack must not collapse into its release");
    let (start, end) = actual[0];
    assert_eq!(&buf[0][start..end], reference);
    Ok(())
}
