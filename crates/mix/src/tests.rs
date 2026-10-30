//! Rule: tests never call the real plug-in hosts. Third-party ids are not created here
//! (create_plugin returns None under cfg(test)); a test that needs a hosted plug-in
//! adopts a fake under a clap:test.* id via MixEngine::adopt.

use super::*;
use soundcraft_audio_io::AudioBuffer;
use soundcraft_model::{Insert, SourceAudio, SourceId};
use std::sync::Arc;

fn synth_session(level_db: f32) -> Session {
    use soundcraft_midi::{Note, Sequence};
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let mut seq = Sequence::default();
    seq.notes.push(Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 });
    let cid = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::midi(cid, "n", 0, 24_000, seq));
    let mut ins = Insert::new("subtractive_synth");
    ins.params.insert("level".into(), level_db);
    tr.instrument = Some(ins);
    s
}

#[test]
fn delay_line_change_keeps_buffers_and_history() {
    // #889: a latency change must not reallocate or zero the delay line in the callback.
    let mut d = DelayLine::default();
    d.set(480, 2);
    let ptrs: Vec<_> = d.bufs.iter().map(|b| b.as_ptr()).collect();
    let mut io = vec![(1..=480).map(|v| v as f32).collect::<Vec<f32>>(); 2];
    d.process(&mut io, 480);
    assert!(io.iter().all(|c| c.iter().all(|&x| x == 0.0)));
    // Shorter delay: the line keeps its storage and outputs what it already holds.
    d.set(240, 2);
    let after: Vec<_> = d.bufs.iter().map(|b| b.as_ptr()).collect();
    assert_eq!(ptrs, after, "delay line reallocated on a shorter delay");
    let mut io = vec![vec![0.0f32; 240]; 2];
    d.process(&mut io, 240);
    let want: Vec<f32> = (241..=480).map(|v| v as f32).collect();
    for c in &io {
        assert_eq!(c, &want, "held samples lost on a latency change");
    }
    // Back to the longer delay: still the same storage.
    d.set(480, 2);
    let after: Vec<_> = d.bufs.iter().map(|b| b.as_ptr()).collect();
    assert_eq!(ptrs, after, "delay line reallocated going back to its size");
}

#[test]
fn stored_instrument_params_reach_the_plugin() {
    // #152: a saved instrument level of -12 dB must render quieter than -6 dB.
    // RMS over the held part of the note, past any parameter smoothing.
    let rms = |db: f32| {
        let out = render_range(&synth_session(db), Range::new(0, 12_000), 512);
        let tail = out[0].get(6_000..).unwrap_or_default();
        (tail.iter().map(|x| x * x).sum::<f32>() / tail.len().max(1) as f32).sqrt()
    };
    let (loud, quiet) = (rms(-6.0), rms(-12.0));
    assert!(loud > 0.0, "the synth should sound");
    assert!((quiet / loud - 0.5).abs() < 0.05, "-12 dB vs -6 dB should halve the level: {quiet} / {loud}");
}

#[test]
fn insert_slot_a_automation_does_not_drive_the_instrument() {
    let mut s = synth_session(-6.0);
    let reference = render_range(&s, Range::new(0, 12_000), 512);
    let tr = &mut s.tracks[0];
    let mut lane = soundcraft_model::AutomationLane::new(AutoParam::Plugin { slot: 0, param: "level".into() });
    lane.set_point(0, -48.0);
    tr.automation.push(lane);
    assert_eq!(render_range(&s, Range::new(0, 12_000), 512), reference);
}

fn session_with_dc(level: f32, frames: usize) -> (Session, TrackId) {
    let mut s = Session::default();
    let t = add_dc_track(&mut s, level, frames);
    (s, t)
}

fn add_dc_track(s: &mut Session, level: f32, frames: usize) -> TrackId {
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![level; frames]] };
    s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(buf)));
    let id = s.new_clip_id();
    if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
        pl.clips.push(Clip::audio(id, "dc", SourceId(900), 0, 0, frames as i64));
    }
    t
}

#[test]
fn mono_center_pan_is_minus_3db() {
    let (s, _) = session_with_dc(0.5, 4800);
    let out = render_range(&s, Range::new(0, 4800), 512);
    let expect = 0.5 * std::f32::consts::FRAC_1_SQRT_2;
    assert!((out[0][1000] - expect).abs() < 1e-3, "{}", out[0][1000]);
    assert!((out[1][1000] - expect).abs() < 1e-3);
}

#[test]
fn fader_mute_and_solo() {
    let (mut s, t) = session_with_dc(0.5, 4800);
    s.track_mut(t).unwrap().mixer.volume_db = -6.0;
    let out = render_range(&s, Range::new(0, 4800), 512);
    let expect = 0.5 * std::f32::consts::FRAC_1_SQRT_2 * db_to_gain(-6.0);
    assert!((out[0][2000] - expect).abs() < 1e-3);
    s.track_mut(t).unwrap().mixer.mute = true;
    let out = render_range(&s, Range::new(0, 4800), 512);
    assert!(out[0][2000].abs() < 1e-6);
    // Soloing another track mutes this one.
    s.track_mut(t).unwrap().mixer.mute = false;
    let other = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    s.track_mut(other).unwrap().mixer.solo = true;
    let out = render_range(&s, Range::new(0, 4800), 512);
    assert!(out[0][2000].abs() < 1e-6);
}

#[test]
fn hard_pans() {
    let (mut s, t) = session_with_dc(0.5, 4800);
    s.track_mut(t).unwrap().mixer.pan = vec![-1.0];
    let out = render_range(&s, Range::new(0, 4800), 256);
    assert!(out[0][100] > 0.49 && out[1][100].abs() < 1e-4);
}

#[test]
fn a_stretched_clip_before_its_source_start_reads_from_the_first_sample() {
    // A clip that extends before its source (a negative offset) plays silence there. When it
    // is stretched, the read position passes through [-1, 0), which must interpolate from
    // that silence towards sample 0 of the file -- not towards sample 1.
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![1.0, 0.5, 0.25, 0.125]] };
    s.pool.insert(SourceId(901), Arc::new(SourceAudio::new(buf)));
    let id = s.new_clip_id();
    let mut c = Clip::audio(id, "late", SourceId(901), -1, 0, 4);
    c.stretch = 2.0;
    if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
        pl.clips.push(c);
    }
    let out = render_clips(&s, t, Range::new(0, 4));
    // rel 0 -> source -1.0: silence; rel 1 -> source -0.5: halfway from silence to sample 0;
    // rel 2 -> source 0.0: sample 0; rel 3 -> source 0.5: halfway to sample 1.
    let want = [0.0, 0.5, 1.0, 0.75];
    for (i, w) in want.iter().enumerate() {
        assert!((out[0][i] - w).abs() < 1e-6, "frame {i}: {} vs {w}", out[0][i]);
    }
}

#[test]
fn fade_curvature_reaches_the_rendered_audio() {
    for fade_in in [true, false] {
        for curvature in [-0.5, 0.0, 0.5] {
            let (mut s, t) = session_with_dc(1.0, 8192);
            s.track_mut(t).unwrap().mixer.pan = vec![-1.0];
            let fade = soundcraft_model::Fade { len: 8192, shape: soundcraft_model::FadeShape::SCurve, curvature };
            let c = &mut s.track_mut(t).unwrap().playlist_mut().unwrap().clips[0];
            if fade_in {
                c.fade_in = fade;
            } else {
                c.fade_out = fade;
            }
            let out = render_range(&s, Range::new(0, 8192), 512);
            let expected = if curvature < 0.0 {
                0.2
            } else if curvature > 0.0 {
                0.8
            } else {
                0.5
            };
            assert!((out[0][4096] - expected).abs() < 1e-5, "{fade_in}, {curvature}: {} vs {expected}", out[0][4096]);
            assert!(out[1][4096].abs() < 1e-6);
            assert!(out[0][if fade_in { 0 } else { 8191 }].abs() < 1e-4);
        }
    }
}

#[test]
fn pan_automation_does_not_depend_on_the_block_size() {
    // Bounces render in 1024-frame blocks, playback in whatever its callback asks for (#244).
    let (mut s, t) = session_with_dc(0.5, 4800);
    {
        let l = s.track_mut(t).unwrap().lane_mut(&AutoParam::Pan(0));
        l.set_point(0, -1.0);
        l.set_point(480, 1.0);
    }
    let bounce = render_range(&s, Range::new(0, 4800), 1024);
    assert!(bounce[0][600].abs() < 1e-4 && bounce[1][600] > 0.49, "{} {}", bounce[0][600], bounce[1][600]);
    for block in [512, 100, 37] {
        assert_eq!(render_range(&s, Range::new(0, 4800), block), bounce, "block {block}");
    }
}

#[test]
fn fades_and_clip_gain_apply() {
    let (mut s, t) = session_with_dc(1.0, 48_000);
    {
        let c = &mut s.track_mut(t).unwrap().playlist_mut().unwrap().clips[0];
        c.fade_in = soundcraft_model::Fade { len: 1000, shape: soundcraft_model::FadeShape::Linear, curvature: 0.0 };
        c.gain_db = -6.0;
    }
    let out = render_range(&s, Range::new(0, 2000), 512);
    let g = std::f32::consts::FRAC_1_SQRT_2 * db_to_gain(-6.0);
    assert!(out[0][0].abs() < 1e-3);
    assert!((out[0][500] - 0.5 * g).abs() < 0.02, "{}", out[0][500]);
    assert!((out[0][1500] - g).abs() < 1e-3);
}

#[test]
fn sends_feed_aux_via_bus() {
    let (mut s, t) = session_with_dc(0.5, 4800);
    let bus = s.add_bus("Verb", ChannelFormat::Stereo);
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Verb"));
    s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
    {
        let tr = s.track_mut(t).unwrap();
        tr.mixer.output = Route::None;
        let mut snd = soundcraft_model::SendSlot::new(Route::Bus(bus));
        snd.level_db = 0.0;
        tr.mixer.sends[0] = Some(snd);
    }
    let out = render_range(&s, Range::new(0, 4800), 512);
    assert!(out[0][1000] > 0.1, "aux should carry the send: {}", out[0][1000]);
}

#[test]
fn render_bus_is_the_bus_not_the_main_mix() {
    let (mut s, t) = session_with_dc(0.5, 4800);
    let stem = s.add_bus("Stem", ChannelFormat::Stereo);
    let empty = s.add_bus("Empty", ChannelFormat::Stereo);
    s.track_mut(t).unwrap().mixer.output = Route::Bus(stem);
    let main = render_range(&s, Range::new(0, 4800), 512);
    let bus = render_bus(&s, stem, Range::new(0, 4800), 512);
    let silent = render_bus(&s, empty, Range::new(0, 4800), 512);
    assert!(main[0][1000].abs() < 1e-4, "main should not carry a track routed to the bus: {}", main[0][1000]);
    assert!(bus[0][1000].abs() > 0.1, "bus should carry the track: {}", bus[0][1000]);
    assert!(silent.iter().all(|c| c.iter().all(|x| x.abs() < 1e-6)));
}

#[test]
fn volume_automation_ramps() {
    let (mut s, t) = session_with_dc(0.5, 48_000);
    {
        let tr = s.track_mut(t).unwrap();
        let l = tr.lane_mut(&AutoParam::Volume);
        l.set_point(0, 0.0);
        l.set_point(24_000, -144.0);
    }
    let out = render_range(&s, Range::new(0, 48_000), 512);
    assert!(out[0][100] > 0.3);
    assert!(out[0][30_000].abs() < 1e-4);
}

#[test]
fn master_volume_breakpoint_does_not_ramp_the_previous_block() {
    for block in [1, 2, 37, 512, 1024] {
        let start = 37;
        let boundary = start + block as i64;
        let end = boundary + block as i64;
        let (mut s, t) = session_with_dc(0.5, end as usize);
        s.track_mut(t).unwrap().mixer.pan = vec![-1.0];
        let master = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
        let lane = s.track_mut(master).unwrap().lane_mut(&AutoParam::Volume);
        lane.set_point(start, 0.0);
        lane.set_point(boundary - 1, 0.0);
        lane.set_point(boundary, -60.0);
        lane.set_point(end - 1, -60.0);

        let mut mixer = MixEngine::new(48_000.0, block);
        let mut out = vec![vec![0.0; block]; 2];
        for (pos, expected) in [(start, 0.5), (boundary, 0.0005)] {
            mixer.render(&s, pos, block, &mut out);
            for (i, &sample) in out[0].iter().enumerate() {
                assert!((sample - expected).abs() < 1e-6, "block {block}, position {pos}, sample {i}: {sample} vs {expected}");
            }
        }
    }
}

#[test]
fn master_volume_ramp_reaches_the_last_sample_of_the_block() {
    for block in [2, 37, 512, 1024] {
        let start = 37;
        let end = start + block as i64;
        let (mut s, t) = session_with_dc(0.5, end as usize);
        s.track_mut(t).unwrap().mixer.pan = vec![-1.0];
        let master = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
        let lane = s.track_mut(master).unwrap().lane_mut(&AutoParam::Volume);
        lane.set_point(start, 0.0);
        lane.set_point(end - 1, -12.0);

        let out = render_range(&s, Range::new(start, end), block);
        assert_eq!(out[0].len(), block);
        assert!((out[0][0] - 0.5).abs() < 1e-6);
        let expected = 0.5 * db_to_gain(-12.0);
        assert!((out[0][block - 1] - expected).abs() < 1e-6, "block {block}: {} vs {expected}", out[0][block - 1]);
    }
}

#[test]
fn inserts_process_and_survive_unknown_plugins() {
    let (mut s, t) = session_with_dc(0.5, 4800);
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::new("does-not-exist"));
    let out = render_range(&s, Range::new(0, 4800), 512);
    assert!(out[0][100] > 0.3);
    let id = soundcraft_dsp::plugins().iter().find(|p| p.id.contains("gain")).map(|p| p.id).unwrap_or("gain");
    let mut ins = Insert::new(id);
    ins.params.insert("gain".into(), -6.0);
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(ins);
    let _ = render_range(&s, Range::new(0, 4800), 512);
}

/// A built-in gain behind a third-party id, with a log of parameter sets.
struct Stateful {
    inner: Box<dyn Plugin>,
    blob: Vec<u8>,
    sets: Arc<std::sync::atomic::AtomicUsize>,
}

impl Plugin for Stateful {
    fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.sets.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.inner.process(io, frames);
    }
    fn state_reader(&mut self) -> Option<Box<dyn soundcraft_dsp::PluginStateReader>> {
        let blob = self.blob.clone();
        Some(Box::new(move || Some(blob.clone())))
    }
}

struct Clock {
    inner: Box<dyn Plugin>,
    seen: Arc<std::sync::Mutex<Vec<Transport>>>,
}

impl Plugin for Clock {
    fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.inner.process(io, frames);
    }
    fn set_transport(&mut self, t: &Transport) {
        self.seen.lock().unwrap().push(*t);
    }
}

#[test]
fn plugins_get_the_session_transport() {
    let (mut s, t) = session_with_dc(0.5, 48_000 * 4);
    s.tempo.set_tempo(0, 90.0).unwrap();
    s.tempo.set_meter(0, 3, 4).unwrap();
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.clock"));
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let clock = Clock { inner: soundcraft_dsp::create("gain").unwrap(), seen: Arc::clone(&seen) };
    let mut eng = MixEngine::new(48_000.0, 512);
    eng.set_external_instances(true);
    eng.adopt(vec![PreparedInstance { track: t, slot: 0, id: "clap:test.clock".into(), ch: 1, plugin: Box::new(clock) }]);
    render_block(&mut eng, &s, 96_000);
    eng.monitor_only = true;
    render_block(&mut eng, &s, 96_000);
    let seen = seen.lock().unwrap();
    let t0 = seen[0];
    assert!(t0.playing && !t0.recording);
    assert_eq!((t0.tempo, t0.numerator, t0.denominator, t0.samples, t0.bar), (90.0, 3, 4, 96_000, 1));
    assert!((t0.seconds - 2.0).abs() < 1e-9 && (t0.quarters - 3.0).abs() < 1e-9 && (t0.bar_start - 3.0).abs() < 1e-9);
    assert!(!seen[1].playing);
}

struct Ears {
    inner: Box<dyn Plugin>,
    heard: Arc<std::sync::Mutex<Vec<(usize, [u8; 3])>>>,
}

impl Plugin for Ears {
    fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.inner.process(io, frames);
    }
    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        self.heard.lock().unwrap().push((offset, msg));
    }
}

#[test]
fn controllers_play_at_their_offsets_and_are_chased_after_a_seek() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.ears"));
    let cc = |tick, kind, value| soundcraft_midi::CtrlEvent { tick, channel: 0, kind, value };
    let ctrls = vec![cc(4, CtrlKind::Cc(64), 127), cc(960, CtrlKind::PitchBend, -8192), cc(1920, CtrlKind::Cc(64), 0)];
    let seq = soundcraft_midi::Sequence { ctrls, ..Default::default() };
    let cid = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "m", 0, 48_000 * 4, seq));
    let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
    let ears = Ears { inner: soundcraft_dsp::create("gain").unwrap(), heard: Arc::clone(&heard) };
    let mut eng = MixEngine::new(48_000.0, 512);
    eng.set_external_instances(true);
    eng.adopt(vec![PreparedInstance { track: t, slot: INSTRUMENT_SLOT, id: "clap:test.ears".into(), ch: 2, plugin: Box::new(ears) }]);
    render_block(&mut eng, &s, 0);
    assert_eq!(std::mem::take(&mut *heard.lock().unwrap()), vec![(0, [0xB0, 121, 0]), (100, [0xB0, 64, 127])]);
    render_block(&mut eng, &s, 36_000);
    assert_eq!(std::mem::take(&mut *heard.lock().unwrap()), vec![(0, [0xB0, 121, 0]), (0, [0xB0, 64, 127]), (0, [0xE0, 0, 0])]);
    render_block(&mut eng, &s, 36_512);
    assert!(heard.lock().unwrap().is_empty());
}

struct Keys {
    inner: Box<dyn Plugin>,
    heard: Arc<std::sync::Mutex<Vec<(u8, u8)>>>,
}

impl Plugin for Keys {
    fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.inner.process(io, frames);
    }
    fn note_on(&mut self, _offset: usize, note: u8, velocity: u8) {
        self.heard.lock().unwrap().push((note, velocity));
    }
}

#[test]
fn real_time_properties_reach_the_instrument() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.keys"));
    let mut seq = soundcraft_midi::Sequence::default();
    seq.notes.push(soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 480 });
    let cid = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "m", 0, 48_000, seq));
    s.edit.values.insert(format!("rtp.{}.transpose", t.0), 12.0);
    s.edit.values.insert(format!("rtp.{}.velocity", t.0), -20.0);
    let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
    let keys = Keys { inner: soundcraft_dsp::create("gain").unwrap(), heard: Arc::clone(&heard) };
    let mut eng = MixEngine::new(48_000.0, 512);
    eng.set_external_instances(true);
    eng.adopt(vec![PreparedInstance { track: t, slot: INSTRUMENT_SLOT, id: "clap:test.keys".into(), ch: 2, plugin: Box::new(keys) }]);
    render_block(&mut eng, &s, 0);
    assert_eq!(*heard.lock().unwrap(), vec![(72, 80)]);
}

fn render_block(eng: &mut MixEngine, s: &Session, pos: i64) -> f32 {
    let mut out = vec![vec![0.0f32; 512]; 2];
    eng.render(s, pos, 512, &mut out);
    out[0][500]
}

#[test]
fn external_instances_are_adopted_and_retired() {
    let (mut s, t) = session_with_dc(0.5, 48_000);
    let mut ins = Insert::new("clap:test.fake");
    ins.params.insert("gain".into(), -6.0);
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(ins);
    let specs = instance_specs(&s);
    assert_eq!(specs, vec![InstanceSpec { track: t, slot: 0, id: "clap:test.fake".into(), ch: 1, state: None }]);
    let mut eng = MixEngine::new(48_000.0, 512);
    eng.set_external_instances(true);
    // Nothing to adopt yet: the slot passes audio through (and sync never tried to load a
    // CLAP plugin on this thread).
    let dry = render_block(&mut eng, &s, 0);
    assert!(dry > 0.3, "{dry}");
    let sets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut gain = soundcraft_dsp::create("gain").unwrap();
    gain.prepare(48_000.0, 512, 1);
    let fake = Stateful { inner: gain, blob: Vec::new(), sets: Arc::clone(&sets) };
    // An instance for the wrong channel count is retired, not used.
    let wrong = Stateful { inner: soundcraft_dsp::create("gain").unwrap(), blob: Vec::new(), sets: Arc::clone(&sets) };
    eng.adopt(vec![PreparedInstance { track: t, slot: 0, id: "clap:test.fake".into(), ch: 2, plugin: Box::new(wrong) }]);
    assert!(render_block(&mut eng, &s, 512) > 0.3);
    assert_eq!(eng.take_retired().len(), 1);
    eng.adopt(vec![PreparedInstance { track: t, slot: 0, id: "clap:test.fake".into(), ch: 1, plugin: Box::new(fake) }]);
    let mut wet = 0.0;
    for b in 2..20 {
        wet = render_block(&mut eng, &s, b * 512);
    }
    assert!((wet / dry - 0.501).abs() < 0.01, "{wet} vs {dry}");
    assert!(sets.load(std::sync::atomic::Ordering::Relaxed) > 0, "no stored state: session params are pushed");
    assert!(eng.take_retired().is_empty());
    // Removing the insert retires the instance instead of dropping it here.
    s.track_mut(t).unwrap().mixer.inserts[0] = None;
    render_block(&mut eng, &s, 20 * 512);
    assert_eq!(eng.take_retired().len(), 1);
    // Instances nobody claims are retired too; drain_plugins empties the engine.
    eng.adopt(vec![PreparedInstance { track: t, slot: 3, id: "clap:x".into(), ch: 1, plugin: soundcraft_dsp::create("gain").unwrap() }]);
    render_block(&mut eng, &s, 21 * 512);
    assert_eq!(eng.take_retired().len(), 1);
    assert!(eng.drain_plugins().is_empty());
}

struct Nan(Box<dyn Plugin>);

impl Plugin for Nan {
    fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
        self.0.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.0.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.0.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.0.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.0.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        io.iter_mut().for_each(|c| c.iter_mut().take(frames).for_each(|x| *x = f32::NAN));
    }
    fn latency(&self) -> usize {
        self.0.latency()
    }
}

fn nan(track: TrackId, slot: usize, ch: usize) -> Vec<PreparedInstance> {
    vec![PreparedInstance { track, slot, id: "clap:test.nan".into(), ch, plugin: Box::new(Nan(soundcraft_dsp::create("gain").unwrap())) }]
}

#[test]
fn an_insert_that_puts_out_nan_is_taken_out_until_the_next_reset() {
    let (mut s, a) = session_with_dc(0.5, 48_000);
    let b = add_dc_track(&mut s, 0.5, 48_000);
    s.track_mut(a).unwrap().mixer.inserts[2] = Some(Insert::new("clap:test.nan"));
    let mut eng = MixEngine::new(48_000.0, 512);
    eng.set_external_instances(true);
    let both = render_block(&mut eng, &s, 0);
    eng.adopt(nan(a, 2, 1));
    let only_b = render_block(&mut eng, &s, 512);
    assert!(only_b > 0.3 && (only_b - both / 2.0).abs() < 1e-6, "{only_b} vs {both}");
    assert_eq!(eng.meters[&a].plugin_failures, PluginFailures { count: 1, last: 2 });
    assert_eq!(eng.meters[&b].plugin_failures, PluginFailures::default());
    assert_eq!(render_block(&mut eng, &s, 1024), both);
    eng.reset();
    assert_eq!(render_block(&mut eng, &s, 1536), only_b);
    assert_eq!(eng.meters[&a].plugin_failures.count, 2);
}

#[test]
fn nan_from_an_instrument_or_a_master_insert_is_contained() {
    let mut s = synth_session(-6.0);
    let synth = s.tracks[0].id;
    s.track_mut(synth).unwrap().instrument = Some(Insert::new("clap:test.nan"));
    let b = add_dc_track(&mut s, 0.5, 48_000);
    let mut eng = MixEngine::new(48_000.0, 512);
    eng.set_external_instances(true);
    eng.adopt(nan(synth, INSTRUMENT_SLOT, 2));
    let only_b = render_block(&mut eng, &s, 0);
    assert!(only_b > 0.3, "{only_b}");
    assert_eq!(eng.meters[&synth].plugin_failures, PluginFailures { count: 1, last: INSTRUMENT_SLOT });
    assert_eq!(eng.meters[&b].plugin_failures, PluginFailures::default());
    let m = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
    s.track_mut(m).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.nan"));
    eng.adopt(nan(m, 0, 2));
    assert_eq!(render_block(&mut eng, &s, 512), 0.0);
    assert_eq!(eng.meters[&m].plugin_failures, PluginFailures { count: 1, last: 0 });
    assert_eq!(render_block(&mut eng, &s, 1024), only_b);
}

#[test]
fn failed_inserts_stop_contributing_latency_until_reset() {
    for kind in [TrackKind::Audio, TrackKind::Aux, TrackKind::Folder, TrackKind::Master] {
        for block in [64, 512] {
            let mut s = Session::default();
            s.edit.delay_compensation = true;
            let source = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
            let mut impulse = vec![0.0; block * 6];
            for at in [block, block * 2, block * 4] {
                impulse[at] = 0.25;
            }
            s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![impulse.clone(), impulse] })));
            let cid = s.new_clip_id();
            s.track_mut(source).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(cid, "impulses", SourceId(900), 0, 0, (block * 6) as i64));
            let target = if kind == TrackKind::Audio { source } else { s.add_track(kind, ChannelFormat::Stereo, None) };
            if matches!(kind, TrackKind::Aux | TrackKind::Folder) {
                let bus = s.add_bus("Return", ChannelFormat::Stereo);
                s.track_mut(source).unwrap().mixer.output = Route::Bus(bus);
                s.track_mut(target).unwrap().mixer.input = Route::Bus(bus);
            }
            let mut failed = Insert::new("clap:test.nan");
            failed.params.insert("lookahead".into(), 5.0);
            let mut healthy = Insert::new("compressor");
            healthy.params.insert("lookahead".into(), 1.0);
            healthy.params.insert("ratio".into(), 1.0);
            let tr = s.track_mut(target).unwrap();
            tr.mixer.inserts[0] = Some(failed);
            tr.mixer.inserts[1] = Some(healthy);
            let mut plugin = soundcraft_dsp::create("compressor").unwrap();
            plugin.prepare(48_000.0, block, 2);
            let mut eng = MixEngine::new(48_000.0, block);
            eng.set_external_instances(true);
            eng.adopt(vec![PreparedInstance { track: target, slot: 0, id: "clap:test.nan".into(), ch: 2, plugin: Box::new(Nan(plugin)) }]);
            let mut out = vec![vec![0.0; block]; 2];
            eng.render(&s, 0, block, &mut out);
            assert!(out.iter().flatten().all(|x| *x == 0.0));
            assert_eq!(eng.meters[&target].plugin_failures.count, 1);

            for b in [1, 2, 4] {
                if b == 4 {
                    // Reset retries the failed plugin once; the next block must drop its
                    // latency again, while keeping the healthy insert's 48-sample delay.
                    eng.reset();
                    eng.render(&s, (block * 3) as i64, block, &mut out);
                    assert_eq!(eng.meters[&target].plugin_failures.count, 2);
                }
                eng.render(&s, (block * b) as i64, block, &mut out);
                assert_eq!(eng.latency, 48, "{kind:?}, block {block}, pass {b}: only the healthy insert delays audio");
                for channel in &out {
                    assert_eq!(impulse_at(channel), vec![48], "{kind:?}, block {block}, pass {b}: no compensation for the failed insert");
                    assert!((channel[48] - 0.25).abs() < 1e-5);
                }
            }
        }
    }
}

#[test]
fn a_new_instance_replaces_a_matching_one_in_its_slot() {
    // The host removed and re-added the plugin, and both session updates reached the audio
    // thread in one go: the slot still holds the old instance (same id, same channels), but
    // the host made a new one and drives its editor. The new one must win, or the editor
    // talks to an instance that was retired ("the plugin is gone").
    let (mut s, t) = session_with_dc(0.5, 48_000);
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.fake"));
    let mut eng = MixEngine::new(48_000.0, 512);
    eng.set_external_instances(true);
    let sets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let make = |blob: &[u8]| {
        let mut gain = soundcraft_dsp::create("gain").unwrap();
        gain.prepare(48_000.0, 512, 1);
        Box::new(Stateful { inner: gain, blob: blob.to_vec(), sets: Arc::clone(&sets) })
    };
    eng.adopt(vec![PreparedInstance { track: t, slot: 0, id: "clap:test.fake".into(), ch: 1, plugin: make(b"old") }]);
    render_block(&mut eng, &s, 0);
    assert!(eng.take_retired().is_empty());
    eng.adopt(vec![PreparedInstance { track: t, slot: 0, id: "clap:test.fake".into(), ch: 1, plugin: make(b"new") }]);
    render_block(&mut eng, &s, 512);
    let mut retired = eng.take_retired();
    assert_eq!(retired.len(), 1, "the old instance is retired");
    assert_eq!(retired[0].save_state(), Some(b"old".to_vec()), "the new instance stays");
}

#[test]
fn stored_state_comes_first_then_session_params() {
    let (mut s, t) = session_with_dc(0.5, 48_000);
    let mut ins = Insert::new("vst3:00000000000000000000000000000001");
    ins.params.insert("gain".into(), -6.0);
    ins.set_state_bytes(b"stored");
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(ins);
    assert_eq!(instance_specs(&s)[0].state.as_deref(), Some(&b"stored"[..]));
    let mut eng = MixEngine::new(48_000.0, 512);
    eng.set_external_instances(true);
    let sets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fake = Stateful { inner: soundcraft_dsp::create("gain").unwrap(), blob: Vec::new(), sets: Arc::clone(&sets) };
    eng.adopt(vec![PreparedInstance { track: t, slot: 0, id: "vst3:00000000000000000000000000000001".into(), ch: 1, plugin: Box::new(fake) }]);
    render_block(&mut eng, &s, 0);
    assert_eq!(sets.load(std::sync::atomic::Ordering::Relaxed), 1, "session params apply once");
    render_block(&mut eng, &s, 512);
    assert_eq!(sets.load(std::sync::atomic::Ordering::Relaxed), 1);
    s.track_mut(t).unwrap().mixer.inserts[0].as_mut().unwrap().params.insert("gain".into(), -12.0);
    render_block(&mut eng, &s, 1024);
    assert_eq!(sets.load(std::sync::atomic::Ordering::Relaxed), 2, "later session edits apply");
    // Offline (not external): third-party plugins are simply absent (tests never load real ones).
    let out = render_range(&s, Range::new(0, 1024), 512);
    assert!(out[0][100] > 0.3);
}

#[test]
fn clip_effects_gain_and_trim_automation_apply() {
    let (mut s, t) = session_with_dc(0.5, 4800);
    let cid = s.track(t).unwrap().clips()[0].id.0;
    s.edit.values.insert(format!("clip_fx.{cid}.gain"), -6.0);
    let out = render_range(&s, Range::new(0, 4800), 512);
    let expect = 0.5 * std::f32::consts::FRAC_1_SQRT_2 * db_to_gain(-6.0);
    assert!((out[0][2000] - expect).abs() < 0.01, "{}", out[0][2000]);
    s.edit.set_flag(&format!("clip_fx.bypass.{cid}"), true);
    let out = render_range(&s, Range::new(0, 4800), 512);
    assert!((out[0][2000] - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
    s.track_mut(t).unwrap().lane_mut(&AutoParam::Plugin { slot: u8::MAX, param: "trim".into() }).set_point(0, -12.0);
    let out = render_range(&s, Range::new(0, 4800), 512);
    assert!(out[0][2000] < 0.2, "{}", out[0][2000]);
}

#[test]
fn delay_compensation_aligns_latent_tracks() {
    let mut s = Session::default();
    let mut imp = vec![0.0f32; 48_000];
    imp[1000] = 1.0;
    s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![imp] })));
    let mut ids = Vec::new();
    for (pan, fx) in [(-1.0f32, true), (1.0, false)] {
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let cid = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::audio(cid, "i", SourceId(900), 0, 0, 48_000));
        tr.mixer.pan = vec![pan];
        if fx {
            tr.mixer.inserts[0] = Some(Insert::new("maximizer"));
        }
        ids.push(t);
    }
    let argmax = |c: &[f32]| c.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).map(|(i, _)| i).unwrap();
    let lat = soundcraft_dsp::create("maximizer").unwrap().latency();
    assert!(lat > 0, "test needs a latent plugin");
    let out = render_range(&s, Range::new(0, 8000), 512);
    assert_eq!(argmax(&out[0]), argmax(&out[1]), "PDC should align L and R");
    s.edit.delay_compensation = false;
    let out = render_range(&s, Range::new(0, 8000), 512);
    assert_ne!(argmax(&out[0]), argmax(&out[1]));
}

/// A reset (stop, locate) must not leave the previous position's audio in the alignment delay.
#[test]
fn reset_clears_delay_compensation_history() {
    let (mut s, t) = session_with_dc(0.5, 4800);
    s.edit.click = false;
    s.track_mut(t).unwrap().mixer.pan = vec![1.0];
    let latent = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let mut comp = Insert::new("compressor");
    comp.params.insert("lookahead".into(), 5.0);
    s.track_mut(latent).unwrap().mixer.inserts[0] = Some(comp);
    let mut eng = MixEngine::new(48_000.0, 512);
    let mut out = vec![vec![0.0f32; 512]; 2];
    for b in 0..9 {
        eng.render(&s, b * 512, 512, &mut out);
    }
    assert!(eng.latency > 0 && out[1][500] > 0.3, "the fixture plays through the alignment delay");
    eng.reset();
    eng.render(&s, 1_000_000, 512, &mut out);
    assert!(out.iter().flatten().all(|x| *x == 0.0), "old aligned audio after a reset");
}

/// A reset (stop, locate) must clear the cached clip EQ/compressor history too.
#[test]
fn reset_clears_clip_effect_history() {
    // Compressor lookahead delay, then EQ high-pass filter state, each primed with DC.
    for (param, value) in [("comp.lookahead", 5.0), ("eq.hpf_on", 1.0)] {
        let (mut s, t) = session_with_dc(0.5, 9600);
        s.edit.click = false;
        let cid = s.track(t).unwrap().clips()[0].id.0;
        s.edit.values.insert(format!("clip_fx.{cid}.{param}"), value);
        let mut out = vec![vec![0.0f32; 512]; 2];
        let mut eng = MixEngine::new(48_000.0, 512);
        for b in 0..4 {
            eng.render(&s, b * 512, 512, &mut out);
        }
        eng.reset();
        eng.render(&s, 4096, 512, &mut out);
        let mut fresh = MixEngine::new(48_000.0, 512);
        let mut expect = vec![vec![0.0f32; 512]; 2];
        fresh.render(&s, 4096, 512, &mut expect);
        assert_eq!(out, expect, "{param}: clip effect state survived the reset");
    }
}

fn impulse_at(ch: &[f32]) -> Vec<usize> {
    ch.iter().enumerate().filter(|(_, v)| v.abs() > 1e-4).map(|(i, _)| i).collect()
}

/// An aux insert's lookahead is part of the mix latency on the first block, not the block after.
#[test]
fn delay_compensation_aligns_an_aux_insert() {
    let mut s = Session::default();
    let n = 8192;
    let mut left = vec![0.0f32; n];
    for at in [0usize, 2048, 8000] {
        left[at] = 0.25;
    }
    s.pool.insert(SourceId(901), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![left.clone(), left] })));
    let bus = s.add_bus("Test Bus", ChannelFormat::Stereo);
    let source = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Source"));
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Return"));
    let cid = s.new_clip_id();
    s.track_mut(source).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(cid, "impulses", SourceId(901), 0, 0, n as i64));
    s.track_mut(source).unwrap().mixer.output = Route::Bus(bus);
    s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
    let render = |s: &Session, start: i64, end: i64| render_range(s, Range::new(start, end), 1024);
    let control = render(&s, 0, n as i64);
    assert_eq!(impulse_at(&control[0]), vec![0, 2048, 8000]);
    assert_eq!(impulse_at(&control[1]), impulse_at(&control[0]));

    let mut comp = Insert::new("compressor");
    comp.params.insert("lookahead".into(), 5.0);
    comp.params.insert("ratio".into(), 1.0);
    s.track_mut(source).unwrap().mixer.inserts[0] = Some(comp.clone());
    let on_source = render(&s, 0, n as i64);
    assert_eq!(impulse_at(&on_source[0]), impulse_at(&control[0]));

    s.track_mut(source).unwrap().mixer.inserts[0] = None;
    s.track_mut(aux).unwrap().mixer.inserts[0] = Some(comp);
    let on_aux = render(&s, 0, n as i64);
    assert_eq!(impulse_at(&on_aux[0]), vec![0, 2048, 8000], "aux lookahead must stay inside the bounce");
    assert_eq!(impulse_at(&on_aux[1]), impulse_at(&on_aux[0]));
    assert!((on_aux[0][8000] - 0.25).abs() < 1e-3, "{}", on_aux[0][8000]);
    let partial = render(&s, 2048, 3072);
    assert_eq!(impulse_at(&partial[0]), vec![0]);

    s.track_mut(aux).unwrap().mixer.inserts[0].as_mut().unwrap().params.insert("lookahead".into(), 0.0);
    let zero = render(&s, 0, n as i64);
    assert_eq!(impulse_at(&zero[0]), impulse_at(&control[0]));
}

#[test]
fn offline_renders_line_up_with_the_timeline_in_any_block_size() {
    let mut s = Session::default();
    let mut imp = vec![0.0f32; 48_000];
    imp[1000] = 1.0;
    s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![imp] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let bus = s.add_bus("Stem", ChannelFormat::Stereo);
    let cid = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(cid, "i", SourceId(900), 0, 0, 48_000));
    tr.mixer.inserts[0] = Some(Insert::new("maximizer"));
    let mut send = soundcraft_model::SendSlot::new(Route::Bus(bus));
    send.level_db = 0.0;
    tr.mixer.sends[0] = Some(send);
    let latency = soundcraft_dsp::create("maximizer").unwrap().latency();
    assert!(latency > 16);
    for source in [None, Some(bus)] {
        for block in [16, 100, 512] {
            let mut got = Vec::new();
            let mut skipped = 0;
            let mut render = RangeRender::new(&s, source, Range::new(900, 2900), block);
            while let Some((chunk, n)) = render.next_block(&s) {
                assert!(n <= block);
                skipped += usize::from(n == 0);
                got.extend_from_slice(&chunk[0][..n]);
            }
            assert_eq!(got.len(), 2000, "{source:?} block {block}");
            assert_eq!(skipped, latency / block, "{source:?} block {block}");
            let peak = got.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).map(|(i, _)| i);
            assert_eq!(peak, Some(100), "{source:?} block {block}");
        }
    }
}

#[test]
fn a_plugins_bogus_latency_is_capped() {
    struct Latent(Box<dyn Plugin>);
    impl Plugin for Latent {
        fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
            self.0.info()
        }
        fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
            self.0.prepare(sr, mb, ch);
        }
        fn reset(&mut self) {
            self.0.reset();
        }
        fn set_param(&mut self, id: &str, v: f32) -> bool {
            self.0.set_param(id, v)
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.0.param(id)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            self.0.process(io, frames);
        }
        fn latency(&self) -> usize {
            u32::MAX as usize
        }
    }
    let (mut s, t) = session_with_dc(0.5, 48_000);
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Mono, None);
    let mut eng = MixEngine::new(48_000.0, 512);
    eng.set_external_instances(true);
    for track in [t, aux] {
        s.track_mut(track).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.latent"));
        let mut gain = soundcraft_dsp::create("gain").unwrap();
        gain.prepare(48_000.0, 512, 1);
        eng.adopt(vec![PreparedInstance { track, slot: 0, id: "clap:test.latent".into(), ch: 1, plugin: Box::new(Latent(gain)) }]);
    }
    render_block(&mut eng, &s, 0);
    assert_eq!((eng.track_latency, eng.latency), (MAX_LATENCY, 2 * MAX_LATENCY));
}

#[test]
fn input_monitoring_replaces_clips() {
    let (mut s, t) = session_with_dc(0.5, 4800);
    let mut eng = MixEngine::new(48_000.0, 512);
    eng.input = vec![vec![0.25; 512]];
    let mut out = vec![vec![0.0; 512]; 2];
    // Not monitored: the clip plays.
    eng.render(&s, 0, 512, &mut out);
    assert!((out[0][100] - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
    // Monitored: the input replaces it, and it still sounds when stopped.
    s.track_mut(t).unwrap().mixer.input_monitor = true;
    eng.render(&s, 512, 512, &mut out);
    assert!((out[0][100] - 0.25 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3, "{}", out[0][100]);
    eng.monitor_only = true;
    eng.render(&s, 1024, 512, &mut out);
    assert!(out[0][100] > 0.1);
    s.track_mut(t).unwrap().mixer.input_monitor = false;
    eng.render(&s, 1024, 512, &mut out);
    assert!(out[0][100].abs() < 1e-6, "stopped, unmonitored: silence");
}

#[test]
fn empty_session_is_silent() {
    let s = Session::default();
    let out = render_range(&s, Range::new(0, 1000), 128);
    assert!(out.iter().all(|c| c.iter().all(|x| *x == 0.0)));
}

#[test]
fn strip_workspace_keeps_its_allocation_between_blocks() {
    let mut s = Session::default();
    for _ in 0..6 {
        s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    }
    let mut eng = MixEngine::new(48_000.0, 512);
    let mut out = vec![vec![0.0; 512]; 2];
    eng.render(&s, 0, 512, &mut out);
    let capacity = eng.work.capacity();
    assert!(capacity >= 6);
    eng.render(&s, 512, 512, &mut out);
    assert_eq!(eng.work.capacity(), capacity);
}

#[test]
fn stopped_monitoring_does_not_click() {
    let mut s = Session::default();
    s.edit.click = true;
    let mut eng = MixEngine::new(48_000.0, 512);
    eng.metronome = true;
    eng.monitor_only = true;
    let mut out = vec![vec![0.0f32; 512]; 2];
    for _ in 0..4 {
        eng.render(&s, 0, 512, &mut out);
        assert!(out.iter().all(|c| c.iter().all(|x| *x == 0.0)));
    }
}

/// Renders `len` frames from zero the way playback does (metronome on), in blocks of `block`.
fn render_playback(s: &Session, len: usize, block: usize, recording: bool) -> Vec<Vec<f32>> {
    let mut eng = MixEngine::new(s.sample_rate.as_f64() as f32, block);
    eng.metronome = true;
    eng.recording = recording;
    let mut out = vec![vec![0.0f32; len]; 2];
    let mut tmp = vec![vec![0.0f32; block]; 2];
    let mut done = 0usize;
    while done < len {
        let n = (len - done).min(block);
        eng.render(s, done as i64, n, &mut tmp);
        for (o, t) in out.iter_mut().zip(tmp.iter()) {
            if let (Some(d), Some(src)) = (o.get_mut(done..done + n), t.get(..n)) {
                d.copy_from_slice(src);
            }
        }
        done += n;
    }
    out
}

#[test]
fn click_sounds_on_every_beat_only_in_playback() {
    let mut s = Session::default();
    let beat = s.sample_rate.as_f64() as usize * 60 / 120; // 120 BPM quarter notes
    let len = beat * 8;
    s.edit.click = true;
    // Offline renders (bounces, stems) never print the metronome.
    let offline = render_range(&s, Range::new(0, len as i64), 512);
    assert!(offline.iter().all(|c| c.iter().all(|x| *x == 0.0)));

    let out = render_playback(&s, len, 512, false);
    for b in 0..8usize {
        let at = b * beat;
        let on: f32 = out[0][at..at + 200].iter().map(|x| x.abs()).fold(0.0, f32::max);
        let off: f32 = out[0][at + beat / 2..at + beat / 2 + 200].iter().map(|x| x.abs()).fold(0.0, f32::max);
        assert!(on > 0.1, "beat {b} should click, peak {on}");
        assert_eq!(off, 0.0, "between beats must stay silent");
        assert_eq!(out[0][at], out[1][at], "both channels get the click");
    }
    // Blocks that start mid-beat render the same clicks as a single pass.
    let split = render_playback(&s, len, 97, false);
    assert_eq!(out, split);
}

#[test]
fn click_only_during_record_follows_recording() {
    let mut s = Session::default();
    s.edit.click = true;
    s.edit.set_flag("click.only_during_record", true);
    let idle = render_playback(&s, 48_000, 512, false);
    assert!(idle.iter().all(|c| c.iter().all(|x| *x == 0.0)));
    let rec = render_playback(&s, 48_000, 512, true);
    assert!(rec.iter().any(|c| c.iter().any(|x| x.abs() > 0.1)));
}

/// A session with one track of `fmt` playing a distinct DC level per channel (0.1, 0.2, …).
fn multichannel_session(fmt: ChannelFormat, main: ChannelFormat, frames: usize) -> (Session, TrackId) {
    let mut s = Session::default();
    s.outputs[0].format = main;
    let t = s.add_track(TrackKind::Audio, fmt, None);
    let chans: Vec<Vec<f32>> = (0..fmt.channels()).map(|c| (0..frames).map(|i| 0.1 * (c + 1) as f32 + (i % 7) as f32 * 1e-3).collect()).collect();
    s.pool.insert(SourceId(901), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: chans })));
    let id = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "mc", SourceId(901), 0, 0, frames as i64));
    (s, t)
}

#[test]
fn surround_track_passes_through_bit_exact() {
    let (s, _) = multichannel_session(ChannelFormat::Surround51, ChannelFormat::Surround51, 4800);
    let src = s.pool.get(SourceId(901)).unwrap().buffer.channels.clone();
    let out = render_range(&s, Range::new(0, 4800), 512);
    assert_eq!(out.len(), 6);
    for (o, x) in out.iter().zip(src.iter()) {
        assert_eq!(o, x, "5.1 → 5.1 must be bit-exact");
    }
}

#[test]
fn surround_to_stereo_folds_down_itu() {
    let (s, _) = multichannel_session(ChannelFormat::Surround51, ChannelFormat::Stereo, 4800);
    let out = render_range(&s, Range::new(0, 4800), 512);
    assert_eq!(out.len(), 2);
    let h = std::f32::consts::FRAC_1_SQRT_2;
    let x = |c: usize| 0.1 * (c + 1) as f32 + (1000 % 7) as f32 * 1e-3;
    // L R C LFE Ls Rs.
    let l = x(0) + h * x(2) + h * x(4);
    let r = x(1) + h * x(2) + h * x(5);
    assert!((out[0][1000] - l).abs() < 1e-5, "{} vs {l}", out[0][1000]);
    assert!((out[1][1000] - r).abs() < 1e-5, "{} vs {r}", out[1][1000]);
}

#[test]
fn mono_track_on_surround_main_pans() {
    let (mut s, t) = session_with_dc(0.5, 4800);
    s.outputs[0].format = ChannelFormat::Surround51;
    // No surround pan: stereo behaviour on L/R.
    let out = render_range(&s, Range::new(0, 4800), 512);
    assert_eq!(out.len(), 6);
    let h = 0.5 * gains(0.0, PanLaw::Minus3).0;
    assert!((out[0][100] - h).abs() < 1e-6 && (out[1][100] - h).abs() < 1e-6);
    assert!(out[2..].iter().all(|c| c[100].abs() < 1e-7));
    // Front centre: the centre speaker only.
    s.track_mut(t).unwrap().mixer.surround = Some(SurroundPan::default());
    let out = render_range(&s, Range::new(0, 4800), 512);
    assert!((out[2][100] - 0.5).abs() < 1e-4, "{}", out[2][100]);
    assert!(out.iter().enumerate().all(|(c, x)| c == 2 || x[100].abs() < 1e-4));
    // Rear right with LFE: Rs plus the LFE send.
    s.track_mut(t).unwrap().mixer.surround = Some(SurroundPan { x: 1.0, y: -1.0, lfe_db: -6.0, ..SurroundPan::default() });
    let mut eng = MixEngine::new(48_000.0, 512);
    let mut buf = vec![vec![0.0; 512]; 6];
    eng.render(&s, 0, 512, &mut buf);
    assert!((buf[5][100] - 0.5).abs() < 1e-4 && (buf[3][100] - 0.5 * db_to_gain(-6.0)).abs() < 1e-4);
    assert_eq!(eng.main_meter.peaks.len(), 6);
    assert_eq!(eng.meters.get(&t).map(|m| m.peaks.len()), Some(1));
    // A stereo device buffer gets the first two channels.
    let mut two = vec![vec![1.0; 512]; 2];
    eng.render(&s, 512, 512, &mut two);
    assert!(two[0][10].abs() < 1e-6);
}

#[test]
fn surround_busses_and_master_fader() {
    let (mut s, t) = multichannel_session(ChannelFormat::Surround51, ChannelFormat::Surround51, 4800);
    let bus = s.add_bus("Stem", ChannelFormat::Surround51);
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Surround51, Some("Stem"));
    s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
    s.track_mut(t).unwrap().mixer.output = Route::Bus(bus);
    let m = s.add_track(TrackKind::Master, ChannelFormat::Surround51, None);
    s.track_mut(m).unwrap().mixer.volume_db = -6.0;
    let out = render_range(&s, Range::new(0, 4800), 512);
    let g = db_to_gain(-6.0);
    for (c, ch) in out.iter().enumerate() {
        let x = 0.1 * (c + 1) as f32 + (1000 % 7) as f32 * 1e-3;
        assert!((ch[1000] - x * g).abs() < 1e-5, "channel {c}: {} vs {}", ch[1000], x * g);
    }
    // A mono bus sums a stereo source like the old stereo bus did.
    let (mut s, t) = session_with_dc(0.5, 4800);
    let bus = s.add_bus("Mono", ChannelFormat::Mono);
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Mono, Some("M"));
    s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
    s.track_mut(aux).unwrap().mixer.pan = vec![-1.0];
    s.track_mut(t).unwrap().mixer.output = Route::Bus(bus);
    let out = render_range(&s, Range::new(0, 4800), 512);
    assert!((out[0][1000] - 0.5 * gains(0.0, PanLaw::Minus3).0).abs() < 1e-6, "{}", out[0][1000]);
}

#[test]
fn surround_upmix_and_downmix_between_layouts() {
    // 5.1 → 7.1: shared speakers map 1:1; 5.x surrounds pan between the 7.1 sides and rears.
    let (s, _) = multichannel_session(ChannelFormat::Surround51, ChannelFormat::Surround71, 2000);
    let out = render_range(&s, Range::new(0, 2000), 512);
    assert_eq!(out.len(), 8);
    let x = |c: usize| 0.1 * (c + 1) as f32 + (100 % 7) as f32 * 1e-3;
    for (c, ch) in out.iter().enumerate().take(4) {
        assert!((ch[100] - x(c)).abs() < 1e-6);
    }
    let e_ls = out[4][100].powi(2) + out[6][100].powi(2);
    assert!((e_ls - x(4).powi(2)).abs() < 1e-5, "Ls energy is preserved");
}

#[test]
fn apply_matrix_span_matches_the_plain_per_sample_sum() {
    let src: Vec<Vec<f32>> = (0..3).map(|c| (0..300).map(|i| ((i * 7 + c * 13) % 23) as f32 * 0.037 - 0.4).collect()).collect();
    let short = vec![src[0].clone(), src[1][..150].to_vec(), src[2].clone()];
    let mut layouts: Vec<Vec<(usize, usize, f32)>> =
        vec![vec![(0, 0, 0.7)], vec![(0, 0, 0.7), (1, 1, -0.3)], vec![(0, 0, 0.6), (1, 0, 0.45), (1, 1, 1.0)]];
    layouts.push(vec![(0, 0, 0.5), (1, 0, 0.25), (2, 0, 0.125), (2, 1, 0.9)]);
    for taps in &layouts {
        let mut m = [[0.0f32; MAX_CHANNELS]; MAX_CHANNELS];
        for &(s, d, g) in taps {
            m[s][d] = g;
        }
        for input in [&src, &short] {
            for span in [0..300, 17..211, 140..160, 250..400, 5..5] {
                let mut got: Vec<Vec<f32>> = (0..2).map(|d| (0..300).map(|i| (i + d) as f32 * 1e-3).collect()).collect();
                let mut want = got.clone();
                pan::apply_matrix_span(input, span.clone(), &mut got, &m, 0.8);
                for (d, out) in want.iter_mut().enumerate() {
                    for (i, o) in out.iter_mut().enumerate().take(span.end).skip(span.start) {
                        let mut acc = 0.0f32;
                        for (s, row) in m.iter().enumerate().take(input.len()) {
                            if row[d] != 0.0 {
                                acc += input[s].get(i).copied().unwrap_or(0.0) * 0.8 * row[d];
                            }
                        }
                        if m.iter().take(input.len()).any(|row| row[d] != 0.0) {
                            *o += acc;
                        }
                    }
                }
                let bits = |v: &Vec<Vec<f32>>| v.iter().flatten().map(|x| x.to_bits()).collect::<Vec<_>>();
                assert_eq!(bits(&got), bits(&want), "taps {taps:?}, span {span:?}");
            }
        }
    }
}

#[test]
fn gain_ramp_matches_converting_both_ends_every_sample() {
    for (db0, db1, n) in [(-6.0f32, 3.0f32, 512usize), (0.0, 0.0, 1), (-144.0, 0.0, 300), (2.5, -60.0, 0), (f32::NAN, -3.0, 64), (-200.0, 12.0, 7)] {
        let ramp = crate::automation::GainRamp::new(db0, db1, n);
        let (g0, g1) = (db_to_gain(db0), db_to_gain(db1));
        for i in 0..=n {
            let want = if n == 0 { g0 } else { g0 + (g1 - g0) * (i as f32 / n as f32) };
            assert_eq!(ramp.at(i).to_bits(), want.to_bits(), "{db0} -> {db1} dB, sample {i} of {n}");
        }
    }
}
