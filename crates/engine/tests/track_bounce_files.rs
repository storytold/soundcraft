//! Track Bounce delivers independent files and never commits into the document.
#![cfg(unix)]
use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::{Engine, io};
use soundcraft_model::{AutoParam, ChannelFormat, Clip, ClipContent, Insert, Route, Session, TrackId, TrackKind};
use soundcraft_time::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("sc-track-files-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn audio(s: &mut Session, format: ChannelFormat, name: &str, frames: usize) -> TrackId {
    let t = s.add_track(TrackKind::Audio, format, Some(name));
    let channels =
        (0..format.channels()).map(|c| (0..frames).map(|i| (c + 1) as f32 * 0.025 * if i % 2 == 0 { 1.0 } else { -1.0 }).collect()).collect();
    let src = io::add_source(s, name, AudioBuffer { sample_rate: s.sample_rate.hz(), channels }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, name, src, 0, 0, frames as i64));
    t
}

fn decoded(path: &Path) -> (soundcraft_audio_io::AudioInfo, AudioBuffer) {
    soundcraft_audio_io::decode(&std::fs::read(path).unwrap(), None).unwrap()
}

fn export(e: &mut Engine, t: TrackId, path: &Path, more: Value) -> Value {
    let mut p = json!({"tracks": [t], "path": path, "render_pan": false, "bit_depth": "32f", "dither": false});
    for (key, value) in more.as_object().unwrap() {
        p[key] = value.clone();
    }
    e.execute("track.bounce", &p).unwrap()
}

#[test]
fn all_supported_layouts_keep_channels_mask_frames_and_exact_unpanned_mono() {
    let temp = Temp::new();
    for fmt in ChannelFormat::ALL.into_iter().filter(|f| f.channels() <= 16) {
        let mut s = Session::default();
        let t = audio(&mut s, fmt, "Original", 128);
        s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::new("invert"));
        let mut e = Engine::new(s);
        let before = e.session_arc();
        let path = temp.0.join(format!("{:?}.wav", fmt));
        let out = export(&mut e, t, &path, json!({"render_volume": false}));
        let (_, a) = decoded(&path);
        assert_eq!(a.channels.len(), fmt.channels(), "{fmt:?}");
        assert_eq!(a.frames(), 128);
        assert_eq!(soundcraft_audio_io::wav_channel_mask(&std::fs::read(&path).unwrap()), Some(fmt.channel_mask()));
        for (c, channel) in a.channels.iter().enumerate() {
            assert!((channel[0] + (c + 1) as f32 * 0.025).abs() < 1e-6, "{fmt:?} channel {c}");
        }
        assert_eq!(out["files"][0]["frames"], 128);
        assert!(Arc::ptr_eq(&before, &e.session_arc()));
    }
}

#[test]
fn volume_pan_trim_vca_mute_and_unrelated_paths_have_independent_policies() {
    let temp = Temp::new();
    let mut s = Session::default();
    let t = audio(&mut s, ChannelFormat::Mono, "Voice", 4096);
    let other = audio(&mut s, ChannelFormat::Stereo, "Other", 4096);
    let vca = s.add_track(TrackKind::Vca, ChannelFormat::Mono, None);
    let bus = s.add_bus("Narrow", ChannelFormat::Mono);
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
    s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
    // Unrelated licensed/unknown slots cannot be instantiated or leak into this export.
    s.track_mut(other).unwrap().instrument = Some(Insert::new("clap:unavailable"));
    s.track_mut(other).unwrap().mixer.inserts[0] = Some(Insert::new("vst3:unavailable"));
    s.track_mut(other).unwrap().mixer.solo = true;
    s.track_mut(vca).unwrap().mixer.volume_db = -6.0;
    s.track_mut(vca).unwrap().mixer.inserts[0] = Some(Insert::new("au:unavailable"));
    {
        let t = s.track_mut(t).unwrap();
        t.mixer.trim_db = -6.0;
        t.mixer.phase_invert = true;
        t.mixer.vca = Some(vca.0);
        t.lane_mut(&AutoParam::Volume).set_point(0, -6.0);
        t.lane_mut(&AutoParam::Pan(0)).set_point(0, 1.0);
        t.lane_mut(&AutoParam::Plugin { slot: u8::MAX, param: "trim".into() }).set_point(0, -6.0);
        t.mixer.sends[0] = Some(soundcraft_model::SendSlot::new(Route::Bus(bus)));
    }
    let mut e = Engine::new(s);
    let dry_path = temp.0.join("dry.wav");
    export(&mut e, t, &dry_path, json!({"render_volume": false}));
    let (_, dry) = decoded(&dry_path);
    assert!((dry.channels[0][200].abs() - 0.025 * soundcraft_dsp::db_to_gain(-6.0)).abs() < 1e-6);
    let wet_path = temp.0.join("wet.wav");
    export(&mut e, t, &wet_path, json!({"render_volume": true, "render_pan": true}));
    let (_, wet) = decoded(&wet_path);
    assert_eq!(wet.channels.len(), 2);
    assert!(wet.channels[0].iter().all(|x| x.abs() < 1e-6));
    assert!((wet.channels[1][200].abs() - dry.channels[0][200].abs() * soundcraft_dsp::db_to_gain(-18.0)).abs() < 1e-6);
    e.session_mut().track_mut(t).unwrap().mixer.output = Route::Bus(bus);
    let mono_path = temp.0.join("mono-bus.wav");
    export(&mut e, t, &mono_path, json!({"render_volume": false, "render_pan": true}));
    assert_eq!(decoded(&mono_path).1.channels.len(), 1);
    e.session_mut().track_mut(vca).unwrap().mixer.mute = true;
    let muted = temp.0.join("vca-muted.wav");
    export(&mut e, t, &muted, json!({"render_volume": false}));
    assert!(decoded(&muted).1.channels[0].iter().all(|x| x.abs() < 1e-8));
}

#[test]
fn selection_tail_formats_and_session_history_stay_independent() {
    let temp = Temp::new();
    let mut s = Session::default();
    let t = audio(&mut s, ChannelFormat::Stereo, "Name/a", 1000);
    let second = audio(&mut s, ChannelFormat::Stereo, "Name:a", 1500);
    s.edit.selection = Range::new(100, 200);
    let mut e = Engine::new(s);
    e.execute("mix.volume", &json!({"track": t, "db": -3})).unwrap();
    e.undo();
    let doc = e.session_arc();
    let rev = e.revision;
    let dirty = e.is_dirty();
    let journal = e.journal.clone();
    let undo = e.undo_history();
    let redo = e.redo_label().map(str::to_string);
    let transport = serde_json::to_value(e.transport).unwrap();
    for (n, format, depth, ext) in [(0, "wav", "32f", "wav"), (1, "aiff", "24", "aif"), (2, "flac", "16", "flac")] {
        let path = temp.0.join(format!("file{n}.{ext}"));
        let v = export(&mut e, t, &path, json!({"format":format,"bit_depth":depth,"tail_seconds": 0.001}));
        let (info, a) = decoded(&path);
        assert_eq!(a.frames(), 148);
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(v["files"][0]["start"], 100);
        assert_eq!(v["files"][0]["end"], 248);
    }
    let p = json!({"tracks":[t,second,t],"dir":temp.0,"bit_depth":24,"render_pan":false,"expected_revision": rev});
    let preview = e.execute("track.bounce_preview", &p).unwrap();
    assert_eq!(preview["files"].as_array().unwrap().len(), 2);
    assert_ne!(preview["files"][0]["path"], preview["files"][1]["path"]);
    assert!(!Path::new(preview["files"][0]["path"].as_str().unwrap()).exists());
    e.execute("track.bounce", &p).unwrap();
    assert!(Arc::ptr_eq(&doc, &e.session_arc()));
    assert_eq!(e.revision, rev);
    assert_eq!(e.is_dirty(), dirty);
    assert_eq!(e.journal, journal);
    assert_eq!(e.undo_history(), undo);
    assert_eq!(e.redo_label(), redo.as_deref());
    assert_eq!(serde_json::to_value(e.transport).unwrap(), transport);
    let failed = e.execute("track.bounce", &p).unwrap_err();
    assert!(failed.to_string().contains("exists"));
    assert!(Arc::ptr_eq(&doc, &e.session_arc()));
    e.session_mut().edit.selection = Range::point(0);
    let defaults = e.execute("track.bounce_preview", &json!({"tracks":[t,second],"dir":temp.0.join("missing"),"render_pan":false})).unwrap();
    assert_eq!(defaults["ready"], false);
    let defaults = e.execute("track.bounce_preview", &json!({"tracks":[t,second],"dir":temp.0,"format":"aiff","render_pan":false})).unwrap();
    assert_eq!(defaults["files"][0]["frames"], 1000);
    assert_eq!(defaults["files"][1]["frames"], 1500);
}

#[test]
fn unsupported_and_hostile_plans_create_no_files_and_do_not_mutate() {
    let temp = Temp::new();
    let mut s = Session::default();
    let t = audio(&mut s, ChannelFormat::Mono, "Track", 128);
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Mono, None);
    let mut e = Engine::new(s);
    let path = temp.0.join("out.wav");
    let base = json!({"tracks":[t],"path":path,"render_pan":false});
    for more in [
        json!({"start": 20,"end":10}),
        json!({"start": 0}),
        json!({"tail_seconds":-1}),
        json!({"start":0,"end":1i64<<40}),
        json!({"bit_depth":"bogus"}),
        json!({"render_pan":"no"}),
        json!({"format":"mp3"}),
        json!({"format":"flac","bit_depth":"32f"}),
        json!({"expected_revision":e.revision+1}),
        json!({"sample_rate":44100}),
        json!({"tracks":[t,aux],"path":null}),
    ] {
        let mut p = base.clone();
        for (k, v) in more.as_object().unwrap() {
            p[k] = v.clone();
        }
        let before = e.session_arc();
        assert!(e.execute("track.bounce", &p).is_err(), "{p}");
        assert!(Arc::ptr_eq(&before, &e.session_arc()));
        assert!(!path.exists());
    }
    let p = json!({"tracks":[t,aux],"dir":temp.0,"render_pan":false});
    let preview = e.execute("track.bounce_preview", &p).unwrap();
    assert_eq!(preview["ready"], false);
    assert_eq!(preview["issues"].as_array().unwrap().len(), 1);
    assert!(e.execute("track.bounce", &p).is_err());
    assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 0);
    let tr = e.session_mut().track_mut(t).unwrap();
    tr.mixer.inserts[0] = Some(Insert::new("clap:missing"));
    assert!(e.execute("track.bounce", &base).is_err());
    e.session_mut().track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::new("pitch_shifter"));
    assert!(e.execute("track.bounce", &base).is_err());
    e.session_mut().track_mut(t).unwrap().mixer.inserts[0] = None;
    e.session_mut().outputs = vec![soundcraft_model::OutputPath { name: "Wide".into(), first_channel: 0, format: ChannelFormat::Ambisonics(7) }];
    let mut pan = base.clone();
    pan["render_pan"] = json!(true);
    assert!(e.execute("track.bounce", &pan).is_err());
    e.session_mut().track_mut(t).unwrap().format = ChannelFormat::Ambisonics(7);
    assert!(e.execute("track.bounce", &base).is_err());
    assert!(!path.exists());
}

#[test]
fn builtin_instrument_note_end_tail_and_rejecting_unrendered_inputs() {
    let temp = Temp::new();
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    s.track_mut(t).unwrap().instrument = Some(Insert::new("subtractive_synth"));
    let mut sequence = soundcraft_midi::Sequence::default();
    sequence.notes.push(soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 480 });
    let cid = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "Note", 0, 48000, sequence));
    let mut e = Engine::new(s);
    let path = temp.0.join("instrument.wav");
    let out = export(&mut e, t, &path, json!({"tail_seconds":0.1}));
    assert_eq!(out["files"][0]["frames"], 16800);
    let (_, a) = decoded(&path);
    assert!(a.channels[0].iter().any(|x| x.abs() > 0.001));
    let next = temp.0.join("next.wav");
    e.session_mut().track_mut(t).unwrap().format = ChannelFormat::Mono;
    let p = json!({"tracks":[t],"path":next,"render_pan":false});
    assert!(e.execute("track.bounce", &p).is_err());
    e.session_mut().track_mut(t).unwrap().format = ChannelFormat::Stereo;
    if let ClipContent::Midi { sequence } = &mut e.session_mut().track_mut(t).unwrap().playlist_mut().unwrap().clips[0].content {
        sequence.notes = vec![sequence.notes[0]; 129];
    }
    assert!(e.execute("track.bounce", &p).is_err());
    assert!(!next.exists());
}

fn instrument(s: &mut Session, clip_length: i64, note_length: i64) -> TrackId {
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Synthetic synth"));
    let mut ins = Insert::new("subtractive_synth");
    ins.params.insert("amp_release".into(), 1.0);
    s.track_mut(t).unwrap().instrument = Some(ins);
    let mut sequence = soundcraft_midi::Sequence::default();
    sequence.notes.push(soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: note_length });
    let cid = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "Original note", 0, clip_length, sequence));
    t
}

#[test]
fn instrument_preroll_matches_continuous_audio_and_exact_clip_block_end_releases() {
    let temp = Temp::new();
    let mut s = Session::default();
    // At 120 BPM a 1024-tick note ends at sample 25600, exactly 25 render blocks.
    let t = instrument(&mut s, 25600, 1024);
    let mut e = Engine::new(s);
    let full = temp.0.join("continuous.wav");
    export(&mut e, t, &full, json!({"tail_seconds": 0.01}));
    let whole = decoded(&full).1;
    let cut = temp.0.join("selection.wav");
    let out = export(&mut e, t, &cut, json!({"start": 5000, "end": 25600, "tail_seconds": 0.01}));
    let selected = decoded(&cut).1;
    assert_eq!(out["files"][0]["preroll_frames"], 5000);
    assert_eq!(out["files"][0]["processing_frames"], 26080);
    for (a, b) in selected.channels.iter().zip(&whole.channels) {
        assert_eq!(a, &b[5000..]);
    }
    assert!(selected.channels[0][..1000].iter().any(|x| x.abs() > 0.001), "a note spanning selection start must sound");
    assert!(whole.channels[0][25800..].iter().all(|x| x.abs() < 1e-4), "clip/block-end note-off must enter release during tail");
}

#[test]
fn selection_end_stops_source_input_and_new_notes_while_tail_rings() {
    let temp = Temp::new();
    let mut s = Session::default();
    let audio_t = audio(&mut s, ChannelFormat::Mono, "Audio", 40000);
    let synth_t = instrument(&mut s, 40000, 1400);
    let mut e = Engine::new(s);
    let a = temp.0.join("audio.wav");
    export(&mut e, audio_t, &a, json!({"start": 0, "end": 1500, "tail_seconds": 0.01}));
    let a = decoded(&a).1;
    assert!(a.channels[0][..1500].iter().all(|x| x.abs() > 0.01));
    assert!(a.channels[0][1500..].iter().all(|x| *x == 0.0));
    let b = temp.0.join("synth.wav");
    export(&mut e, synth_t, &b, json!({"start": 1000, "end": 2000, "tail_seconds": 0.02}));
    let b = decoded(&b).1;
    assert!(b.channels[0][..1000].iter().any(|x| x.abs() > 0.001));
    assert!(b.channels[0][1400..].iter().all(|x| x.abs() < 1e-4));
    if let ClipContent::Midi { sequence } = &mut e.session_mut().track_mut(synth_t).unwrap().playlist_mut().unwrap().clips[0].content {
        sequence.notes[0].start = 100; // Starts at 2500, after capture_end but during the requested tail.
    }
    let c = temp.0.join("later-note.wav");
    export(&mut e, synth_t, &c, json!({"start": 1000, "end": 2000, "tail_seconds": 0.02}));
    assert!(decoded(&c).1.channels.iter().flatten().all(|x| *x == 0.0));
}

#[test]
fn unlinked_timeline_range_and_codec_metadata_are_preflighted() {
    let temp = Temp::new();
    let mut s = Session::default();
    let t = audio(&mut s, ChannelFormat::Mono, "Timeline", 4000);
    s.edit.link_timeline_edit = false;
    s.edit.selection = Range::new(0, 100);
    s.edit.timeline_selection = Range::new(500, 1500);
    let mut e = Engine::new(s);
    let path = temp.0.join("timeline.wav");
    let v = export(&mut e, t, &path, json!({}));
    assert_eq!(v["files"][0]["start"], 500);
    assert_eq!(decoded(&path).1.frames(), 1000);
    e.session_mut().track_mut(t).unwrap().format = ChannelFormat::Atmos712;
    let p = json!({"tracks":[t],"path":temp.0.join("too-wide.flac"),"render_pan":false,"format":"flac"});
    assert_eq!(e.execute("track.bounce_preview", &p).unwrap()["ready"], false);
    assert!(e.execute("track.bounce", &p).is_err());
    assert!(!temp.0.join("too-wide.flac").exists());
}

#[test]
fn malformed_source_metadata_and_aggregate_work_fail_before_file_creation() {
    let temp = Temp::new();
    let mut s = Session::default();
    let t = audio(&mut s, ChannelFormat::Mono, "Invalid", 128);
    let mut e = Engine::new(s);
    let p = json!({"tracks":[t],"path":temp.0.join("invalid.wav"),"render_pan":false});
    if let ClipContent::Audio { offset, .. } = &mut e.session_mut().track_mut(t).unwrap().playlist_mut().unwrap().clips[0].content {
        *offset = 1;
    }
    assert!(e.execute("track.bounce", &p).is_err());
    if let ClipContent::Audio { offset, .. } = &mut e.session_mut().track_mut(t).unwrap().playlist_mut().unwrap().clips[0].content {
        *offset = 0;
    }
    e.session_mut().track_mut(t).unwrap().mixer.surround = Some(soundcraft_model::SurroundPan { x: f32::NAN, ..Default::default() });
    assert!(e.execute("track.bounce", &p).is_err());
    e.session_mut().track_mut(t).unwrap().mixer.surround = None;
    e.session_mut().track_mut(t).unwrap().lane_mut(&AutoParam::Volume).points.push(soundcraft_model::AutomationPoint { at: 0, value: f32::NAN });
    assert!(e.execute("track.bounce", &p).is_err());
    e.session_mut().track_mut(t).unwrap().automation.clear();
    let second = audio(e.session_mut(), ChannelFormat::Stereo, "Second", 128);
    let batch = json!({"tracks":[t,second],"dir":temp.0,"start":0,"end":20_000_000,"render_pan":false});
    let preview = e.execute("track.bounce_preview", &batch).unwrap();
    assert_eq!(preview["ready"], false);
    assert!(preview["issues"][0]["reason"].as_str().unwrap().contains("aggregate"));
    assert!(e.execute("track.bounce", &batch).is_err());
    assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 0);
}

#[test]
fn midi_track_offsets_keep_literal_source_ends_pcm_and_selection_preroll() {
    let temp = Temp::new();
    for (offset, note_start, onset, source_end) in [(600i64, 20i64, 1100usize, 4100i64), (-600, 40, 400, 3400)] {
        let mut s = Session::default();
        let t = instrument(&mut s, 40_000, 120);
        if let ClipContent::Midi { sequence } = &mut s.track_mut(t).unwrap().playlist_mut().unwrap().clips[0].content {
            sequence.notes[0].start = note_start;
        }
        let mut e = Engine::new(s);
        e.execute("event.midi_track_offsets", &json!({"track": t, "offset": offset})).unwrap();
        let document = e.session_arc();
        let path = temp.0.join(format!("offset-{offset}.wav"));
        let out = export(&mut e, t, &path, json!({"render_pan": true, "tail_seconds": 0.01}));
        let pcm = decoded(&path).1;
        assert_eq!(out["files"][0]["capture_end"], source_end);
        assert_eq!(out["files"][0]["frames"], source_end + 480);
        let warmup = (-offset).max(0);
        assert_eq!(out["files"][0]["processing_frames"], source_end + 480 + warmup);
        assert_eq!(out["files"][0]["preroll_frames"], warmup);
        assert!(pcm.channels.iter().all(|c| c[..onset].iter().all(|x| *x == 0.0)));
        assert!(pcm.channels[0][onset..onset + 500].iter().any(|x| x.abs() > 0.001));
        assert!(pcm.channels[0][source_end as usize + 200..].iter().all(|x| x.abs() < 1e-4));
        let playback = soundcraft_mix::render_range(e.session(), Range::new(offset.min(0), source_end + 480), 1024);
        for (exported, played) in pcm.channels.iter().zip(&playback) {
            assert_eq!(exported, &played[warmup as usize..], "the exported offset must match current-main playback");
        }
        let selected_path = temp.0.join(format!("offset-{offset}-selected.wav"));
        let selected = export(&mut e, t, &selected_path, json!({"start": 1500, "end": 3200, "render_pan": true}));
        let selected_pcm = decoded(&selected_path).1;
        assert_eq!(selected["files"][0]["preroll_frames"], 1500 + warmup);
        assert_eq!(selected["files"][0]["processing_frames"], 3200 + warmup);
        for (cut, whole) in selected_pcm.channels.iter().zip(&pcm.channels) {
            assert_eq!(cut, &whole[1500..3200]);
        }
        assert!(Arc::ptr_eq(&document, &e.session_arc()));
    }
}

#[test]
fn negative_midi_offset_releases_at_the_shifted_exact_clip_block_end() {
    let temp = Temp::new();
    let mut s = Session::default();
    let t = instrument(&mut s, 2048, 100);
    if let ClipContent::Midi { sequence } = &mut s.track_mut(t).unwrap().playlist_mut().unwrap().clips[0].content {
        sequence.notes[0].start = 40;
    }
    let mut e = Engine::new(s);
    e.execute("event.midi_track_offsets", &json!({"track": t, "offset": -512})).unwrap();
    let path = temp.0.join("shifted-boundary.wav");
    let out = export(&mut e, t, &path, json!({"start": 0, "end": 2000, "tail_seconds": 0.01}));
    let pcm = decoded(&path).1;
    assert_eq!(out["files"][0]["capture_end"], 2000);
    assert_eq!(out["files"][0]["frames"], 2480);
    assert!(pcm.channels[0][..488].iter().all(|x| *x == 0.0));
    assert!(pcm.channels[0][488..1536].iter().any(|x| x.abs() > 0.001));
    assert!(pcm.channels[0][1736..].iter().all(|x| x.abs() < 1e-4), "the clip ends at playback sample 1536, before capture_end");
    let default_path = temp.0.join("shifted-default.wav");
    let preview = e.execute("track.bounce_preview", &json!({"tracks": [t], "path": default_path, "render_pan": false})).unwrap();
    assert_eq!(preview["files"][0]["capture_end"], 1536);
    assert_eq!(preview["files"][0]["frames"], 1536);
    assert!(!default_path.exists());
}

#[test]
fn negative_midi_offset_warms_a_note_shifted_before_session_zero() {
    let temp = Temp::new();
    for (offset, onset, source_end, warmup) in [(-96i64, 0usize, 18i64, 96i64), (96, 146, 210, 0)] {
        let mut s = Session::default();
        let t = instrument(&mut s, 64, 10);
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips[0].start = 50; // Exactly tick 2 at 120 BPM/48 kHz.
        let mut e = Engine::new(s);
        e.execute("event.midi_track_offsets", &json!({"track": t, "offset": offset})).unwrap();
        let path = temp.0.join(format!("zero-crossing-{offset}.wav"));
        let out = export(&mut e, t, &path, json!({"render_pan": true, "tail_seconds": 0.01}));
        let pcm = decoded(&path).1;
        assert_eq!(out["files"][0]["capture_end"], source_end);
        assert_eq!(out["files"][0]["frames"], source_end + 480);
        assert_eq!(out["files"][0]["processing_frames"], source_end + 480 + warmup);
        assert_eq!(out["files"][0]["preroll_frames"], warmup);
        assert!(pcm.channels.iter().all(|c| c[..onset].iter().all(|x| *x == 0.0)));
        assert!(pcm.channels[0][onset..source_end as usize].iter().take(32).any(|x| x.abs() > 0.001));
        assert!(pcm.channels[0][source_end as usize + 200..].iter().all(|x| x.abs() < 1e-4));
        let playback = soundcraft_mix::render_range(e.session(), Range::new(offset.min(0), source_end + 480), 1024);
        for (exported, played) in pcm.channels.iter().zip(&playback) {
            assert_eq!(exported, &played[warmup as usize..], "tick-aligned literal offset edges must match current-main PCM");
        }
        let cutoff = if offset < 0 { 16 } else { 128 };
        let cutoff_path = temp.0.join(format!("offset-cutoff-{offset}.wav"));
        export(&mut e, t, &cutoff_path, json!({"start": 0, "end": cutoff, "tail_seconds": 0.01}));
        let cutoff_pcm = decoded(&cutoff_path).1;
        assert!(cutoff_pcm.channels[0][cutoff + 200..].iter().all(|x| x.abs() < 1e-4));
        if offset > 0 {
            assert!(cutoff_pcm.channels.iter().flatten().all(|x| *x == 0.0), "a shifted future note must not start during the tail");
        }
    }
}

#[test]
fn invalid_midi_offset_metadata_is_rejected_before_file_creation() {
    let temp = Temp::new();
    let mut s = Session::default();
    let t = instrument(&mut s, 40_000, 120);
    let mut e = Engine::new(s);
    for offset in [f64::NAN, f64::INFINITY, -480_001.0, 480_001.0] {
        e.session_mut().edit.values.insert(format!("midi.offset.{}", t.0), offset);
        let document = e.session_arc();
        let p = json!({"tracks": [t], "path": temp.0.join("invalid-offset.wav"), "start": 0, "end": 1000, "render_pan": false});
        let preview = e.execute("track.bounce_preview", &p).unwrap();
        assert_eq!(preview["ready"], false);
        assert!(preview["issues"][0]["reason"].as_str().unwrap().contains("MIDI track offset"));
        assert!(e.execute("track.bounce", &p).is_err());
        assert!(Arc::ptr_eq(&document, &e.session_arc()));
        assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 0);
    }
}

#[test]
fn contained_nonfinite_insert_output_cannot_be_delivered_as_silent_success() {
    let temp = Temp::new();
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Nonfinite source"));
    let source = io::add_source(&mut s, "nonfinite", AudioBuffer { sample_rate: 48_000, channels: vec![vec![f32::NAN; 128]] }, None, FileFormat::Wav);
    let clip = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "nonfinite", source, 0, 0, 128));
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::new("gain"));
    let mut e = Engine::new(s);
    let document = e.session_arc();
    let revision = e.revision;
    let journal = e.journal.clone();
    let undo = e.undo_history();
    let p = json!({"tracks": [t], "path": temp.0.join("nonfinite.wav"), "render_pan": false});
    assert_eq!(e.execute("track.bounce_preview", &p).unwrap()["ready"], true, "the preflight does not scan PCM");
    assert!(e.execute("track.bounce", &p).unwrap_err().to_string().contains("nonfinite plugin audio"));
    assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 0);
    assert!(Arc::ptr_eq(&document, &e.session_arc()));
    assert_eq!(e.revision, revision);
    assert_eq!(e.journal, journal);
    assert_eq!(e.undo_history(), undo);
}

#[test]
fn audio_manifest_reports_zero_preroll_for_a_positive_selection_start() {
    let temp = Temp::new();
    let mut s = Session::default();
    let t = audio(&mut s, ChannelFormat::Mono, "Selected", 128);
    let mut e = Engine::new(s);
    let document = e.session_arc();
    let path = temp.0.join("selected.wav");
    let preview = e.execute("track.bounce_preview", &json!({"tracks": [t], "path": path, "start": 17, "end": 117, "render_pan": false})).unwrap();
    assert_eq!(preview["ready"], true);
    assert_eq!(preview["files"][0]["frames"], 100);
    assert_eq!(preview["files"][0]["processing_frames"], 100);
    assert_eq!(preview["files"][0]["preroll_frames"], 0);
    assert!(Arc::ptr_eq(&document, &e.session_arc()));
    assert!(!path.exists());
}

#[test]
fn automation_write_root_confines_preview_export_and_preserves_existing_destinations() {
    use soundcraft_engine::file_access::{AutomationRoots, confine};
    let temp = Temp::new();
    let inside = temp.0.join("inside");
    let outside = temp.0.join("outside");
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let existing = outside.join("sentinel.wav");
    std::fs::write(&existing, b"external destination").unwrap();
    std::os::unix::fs::symlink(&outside, inside.join("escape")).unwrap();
    let roots = AutomationRoots::new(None, Some(&inside)).unwrap().unwrap();
    let mut s = Session::default();
    let track = audio(&mut s, ChannelFormat::Mono, "Confined source", 128);
    let second = audio(&mut s, ChannelFormat::Mono, "Second source", 128);
    let mut e = Engine::new(s);
    let document = e.session_arc();
    let revision = e.revision;
    let journal = e.journal.clone();
    let undo = e.undo_history();
    confine(Some(&roots), || {
        for path in [outside.join("new.wav"), existing.clone(), inside.join("escape/new.wav")] {
            let params = json!({"tracks":[track],"path":path,"render_pan":false});
            let preview = e.execute("track.bounce_preview", &params).unwrap();
            assert_eq!(preview["ready"], false);
            let reason = preview["issues"][0]["reason"].as_str().unwrap();
            assert!(reason.contains("write root"), "{reason}");
            assert!(!reason.contains("destination already exists"), "reject before probing outside the root");
            assert!(e.execute("track.bounce", &params).unwrap_err().to_string().contains("write root"));
        }
        let batch = json!({"tracks":[track,second],"dir":outside,"render_pan":false});
        let preview = e.execute("track.bounce_preview", &batch).unwrap();
        assert_eq!(preview["ready"], false);
        assert_eq!(preview["issues"].as_array().unwrap().len(), 2);
        assert!(e.execute("track.bounce", &batch).unwrap_err().to_string().contains("write root"));
        let path = inside.join("permitted.wav");
        let params = json!({"tracks":[track],"path":path,"render_pan":false});
        assert_eq!(e.execute("track.bounce_preview", &params).unwrap()["ready"], true);
        e.execute("track.bounce", &params).unwrap();
        let delivered = std::fs::read(&path).unwrap();
        assert_eq!(decoded(&path).1.frames(), 128);
        assert_eq!(e.execute("track.bounce_preview", &params).unwrap()["ready"], false);
        assert!(e.execute("track.bounce", &params).unwrap_err().to_string().contains("destination already exists"));
        assert_eq!(std::fs::read(&path).unwrap(), delivered);
    });
    assert_eq!(std::fs::read(&existing).unwrap(), b"external destination");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
    assert_eq!(std::fs::read_dir(&inside).unwrap().count(), 2, "only permitted output and escape fixture remain; no staging residual");
    assert!(Arc::ptr_eq(&document, &e.session_arc()));
    assert_eq!(e.revision, revision);
    assert_eq!(e.journal, journal);
    assert_eq!(e.undo_history(), undo);
    assert!(!e.is_dirty());
}

#[test]
fn nonfinite_midi_realtime_property_metadata_is_rejected_before_file_creation() {
    let temp = Temp::new();
    let mut s = Session::default();
    let track = instrument(&mut s, 40_000, 120);
    let mut e = Engine::new(s);
    for property in ["velocity", "transpose", "duration", "delay"] {
        let key = format!("rtp.{}.{property}", track.0);
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            e.session_mut().edit.values.insert(key.clone(), value);
            let document = e.session_arc();
            let revision = e.revision;
            let journal = e.journal.clone();
            let dirty = e.is_dirty();
            let undo = e.undo_history();
            let params = json!({"tracks":[track],"path":temp.0.join("invalid-property.wav"),"start":0,"end":1000,"render_pan":false});
            let preview = e.execute("track.bounce_preview", &params).unwrap();
            assert_eq!(preview["ready"], false);
            assert!(preview["issues"][0]["reason"].as_str().unwrap().contains("nonfinite MIDI real-time property"));
            assert!(e.execute("track.bounce", &params).unwrap_err().to_string().contains("nonfinite MIDI real-time property"));
            assert!(Arc::ptr_eq(&document, &e.session_arc()));
            assert_eq!(e.revision, revision);
            assert_eq!(e.journal, journal);
            assert_eq!(e.is_dirty(), dirty);
            assert_eq!(e.undo_history(), undo);
            assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 0);
        }
        e.session_mut().edit.values.remove(&key);
    }
}
