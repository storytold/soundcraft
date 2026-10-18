//! Output-only serial joins use actual edge delays without another routing-order compiler.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use soundcraft_audio_io::AudioBuffer;
use soundcraft_dsp::{Plugin, PluginInfo};
use soundcraft_mix::{MixEngine, PreparedInstance, RangeRender, render_bus, render_range};
use soundcraft_model::{BusId, ChannelFormat, Clip, ClipContent, Insert, Route, SendSlot, Session, SourceAudio, SourceId, TrackId, TrackKind};
use soundcraft_time::Range;
use std::sync::Arc;
const LEN: usize = 256;
const AT: usize = 40;
struct Delay {
    info: Box<dyn Plugin>,
    ring: Vec<Vec<f32>>,
    index: usize,
    len: usize,
    declared: usize,
    bad: bool,
}
impl Plugin for Delay {
    fn info(&self) -> &'static PluginInfo {
        self.info.info()
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {
        for channel in &mut self.ring {
            channel.fill(0.0);
        }
        self.index = 0;
    }
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn latency(&self) -> usize {
        self.declared
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        if self.bad {
            for channel in io {
                channel.iter_mut().take(frames).for_each(|value| *value = f32::NAN);
            }
            return;
        }
        for (channel, ring) in io.iter_mut().zip(&mut self.ring) {
            let mut index = self.index;
            for value in channel.iter_mut().take(frames) {
                if self.len > 0 {
                    std::mem::swap(value, &mut ring[index]);
                    index = (index + 1) % self.len;
                }
            }
        }
        if self.len > 0 {
            self.index = (self.index + frames) % self.len;
        }
    }
}
fn leaf(s: &mut Session, output: Route, name: &str) -> TrackId {
    let track = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some(name));
    let clip = s.new_clip_id();
    let tr = s.track_mut(track).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, name, SourceId(901), 0, 0, LEN as i64));
    tr.mixer.pan = vec![-1.0, 1.0];
    tr.mixer.output = output;
    track
}
fn fixture(depth: usize, direct: bool) -> (Session, Vec<(TrackId, usize)>, Vec<BusId>) {
    let mut s = Session::default();
    s.edit.delay_compensation = true;
    let mut signal = vec![0.0; LEN];
    signal[AT] = 0.25;
    s.pool.insert(SourceId(901), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![signal.clone(), signal] })));
    let buses: Vec<_> = (0..depth).map(|i| s.add_bus(&format!("Bus {i}"), ChannelFormat::Stereo)).collect();
    leaf(&mut s, Route::Bus(buses[0]), "Serial source");
    if direct {
        leaf(&mut s, Route::Main, "Direct reference");
    }
    let mut delays = Vec::new();
    for i in 0..depth {
        let track = s.add_track(if i % 2 == 0 { TrackKind::Aux } else { TrackKind::Folder }, ChannelFormat::Stereo, Some("Return"));
        let tr = s.track_mut(track).unwrap();
        tr.mixer.input = Route::Bus(buses[i]);
        tr.mixer.output = if i + 1 == depth { Route::Main } else { Route::Bus(buses[i + 1]) };
        tr.mixer.pan = vec![-1.0, 1.0];
        tr.mixer.inserts[0] = Some(Insert::new("clap:test.delay"));
        delays.push((track, 3 + 2 * i));
    }
    (s, delays, buses)
}
fn engine(s: &Session, delays: &[(TrackId, usize)], block: usize) -> MixEngine {
    let mut mix = MixEngine::new(48_000.0, block);
    mix.set_external_instances(true);
    mix.adopt(
        delays
            .iter()
            .map(|(track, len)| PreparedInstance {
                track: *track,
                slot: 0,
                id: "clap:test.delay".into(),
                ch: 2,
                plugin: Box::new(Delay {
                    info: soundcraft_dsp::create("gain").unwrap(),
                    ring: vec![vec![0.0; *len]; 2],
                    index: 0,
                    len: *len,
                    declared: *len,
                    bad: false,
                }),
            })
            .collect(),
    );
    mix.ensure_synced(s);
    mix
}
fn raw(s: &Session, delays: &[(TrackId, usize)], block: usize) -> (Vec<f32>, usize) {
    let mut mix = engine(s, delays, block);
    let mut out = Vec::new();
    let mut position = 0;
    while position < LEN {
        let n = (LEN - position).min(block);
        let mut audio = vec![vec![0.0; n]; 2];
        mix.render(s, position as i64, n, &mut audio);
        out.extend_from_slice(&audio[0]);
        position += n;
    }
    (out, mix.latency)
}
fn impulses(audio: &[f32]) -> Vec<(usize, f32)> {
    audio.iter().copied().enumerate().filter(|(_, value)| value.abs() > 1e-5).collect()
}
#[test]
fn two_and_three_serial_aux_folder_outputs_align_direct_reference_from_first_block() {
    for depth in [2, 3] {
        let (s, delays, _) = fixture(depth, true);
        let latency: usize = delays.iter().map(|(_, delay)| *delay).sum();
        for block in [16, 31, 64, 256] {
            let (audio, reported) = raw(&s, &delays, block);
            assert_eq!(reported, latency);
            assert_eq!(impulses(&audio), vec![(AT + latency, 0.5)]);
        }
    }
}
#[test]
fn pdc_off_preserves_the_physical_serial_path_and_direct_reference() {
    let (mut s, delays, _) = fixture(3, true);
    s.edit.delay_compensation = false;
    let (audio, latency) = raw(&s, &delays, 64);
    assert_eq!(latency, 0);
    assert_eq!(impulses(&audio), vec![(AT, 0.25), (AT + 15, 0.25)]);
}
#[test]
fn unconnected_delayed_aux_does_not_move_main_join() {
    let (mut s, mut delays, _) = fixture(2, true);
    let bus = s.add_bus("Empty", ChannelFormat::Stereo);
    let track = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
    let tr = s.track_mut(track).unwrap();
    tr.mixer.input = Route::Bus(bus);
    tr.mixer.output = Route::None;
    tr.mixer.inserts[0] = Some(Insert::new("clap:test.delay"));
    delays.push((track, 23));
    let (audio, latency) = raw(&s, &delays, 64);
    assert_eq!(latency, 8);
    assert_eq!(impulses(&audio), vec![(AT + 8, 0.5)]);
}
#[test]
fn master_insert_delays_add_once_after_the_main_join() {
    let (mut s, mut delays, _) = fixture(2, true);
    for delay in [2, 7] {
        let track = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
        s.track_mut(track).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.delay"));
        delays.push((track, delay));
    }
    let (audio, latency) = raw(&s, &delays, 64);
    assert_eq!(latency, 17);
    assert_eq!(impulses(&audio), vec![(AT + 17, 0.5)]);
}
#[test]
fn a_muted_send_selects_the_whole_legacy_compensation_path() {
    let (mut s, delays, _) = fixture(2, true);
    let bus = s.add_bus("Unused", ChannelFormat::Stereo);
    let mut send = SendSlot::new(Route::Bus(bus));
    send.mute = true;
    s.tracks[0].mixer.sends[0] = Some(send);
    let (audio, latency) = raw(&s, &delays, 64);
    assert_eq!(latency, 5);
    assert_eq!(impulses(&audio), vec![(AT + 5, 0.25), (AT + 10, 0.25)]);
}

fn builtins(s: &mut Session, delays: &[(TrackId, usize)]) {
    for (track, delay) in delays {
        let mut insert = Insert::new("compressor");
        insert.params.insert("lookahead".into(), *delay as f32 / 48.0);
        insert.params.insert("ratio".into(), 1.0);
        insert.params.insert("threshold".into(), 0.0);
        s.track_mut(*track).unwrap().mixer.inserts[0] = Some(insert);
    }
}
#[test]
fn named_bus_joins_and_streamed_main_remove_only_their_selected_path_latency() {
    let (mut s, delays, buses) = fixture(2, true);
    builtins(&mut s, &delays);
    leaf(&mut s, Route::Bus(buses[1]), "Intermediate direct");
    let first = render_bus(&s, buses[0], Range::new(0, LEN as i64), 64);
    let joined = render_bus(&s, buses[1], Range::new(0, LEN as i64), 64);
    assert_eq!(impulses(&first[0]), vec![(AT, 0.25)]);
    assert_eq!(impulses(&joined[0]), vec![(AT, 0.5)]);
    let mut stream = RangeRender::new(&s, None, Range::new(0, LEN as i64), 31);
    let mut audio = Vec::new();
    while let Some((channels, frames)) = stream.next_block(&s) {
        audio.extend_from_slice(&channels[0][..frames]);
    }
    assert_eq!(audio.len(), LEN);
    assert_eq!(impulses(&audio), vec![(AT, 0.75)]);
    // Master processing never changes a selected upstream bus's time origin.
    let master = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
    builtins(&mut s, &[(master, 7)]);
    assert_eq!(render_bus(&s, buses[1], Range::new(0, LEN as i64), 64), joined);
    assert_eq!(impulses(&render_range(&s, Range::new(0, LEN as i64), 64)[0]), vec![(AT, 0.75)]);
}
#[test]
fn ordinary_single_aux_main_path_keeps_legacy_pcm() {
    let (mut s, delays, _) = fixture(1, true);
    let admitted = raw(&s, &delays, 31);
    let unused = s.add_bus("Unused", ChannelFormat::Stereo);
    let mut send = SendSlot::new(Route::Bus(unused));
    send.mute = true;
    s.tracks[0].mixer.sends[0] = Some(send);
    assert_eq!(raw(&s, &delays, 31), admitted);
}
#[test]
fn route_and_pdc_policy_transitions_do_not_release_stale_alignment_samples() {
    let (mut s, delays, _) = fixture(2, true);
    // A single direct impulse is waiting inside the admitted eight-sample history.
    let direct = s.tracks.iter().find(|track| track.name == "Direct reference").unwrap().id;
    s.track_mut(direct).unwrap().playlist_mut().unwrap().clips[0].start = 0;
    if let ClipContent::Audio { offset, .. } = &mut s.track_mut(direct).unwrap().playlist_mut().unwrap().clips[0].content {
        *offset = AT as i64;
    }
    let mut mix = engine(&s, &delays, 16);
    let mut audio = vec![vec![0.0; 1]; 2];
    mix.render(&s, 0, 1, &mut audio);
    assert_eq!(audio[0], vec![0.0]);
    // Remove every live source after the first input frame, without resetting processor tails.
    s.track_mut(direct).unwrap().playlist_mut().unwrap().clips.clear();
    s.tracks[0].playlist_mut().unwrap().clips.clear();
    let unused = s.add_bus("Unused", ChannelFormat::Stereo);
    let mut send = SendSlot::new(Route::Bus(unused));
    send.mute = true;
    s.track_mut(direct).unwrap().mixer.sends[0] = Some(send);
    let mut next = vec![vec![0.0; 16]; 2];
    mix.render(&s, 1, 16, &mut next);
    assert!(next.iter().flatten().all(|sample| *sample == 0.0), "admitted history cannot leak into legacy output");
    s.track_mut(direct).unwrap().mixer.sends[0] = None;
    mix.render(&s, 17, 16, &mut next);
    assert!(next.iter().flatten().all(|sample| *sample == 0.0));
    s.edit.delay_compensation = false;
    mix.render(&s, 33, 16, &mut next);
    s.edit.delay_compensation = true;
    mix.render(&s, 49, 16, &mut next);
    assert!(next.iter().flatten().all(|sample| *sample == 0.0));
}

#[test]
fn cycles_unsupported_instruments_and_clip_effects_keep_exact_legacy_pcm() {
    for policy in [0, 1, 2] {
        let (mut s, delays, buses) = fixture(2, true);
        match policy {
            0 => {
                s.track_mut(delays[1].0).unwrap().mixer.output = Route::Bus(buses[0]);
            }
            1 => {
                s.tracks[0].instrument = Some(Insert::new("clap:test.unsupported-instrument"));
            }
            _ => {
                let clip = s.tracks[0].clips()[0].id.0;
                s.edit.values.insert(format!("clip_fx.{clip}.gain"), 0.0);
            }
        }
        let candidate = raw(&s, &delays, 31);
        let unused = s.add_bus("Unused", ChannelFormat::Stereo);
        let mut send = SendSlot::new(Route::Bus(unused));
        send.mute = true;
        s.tracks[0].mixer.sends[0] = Some(send);
        assert_eq!(raw(&s, &delays, 31), candidate);
    }
}
#[test]
fn reverse_created_deep_order_ties_select_whole_legacy_execution() {
    let (mut s, delays, _) = fixture(11, true);
    // Current processing_order intentionally remains unchanged; its bounded depths can tie.
    let start = s.tracks.len() - 11;
    s.tracks[start..].reverse();
    let candidate = raw(&s, &delays, 31);
    assert_eq!(candidate.1, 23, "legacy maximum Aux latency, not the serial sum");
    let unused = s.add_bus("Unused", ChannelFormat::Stereo);
    let mut send = SendSlot::new(Route::Bus(unused));
    send.mute = true;
    s.tracks[0].mixer.sends[0] = Some(send);
    assert_eq!(raw(&s, &delays, 31), candidate);
}
#[test]
fn a_failed_serial_strip_does_not_silence_the_unrelated_direct_output() {
    let (s, delays, _) = fixture(2, true);
    let mut mix = engine(&s, &delays, 64);
    mix.adopt(vec![PreparedInstance {
        track: delays[0].0,
        slot: 0,
        id: "clap:test.delay".into(),
        ch: 2,
        plugin: Box::new(Delay {
            info: soundcraft_dsp::create("gain").unwrap(),
            ring: vec![vec![0.0; 3]; 2],
            index: 0,
            len: 3,
            declared: 3,
            bad: true,
        }),
    }]);
    let mut audio = vec![vec![0.0; 64]; 2];
    mix.render(&s, 0, 64, &mut audio);
    assert_eq!(impulses(&audio[0]), vec![(AT + 8, 0.25)]);
    assert!(mix.meters.get(&delays[0].0).unwrap().plugin_failures.count > 0);
}
#[test]
fn admitted_output_route_changes_discard_old_edge_history() {
    let (mut s, delays, _) = fixture(2, true);
    let direct = s.tracks.iter().find(|track| track.name == "Direct reference").unwrap().id;
    let clip = &mut s.track_mut(direct).unwrap().playlist_mut().unwrap().clips[0];
    clip.start = 0;
    if let ClipContent::Audio { offset, .. } = &mut clip.content {
        *offset = AT as i64;
    }
    s.tracks[0].playlist_mut().unwrap().clips.clear();
    let mut mix = engine(&s, &delays, 16);
    let mut first = vec![vec![0.0; 1]; 2];
    mix.render(&s, 0, 1, &mut first);
    assert_eq!(first[0], vec![0.0]);
    s.track_mut(direct).unwrap().playlist_mut().unwrap().clips.clear();
    s.track_mut(direct).unwrap().mixer.output = Route::None;
    let mut next = vec![vec![0.0; 16]; 2];
    mix.render(&s, 1, 16, &mut next);
    s.track_mut(direct).unwrap().mixer.output = Route::Main;
    mix.render(&s, 17, 16, &mut next);
    assert!(next.iter().flatten().all(|sample| *sample == 0.0));
}
