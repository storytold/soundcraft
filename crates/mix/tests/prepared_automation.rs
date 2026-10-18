//! Independent legacy sampling references for paired automation lookup preparation.

use soundcraft_audio_io::AudioBuffer;
use soundcraft_dsp::pan::PanLaw;
use soundcraft_dsp::{Category, Plugin, PluginInfo};
use soundcraft_mix::{MixEngine, PreparedInstance, PreparedSession, RangeRender, render_range};
use soundcraft_model::{
    AutoParam, AutomationLane, AutomationMode, AutomationPoint, ChannelFormat, Clip, Insert, Session, SourceAudio, SourceId, TrackId, TrackKind,
};
use soundcraft_time::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

fn fixture() -> (Session, TrackId, Arc<SourceAudio>) {
    let mut source = Session::default();
    let track = source.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Prepared source"));
    let audio = Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 256], vec![0.125; 256]] }));
    source.pool.insert(SourceId(700), Arc::clone(&audio));
    let clip = source.new_clip_id();
    let t = source.track_mut(track).unwrap();
    t.playlist_mut().unwrap().clips.push(Clip::audio(clip, "DC reference", SourceId(700), 0, 0, 256));
    t.mixer.pan = vec![-1.0, 1.0];
    (source, track, audio)
}

fn lane(param: AutoParam, points: &[(i64, f32)]) -> AutomationLane {
    AutomationLane { param, points: points.iter().map(|&(at, value)| AutomationPoint { at, value }).collect(), enabled: false }
}

fn render(mix: &mut MixEngine, pair: &PreparedSession<'_>, at: i64, count: usize) -> Vec<Vec<f32>> {
    let mut output = vec![vec![0.0; count]; 2];
    mix.render_prepared(pair, at, count, &mut output);
    output
}

fn gain(db: f32) -> f32 {
    if db <= -144.0 { 0.0 } else { 10.0f32.powf(db / 20.0) }
}

fn curve(at: i64) -> f32 {
    if at <= 0 {
        -12.0
    } else if at <= 20 {
        -12.0 + 12.0 * at as f32 / 20.0
    } else if at < 50 {
        -6.0 * (at - 20) as f32 / 30.0
    } else {
        -6.0
    }
}

fn assert_samples(actual: &[Vec<f32>], expected: impl Fn(usize, usize) -> f32) {
    for (channel, samples) in actual.iter().enumerate() {
        for (frame, &sample) in samples.iter().enumerate() {
            let reference = expected(channel, frame);
            assert!((sample - reference).abs() < 2e-6, "channel {channel} frame {frame}: {sample} != {reference}");
        }
    }
}

#[test]
fn modes_disabled_lanes_and_trim_keep_the_independent_legacy_block_law() {
    for mode in AutomationMode::ALL {
        let (mut source, track, audio) = fixture();
        let t = source.track_mut(track).unwrap();
        t.mixer.automation_mode = mode;
        t.mixer.volume_db = -3.0;
        t.automation = vec![
            lane(AutoParam::Volume, &[(0, -12.0), (20, 0.0), (50, -6.0)]),
            lane(AutoParam::Mute, &[(0, 0.0), (17, 1.0), (22, 0.0)]),
            lane(AutoParam::Plugin { slot: u8::MAX, param: "trim".into() }, &[(0, -2.0)]),
        ];
        let before = source.to_json().unwrap();
        let pair = PreparedSession::borrowed(&source);
        assert!(pair.is_indexed());
        let mut mix = MixEngine::new(48_000.0, 64);
        mix.pan_law = PanLaw::Zero;
        for (at, count) in [(0, 7), (7, 13), (20, 9), (29, 8)] {
            let reads = mode != AutomationMode::Off;
            let muted = reads && (17..22).contains(&at);
            let volume = |at| if reads && mode != AutomationMode::Write { curve(at) } else { -3.0 };
            let first = gain(if muted { -144.0 } else { volume(at) - 2.0 });
            let last = gain(if muted { -144.0 } else { volume(at + count as i64 - 1) - 2.0 });
            assert_samples(&render(&mut mix, &pair, at, count), |channel, frame| {
                let amplitude = if channel == 0 { 0.25 } else { 0.125 };
                amplitude * (first + (last - first) * frame as f32 / (count - 1) as f32)
            });
        }
        assert_eq!(source.to_json().unwrap(), before);
        assert!(Arc::ptr_eq(source.pool.get(SourceId(700)).unwrap(), &audio));
    }
}

#[test]
fn first_empty_lanes_and_zero_budget_fallback_preserve_static_audio() {
    let (mut source, track, audio) = fixture();
    let t = source.track_mut(track).unwrap();
    t.mixer.volume_db = -6.0;
    t.automation = vec![
        lane(AutoParam::Volume, &[]),
        lane(AutoParam::Volume, &[(0, -144.0)]),
        lane(AutoParam::Pan(0), &[]),
        lane(AutoParam::Pan(0), &[(0, 1.0)]),
        lane(AutoParam::Mute, &[]),
        lane(AutoParam::Mute, &[(0, 1.0)]),
    ];
    let source = Arc::new(source);
    for budget in [0, usize::MAX] {
        let pair = PreparedSession::owned_with_budget(Arc::clone(&source), budget);
        assert_eq!(pair.is_indexed(), budget != 0);
        assert!(Arc::ptr_eq(pair.owned_source().unwrap(), &source));
        let mut mix = MixEngine::new(48_000.0, 64);
        mix.pan_law = PanLaw::Zero;
        assert_samples(&render(&mut mix, &pair, 9, 37), |channel, _| (if channel == 0 { 0.25 } else { 0.125 }) * gain(-6.0));
        assert!(Arc::ptr_eq(pair.source().pool.get(SourceId(700)).unwrap(), &audio));
    }
}

#[test]
fn equal_structure_snapshots_use_their_own_curves_and_keep_prior_pair_usable() {
    let (mut source, track, audio) = fixture();
    source.track_mut(track).unwrap().automation.push(lane(AutoParam::Volume, &[(0, -6.0)]));
    let first = Arc::new(source);
    let mut changed = first.as_ref().clone();
    changed.track_mut(track).unwrap().automation[0].points[0].value = -12.0;
    let first_pair = PreparedSession::owned(Arc::clone(&first));
    let second_pair = PreparedSession::owned(Arc::new(changed));
    let mut mix = MixEngine::new(48_000.0, 64);
    mix.pan_law = PanLaw::Zero;
    for (pair, db) in [(&first_pair, -6.0), (&second_pair, -12.0), (&first_pair, -6.0)] {
        assert_samples(&render(&mut mix, pair, 0, 31), |channel, _| (if channel == 0 { 0.25 } else { 0.125 }) * gain(db));
        assert!(Arc::ptr_eq(pair.source().pool.get(SourceId(700)).unwrap(), &audio));
    }
    assert_eq!(first.track(track).unwrap().automation[0].points[0].value, -6.0);
}

#[test]
fn nonfinite_curves_keep_legacy_finite_output_instead_of_rejecting_the_document() {
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let (mut source, track, _) = fixture();
        source.track_mut(track).unwrap().automation.push(lane(AutoParam::Volume, &[(0, value)]));
        let pair = PreparedSession::borrowed(&source);
        assert!(pair.is_indexed());
        let mut mix = MixEngine::new(48_000.0, 64);
        mix.pan_law = PanLaw::Zero;
        let db = if value == f32::NEG_INFINITY { -144.0 } else { 12.0 };
        assert_samples(&render(&mut mix, &pair, 0, 17), |channel, _| (if channel == 0 { 0.25 } else { 0.125 }) * gain(db));
        assert_eq!(pair.source().track(track).unwrap().automation[0].points[0].value.to_bits(), value.to_bits());
    }
}

#[test]
fn prepared_and_ordinary_range_streams_keep_the_same_lengths_latency_and_samples() {
    let (mut source, track, _) = fixture();
    source.track_mut(track).unwrap().automation.push(lane(AutoParam::Volume, &[(0, -12.0), (20, 0.0), (50, -6.0)]));
    let pair = PreparedSession::borrowed(&source);
    let mut ordinary = RangeRender::new(&source, None, Range::new(7, 101), 37);
    let mut prepared = RangeRender::new(pair.source(), None, Range::new(7, 101), 37);
    let mut collected = vec![Vec::new(); 2];
    while let Some((expected, n)) = ordinary.next_block(&source) {
        let (actual, count) = prepared.next_block_prepared(&pair).unwrap();
        assert_eq!(count, n);
        assert_eq!(actual, expected);
        for (output, channel) in collected.iter_mut().zip(actual) {
            output.extend_from_slice(&channel[..count]);
        }
    }
    assert!(prepared.next_block_prepared(&pair).is_none());
    assert_eq!(collected.iter().map(Vec::len).collect::<Vec<_>>(), vec![94, 94]);
    assert_eq!(render_range(&source, Range::new(7, 101), 37), collected);
}

struct Probe {
    calls: Arc<AtomicUsize>,
    frames: Arc<AtomicUsize>,
    target: Arc<AtomicU32>,
}

impl Plugin for Probe {
    fn info(&self) -> &'static PluginInfo {
        static INFO: PluginInfo = PluginInfo {
            id: "prepared-probe",
            name: "Prepared Probe",
            short_name: "Probe",
            category: Category::Other,
            params: &[],
            is_instrument: false,
            audiosuite: false,
        };
        &INFO
    }
    fn prepare(&mut self, _rate: f32, _block: usize, _channels: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if id == "Gain" {
            self.target.store(value.to_bits(), Ordering::Relaxed);
            true
        } else {
            false
        }
    }
    fn param(&self, id: &str) -> Option<f32> {
        (id == "Gain").then(|| f32::from_bits(self.target.load(Ordering::Relaxed)))
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.frames.fetch_add(frames, Ordering::Relaxed);
        let gain = f32::from_bits(self.target.load(Ordering::Relaxed));
        for channel in io {
            for sample in channel.iter_mut().take(frames) {
                *sample *= gain;
            }
        }
    }
}

#[test]
fn adopted_hosted_processor_keeps_block_cadence_and_only_stored_case_exact_keys() {
    let (mut source, track, _) = fixture();
    let mut insert = Insert::new("clap:prepared.synthetic-probe");
    insert.params.insert("Gain".into(), 1.0);
    let t = source.track_mut(track).unwrap();
    t.mixer.inserts[0] = Some(insert);
    t.automation = vec![
        lane(AutoParam::Plugin { slot: 0, param: "Gain".into() }, &[(0, 1.0), (50, 3.0)]),
        lane(AutoParam::Plugin { slot: 0, param: "gain".into() }, &[(0, 99.0)]),
        lane(AutoParam::Plugin { slot: 0, param: "absent".into() }, &[(0, f32::NAN)]),
    ];
    let calls = Arc::new(AtomicUsize::new(0));
    let frames = Arc::new(AtomicUsize::new(0));
    let target = Arc::new(AtomicU32::new(1.0f32.to_bits()));
    let mut mix = MixEngine::new(48_000.0, 64);
    mix.pan_law = PanLaw::Zero;
    mix.set_external_instances(true);
    mix.adopt(vec![PreparedInstance {
        track,
        slot: 0,
        id: "clap:prepared.synthetic-probe".into(),
        ch: 2,
        plugin: Box::new(Probe { calls: Arc::clone(&calls), frames: Arc::clone(&frames), target: Arc::clone(&target) }),
    }]);
    let pair = PreparedSession::borrowed(&source);
    for (at, count) in [(0, 7), (7, 13), (20, 9)] {
        let expected_gain = 1.0 + 2.0 * (at as f32 / 50.0);
        assert_samples(&render(&mut mix, &pair, at, count), |channel, _| (if channel == 0 { 0.25 } else { 0.125 }) * expected_gain);
        assert_eq!(target.load(Ordering::Relaxed), expected_gain.to_bits());
    }
    assert_eq!(calls.load(Ordering::Relaxed), 3);
    assert_eq!(frames.load(Ordering::Relaxed), 29);
    assert!(mix.take_retired().is_empty());
    assert_eq!(mix.meters.get(&track).unwrap().plugin_failures.count, 0);
}
