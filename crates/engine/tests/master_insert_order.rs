//! Explicit Master insert-order compatibility, command transactions and native persistence.

use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::{Engine, io};
use soundcraft_model::{ChannelFormat, Clip, Insert, MasterInsertOrder, Mixer, Session, TrackId, TrackKind};
use soundcraft_time::Range;
use std::sync::Arc;

const LEN: usize = 128;
const COMMAND: &str = "mix.master_insert_order";

fn limiter() -> Insert {
    let mut insert = Insert::new("compressor");
    insert.params.extend([
        ("threshold".into(), -6.0),
        ("attack".into(), 0.01),
        ("release".into(), 5.0),
        ("knee".into(), 0.0),
        ("makeup".into(), 0.0),
        ("limiter".into(), 1.0),
        ("lookahead".into(), 0.0),
        ("sc_hpf_on".into(), 0.0),
        ("mix".into(), 100.0),
    ]);
    insert
}

fn fixture() -> (Engine, TrackId, [TrackId; 2]) {
    let mut session = Session::default();
    let audio = session.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Original"));
    let source = io::add_source(
        &mut session,
        "Binary-fraction synthetic",
        AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.75; LEN], vec![-0.375; LEN]] },
        None,
        FileFormat::Wav,
    );
    let clip = session.new_clip_id();
    session.track_mut(audio).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "Original", source, 0, 0, LEN as i64));
    let first = session.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("First master"));
    let second = session.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Second master"));
    let master = session.track_mut(first).unwrap();
    master.mixer.volume_db = -6.0;
    master.mixer.inserts[0] = Some(limiter());
    session.track_mut(second).unwrap().inactive = true;
    session.edit.selected_tracks = vec![first, second];
    (Engine::new(session), audio, [first, second])
}

fn render(engine: &Engine) -> Vec<Vec<f32>> {
    let output = soundcraft_mix::render_range(engine.session(), Range::new(0, LEN as i64), 37);
    assert_eq!(output.len(), 2);
    assert!(output.iter().all(|channel| channel.len() == LEN));
    assert!(output.iter().flatten().all(|sample| sample.is_finite()));
    assert!(output.iter().flatten().any(|sample| sample.abs() > 0.05));
    output
}

// Independent constant-signal log-domain limiter reference, with a linked stereo peak,
// threshold -6 dB, ratio 1000:1 and 0.01 ms attack at 48 kHz. No rendering/DSP helper
// determines either reference. The source and fader gain have declared numeric values.
fn reference(post_fader: bool) -> Vec<Vec<f32>> {
    let gain = 10f64.powf(-6.0 / 20.0);
    let input_gain = if post_fader { gain } else { 1.0 };
    let output_gain = if post_fader { 1.0 } else { gain };
    let level = 0.75 * input_gain;
    let target = (20.0 * level.log10() + 6.0).max(0.0) * (1.0 - 1.0 / 1000.0);
    let coefficient = (-1.0f64 / (0.01 * 0.001 * 48_000.0)).exp();
    let mut reduction = 0.0;
    let mut left = vec![0.0; LEN];
    let mut right = vec![0.0; LEN];
    for (left, right) in left.iter_mut().zip(&mut right) {
        reduction = target + (reduction - target) * coefficient;
        let attenuation = 10f64.powf(-reduction / 20.0);
        *left = (0.75 * input_gain * attenuation * output_gain) as f32;
        *right = (-0.375 * input_gain * attenuation * output_gain) as f32;
    }
    vec![left, right]
}

fn check_pcm(actual: &[Vec<f32>], expected: &[Vec<f32>]) {
    assert_eq!(actual.len(), 2);
    assert_eq!(expected.len(), 2);
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(actual.len(), LEN);
        assert_eq!(expected.len(), LEN);
        for (frame, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            assert!(actual.is_finite() && expected.is_finite());
            assert!((actual - expected).abs() < 3e-6, "frame {frame}: {actual} != {expected}");
        }
    }
}

fn with_redo() -> (Engine, TrackId, [TrackId; 2]) {
    let (mut engine, audio, masters) = fixture();
    engine.execute("mix.volume", &json!({"track": masters[0], "db": -3})).unwrap();
    assert!(engine.undo());
    engine.mark_clean();
    assert!(engine.can_redo());
    (engine, audio, masters)
}

fn assert_refused(engine: &mut Engine, params: Value) {
    let before = engine.session_arc();
    let json = engine.session().to_json().unwrap();
    let state = (engine.revision, engine.is_dirty(), engine.undo_history(), engine.redo_label().map(str::to_owned));
    let journal = engine.journal.clone();
    let clipboard = engine.clipboard.clone();
    let path = engine.path.clone();
    let pcm = render(engine);
    assert!(engine.can_redo());
    assert!(engine.execute(COMMAND, &params).is_err(), "unexpected success: {params}");
    assert!(Arc::ptr_eq(&before, &engine.session_arc()));
    assert_eq!(engine.session().to_json().unwrap(), json);
    assert_eq!((engine.revision, engine.is_dirty(), engine.undo_history(), engine.redo_label().map(str::to_owned)), state);
    assert!(engine.can_redo());
    assert_eq!(engine.journal, journal);
    assert_eq!(engine.clipboard, clipboard);
    assert_eq!(engine.path, path);
    assert_eq!(render(engine), pcm);
}

#[test]
fn master_order_defaults_omit_legacy_field_and_round_trip_explicit_values() {
    assert_eq!(MasterInsertOrder::default(), MasterInsertOrder::PreFader);
    for format in [ChannelFormat::Mono, ChannelFormat::Stereo, ChannelFormat::Surround51] {
        let mut mixer = Mixer::new(format);
        assert_eq!(mixer.master_insert_order, MasterInsertOrder::PreFader);
        let omitted = serde_json::to_value(&mixer).unwrap();
        assert!(omitted.get("master_insert_order").is_none());
        let legacy: Mixer = serde_json::from_value(omitted.clone()).unwrap();
        assert_eq!(legacy, mixer);
        let mut explicit_pre = omitted;
        explicit_pre["master_insert_order"] = json!("pre_fader");
        assert_eq!(serde_json::from_value::<Mixer>(explicit_pre).unwrap(), mixer);
        mixer.master_insert_order = MasterInsertOrder::PostFader;
        let explicit_post = serde_json::to_value(&mixer).unwrap();
        assert_eq!(explicit_post["master_insert_order"], "post_fader");
        assert_eq!(serde_json::from_value::<Mixer>(explicit_post).unwrap(), mixer);
    }
    let (engine, _, masters) = fixture();
    for id in masters {
        assert_eq!(engine.session().track(id).unwrap().mixer.master_insert_order, MasterInsertOrder::PreFader);
    }
}

#[test]
fn invalid_saved_master_order_types_fail_instead_of_silently_changing_order() {
    let (engine, _, _) = fixture();
    for invalid in [Value::Null, json!(true), json!(1), json!(1.0), json!([]), json!({}), json!("PreFader"), json!("post"), json!("")] {
        let mut mixer = serde_json::to_value(Mixer::new(ChannelFormat::Stereo)).unwrap();
        mixer["master_insert_order"] = invalid.clone();
        assert!(serde_json::from_value::<Mixer>(mixer).is_err(), "{invalid}");
        let mut session = serde_json::to_value(engine.session()).unwrap();
        session["tracks"][1]["mixer"]["master_insert_order"] = invalid.clone();
        assert!(Session::from_json(&session.to_string()).is_err(), "{invalid}");
    }
}

#[test]
fn invalid_master_order_commands_are_atomic_with_redo_pending() {
    let (mut engine, audio, masters) = with_redo();
    let first = masters[0];
    let mut invalid = vec![
        Value::Null,
        json!(true),
        json!("post_fader"),
        json!([]),
        json!({"track": first}),
        json!({"track": first, "order": "POST_FADER"}),
        json!({"track": first, "order": "post_fader", "replace": true}),
        json!({"track": first, "tracks": masters, "order": "post_fader"}),
        json!({"track": audio, "order": "post_fader"}),
        json!({"tracks": [first, audio], "order": "post_fader"}),
        json!({"tracks": [first, 999_999], "order": "post_fader"}),
        json!({"track": "Unknown master", "order": "post_fader"}),
        json!({"track": 999_999, "order": "post_fader"}),
        json!({"track": false, "order": "post_fader"}),
        json!({"track": {}, "order": "post_fader"}),
        json!({"tracks": [], "order": "post_fader"}),
        json!({"tracks": "First master", "order": "post_fader"}),
        json!({"tracks": [first, false], "order": "post_fader"}),
        json!({"tracks": vec![first; 4097], "order": "post_fader"}),
    ];
    for order in [Value::Null, json!(true), json!(0), json!(0.5), json!([]), json!({}), json!("unknown")] {
        invalid.push(json!({"track": first, "order": order}));
    }
    for params in invalid {
        assert_refused(&mut engine, params);
    }
    engine.session_mut().edit.selected_tracks.clear();
    assert_refused(&mut engine, json!({"order": "post_fader"}));
    engine.session_mut().edit.selected_tracks = vec![first, audio];
    assert_refused(&mut engine, json!({"order": "post_fader"}));
}

#[test]
fn multiple_master_orders_change_once_with_exact_undo_and_redo() {
    let (mut engine, _, masters) = fixture();
    let before = engine.session().clone();
    let history = engine.undo_history();
    let params = json!({"tracks": [masters[1], masters[0], masters[0]], "order": "post_fader"});
    let result = engine.execute(COMMAND, &params).unwrap();
    assert_eq!(result, json!({"tracks": masters, "order": "post_fader", "changed": 2}));
    let mut expected = before.clone();
    for id in masters {
        expected.track_mut(id).unwrap().mixer.master_insert_order = MasterInsertOrder::PostFader;
    }
    assert_eq!(engine.session(), &expected);
    assert_eq!(engine.undo_history().len(), history.len() + 1);
    assert!(engine.is_dirty());
    check_pcm(&render(&engine), &reference(true));
    assert!(engine.undo());
    assert_eq!(engine.session(), &before);
    check_pcm(&render(&engine), &reference(false));
    assert!(engine.redo());
    assert_eq!(engine.session(), &expected);
    check_pcm(&render(&engine), &reference(true));
}

#[test]
fn single_named_and_selected_master_targets_use_the_existing_resolver() {
    let (mut engine, _, masters) = fixture();
    assert_eq!(
        engine.execute(COMMAND, &json!({"track": masters[0], "order": "post_fader"})).unwrap(),
        json!({"tracks": [masters[0]], "order": "post_fader", "changed": 1})
    );
    assert_eq!(
        engine.execute(COMMAND, &json!({"track": "Second master", "order": "post_fader"})).unwrap(),
        json!({"tracks": [masters[1]], "order": "post_fader", "changed": 1})
    );
    engine.session_mut().edit.selected_tracks = vec![masters[1], masters[0], masters[1]];
    assert_eq!(engine.execute(COMMAND, &json!({"order": "pre_fader"})).unwrap(), json!({"tracks": masters, "order": "pre_fader", "changed": 2}));
    for id in masters {
        assert_eq!(engine.session().track(id).unwrap().mixer.master_insert_order, MasterInsertOrder::PreFader);
    }
}

#[test]
fn unchanged_master_order_preserves_redo_but_keeps_normal_request_journaling() {
    for order in ["pre_fader", "post_fader"] {
        let (mut engine, _, masters) = fixture();
        if order == "post_fader" {
            engine.execute(COMMAND, &json!({"tracks": masters, "order": order})).unwrap();
        }
        engine.execute("mix.volume", &json!({"track": masters[0], "db": -3})).unwrap();
        assert!(engine.undo());
        engine.mark_clean();
        let document = engine.session_arc();
        let state = (engine.revision, engine.is_dirty(), engine.undo_history(), engine.redo_label().map(str::to_owned));
        let pcm = render(&engine);
        let mut journal = engine.journal.clone();
        let params = json!({"tracks": masters, "order": order});
        assert_eq!(engine.execute(COMMAND, &params).unwrap(), json!({"tracks": masters, "order": order, "changed": 0}));
        journal.push((COMMAND.into(), params));
        assert!(Arc::ptr_eq(&document, &engine.session_arc()));
        assert_eq!((engine.revision, engine.is_dirty(), engine.undo_history(), engine.redo_label().map(str::to_owned)), state);
        assert_eq!(engine.journal, journal);
        assert_eq!(render(&engine), pcm);
        assert!(engine.can_redo());
        assert!(engine.redo());
        assert_eq!(engine.session().track(masters[0]).unwrap().mixer.volume_db, -3.0);
    }
}

#[test]
fn missing_saved_order_keeps_legacy_nonlinear_pcm_until_explicit_opt_in() {
    let (engine, _, masters) = fixture();
    let mut legacy_json = serde_json::to_value(engine.session()).unwrap();
    for track in legacy_json["tracks"].as_array_mut().unwrap() {
        track["mixer"].as_object_mut().unwrap().remove("master_insert_order");
    }
    let mut restored = Session::from_json(&legacy_json.to_string()).unwrap();
    restored.pool = engine.session().pool.clone();
    let mut restored = Engine::new(restored);
    let legacy_pcm = render(&restored);
    check_pcm(&legacy_pcm, &reference(false));
    restored.execute(COMMAND, &json!({"track": masters[0], "order": "post_fader"})).unwrap();
    let explicit_pcm = render(&restored);
    check_pcm(&explicit_pcm, &reference(true));
    assert!(legacy_pcm.iter().flatten().zip(explicit_pcm.iter().flatten()).any(|(legacy, explicit)| (legacy - explicit).abs() > 0.1));
}

#[test]
fn nonmaster_stored_order_does_not_change_ordinary_insert_processing() {
    let (mut engine, audio, masters) = fixture();
    let session = engine.session_mut();
    for master in masters {
        session.track_mut(master).unwrap().inactive = true;
    }
    let track = session.track_mut(audio).unwrap();
    track.mixer.volume_db = -6.0;
    track.mixer.inserts[0] = Some(limiter());
    track.mixer.master_insert_order = MasterInsertOrder::PostFader;
    let pcm = render(&engine);
    check_pcm(&pcm, &reference(false));
    engine.session_mut().track_mut(audio).unwrap().mixer.master_insert_order = MasterInsertOrder::PreFader;
    assert_eq!(render(&engine), pcm);
}

#[test]
fn native_folder_and_bundle_reopen_preserve_explicit_and_omitted_orders() {
    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let directory = std::env::temp_dir().join(format!("soundcraft-master-order-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let temp = Temp(directory);
    for (order, post_fader) in [("pre_fader", false), ("post_fader", true)] {
        let (mut engine, _, masters) = fixture();
        engine.execute(COMMAND, &json!({"tracks": masters, "order": order})).unwrap();
        let pcm = render(&engine);
        check_pcm(&pcm, &reference(post_fader));
        let path = temp.0.join(format!("{order}.scraft"));
        io::save_session(&mut engine, &path.to_string_lossy()).unwrap();
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        for id in masters {
            let track = saved["tracks"].as_array().unwrap().iter().find(|track| track["id"] == json!(id)).unwrap();
            assert_eq!(track["kind"], "Master");
            assert_eq!(track["mixer"].get("master_insert_order").cloned(), if post_fader { Some(json!("post_fader")) } else { None });
        }
        let mut reopened = Engine::default();
        assert!(io::open_session(&mut reopened, &path.to_string_lossy()).unwrap().is_empty());
        assert_eq!(render(&reopened), pcm);
        for id in masters {
            assert_eq!(
                reopened.session().track(id).unwrap().mixer.master_insert_order,
                if post_fader { MasterInsertOrder::PostFader } else { MasterInsertOrder::PreFader }
            );
        }
        let (bytes, bundled_session) = io::session_bundle(engine.session()).unwrap();
        for id in masters {
            assert_eq!(bundled_session.track(id).unwrap().mixer.master_insert_order, reopened.session().track(id).unwrap().mixer.master_insert_order);
        }
        let mut bundled = Engine::default();
        assert!(io::open_session_bytes(&mut bundled, "Order.scraftz", &bytes).unwrap().is_empty());
        assert_eq!(render(&bundled), pcm);
        check_pcm(&render(&bundled), &reference(post_fader));
        for id in masters {
            assert_eq!(
                bundled.session().track(id).unwrap().mixer.master_insert_order,
                reopened.session().track(id).unwrap().mixer.master_insert_order
            );
        }
    }
}
