//! Ordinary strip mute changes sample gates without splitting processor blocks.
use soundcraft_audio_io::AudioBuffer;
use soundcraft_dsp::Plugin;
use soundcraft_mix::{MixEngine, PreparedInstance, render_range};
use soundcraft_model::{
    AutoParam, AutomationLane, AutomationMode, ChannelFormat, Clip, Insert, Route, SendSlot, Session, SourceAudio, SourceId, TrackId, TrackKind,
};
use soundcraft_time::Range;
use std::sync::{Arc, Mutex};

const LEN: usize = 768;
fn fixture() -> (Session, TrackId) {
    let mut s = Session::default();
    s.pool.insert(SourceId(901), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; LEN]; 2] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Source"));
    let clip = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, "tone", SourceId(901), 0, 0, LEN as i64));
    tr.mixer.pan = vec![-1.0, 1.0];
    tr.mixer.output = Route::Main;
    (s, t)
}
fn mute(s: &mut Session, t: TrackId, start: i64, end: i64) {
    let lane = s.track_mut(t).unwrap().lane_mut(&AutoParam::Mute);
    for (at, value) in [(0, 0.0), (start - 1, 0.0), (start, 1.0), (end - 1, 1.0), (end, 0.0)] {
        lane.set_point(at, value);
    }
}
fn rendered(s: &Session, block: usize) -> Vec<Vec<f32>> {
    render_range(s, Range::new(0, LEN as i64), block)
}
fn gated(actual: &[Vec<f32>], reference: &[Vec<f32>], start: usize, end: usize) {
    assert_eq!(actual.len(), reference.len());
    assert!(reference.iter().flatten().any(|x| x.abs() > 0.01));
    for (got, want) in actual.iter().zip(reference) {
        assert_eq!(got.len(), LEN);
        for i in 0..LEN {
            assert_eq!(got[i], if (start..end).contains(&i) { 0.0 } else { want[i] }, "frame {i}");
        }
    }
}
#[test]
fn internal_and_exact_block_edges_are_partition_independent() {
    for (start, end) in [(101, 200), (128, 256), (1, 767)] {
        let (mut s, t) = fixture();
        let reference = rendered(&s, 512);
        mute(&mut s, t, start, end);
        for block in [16, 63, 128, 512, 1024] {
            gated(&rendered(&s, block), &reference, start as usize, end as usize);
        }
    }
}
#[test]
fn existing_held_mute_curve_and_threshold_are_preserved() {
    let (mut s, t) = fixture();
    let reference = rendered(&s, 512);
    let lane = s.track_mut(t).unwrap().lane_mut(&AutoParam::Mute);
    lane.set_point(0, 0.49);
    lane.set_point(200, 0.5);
    gated(&rendered(&s, 512), &reference, 200, LEN);
}
#[test]
fn pre_and_post_fader_send_audio_share_the_track_mute_gate() {
    for pre in [false, true] {
        let (mut s, t) = fixture();
        let bus = s.add_bus("Send", ChannelFormat::Stereo);
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Return"));
        s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
        s.track_mut(aux).unwrap().mixer.output = Route::Main;
        let tr = s.track_mut(t).unwrap();
        tr.mixer.output = Route::None;
        tr.mixer.volume_db = -9.0;
        let mut send = SendSlot::new(Route::Bus(bus));
        send.pre_fader = pre;
        send.level_db = -3.0;
        tr.mixer.sends[0] = Some(send);
        let reference = rendered(&s, 512);
        mute(&mut s, t, 101, 200);
        for block in [63, 512] {
            gated(&rendered(&s, block), &reference, 101, 200);
        }
    }
}
#[test]
fn off_empty_and_first_empty_lane_keep_exact_unautomated_pcm() {
    let (mut s, t) = fixture();
    let reference = rendered(&s, 512);
    s.track_mut(t).unwrap().lane_mut(&AutoParam::Mute);
    assert_eq!(rendered(&s, 512), reference);
    let mut second = AutomationLane::new(AutoParam::Mute);
    second.set_point(0, 1.0);
    s.track_mut(t).unwrap().automation.push(second);
    assert_eq!(rendered(&s, 512), reference, "first empty lane remains authoritative");
    s.track_mut(t).unwrap().automation.clear();
    mute(&mut s, t, 101, 200);
    s.track_mut(t).unwrap().mixer.automation_mode = AutomationMode::Off;
    assert_eq!(rendered(&s, 512), reference);
}
#[test]
fn reading_modes_retain_existing_policy() {
    for mode in AutomationMode::ALL.into_iter().filter(|mode| mode.reads()) {
        let (mut s, t) = fixture();
        let reference = rendered(&s, 512);
        mute(&mut s, t, 101, 200);
        s.track_mut(t).unwrap().mixer.automation_mode = mode;
        gated(&rendered(&s, 512), &reference, 101, 200);
    }
}
#[test]
fn static_mute_and_solo_suppression_cannot_be_reopened_by_automation() {
    for solo in [false, true] {
        let (mut s, t) = fixture();
        if solo {
            let other = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
            s.track_mut(other).unwrap().mixer.solo = true;
        } else {
            s.track_mut(t).unwrap().mixer.mute = true;
        }
        let reference = rendered(&s, 512);
        assert!(reference.iter().flatten().all(|x| x.abs() < 1e-6));
        mute(&mut s, t, 101, 200);
        gated_silent(&rendered(&s, 512));
    }
}
fn gated_silent(output: &[Vec<f32>]) {
    assert!(output.iter().flatten().all(|x| x.abs() < 1e-6));
}
#[test]
fn aux_and_folder_inputs_gate_their_audio_inside_the_block() {
    for kind in [TrackKind::Aux, TrackKind::Folder] {
        let (mut s, source) = fixture();
        let bus = s.add_bus("Input", ChannelFormat::Stereo);
        s.track_mut(source).unwrap().mixer.output = Route::Bus(bus);
        let target = s.add_track(kind, ChannelFormat::Stereo, Some("Return"));
        s.track_mut(target).unwrap().mixer.input = Route::Bus(bus);
        s.track_mut(target).unwrap().mixer.output = Route::Main;
        let reference = rendered(&s, 512);
        mute(&mut s, target, 101, 200);
        gated(&rendered(&s, 512), &reference, 101, 200);
    }
}
#[test]
fn no_active_mute_lane_preserves_fader_and_pan_schedules_exactly() {
    let (mut s, t) = fixture();
    for (param, a, b) in [(AutoParam::Volume, -12.0, -3.0), (AutoParam::Pan(0), -1.0, 0.5)] {
        let lane = s.track_mut(t).unwrap().lane_mut(&param);
        lane.set_point(0, a);
        lane.set_point(LEN as i64, b);
    }
    for block in [63, 256, 512] {
        let reference = rendered(&s, block);
        let mut with_lane = s.clone();
        with_lane.track_mut(t).unwrap().lane_mut(&AutoParam::Mute).set_point(0, 0.0);
        assert_eq!(rendered(&with_lane, block), reference);
        with_lane.track_mut(t).unwrap().mixer.automation_mode = AutomationMode::Off;
        let mut no_lane = with_lane.clone();
        no_lane.track_mut(t).unwrap().automation.retain(|lane| lane.param != AutoParam::Mute);
        assert_eq!(rendered(&with_lane, block), rendered(&no_lane, block));
    }
}
#[test]
fn static_vca_mute_remains_dominant_over_member_mute_automation() {
    let (mut s, t) = fixture();
    let vca = s.add_track(TrackKind::Vca, ChannelFormat::Stereo, Some("VCA"));
    s.track_mut(vca).unwrap().mixer.mute = true;
    s.track_mut(t).unwrap().mixer.vca = Some(vca.0);
    let reference = rendered(&s, 512);
    gated_silent(&reference);
    mute(&mut s, t, 101, 200);
    gated_silent(&rendered(&s, 512));
}

struct Counted {
    inner: Box<dyn Plugin>,
    calls: Arc<Mutex<Vec<usize>>>,
    instrument: bool,
}
impl Plugin for Counted {
    fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, block: usize, ch: usize) {
        self.inner.prepare(sr, block, ch);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.inner.set_param(id, value)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.calls.lock().unwrap().push(frames);
        if self.instrument {
            for channel in io {
                channel.iter_mut().take(frames).for_each(|sample| *sample = 0.25);
            }
        } else {
            self.inner.process(io, frames);
        }
    }
}
#[test]
fn hosted_processing_keeps_the_callers_blocks_and_unmuted_pcm() {
    for instrument in [false, true] {
        let (mut s, t) = fixture();
        let reference = rendered(&s, 256);
        mute(&mut s, t, 101, 200);
        let slot = if instrument {
            s.track_mut(t).unwrap().kind = TrackKind::Instrument;
            s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.mute-cadence"));
            soundcraft_mix::INSTRUMENT_SLOT
        } else {
            s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.mute-cadence"));
            0
        };
        let calls = Arc::new(Mutex::new(Vec::with_capacity(4)));
        let mut plugin = soundcraft_dsp::create("gain").unwrap();
        plugin.prepare(48_000.0, 256, 2);
        let mut mix = MixEngine::new(48_000.0, 256);
        mix.set_external_instances(true);
        mix.adopt(vec![PreparedInstance {
            track: t,
            slot,
            id: "clap:test.mute-cadence".into(),
            ch: 2,
            plugin: Box::new(Counted { inner: plugin, calls: Arc::clone(&calls), instrument }),
        }]);
        let mut output = vec![Vec::new(); 2];
        for (pos, frames) in [(0, 256), (256, 128), (384, 256), (640, 128)] {
            let mut block = vec![vec![0.0; frames]; 2];
            mix.render(&s, pos, frames, &mut block);
            for (out, channel) in output.iter_mut().zip(block) {
                out.extend(channel);
            }
        }
        assert_eq!(*calls.lock().unwrap(), vec![256, 128, 256, 128]);
        gated(&output, &reference, 101, 200);
    }
}
