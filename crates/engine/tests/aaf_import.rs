use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_engine::{Engine, file_access};
use soundcraft_model::{ChannelFormat, Session, TrackKind};
use soundcraft_time::SampleRate;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Fixture {
    dir: PathBuf,
    params: Value,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("soundcraft-aaf-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("Source á with spaces.wav");
        let mut ch = vec![0.0; 480000];
        ch[96000] = 0.5;
        let bytes = soundcraft_audio_io::encode(
            &AudioBuffer { sample_rate: 48000, channels: vec![ch] },
            &EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None },
        )
        .unwrap();
        std::fs::write(&path, bytes).unwrap();
        let params = json!({"plan":{"schema_version":1,"name":"AAF test","sample_rate":48000,"timecode_start":[3600,1],"frame_rate":{"rate":[25,1],"drop":false},"warnings":["Test warning"],
            "sources":[{"key":"source","name":"Original source","path":path,"channels":1,"frames":480000,"sample_rate":48000}],
            "tracks":[{"name":"DX","channels":1,"clips":[{"name":"Cut","source":"source","start":[1,1],"end":[3,1],"offset":[2,1]}]},
                      {"name":"Empty","channels":1,"clips":[]}]},"selected":[0],"mapping":"absolute","media":"copy","offset_samples":0});
        Self { dir, params }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn existing() -> Engine {
    let mut s = Session::new("Existing", SampleRate::HZ_48000);
    s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Keep"));
    Engine::new(s)
}
#[test]
fn absolute_mapping_preserves_source_window_and_is_one_undo_step() {
    let f = Fixture::new();
    let mut e = existing();
    let before = e.session().to_json().unwrap();
    let report = e.execute("file.import_aaf", &f.params).unwrap();
    assert_eq!(report["tracks"].as_array().unwrap().len(), 1);
    assert_eq!(e.session().tracks.len(), 2);
    let c = &e.session().tracks[1].clips()[0];
    assert_eq!((c.start, c.length, c.source_offset()), (172848000, 96000, 96000));
    assert_eq!(e.session().sources[0].frames, 480000);
    assert!(e.session().sources[0].unsaved);
    let source = c.source().unwrap();
    assert_eq!(e.session().pool.get(source).unwrap().channel(0).unwrap()[96000], 0.5);
    assert_eq!(e.undo_label(), Some("Session Data (AAF)..."));
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(e.session().to_json().unwrap(), before);
}
#[test]
fn relative_mapping_and_track_selection_do_not_replace_existing_tracks() {
    let mut f = Fixture::new();
    let mut e = existing();
    f.params["mapping"] = json!("relative");
    f.params["offset_samples"] = json!(96000);
    e.execute("file.import_aaf", &f.params).unwrap();
    assert_eq!(e.session().tracks[0].name, "Keep");
    assert_eq!(e.session().tracks[1].name, "DX");
    assert_eq!(e.session().tracks[1].clips()[0].start, 144000);
    assert!(e.session().sources[0].unsaved);
}
#[test]
fn empty_session_adopts_source_clock_and_save_collects_the_whole_source() {
    let f = Fixture::new();
    let mut e = Engine::default();
    e.execute("file.import_aaf", &f.params).unwrap();
    assert_eq!(e.session().timecode_start, 172800000);
    assert_eq!(e.session().tracks[0].clips()[0].start, 48000);
    let dest = f.dir.join("Saved/session.scraft");
    soundcraft_engine::io::save_session(&mut e, &dest.to_string_lossy()).unwrap();
    let file = f.dir.join("Saved").join(&e.session().sources[0].path);
    let info = soundcraft_audio_io::probe(&std::fs::read(file).unwrap(), Some("wav")).unwrap();
    assert_eq!(info.frames, 480000);
    assert_eq!(e.session().tracks[0].clips()[0].source_offset(), 96000);
}
#[test]
fn invalid_inputs_fail_without_mutation() {
    let f = Fixture::new();
    let mut e = existing();
    let before = e.session().to_json().unwrap();
    for update in [
        json!({"selected":[]}),
        json!({"selected":[99]}),
        json!({"selected":[0,0]}),
        json!({"mapping":"unknown"}),
        json!({"media":"link"}),
        json!({"offset_samples":-172900000}),
    ] {
        let mut p = f.params.clone();
        for (k, v) in update.as_object().unwrap() {
            p[k] = v.clone();
        }
        assert!(e.execute("file.import_aaf", &p).is_err());
        assert_eq!(e.session().to_json().unwrap(), before);
        assert!(e.undo_label().is_none());
    }
    std::fs::remove_file(f.dir.join("Source á with spaces.wav")).unwrap();
    assert!(e.execute("file.import_aaf", &f.params).is_err());
    assert_eq!(e.session().to_json().unwrap(), before);
}
#[test]
fn sample_rate_changes_and_confined_media_are_rejected() {
    let f = Fixture::new();
    let mut s = Session::new("44.1", SampleRate::HZ_44100);
    s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Existing"));
    let mut e = Engine::new(s);
    assert!(e.execute("file.import_aaf", &f.params).unwrap_err().to_string().contains("sample rates differ"));
    let restricted = f.dir.join("allowed");
    std::fs::create_dir(&restricted).unwrap();
    let roots = file_access::AutomationRoots::new(Some(&restricted), None).unwrap();
    let mut e = existing();
    assert!(file_access::confine(roots.as_ref(), || e.execute("file.import_aaf", &f.params)).is_err());
    assert_eq!(e.session().tracks.len(), 1);
}
#[test]
fn playing_import_cannot_bypass_the_transport_guard_with_target_params() {
    let f = Fixture::new();
    let mut e = existing();
    e.transport.playing = true;
    let mut p = f.params.clone();
    p["tracks"] = json!([1]);
    assert!(e.execute("file.import_aaf", &p).is_err());
    assert_eq!(e.session().tracks.len(), 1);
}

/// A synthetic AAF (two tracks, three edits, chained source offsets) written by pyaaf2. Its
/// external WAVs are regenerated here with the same names, layouts and lengths.
struct AafFolder {
    dir: PathBuf,
}
impl AafFolder {
    fn new() -> Self {
        let dir = Fixture::new().dir.with_extension("aaf");
        std::fs::create_dir(&dir).unwrap();
        std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/synthetic-two-tracks.aaf"), dir.join("Montaje sintético.aaf")).unwrap();
        for (name, channels) in [("Diálogo á #1.wav", 1), ("Ambiente estéreo.wav", 2)] {
            let audio = AudioBuffer { sample_rate: 48000, channels: vec![vec![0.25; 384000]; channels] };
            let options = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None };
            std::fs::write(dir.join(name), soundcraft_audio_io::encode(&audio, &options).unwrap()).unwrap();
        }
        Self { dir }
    }
    fn preview(&self) -> soundcraft_engine::Result<Value> {
        let aaf = self.dir.join("Montaje sintético.aaf");
        Engine::default().execute("file.preview_aaf", &json!({"path": aaf, "media_dir": self.dir}))
    }
}
impl Drop for AafFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn rust_reader_turns_an_aaf_file_into_editable_tracks() {
    let f = AafFolder::new();
    let preview = f.preview().unwrap();
    let plan = &preview["plan"];
    assert_eq!(plan["name"], "Clase AAF — cortes y handles");
    assert_eq!(plan["timecode_start"], json!([3600, 1]));
    assert_eq!(plan["frame_rate"], json!({"rate": [25, 1], "drop": false}));
    let names: Vec<_> = plan["tracks"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["01 Diálogo", "02 Ambiente estéreo"]);
    assert_eq!(plan["tracks"][0]["clips"][1]["offset"], json!([7, 2]));
    let channels: Vec<_> = plan["sources"].as_array().unwrap().iter().map(|s| s["channels"].as_u64().unwrap()).collect();
    assert_eq!(channels, [1, 2]);
    assert!(plan["sources"][0]["path"].as_str().unwrap().ends_with("Diálogo á #1.wav"));

    let mut e = Engine::default();
    e.execute("file.import_aaf", &json!({"plan": plan, "selected": [0, 1]})).unwrap();
    let edits: Vec<_> = e.session().tracks.iter().flat_map(|t| t.clips()).map(|c| (c.start, c.length, c.source_offset())).collect();
    assert_eq!(edits, [(48000, 96000, 48000), (192000, 48000, 168000), (0, 240000, 72000)]);
    assert_eq!(e.session().timecode_start, 3600 * 48000);
    assert_eq!(e.session().tracks[1].channels(), 2);
}

#[test]
fn rust_reader_requires_every_locator_in_the_chosen_media_folder() {
    let f = AafFolder::new();
    std::fs::remove_file(f.dir.join("Diálogo á #1.wav")).unwrap();
    let err = f.preview().unwrap_err().to_string();
    assert!(err.contains("Missing media"), "{err}");
    std::fs::write(f.dir.join("Diálogo á #1.wav"), b"RIFF\0\0\0\0WAVEnot audio").unwrap();
    assert!(f.preview().is_err());
}
