//! Setter refusal cannot become an applied target; processor receipts remain independent.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use soundcraft_audio_io::AudioBuffer;
use soundcraft_dsp::{Plugin, PluginInfo};
use soundcraft_mix::{MixEngine, PreparedInstance};
use soundcraft_model::{AutoParam, AutomationMode, ChannelFormat, Clip, Insert, Route, Session, SourceAudio, SourceId, TrackId, TrackKind};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Receipt {
    attempts: Vec<f32>,
    processed: Vec<(usize, f32)>,
}
struct Refusing {
    info: Box<dyn Plugin>,
    failures: usize,
    accepted: f32,
    receipt: Arc<Mutex<Receipt>>,
}
impl Plugin for Refusing {
    fn info(&self) -> &'static PluginInfo {
        self.info.info()
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if id != "gain" {
            return false;
        }
        self.receipt.lock().unwrap().attempts.push(value);
        if self.failures > 0 {
            self.failures -= 1;
            return false;
        }
        self.accepted = value;
        true
    }
    fn param(&self, id: &str) -> Option<f32> {
        (id == "gain").then_some(self.accepted)
    }
    fn process(&mut self, audio: &mut [Vec<f32>], frames: usize) {
        // This receipt is written only by the processor, not set_param or Mix's applied cache.
        self.receipt.lock().unwrap().processed.push((frames, self.accepted));
        for channel in audio {
            channel.iter_mut().take(frames).for_each(|value| *value *= self.accepted);
        }
    }
}
fn fixture(neighbor: bool, failures: usize) -> (Session, TrackId, MixEngine, Arc<Mutex<Receipt>>) {
    let mut session = Session::default();
    session.pool.insert(SourceId(901), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 2048]; 2] })));
    let mut selected = None;
    for i in 0..if neighbor { 2 } else { 1 } {
        let track = session.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let clip = session.new_clip_id();
        let tr = session.track_mut(track).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, "dc", SourceId(901), 0, 0, 2048));
        tr.mixer.pan = vec![-1.0, 1.0];
        tr.mixer.output = Route::Main;
        if i == 0 {
            let mut insert = Insert::new("clap:test.refusing-setter");
            insert.params.insert("gain".into(), 0.5);
            tr.mixer.inserts[0] = Some(insert);
            selected = Some(track);
        }
    }
    let track = selected.unwrap();
    let receipt = Arc::new(Mutex::new(Receipt { attempts: Vec::with_capacity(16), processed: Vec::with_capacity(16) }));
    let mut mix = MixEngine::new(48_000.0, 256);
    mix.set_external_instances(true);
    mix.adopt(vec![PreparedInstance {
        track,
        slot: 0,
        id: "clap:test.refusing-setter".into(),
        ch: 2,
        plugin: Box::new(Refusing { info: soundcraft_dsp::create("gain").unwrap(), failures, accepted: 1.0, receipt: Arc::clone(&receipt) }),
    }]);
    (session, track, mix, receipt)
}
fn block(mix: &mut MixEngine, session: &Session, pos: i64, frames: usize, expected: f32) {
    let mut output = vec![vec![0.0; frames]; 2];
    mix.render(session, pos, frames, &mut output);
    assert!(output.iter().flatten().all(|sample| *sample == expected));
}
#[test]
fn unchanged_refused_target_retries_and_neighbor_remains_audible_at_normal_blocks() {
    let (session, _, mut mix, receipt) = fixture(true, 1);
    block(&mut mix, &session, 0, 256, 0.5);
    block(&mut mix, &session, 256, 128, 0.375);
    block(&mut mix, &session, 384, 256, 0.375);
    let receipt = receipt.lock().unwrap();
    assert_eq!(receipt.attempts, vec![0.5, 0.5]);
    assert_eq!(receipt.processed, vec![(256, 1.0), (128, 0.5), (256, 0.5)]);
}
#[test]
fn newer_absolute_target_supersedes_a_refused_value() {
    let (mut session, track, mut mix, receipt) = fixture(false, 1);
    block(&mut mix, &session, 0, 256, 0.25);
    session.track_mut(track).unwrap().mixer.inserts[0].as_mut().unwrap().params.insert("gain".into(), 0.75);
    block(&mut mix, &session, 256, 128, 0.1875);
    assert_eq!(receipt.lock().unwrap().attempts, vec![0.5, 0.75]);
    assert_eq!(receipt.lock().unwrap().processed, vec![(256, 1.0), (128, 0.75)]);
}
#[test]
fn readable_stored_key_automation_retries_without_changing_setter_schedule() {
    let (mut session, track, mut mix, receipt) = fixture(false, 1);
    let lane = session.track_mut(track).unwrap().lane_mut(&AutoParam::Plugin { slot: 0, param: "gain".into() });
    lane.set_point(0, 0.25);
    lane.set_point(256, 0.75);
    block(&mut mix, &session, 0, 256, 0.25);
    block(&mut mix, &session, 256, 128, 0.1875);
    assert_eq!(receipt.lock().unwrap().attempts, vec![0.25, 0.75]);
}
#[test]
fn off_empty_and_missing_stored_keys_keep_existing_policy() {
    for policy in [0, 1, 2] {
        let (mut session, track, mut mix, receipt) = fixture(false, 0);
        let tr = session.track_mut(track).unwrap();
        let lane = tr.lane_mut(&AutoParam::Plugin { slot: 0, param: "gain".into() });
        if policy != 1 {
            lane.set_point(0, 0.25);
        }
        if policy == 0 {
            tr.mixer.automation_mode = AutomationMode::Off;
        }
        if policy == 2 {
            tr.mixer.inserts[0].as_mut().unwrap().params.clear();
        }
        block(&mut mix, &session, 0, 256, if policy == 2 { 0.25 } else { 0.125 });
        assert_eq!(receipt.lock().unwrap().attempts, if policy == 2 { vec![] } else { vec![0.5] });
    }
}
#[test]
fn unknown_stored_id_does_not_silence_known_parameters_or_other_strips() {
    let (mut session, track, mut mix, receipt) = fixture(true, 0);
    session.track_mut(track).unwrap().mixer.inserts[0].as_mut().unwrap().params.insert("unknown".into(), 0.9);
    block(&mut mix, &session, 0, 256, 0.375);
    block(&mut mix, &session, 256, 128, 0.375);
    assert_eq!(receipt.lock().unwrap().processed, vec![(256, 0.5), (128, 0.5)]);
    assert_eq!(receipt.lock().unwrap().attempts, vec![0.5]);
}
#[test]
fn aux_and_master_prepasses_retry_at_the_existing_same_position_opportunity() {
    for kind in [TrackKind::Aux, TrackKind::Master] {
        let (mut session, target, mut mix, receipt) = fixture(false, 1);
        let source = session.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Upstream"));
        let clip = session.new_clip_id();
        let tr = session.track_mut(source).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, "dc", SourceId(901), 0, 0, 2048));
        tr.mixer.pan = vec![-1.0, 1.0];
        tr.mixer.output = Route::Main;
        session.track_mut(target).unwrap().kind = kind;
        if kind == TrackKind::Aux {
            let bus = session.add_bus("Input", ChannelFormat::Stereo);
            session.track_mut(source).unwrap().mixer.output = Route::Bus(bus);
            session.track_mut(target).unwrap().mixer.input = Route::Bus(bus);
        }
        session.edit.delay_compensation = true;
        block(&mut mix, &session, 0, 256, 0.125);
        block(&mut mix, &session, 256, 128, 0.125);
        let receipt = receipt.lock().unwrap();
        assert_eq!(receipt.attempts, vec![0.5, 0.5]);
        assert_eq!(receipt.processed, vec![(256, 0.5), (128, 0.5)]);
    }
}
