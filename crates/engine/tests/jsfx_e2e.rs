//! A JSFX effect through the whole stack: `mix.insert` / `mix.insert_param` accept a `jsfx:` id and
//! the mix engine renders through it. The effect is a text file written to a temporary Effects folder.

use serde_json::json;
use soundcraft_audio_io::AudioBuffer;
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};
use soundcraft_time::Range;
use std::path::Path;
use std::sync::Arc;

const ID: &str = "jsfx:test/gain";
const GAIN: &str = "desc:Test Gain\nslider1:1<0,4,0.01>Gain\n@sample\nspl0*=slider1;\nspl1*=slider1;\n";

fn install_effects() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("engine-jsfx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("test")).unwrap();
        std::fs::write(dir.join("test/gain"), GAIN).unwrap();
        assert_eq!(soundcraft_jsfx_host::add_search_dir(&dir), 1);
    });
}

fn engine_with_dc_track() -> (Engine, soundcraft_model::TrackId) {
    let mut e = Engine::default();
    let s = e.session_mut();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    s.pool.insert(SourceId(79), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 4800]] })));
    let id = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "dc", SourceId(79), 0, 0, 4800));
    (e, t)
}

fn sample(e: &Engine) -> f32 {
    soundcraft_mix::render_range(e.session(), Range::new(0, 4800), 512)[0][2000]
}

#[test]
fn jsfx_is_listed_and_inserts_by_id() {
    install_effects();
    let mut e = Engine::default();
    let list = e.execute("engine.jsfx_plugins", &json!({})).unwrap();
    let entry = list.as_array().unwrap().iter().find(|d| d["id"] == ID).expect("the test effect is listed");
    assert_eq!((entry["name"].as_str(), entry["vendor"].as_str()), (Some("Test Gain"), Some("test")));
    assert!(entry["unsupported"].is_null(), "it compiles, so no reason is given");
}

#[test]
fn jsfx_insert_renders_through_the_mix() {
    install_effects();
    let (mut e, t) = engine_with_dc_track();
    let dry = sample(&e);
    assert!(dry.abs() > 0.01);

    let r = e.execute("mix.insert", &json!({"track": t.0, "plugin": ID, "params": {"slider1": 2.0}})).unwrap();
    assert_eq!(r["plugin"], ID);
    let wet = sample(&e);
    assert!((wet - 2.0 * dry).abs() < 1e-4, "dry {dry} wet {wet}");

    e.execute("mix.insert_param", &json!({"track": t.0, "slot": 0, "param": "slider1", "value": 0.5})).unwrap();
    let half = sample(&e);
    assert!((half - 0.5 * dry).abs() < 1e-4, "dry {dry} half {half}");
    assert!(e.execute("mix.insert_param", &json!({"track": t.0, "slot": 0, "param": "Gain", "value": 1.0})).is_err());
}

#[test]
fn jsfx_survives_a_session_round_trip() {
    install_effects();
    let (mut e, t) = engine_with_dc_track();
    e.execute("mix.insert", &json!({"track": t.0, "plugin": ID, "params": {"slider1": 3.0}})).unwrap();
    let before = sample(&e);
    let json = serde_json::to_string(e.session()).unwrap();
    let mut back: soundcraft_model::Session = serde_json::from_str(&json).unwrap();
    // The audio pool is not part of the session text (sources are reloaded from disk).
    back.pool = e.session().pool.clone();
    let ins = back.track(t).unwrap().mixer.inserts[0].as_ref().unwrap();
    assert_eq!(ins.plugin, ID);
    let after = soundcraft_mix::render_range(&back, Range::new(0, 4800), 512)[0][2000];
    assert!((before - after).abs() < 1e-6, "{before} vs {after}");
}
