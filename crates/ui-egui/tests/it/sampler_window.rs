//! The Sampler's instrument window: audio dropped on it loads the sample (by path on the
//! desktop, by bytes in the browser), and built-in instruments show their parameters.

use egui::{DroppedFile, Event, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_model::TrackId;
use soundcraft_ui_egui::{Services, SoundApp};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug)]
struct Dropped {
    path: PathBuf,
    bytes: Vec<u8>,
}

impl DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.path
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        Ok(self.bytes.clone())
    }
}

fn wav(secs: f32) -> Vec<u8> {
    let n = (48_000.0 * secs) as usize;
    let one: Vec<f32> = (0..n).map(|i| 0.4 * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin()).collect();
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![one; 2] };
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None };
    soundcraft_audio_io::encode(&buf, &opts).unwrap()
}

/// Feed one harness frame through the raw-input hook before stepping, so dropped files and scripted input are
/// handled.
fn step(h: &mut Harness<'_, SoundApp>) {
    let mut raw = std::mem::take(h.input_mut());
    let ctx = h.ctx.clone();
    h.state_mut().raw_input_hook(&ctx, &mut raw);
    *h.input_mut() = raw;
    h.step();
}

/// A demo app with the Keys sampler window open and six settled frames, plus that track's id.
fn harness() -> (Harness<'static, SoundApp>, TrackId) {
    let engine = soundcraft_engine::demo::demo_engine();
    let keys = engine.session().tracks.iter().find(|t| t.name == "Keys").unwrap().id;
    let mut app = SoundApp::new(engine, None, Services::default());
    app.ui.plugin_windows.push((keys, soundcraft_mix::INSTRUMENT_SLOT));
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..6 {
        step(&mut h);
    }
    (h, keys)
}

fn instrument(h: &Harness<'_, SoundApp>, t: TrackId) -> soundcraft_model::Insert {
    h.state().engine.session().track(t).unwrap().instrument.clone().unwrap()
}

/// Drops `file` with the pointer at the centre of the Keys sampler window.
fn drop_on_window(h: &mut Harness<'_, SoundApp>, keys: TrackId, file: Dropped) {
    let r = h.state().sample_drop_targets.iter().rev().find(|(_, t)| *t == keys).map(|(r, _)| *r).expect("the sampler window is a drop target");
    h.state_mut().synthetic.push(Event::PointerMoved(r.center()));
    for _ in 0..3 {
        step(h);
    }
    h.input_mut().dropped_files.push(Arc::new(file));
    step(h);
    for _ in 0..3 {
        step(h);
    }
}

#[test]
fn built_in_instruments_show_their_parameters() {
    let (h, keys) = harness();
    assert_eq!(instrument(&h, keys).plugin, "subtractive_synth");
    let _ = h.get_by_label("Cutoff");
}

#[test]
fn dropping_audio_with_a_path_loads_the_sampler() {
    let dir = std::env::temp_dir().join(format!("sc-sampler-window-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pluck.wav");
    std::fs::write(&path, wav(0.5)).unwrap();
    let (mut h, keys) = harness();
    drop_on_window(&mut h, keys, Dropped { path: path.clone(), bytes: Vec::new() });
    let ins = instrument(&h, keys);
    assert_eq!(ins.plugin, "sampler");
    let src = h.state().engine.session().source(ins.sample.unwrap()).cloned().unwrap();
    assert_eq!(src.name, "pluck");
    assert_eq!(src.frames, 24_000);
    assert!(h.state().engine.session().pool.get(src.id).is_some());
    assert!(h.state().ui.status.contains("loaded into the sampler"), "{}", h.state().ui.status);
    // The window now shows the sample strip and the sampler's own parameters.
    let _ = h.get_by_label("Load Sample…");
    let _ = h.get_by_label("Root Note");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dropping_audio_without_a_path_reads_its_bytes() {
    let (mut h, keys) = harness();
    drop_on_window(&mut h, keys, Dropped { path: PathBuf::from("web drop.wav"), bytes: wav(0.25) });
    // Nothing loads until the shell has read the bytes (asynchronously, in the browser).
    assert_eq!(instrument(&h, keys).plugin, "subtractive_synth");
    let reads = h.state_mut().take_sample_reads();
    assert_eq!(reads.len(), 1);
    assert_eq!(reads[0].track, keys);
    for r in &reads {
        let bytes = r.file.bytes();
        h.state_mut().load_sample_bytes(r, bytes);
    }
    let ins = instrument(&h, keys);
    assert_eq!(ins.plugin, "sampler");
    assert_eq!(h.state().engine.session().source(ins.sample.unwrap()).map(|s| s.name.as_str()), Some("web drop"));
    assert_eq!(h.state().engine.undo_label(), Some("Load Sampler Sample"));
    // Unreadable bytes leave the track alone and say why.
    let before = h.state().engine.session().sources.len();
    h.state_mut().load_sample_bytes(&reads[0], Err("permission denied".into()));
    assert!(h.state().ui.status.contains("permission denied"));
    assert_eq!(h.state().engine.session().sources.len(), before);
}

/// A non-audio drop goes to the shell exactly once and does not change the instrument or queue a sample read.
#[test]
fn non_audio_drops_are_left_to_the_shell() {
    let (mut h, keys) = harness();
    drop_on_window(&mut h, keys, Dropped { path: PathBuf::from("/tmp/song.scraft"), bytes: Vec::new() });
    assert_eq!(instrument(&h, keys).plugin, "subtractive_synth");
    assert!(h.state_mut().take_sample_reads().is_empty());
    let host = h.state_mut().take_dropped_files();
    assert_eq!(host.len(), 1, "a session drop is handed to the shell once");
    assert!(host.first().is_some_and(|f| f.path().ends_with("song.scraft")));
    assert!(h.state_mut().take_dropped_files().is_empty(), "later steps do not replay it");
}

#[test]
fn a_built_in_instruments_menu_opens_its_parameters() {
    let engine = soundcraft_engine::demo::demo_engine();
    let keys = engine.session().tracks.iter().find(|t| t.name == "Keys").unwrap().id;
    let app = SoundApp::new(engine, None, Services::default());
    let mut h = Harness::builder()
        .with_size(vec2(400.0, 400.0))
        .build_ui_state(move |ui, app: &mut SoundApp| soundcraft_ui_egui::mix_window::instrument_menu(app, ui, keys), app);
    h.step();
    assert!(h.state().ui.plugin_windows.is_empty());
    h.get_by_label("Edit Parameters…").click();
    h.step();
    assert_eq!(h.state().ui.plugin_windows, vec![(keys, soundcraft_mix::INSTRUMENT_SLOT)]);
}
