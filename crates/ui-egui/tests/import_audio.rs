//! Importing audio: files dropped on the Edit window land where they are dropped (a track from
//! the time under the pointer, new tracks, or the Clip List only), and File › Import › Audio
//! imports any number of files to the chosen destination.

use egui::{DroppedFile, Event, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::edit_window::rect;
use soundcraft_ui_egui::import_audio::{Destination, DropTarget, ImportAudio, drop_target};
use soundcraft_ui_egui::{Services, SoundApp};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug)]
struct Dropped(PathBuf);

impl DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

/// Half a second of tone at 48 kHz (24 000 frames).
fn wav(dir: &Path, name: &str) -> PathBuf {
    let one: Vec<f32> = (0..24_000).map(|i| 0.3 * (std::f32::consts::TAU * 330.0 * i as f32 / 48_000.0).sin()).collect();
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![one] };
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None };
    let path = dir.join(name);
    std::fs::write(&path, soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();
    path
}

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sc-import-ui-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn step(h: &mut Harness<'_, SoundApp>) {
    let mut raw = std::mem::take(h.input_mut());
    h.state_mut().raw_input_hook(&mut raw);
    *h.input_mut() = raw;
    h.step();
}

fn harness() -> Harness<'static, SoundApp> {
    let app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let mut h = Harness::builder().with_size(vec2(1600.0, 2000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            let _ = app.take_dropped_files(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..6 {
        step(&mut h);
    }
    h
}

fn drop_at(h: &mut Harness<'_, SoundApp>, p: Pos2, files: &[PathBuf]) {
    h.state_mut().synthetic.push(Event::PointerMoved(p));
    for _ in 0..3 {
        step(h);
    }
    for f in files {
        h.input_mut().dropped_files.push(Arc::new(Dropped(f.clone())));
    }
    for _ in 0..4 {
        step(h);
    }
}

fn track_named(h: &Harness<'_, SoundApp>, name: &str) -> soundcraft_model::TrackId {
    h.state().engine.session().tracks.iter().find(|t| t.name == name).unwrap().id
}

#[test]
fn files_dropped_on_a_track_go_there_one_after_another() {
    let d = dir("track");
    let files = [wav(&d, "one.wav"), wav(&d, "two.wav")];
    let mut h = harness();
    let bass = track_named(&h, "Bass");
    let row = h.state().edit_layout.rows.iter().find(|(id, _)| *id == bass.0).map(|(_, r)| rect(*r)).unwrap();
    let tl = rect(h.state().edit_layout.timeline);
    let p = pos2(tl.min.x + 300.0, row.center().y);
    let Some(DropTarget::Track(t, at)) = drop_target(h.state(), Some(p)) else { panic!("an audio track's row takes the files") };
    assert_eq!(t, bass);
    let (tracks, clips) = {
        let s = h.state().engine.session();
        (s.tracks.len(), s.track(bass).unwrap().clips().len())
    };
    drop_at(&mut h, p, &files);
    let s = h.state().engine.session();
    assert_eq!(s.tracks.len(), tracks, "no new tracks");
    let mut starts: Vec<i64> = s.track(bass).unwrap().clips().iter().filter(|c| c.name == "one" || c.name == "two").map(|c| c.start).collect();
    starts.sort_unstable();
    assert_eq!(starts, [at, at + 24_000]);
    assert!(h.state().ui.status.starts_with("Imported 2 files onto Bass"), "{}", h.state().ui.status);
    // One undo step takes the whole drop back.
    assert!(h.state_mut().engine.undo());
    assert_eq!(h.state().engine.session().track(bass).unwrap().clips().len(), clips);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn files_dropped_below_the_tracks_make_new_tracks_and_on_the_clip_list_stay_there() {
    let d = dir("new");
    let files = [wav(&d, "kick.wav"), wav(&d, "snare.wav")];
    let mut h = harness();
    let tl = rect(h.state().edit_layout.timeline);
    let below = pos2(tl.min.x + 200.0, tl.max.y - 10.0);
    assert!(matches!(drop_target(h.state(), Some(below)), Some(DropTarget::NewTracks(_))), "empty timeline space makes new tracks");
    let tracks = h.state().engine.session().tracks.len();
    drop_at(&mut h, below, &files);
    assert_eq!(h.state().engine.session().tracks.len(), tracks + 2);

    let list = h.state().clip_list_rect.expect("the Clip List is shown");
    assert_eq!(drop_target(h.state(), Some(list.center())), Some(DropTarget::ClipList));
    let (tracks, sources) = (h.state().engine.session().tracks.len(), h.state().engine.session().sources.len());
    drop_at(&mut h, list.center(), &[wav(&d, "pad.wav")]);
    let s = h.state().engine.session();
    assert_eq!((s.tracks.len(), s.sources.len()), (tracks, sources + 1), "a source, no track");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_import_dialog_imports_its_list_to_the_chosen_destination() {
    let d = dir("dialog");
    let files: Vec<String> = [wav(&d, "a.wav"), wav(&d, "b.wav")].iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let mut h = harness();
    let (tracks, sources) = (h.state().engine.session().tracks.len(), h.state().engine.session().sources.len());
    let mut imp = ImportAudio { destination: Destination::ClipList, ..ImportAudio::default() };
    imp.add(files);
    h.state_mut().dialogs.open = Some(Dialog::ImportAudio(imp));
    h.run_steps(3);
    let _ = h.get_by_label("Clip List only");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Import").click();
    h.run_steps(3);
    let s = h.state().engine.session();
    assert_eq!((s.tracks.len(), s.sources.len()), (tracks, sources + 2));
    assert!(h.state().dialogs.open.is_none(), "the dialog closes");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn import_with_an_empty_list_does_nothing() {
    let mut h = harness();
    let tracks = h.state().engine.session().tracks.len();
    h.state_mut().dialogs.open = Some(Dialog::ImportAudio(ImportAudio::default()));
    h.run_steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Import").click();
    h.run_steps(3);
    assert_eq!(h.state().engine.session().tracks.len(), tracks);
    assert!(h.state().dialogs.open.is_some(), "still open, waiting for files");
}
