use egui::{Color32, Pos2, RawInput, Rect, Shape, vec2};
use serde_json::json;
use soundcraft_audio_io::AudioBuffer;
use soundcraft_model::{Clip, ClipId, Session, SourceAudio, SourceId};
use soundcraft_time::SampleRate;
use soundcraft_ui_egui::{Services, SoundApp, waveform};

const WAVE: Color32 = Color32::from_rgb(12, 12, 14);

/// The lines and meshes `f` paints across a 400×100 band on a 2x screen.
fn paint(samples_per_px: f64, f: impl Fn(&egui::Painter, &Session, Rect)) -> Vec<Shape> {
    let mut s = Session::new("t", SampleRate::HZ_48000);
    s.edit.zoom.samples_per_px = samples_per_px;
    let band = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 100.0));
    let ctx = egui::Context::default();
    ctx.set_pixels_per_point(2.0);
    let mut out = ctx.run_ui(RawInput { screen_rect: Some(band), ..Default::default() }, |ui| f(ui.painter(), &s, band));
    out.textures_delta.clear();
    out.shapes.into_iter().map(|c| c.shape).filter(|s| matches!(s, Shape::Path(_) | Shape::Mesh(_))).collect()
}

fn draw(samples: Vec<f32>, samples_per_px: f64) -> Vec<Shape> {
    let clip = Clip::audio(ClipId(1), "c", SourceId(1), 0, 0, samples.len() as i64);
    let audio = SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![samples] });
    paint(samples_per_px, |p, s, band| waveform::draw(p, s, band, &clip, &audio, 1, band, WAVE))
}

#[test]
fn a_take_draws_its_overview_while_it_records() {
    let blocks = vec![vec![(-0.5, 0.5); 100]];
    let shapes = paint(128.0, |p, s, band| waveform::draw_live(p, s, band, 1280.0, 128.0, &blocks, band, WAVE));
    let [Shape::Mesh(mesh)] = shapes.as_slice() else { panic!("{shapes:?}") };
    let span = |f: fn(&Pos2) -> f32| mesh.vertices.iter().map(|v| f(&v.pos)).fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v), b.max(v)));
    let ((left, right), (top, bottom)) = (span(|p| p.x), span(|p| p.y));
    assert!((left - 10.0).abs() <= 1.0 && (right - 110.0).abs() <= 1.0, "drawn from {left} to {right}");
    assert!((top - 27.0).abs() <= 1.0 && (bottom - 73.0).abs() <= 1.0, "drawn from {top} to {bottom}");
}

#[test]
fn at_the_sample_level_the_waveform_is_a_line_through_the_samples() {
    let x: Vec<f32> = (0..200).map(|i| (i as f32 * 0.3).sin() * 0.8).collect();
    let shapes = draw(x.clone(), 0.25);
    let [Shape::Path(line)] = shapes.as_slice() else { panic!("{shapes:?}") };
    assert!(line.points.len() > 100);
    for p in &line.points {
        let i = (p.x / 4.0).round() as usize;
        assert!((p.x - i as f32 * 4.0).abs() < 1e-3 && (p.y - (50.0 - x[i] * 46.0)).abs() < 1e-3, "{p:?}");
    }
}

#[test]
fn zoomed_out_a_one_sample_spike_stays_a_point_wide() {
    let mut x = vec![0.0f32; 4800];
    x[1600] = 1.0;
    let shapes = draw(x, 8.0);
    let [Shape::Mesh(mesh)] = shapes.as_slice() else { panic!("{shapes:?}") };
    let tall: Vec<f32> = mesh.vertices.iter().filter(|v| v.pos.y < 40.0).map(|v| v.pos.x).collect();
    let (left, right) = tall.iter().fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
    assert!(right - left <= 1.0 && (199.0..=201.0).contains(&left), "spike drawn from {left} to {right}");
    assert!(mesh.vertices.iter().any(|v| v.pos.y < 5.0 && v.color == WAVE));
}

/// Waveform meshes drawn in a track's row, from `below` its top, once the app has settled.
fn waveforms(ctx: &egui::Context, app: &mut SoundApp, name: &str, below: f32) -> usize {
    let mut shapes = Vec::new();
    for _ in 0..2 {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        shapes = out.shapes;
    }
    let id = app.engine.session().track_by_name(name).unwrap().id.0;
    let row = app.edit_layout.rows.iter().find(|(i, _)| *i == id).unwrap().1;
    let [x0, _, x1, _] = app.edit_layout.timeline;
    let lane = Rect::from_min_max(egui::pos2(x0, row[1] + below), egui::pos2(x1, row[3]));
    shapes.iter().filter(|c| matches!(&c.shape, Shape::Mesh(m) if m.vertices.iter().any(|v| lane.contains(v.pos)))).count()
}

#[test]
fn blocks_hide_waveforms_and_playlist_lanes_show_them() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.engine.execute("track.playlist_duplicate", &json!({"track": "Bass"})).unwrap();
    app.engine.execute("track.view", &json!({"tracks": ["Bass"], "view": "playlists"})).unwrap();
    app.engine.execute("track.view", &json!({"tracks": ["Snare"], "view": "blocks"})).unwrap();
    let main = app.engine.session().track_by_name("Bass").unwrap().height.points();
    let ctx = egui::Context::default();
    assert!(waveforms(&ctx, &mut app, "Kick", 0.0) > 0);
    assert_eq!(waveforms(&ctx, &mut app, "Snare", 0.0), 0);
    assert!(waveforms(&ctx, &mut app, "Bass", main) > 0, "the alternate playlist's lane");
}

#[test]
fn overlapped_crossfades_show_the_outgoing_clip_too() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.engine.execute("edit.fades_create", &json!({"tracks": ["Bass"], "start": {"seconds": 18.2}, "end": {"seconds": 18.7}})).unwrap();
    app.engine.execute("view.zoom_set", &json!({"samples_per_px": 64})).unwrap();
    app.engine.execute("view.scroll", &json!({"to": {"seconds": 18.0}})).unwrap();
    let ctx = egui::Context::default();
    let plain = waveforms(&ctx, &mut app, "Bass", 0.0);
    app.engine.execute("view.waveform_overlapped_crossfades", &json!({"value": true})).unwrap();
    assert_eq!(waveforms(&ctx, &mut app, "Bass", 0.0), plain + 1);
}
