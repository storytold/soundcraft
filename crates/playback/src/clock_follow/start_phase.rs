//! An external Start or Continue arrives between two device callbacks. Playback begins
//! at the next callback, so the first audible frame must be placed from that callback's
//! presentation time, not from the moment the input thread queued the start.
use super::{Action, PreparedResume, State};
use crate::sync_clock::DeviceTiming;
use crate::sync_output::Output;
use crate::{Cmd, Shared, callback::AudioState, midi::MidiFeed};
use soundcraft_model::Session;
use soundcraft_time::{SampleRate, TICKS_PER_QUARTER, TempoMap};
use std::sync::{Arc, Mutex, atomic::Ordering, mpsc::channel};
use std::time::{Duration, Instant};

const RATE: i64 = 48_000;
const FRAMES: usize = 240;
const CALLBACK_US: i64 = 5_000;
const LEAD_US: i64 = 6_000;

/// Timeline sample presented by the first frame of the first callback after the start.
fn first_presented_sample(spp: u8, arrival_after_callback_us: i64) -> Result<(i64, i64), &'static str> {
    let song =
        Session { sample_rate: SampleRate::HZ_48000, tempo: TempoMap::new(120., 4, 4).map_err(|_| "invalid fixture tempo")?, ..Default::default() };
    let doc = Arc::new(song.clone());
    let shared = Arc::new(Shared { position: 0.into(), playing: false.into(), speed: 1000.into(), meters: Mutex::default() });
    let (sender, rx) = channel();
    let tx = Arc::new(sender);
    let mut audio = AudioState::new(rx, shared, Arc::new(song), RATE as f64, channel().0);
    audio.device_buffer_frames = FRAMES;
    let feed = Arc::new(MidiFeed::default());
    feed.set_clock_follow(Some(0), true, 0, false);
    assert!(tx.send(Cmd::Midi(feed.clone())).is_ok());
    let output: Arc<Mutex<Output>> = Arc::default();
    output.lock().map_err(|_| "output lock poisoned")?.configure(&doc, &tx, None);

    let base = Instant::now();
    let base_us = feed.timestamp_at(base);
    let lead = Duration::from_micros(LEAD_US as u64);
    let mut out = vec![0.; FRAMES * 2];
    let mut callback = |audio: &mut AudioState, k: i64| {
        audio.device_timing = Some(DeviceTiming::new(base + Duration::from_micros((k * CALLBACK_US) as u64), Some(lead), 0));
        audio.fill(&mut out, 2);
    };
    // Idle callbacks publish the device lead that the prepared start reads.
    for k in 0..4 {
        callback(&mut audio, k);
    }
    assert_eq!(output.lock().map_err(|_| "output lock poisoned")?.reported_lead_us(), Some(LEAD_US as u64));

    let pulse = i64::from(spp) * 6;
    let origin = doc.tempo.tick_to_samples(pulse * (TICKS_PER_QUARTER / 24), doc.sample_rate);
    feed.set_clock(base_us + 3 * CALLBACK_US);
    feed.receive(0, &[0xf2, spp, 0]);
    let mut events = Vec::new();
    feed.clock_follow_status(&mut events);
    assert_eq!(events, [Action::Locate(pulse)]);
    let revision = Arc::new(Mutex::new(true));
    assert!(feed.arm_clock_resume(
        0,
        PreparedResume {
            revision: Arc::downgrade(&revision),
            pulse,
            origin,
            offset: 0,
            sr: doc.sample_rate,
            shared: Arc::downgrade(&audio.shared),
            tx: Arc::downgrade(&tx),
            output: Arc::downgrade(&output),
            silent: false,
        }
    ));
    // Continue and the first F8 arrive after callback 3 and before callback 4.
    let arrival = base_us + 3 * CALLBACK_US + arrival_after_callback_us;
    feed.set_clock(arrival);
    feed.receive(0, &[0xfb, 0xf8]);
    events.clear();
    assert_eq!(feed.clock_follow_status(&mut events).state, State::Locked);
    assert!(matches!(events[..], [Action::Started { at, .. }] if at == arrival), "{events:?}");

    feed.set_clock(base_us + 4 * CALLBACK_US);
    callback(&mut audio, 4);
    let after = audio.shared.position.load(Ordering::Acquire);
    // The first frame of callback 4 is heard LEAD_US after it; by then the source is that far past `arrival`.
    let heard = base_us + 4 * CALLBACK_US + LEAD_US;
    let expected = origin + (heard - arrival) * RATE / 1_000_000;
    Ok((after - FRAMES as i64, expected))
}

#[test]
fn external_start_between_callbacks_presents_the_source_position_at_the_next_callback() -> Result<(), &'static str> {
    let mut failures = Vec::new();
    for spp in [0, 8] {
        for arrival_after_callback_us in [250, 2_500, 4_750] {
            let (actual, expected) = first_presented_sample(spp, arrival_after_callback_us)?;
            eprintln!("spp={spp} arrival=+{arrival_after_callback_us}us actual={actual} expected={expected} error={}", actual - expected);
            if (actual - expected).abs() > 1 {
                failures.push((spp, arrival_after_callback_us, actual - expected));
            }
        }
    }
    assert!(failures.is_empty(), "start phase depends on where the external start fell within the device buffer: {failures:?}");
    Ok(())
}
