use super::*;
use crate::Services;
use egui::{Color32, ColorImage, RawInput, Rect, ViewportCommand, ViewportId, ViewportInfo, pos2, vec2};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::time::Duration;

/// A host frame with the given logical size and texture limit, native scale 1.0 and no events.
fn raw_input(width: f32, height: f32, max_texture_side: usize) -> RawInput {
    let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(width, height));
    let viewport = ViewportInfo { inner_rect: Some(rect), native_pixels_per_point: Some(1.0), ..Default::default() };
    RawInput {
        screen_rect: Some(rect),
        max_texture_side: Some(max_texture_side),
        viewports: [(ViewportId::ROOT, viewport)].into_iter().collect(),
        ..Default::default()
    }
}

/// A demo-backed app with no player, for control tests.
fn app() -> SoundApp {
    SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default())
}

/// Run one real UI pass: feed raw input, then logic and UI, dropping texture deltas the tests do not inspect.
fn frame(app: &mut SoundApp, ctx: &egui::Context, mut raw: RawInput) -> egui::FullOutput {
    app.raw_input_hook(ctx, &mut raw);
    let mut out = ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    out
}

/// Run four frames so the first layout and demo state have settled.
fn warm(app: &mut SoundApp, ctx: &egui::Context) {
    for _ in 0..4 {
        let _ = frame(app, ctx, raw_input(800.0, 600.0, 8192));
    }
}

/// Run real UI passes until every queued scripted gesture has reached one and its reply is sent.
fn deliver_gestures(app: &mut SoundApp, ctx: &egui::Context) {
    for _ in 0..MAX_SYNTHETIC_EVENTS + 16 {
        if !gesture_pending(app) {
            return;
        }
        let _ = frame(app, ctx, raw_input(800.0, 600.0, 8192));
    }
}

/// Queue one screenshot request and install its receiver for [`drain`].
fn request(app: &mut SoundApp, path: Option<&PathBuf>) -> std::sync::mpsc::Receiver<Value> {
    let (tx, rx) = channel();
    let params = path.map_or_else(|| json!({}), |p| json!({"path": p}));
    let (req, reply) = ControlRequest::new("ui.screenshot", params);
    tx.send(req).unwrap();
    app.control_rx = Some(rx);
    reply
}

/// First screenshot callback emitted in this frame, if any.
fn take_callback(out: &egui::FullOutput) -> Option<egui::ScreenshotCallback> {
    take_callbacks(out).into_iter().next()
}

/// All screenshot callbacks emitted for the root viewport in this frame.
fn take_callbacks(out: &egui::FullOutput) -> Vec<egui::ScreenshotCallback> {
    out.viewport_output
        .get(&ViewportId::ROOT)
        .map(|o| {
            o.commands
                .iter()
                .filter_map(|c| match c {
                    ViewportCommand::Screenshot(cb) => Some(cb.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A process-unique temporary PNG path for one test.
fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("soundcraft-shot-{}-{name}.png", std::process::id()))
}

/// A completed capture writes one file and returns its dimensions once; draining the single completion leaves no
/// further reply.
#[test]
fn a_screenshot_completes_once_and_writes_one_file() {
    let (mut a, ctx) = (app(), egui::Context::default());
    warm(&mut a, &ctx);
    let path = tmp("success");
    let _ = std::fs::remove_file(&path);
    let reply = request(&mut a, Some(&path));
    let out = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192));
    let callback = take_callback(&out).expect("the request is issued from the UI pass");
    callback.complete(Arc::new(ColorImage::filled([800, 600], Color32::RED)));
    let _ = ctx.run_logic(&raw_input(800.0, 600.0, 8192), |ctx| a.logic(ctx));
    let v = reply.try_recv().expect("one reply");
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["result"]["width"], 800);
    assert!(path.exists());
    assert!(reply.try_recv().is_err(), "a late callback must not produce a second reply");
    assert_eq!(pending_shot_count(&a), 0);
    let _ = std::fs::remove_file(&path);
}

/// A callback after the deadline sends a timeout and must not create the requested file.
#[test]
fn a_callback_after_the_deadline_times_out_without_writing() {
    let (mut a, ctx) = (app(), egui::Context::default());
    warm(&mut a, &ctx);
    let path = tmp("late");
    let reply = request(&mut a, Some(&path));
    let out = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192));
    let callback = take_callback(&out).expect("issued");
    a.advance_test_clock(Duration::from_secs(9));
    callback.complete(Arc::new(ColorImage::filled([800, 600], Color32::RED)));
    let _ = ctx.run_logic(&raw_input(800.0, 600.0, 8192), |ctx| a.logic(ctx));
    let v = reply.try_recv().expect("one timeout reply");
    assert_eq!(v["ok"], false);
    assert!(v["error"].as_str().unwrap_or_default().contains("timed out"), "{v}");
    assert!(!path.exists(), "a late result must not create a file");
}

/// Replacing the document cancels a pending capture and frees its reservation without writing a file.
#[test]
fn a_session_change_cancels_the_request_and_releases_its_pixels() {
    let (mut a, ctx) = (app(), egui::Context::default());
    warm(&mut a, &ctx);
    let path = tmp("document");
    let reply = request(&mut a, Some(&path));
    let out = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192));
    let callback = take_callback(&out).expect("issued");
    a.doc_epoch = a.doc_epoch.wrapping_add(1);
    callback.complete(Arc::new(ColorImage::filled([800, 600], Color32::RED)));
    let _ = ctx.run_logic(&raw_input(800.0, 600.0, 8192), |ctx| a.logic(ctx));
    let v = reply.try_recv().expect("one cancellation reply");
    assert_eq!(v["ok"], false);
    assert!(v["error"].as_str().unwrap_or_default().contains("session changed"), "{v}");
    assert!(!path.exists());
}

/// Admission caps the queue at [`MAX_PENDING_SHOTS`]; the next request is refused before any capture is issued.
#[test]
fn admission_rejects_more_than_eight_pending_requests() {
    let (mut a, ctx) = (app(), egui::Context::default());
    warm(&mut a, &ctx);
    for i in 0..MAX_PENDING_SHOTS {
        let (req, _reply) = ControlRequest::new("ui.screenshot", json!({}));
        admit(&mut a, &ctx, &req).unwrap_or_else(|e| panic!("request {i}: {e}"));
    }
    let (req, _reply) = ControlRequest::new("ui.screenshot", json!({}));
    let e = admit(&mut a, &ctx, &req).expect_err("the ninth request is rejected");
    assert_eq!(e, "screenshot queue full");
}

/// A single oversized viewport is refused, and aggregate accounting uses encode peak so two medium captures fit
/// while a third does not.
#[test]
fn admission_rejects_an_image_over_the_per_image_and_aggregate_budgets() {
    let (mut a, ctx) = (app(), egui::Context::default());
    let mut big = raw_input(5000.0, 5000.0, 8192);
    a.raw_input_hook(&ctx, &mut big);
    let (req, _reply) = ControlRequest::new("ui.screenshot", json!({}));
    let e = admit(&mut a, &ctx, &req).expect_err("5000x5000 does not fit 64 MiB");
    assert!(e.contains("too large"), "{e}");

    // 24 MB of pixels plus the PNG/base64 copies is about 84 MB of encode peak. Two fit the
    // 192 MiB aggregate; a third does not even though the retained images total only 72 MB.
    let mut medium = raw_input(3000.0, 2000.0, 8192);
    a.raw_input_hook(&ctx, &mut medium);
    for _ in 0..2 {
        let (req, _reply) = ControlRequest::new("ui.screenshot", json!({}));
        admit(&mut a, &ctx, &req).expect("two encode peaks fit the 192 MiB budget");
    }
    assert_eq!(a.pending_shots.iter().map(|s| s.image_bytes).sum::<usize>(), 2 * 3000 * 2000 * 4);
    assert!(a.pending_shots.iter().all(|s| s.reserved_bytes > s.image_bytes));
    let (req, _reply) = ControlRequest::new("ui.screenshot", json!({}));
    let e = admit(&mut a, &ctx, &req).expect_err("the third encode peak exceeds the aggregate");
    assert!(e.contains("budget"), "{e}");
}

/// A resize while a capture is in flight retries against the new geometry without writing the stale image.
#[test]
fn a_changed_window_retries_once_instead_of_writing_stale_pixels() {
    let (mut a, ctx) = (app(), egui::Context::default());
    warm(&mut a, &ctx);
    let path = tmp("retry");
    let reply = request(&mut a, Some(&path));
    let out = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192));
    let first = take_callback(&out).expect("issued at 800x600");
    // Resize while the capture is in flight: the completed 800x600 image is stale.
    let _ = frame(&mut a, &ctx, raw_input(1000.0, 600.0, 8192));
    first.complete(Arc::new(ColorImage::filled([800, 600], Color32::RED)));
    let _ = ctx.run_logic(&raw_input(1000.0, 600.0, 8192), |ctx| a.logic(ctx));
    assert_eq!(pending_shot_count(&a), 1, "the request was retried, not failed");
    let out = frame(&mut a, &ctx, raw_input(1000.0, 600.0, 8192));
    let second = take_callback(&out).expect("retried at 1000x600");
    second.complete(Arc::new(ColorImage::filled([1000, 600], Color32::BLUE)));
    let _ = ctx.run_logic(&raw_input(1000.0, 600.0, 8192), |ctx| a.logic(ctx));
    let v = reply.try_recv().expect("one success reply");
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["result"]["width"], 1000);
    let _ = std::fs::remove_file(&path);
}

/// A tiny partial image for a normal window is malformed, while a full-size image passes.
#[test]
fn a_truncated_image_is_malformed_not_a_successful_screenshot() {
    let tiny = ColorImage::filled([1, 1], Color32::RED);
    assert!(matches!(check_image(&tiny, [800, 600]), Err(ImageProblem::Malformed(_))));
    let full = ColorImage::filled([800, 600], Color32::RED);
    assert!(check_image(&full, [800, 600]).is_ok());
}

/// The byte bound allows the same one-pixel-per-axis tolerance as the dimension check; both encoding paths
/// reserve more than the image itself.
#[test]
fn the_byte_limit_tolerates_the_same_one_pixel_as_the_dimension_check() {
    let g = CaptureGeometry { points: egui::vec2(800.0, 600.0), pixels_per_point: 1.0, max_texture_side: 8192, focused: true };
    assert_eq!(g.reserved_bytes(), Some(800 * 600 * 4));
    assert!(g.encode_peak_bytes(true).unwrap_or(0) > g.reserved_bytes().unwrap_or(0), "base64 needs headroom");
    assert!(g.encode_peak_bytes(false).unwrap_or(0) > g.reserved_bytes().unwrap_or(0), "PNG encoding needs headroom");
    assert_eq!(g.tolerated_bytes(), Some(801 * 601 * 4), "one pixel more on each axis");
    let one_pixel_wider = ColorImage::filled([801, 600], Color32::RED);
    assert!(check_image(&one_pixel_wider, [800, 600]).is_ok(), "the staleness rule accepts it, so the byte limit must too");
}

/// A raw input without a texture limit uses egui's last known limit instead of inventing a smaller default.
#[test]
fn a_missing_max_texture_side_uses_the_last_known_limit() {
    let ctx = egui::Context::default();
    let mut first = ctx.run_ui(raw_input(400.0, 300.0, 8192), |_ui| {});
    first.textures_delta.clear();
    let mut raw = raw_input(400.0, 300.0, 8192);
    raw.max_texture_side = None;
    let g = CaptureGeometry::from_raw(&ctx, &raw).expect("geometry");
    assert_eq!(g.max_texture_side, 8192, "a missing field must not invent the 2048 fallback");
}

/// Zero-sized host geometry has no capturable size and is refused at admission.
#[test]
fn a_window_without_geometry_reports_no_capturable_size() {
    let (mut a, ctx) = (app(), egui::Context::default());
    a.host_geometry = Some(CaptureGeometry { points: egui::Vec2::ZERO, pixels_per_point: 1.0, max_texture_side: 8192, focused: true });
    let (req, _reply) = ControlRequest::new("ui.screenshot", json!({}));
    let e = admit(&mut a, &ctx, &req).expect_err("no capturable size");
    assert!(e.contains("no capturable size"), "{e}");
}

/// Recover and a session the browser read replace the document without `SoundApp::run`; both go
/// through `document_replaced`, so a pending capture is still canceled.
#[test]
fn a_document_replaced_outside_run_cancels_a_pending_capture() {
    let (mut a, ctx) = (app(), egui::Context::default());
    warm(&mut a, &ctx);
    let reply = request(&mut a, None);
    let out = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192));
    let callback = take_callback(&out).expect("issued");
    a.document_replaced();
    callback.complete(Arc::new(ColorImage::filled([800, 600], Color32::RED)));
    let _ = ctx.run_logic(&raw_input(800.0, 600.0, 8192), |ctx| a.logic(ctx));
    let v = reply.try_recv().expect("one cancellation reply");
    assert_eq!(v["ok"], false);
    assert!(v["error"].as_str().unwrap_or_default().contains("session changed"), "{v}");
}

/// The control connection sends one request at a time, but it can pipeline requests in one
/// write. A queued click must be applied (its reply delivered) before the following inspect and
/// screenshot run; otherwise the inspect reports the pre-click state and the screenshot captures
/// the old frame.
#[test]
fn a_queued_click_is_applied_before_a_following_inspect_and_screenshot() {
    let (mut a, ctx) = (app(), egui::Context::default());
    warm(&mut a, &ctx);
    let kick = a.engine.session().track_by_name("Kick").expect("demo has Kick").id;
    assert!(!a.engine.session().track(kick).expect("kick").mixer.mute);
    let row = a.edit_layout.rows[0].1;
    // Mute is the fourth header key (Record, Input, Solo, Mute) at y=20, 16x16.
    let pos = egui::pos2(row[0] + 101.0 + 8.0, row[1] + 20.0 + 8.0);
    let (tx, rx) = channel();
    let (req, click_reply) = ControlRequest::new("ui.click", json!({"x": pos.x, "y": pos.y}));
    tx.send(req).unwrap();
    let (req, inspect_reply) = ControlRequest::new("session.inspect", json!({}));
    tx.send(req).unwrap();
    let path = tmp("ordered-gesture");
    let _ = std::fs::remove_file(&path);
    let (req, shot_reply) = ControlRequest::new("ui.screenshot", json!({"path": path}));
    tx.send(req).unwrap();
    a.control_rx = Some(rx);

    // The first pass admits only the click; the following requests stay queued.
    let out = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192));
    assert!(take_callbacks(&out).is_empty(), "no capture before the click was delivered");
    assert!(click_reply.try_recv().is_err(), "click is not acknowledged yet");
    assert!(inspect_reply.try_recv().is_err(), "inspect must wait for the click");
    assert_eq!(pending_shot_count(&a), 0);

    // A click is a move, press and release: one event per UI pass.
    let mut clicked = None;
    for _ in 0..5 {
        let out = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192));
        assert!(take_callbacks(&out).is_empty(), "capture cannot jump the click");
        if let Ok(v) = click_reply.try_recv() {
            clicked = Some(v);
            break;
        }
    }
    let clicked = clicked.expect("the click reply arrives after all three events were applied");
    assert_eq!(clicked["ok"], true, "{clicked}");
    assert!(a.engine.session().track(kick).expect("kick").mixer.mute, "the click changed the track before its reply");

    // Now the next pass may run the inspect and admit the screenshot.
    let out = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192));
    let inspect = inspect_reply.try_recv().expect("inspect runs after the click");
    let kick_state = inspect["result"]["tracks"]
        .as_array()
        .and_then(|tracks| tracks.iter().find(|t| t["id"] == json!(kick.0)))
        .expect("Kick in the inspected session");
    assert_eq!(kick_state["mute"], true, "inspect saw the clicked state");
    let callback = take_callback(&out).expect("the screenshot is admitted only after the click");
    callback.complete(Arc::new(ColorImage::filled([800, 600], Color32::BLUE)));
    let _ = ctx.run_logic(&raw_input(800.0, 600.0, 8192), |ctx| a.logic(ctx));
    let shot = shot_reply.try_recv().expect("one screenshot reply");
    assert_eq!(shot["ok"], true, "{shot}");
    assert!(path.exists());
    let _ = std::fs::remove_file(&path);
}

/// Each capture slot is bounded by its request's own reservation, not the 64 MiB ceiling: a
/// small request cannot retain a full-size image and blow the documented aggregate budget.
#[test]
fn a_capture_slot_is_bounded_by_its_own_reservation() {
    let (mut a, ctx) = (app(), egui::Context::default());
    warm(&mut a, &ctx);
    let (tx, rx) = channel();
    let (req, first) = ControlRequest::new("ui.screenshot", json!({}));
    tx.send(req).unwrap();
    let (req, second) = ControlRequest::new("ui.screenshot", json!({}));
    tx.send(req).unwrap();
    a.control_rx = Some(rx);
    let out = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192));
    let mut calls = take_callbacks(&out).into_iter();
    let oversized = calls.next().expect("the first admitted capture is issued");
    let valid = calls.next().expect("the second admitted capture is issued");
    assert!(calls.next().is_none(), "no extra captures");
    assert_eq!(pending_shot_count(&a), 2);
    // 12 MB is under the old 64 MiB slot bound but far over this request's ~1.9 MB image.
    oversized.complete(Arc::new(ColorImage::filled([2000, 1500], Color32::RED)));
    let _ = ctx.run_logic(&raw_input(800.0, 600.0, 8192), |ctx| a.logic(ctx));
    let v = first.try_recv().expect("oversized completion has one reply");
    assert_eq!(v["ok"], false, "{v}");
    assert!(v["error"].as_str().unwrap_or_default().contains("per-image limit"), "{v}");
    assert_eq!(pending_shot_count(&a), 1, "the oversized image was released, the other request kept");
    valid.complete(Arc::new(ColorImage::filled([800, 600], Color32::BLUE)));
    let _ = ctx.run_logic(&raw_input(800.0, 600.0, 8192), |ctx| a.logic(ctx));
    let v = second.try_recv().expect("the valid capture still succeeds");
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(pending_shot_count(&a), 0, "no reservation or image outlives its reply");
}

/// Overflowing dimensions and a pixel-count mismatch are errors, not allocation overflows.
#[test]
fn png_bytes_checks_its_dimensions_and_allocation() {
    let huge = ColorImage { size: [usize::MAX, 2], source_size: egui::vec2(0.0, 0.0), pixels: Vec::new() };
    assert!(png_bytes(&huge).is_err(), "an overflowing size is an error, not a capacity overflow");
    let mismatched = ColorImage { size: [4, 4], source_size: egui::vec2(4.0, 4.0), pixels: vec![Color32::RED; 3] };
    assert!(png_bytes(&mismatched).is_err(), "a short pixel buffer is rejected before it is copied");
}

/// The synthetic event queue and text payload are bounded; a queued text gesture is acknowledged only after a UI
/// pass applies it.
#[test]
fn control_input_queues_and_text_are_bounded() {
    let (mut a, ctx) = (app(), egui::Context::default());
    a.synthetic = (0..MAX_SYNTHETIC_EVENTS).map(|_| egui::Event::PointerMoved(egui::pos2(0.0, 0.0))).collect();
    let (req, _reply) = ControlRequest::new("ui.move", json!({"x": 1.0, "y": 2.0}));
    let v = handle(&mut a, &ctx, &req).expect("the refusal replies immediately");
    assert_eq!(v["ok"], false, "{v}");
    assert!(v["error"].as_str().unwrap_or_default().contains("queue full"), "{v}");
    assert_eq!(a.synthetic.len(), MAX_SYNTHETIC_EVENTS, "a refused request queues nothing");

    a.synthetic.clear();
    let text = "x".repeat(MAX_CONTROL_TEXT_BYTES + 1);
    let (req, _reply) = ControlRequest::new("ui.text", json!({"text": text}));
    let v = handle(&mut a, &ctx, &req).expect("the refusal replies immediately");
    assert_eq!(v["ok"], false, "{v}");
    assert!(a.synthetic.is_empty(), "an oversized text request queues no event");

    let (req, reply) = ControlRequest::new("ui.text", json!({"text": "x"}));
    assert!(handle(&mut a, &ctx, &req).is_none(), "text is acknowledged only after delivery");
    assert!(reply.try_recv().is_err());
    deliver_gestures(&mut a, &ctx);
    let v = reply.try_recv().expect("text reply arrives after its event was applied");
    assert_eq!(v["ok"], true, "{v}");
}

/// A path over the retained-path limit is refused before a screenshot request is queued.
#[test]
fn a_very_long_screenshot_path_is_refused_before_it_is_retained() {
    let (mut a, ctx) = (app(), egui::Context::default());
    warm(&mut a, &ctx);
    let path = "x".repeat(MAX_SHOT_PATH_BYTES + 1);
    let (req, _reply) = ControlRequest::new("ui.screenshot", json!({"path": path}));
    let e = admit(&mut a, &ctx, &req).expect_err("the path exceeds the retained-path limit");
    assert!(e.contains("path longer"), "{e}");
    assert_eq!(pending_shot_count(&a), 0);
}

/// An expired undelivered gesture is canceled with one reply, drops its events and does not block the next
/// request.
#[test]
fn an_undelivered_gesture_is_canceled_without_wedging_the_next_request() {
    let (mut a, ctx) = (app(), egui::Context::default());
    let (req, reply) = ControlRequest::new("ui.click", json!({"x": 10.0, "y": 10.0}));
    assert!(handle(&mut a, &ctx, &req).is_none(), "the click waits for a UI pass");
    a.advance_test_clock(Duration::from_secs(16));
    expire_gestures(&mut a);
    let v = reply.try_recv().expect("the canceled gesture has exactly one reply");
    assert_eq!(v["ok"], false, "{v}");
    assert!(v["error"].as_str().unwrap_or_default().contains("not delivered"), "{v}");
    assert!(a.synthetic.is_empty(), "canceled events are dropped");
    assert!(a.synthetic_batch.is_none());
    // The control channel is not wedged: the next request runs normally.
    let (req, reply) = ControlRequest::new("session.inspect", json!({}));
    let v = handle(&mut a, &ctx, &req).expect("inspect replies immediately");
    assert_eq!(v["ok"], true, "{v}");
    assert!(reply.try_recv().is_err(), "handle returns the reply for drain to send");
}

/// An expired scripted click sends no synthetic release, so it cannot click or drop, and a fresh click still
/// works.
#[test]
fn a_timed_out_consumed_press_cancels_without_clicking_and_the_next_click_works() {
    for expires_after_hook in [false, true] {
        let (mut a, ctx) = (app(), egui::Context::default());
        a.advance_test_clock(Duration::ZERO);
        warm(&mut a, &ctx);
        let kick = a.engine.session().track_by_name("Kick").unwrap().id;
        let row = a.edit_layout.rows[0].1;
        let pos = pos2(row[0] + 109.0, row[1] + 28.0);
        let (req, reply) = ControlRequest::new("ui.click", json!({"x": pos.x, "y": pos.y}));
        assert!(handle(&mut a, &ctx, &req).is_none());
        let _ = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192)); // move
        let _ = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192)); // press
        assert!(ctx.input(|i| i.pointer.primary_down()), "the real egui pass consumed the scripted press");
        assert!(!a.engine.session().track(kick).unwrap().mixer.mute);
        assert!(reply.try_recv().is_err(), "the release is still pending");

        if expires_after_hook {
            // The deadline can cross after the hook handed egui the release but before logic
            // and widgets run. That pass must not turn a timed-out gesture into a click.
            let mut raw = raw_input(800.0, 600.0, 8192);
            a.raw_input_hook(&ctx, &mut raw);
            a.advance_test_clock(Duration::from_secs(16));
            let mut out = ctx.run_ui(raw, |ui| {
                a.logic(ui.ctx());
                a.ui(ui);
            });
            out.textures_delta.clear();
        } else {
            a.advance_test_clock(Duration::from_secs(16));
            let _ = ctx.run_logic(&raw_input(800.0, 600.0, 8192), |ctx| a.logic(ctx));
        }
        let canceled = reply.try_recv().expect("one cancellation reply");
        assert_eq!(canceled["ok"], false, "{canceled}");
        let _ = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192));
        assert!(!ctx.input(|i| i.pointer.any_down() || i.pointer.any_released()), "canceling is not a synthetic release");
        assert!(ctx.dragged_id().is_none());
        assert!(ctx.drag_stopped_id().is_none(), "canceling cannot dispatch a drop");
        assert!(!a.engine.session().track(kick).unwrap().mixer.mute, "the abandoned release never clicks Mute");
        assert!(reply.try_recv().is_err(), "the canceled gesture replies once");

        let (req, reply) = ControlRequest::new("ui.click", json!({"x": pos.x, "y": pos.y}));
        assert!(handle(&mut a, &ctx, &req).is_none());
        deliver_gestures(&mut a, &ctx);
        assert_eq!(reply.try_recv().unwrap()["ok"], true);
        assert!(a.engine.session().track(kick).unwrap().mixer.mute, "a completed click still works normally");
        assert!(!a.synthetic_pointer_active && !a.cancel_synthetic_pointer);
    }
}

/// An expired scripted shortcut never reaches widgets or end-of-pass handling, while a simultaneous host shortcut
/// still runs and a later script succeeds.
#[test]
fn a_timed_out_scripted_key_after_the_hook_does_not_run_and_the_next_key_works() {
    let (mut a, ctx) = (app(), egui::Context::default());
    a.advance_test_clock(Duration::ZERO);
    warm(&mut a, &ctx);
    a.run("track.new", json!({"count": 1, "format": "stereo"})).unwrap();
    let depth = a.engine.undo_history().len();
    assert!(depth > 0, "the test needs an undoable edit");
    let applied = a.synthetic_applied_batches;
    let quickpunch = a.engine.session().edit.quickpunch;
    let (req, reply) = ControlRequest::new("ui.key", json!({"key": "z", "cmd": true}));
    assert!(handle(&mut a, &ctx, &req).is_none());
    let mut raw = raw_input(800.0, 600.0, 8192);
    // A real, unrelated shortcut in the same pass must still be dispatched after the expired
    // scripted batch is removed.
    let command_shift = egui::Modifiers { command: true, shift: true, ..egui::Modifiers::NONE };
    raw.events.push(egui::Event::Key { key: egui::Key::P, physical_key: None, pressed: true, repeat: false, modifiers: command_shift });
    raw.events.push(egui::Event::Key { key: egui::Key::P, physical_key: None, pressed: false, repeat: false, modifiers: command_shift });
    a.raw_input_hook(&ctx, &mut raw);
    assert!(a.synthetic_batch.is_some(), "the key batch waits for a UI pass");

    // The deadline crosses after the hook handed the key to egui but before logic and widgets.
    a.advance_test_clock(Duration::from_secs(16));
    let mut out = ctx.run_ui(raw, |ui| {
        a.logic(ui.ctx());
        a.ui(ui);
    });
    out.textures_delta.clear();
    let canceled = reply.try_recv().expect("one cancellation reply");
    assert_eq!(canceled["ok"], false, "{canceled}");
    assert!(a.synthetic_batch.is_none(), "the expired batch is dropped");
    assert_eq!(a.synthetic_applied_batches, applied, "an expired batch is not delivered");
    assert_eq!(a.engine.undo_history().len(), depth, "the expired key must not run its shortcut");
    assert_eq!(a.engine.session().edit.quickpunch, !quickpunch, "a simultaneous host shortcut still runs");
    assert!(!a.cancel_synthetic_keys);
    assert!(
        ctx.input(|i| !i.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::Z, .. }))),
        "the expired key is gone from the pass, not left for widgets or end-of-pass shortcuts"
    );
    let _ = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192));
    assert_eq!(a.engine.undo_history().len(), depth, "a later frame must not replay it");

    // A fresh gesture after the timeout still works and runs exactly once.
    let (req, reply) = ControlRequest::new("ui.key", json!({"key": "z", "cmd": true}));
    assert!(handle(&mut a, &ctx, &req).is_none());
    deliver_gestures(&mut a, &ctx);
    assert_eq!(reply.try_recv().unwrap()["ok"], true);
    assert_eq!(a.engine.undo_history().len(), depth - 1, "a completed key still undoes exactly once");
}

/// Current timeline zoom, horizontal scroll and vertical scroll, compared before and after a gesture.
fn view(a: &SoundApp) -> (f64, i64, f32) {
    let zoom = a.engine.session().edit.zoom;
    (zoom.samples_per_px, zoom.scroll, a.edit_layout.scroll_y)
}

/// The centre of the edit window's timeline.
fn timeline_center(a: &SoundApp) -> egui::Pos2 {
    let [x0, y0, x1, y1] = a.edit_layout.timeline;
    pos2((x0 + x1) * 0.5, (y0 + y1) * 0.5)
}

/// A host keyboard event with no modifiers; `pressed` selects press or release.
fn host_key(key: egui::Key, pressed: bool) -> egui::Event {
    egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE }
}

/// The same physical wheel notch `ui.scroll {dy:-30, unit:"line", steps:1}` queues.
fn host_line_wheel(dy: f32) -> egui::Event {
    egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: egui::vec2(0.0, dy),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
        modifiers: egui::Modifiers::NONE,
    }
}

/// A wheel batch that expires after the hook already handed it to egui must not scroll. Draining
/// its event is not enough: egui has folded it into `smooth_scroll_delta` and the wheel smoothing
/// state that continues on later frames. The pass input is replayed from before the injection, so
/// the stale notch is gone while a simultaneous host key still arrives and later input still works.
#[test]
fn a_timed_out_scripted_scroll_after_the_hook_does_not_scroll_or_keep_smoothing() {
    let (mut a, ctx) = (app(), egui::Context::default());
    a.advance_test_clock(Duration::ZERO);
    warm(&mut a, &ctx);
    // Make the track area overflow so a real wheel would actually move the view.
    a.run("track.new", json!({"count": 20, "format": "stereo"})).unwrap();
    warm(&mut a, &ctx);
    let at = timeline_center(&a);
    let before = view(&a);

    let (req, reply) = ControlRequest::new("ui.scroll", json!({"x": at.x, "y": at.y, "dy": -30.0, "unit": "line", "steps": 1}));
    assert!(handle(&mut a, &ctx, &req).is_none(), "the reply waits for delivery");
    let _ = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192)); // the hover move is its own batch
    assert!(a.synthetic_batch.is_none());
    assert_eq!(a.synthetic.len(), 1, "the wheel event is next");

    let mut raw = raw_input(800.0, 600.0, 8192);
    raw.events.push(host_key(egui::Key::F24, true));
    a.raw_input_hook(&ctx, &mut raw);
    assert!(a.synthetic_batch.as_ref().is_some_and(|b| b.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. }))));
    assert!(a.input_before_scroll.is_some(), "the pre-injection input was saved for a possible expiry");

    // The deadline crosses after the hook handed the wheel to egui but before logic and widgets.
    a.advance_test_clock(Duration::from_secs(16));
    let mut out = ctx.run_ui(raw, |ui| {
        a.logic(ui.ctx());
        a.ui(ui);
    });
    out.textures_delta.clear();

    let canceled = reply.try_recv().expect("one cancellation reply");
    assert_eq!(canceled["ok"], false, "{canceled}");
    assert!(a.synthetic_batch.is_none(), "the expired batch is dropped");
    assert!(!a.cancel_synthetic_scroll);
    assert_eq!(view(&a), before, "the expired wheel must not scroll");
    assert!(ctx.input(|i| i.keys_down.contains(&egui::Key::F24)), "a host key pressed in the same pass survives");
    assert!(
        ctx.input(|i| i.raw.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::F24, pressed: true, .. }))),
        "the host key event is kept in the pass input"
    );
    assert!(
        !ctx.input(|i| i.raw.events.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. }))),
        "the expired wheel is gone from the pass input"
    );
    assert!(
        !ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. }))),
        "no wheel event reaches widgets or egui's aggregates"
    );

    // Its smoothing state must not keep scrolling in later frames either.
    let mut release = raw_input(800.0, 600.0, 8192);
    release.events.push(host_key(egui::Key::F24, false));
    let _ = frame(&mut a, &ctx, release);
    assert!(!ctx.input(|i| i.keys_down.contains(&egui::Key::F24)), "the host release still applies");
    for _ in 0..12 {
        let _ = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192));
    }
    assert_eq!(view(&a), before, "canceling cannot leave egui smoothing the stale notch");

    // The control chain is not wedged and the next gesture applies exactly once.
    let (req, reply) = ControlRequest::new("ui.zoom", json!({"x": at.x, "y": at.y, "factor": 4.0, "steps": 1}));
    assert!(handle(&mut a, &ctx, &req).is_none());
    let _ = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192)); // hover move
    deliver_gestures(&mut a, &ctx);
    assert_eq!(reply.try_recv().unwrap()["ok"], true);
    let after = view(&a);
    assert!((after.0 * 4.0 / before.0 - 1.0).abs() < 1e-4, "a later zoom works once: {} -> {}", before.0, after.0);
    for _ in 0..6 {
        let _ = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192));
    }
    assert!((view(&a).0 - after.0).abs() < 1e-9, "the later zoom is not replayed");
}

/// The same late expiry for a pinch `Zoom` event: no view change, a simultaneous host key is
/// kept, and a fresh zoom afterwards scales the view exactly once.
#[test]
fn a_timed_out_scripted_zoom_after_the_hook_does_not_zoom_and_the_next_zoom_works_once() {
    let (mut a, ctx) = (app(), egui::Context::default());
    a.advance_test_clock(Duration::ZERO);
    warm(&mut a, &ctx);
    let at = timeline_center(&a);
    let before = view(&a);

    let (req, reply) = ControlRequest::new("ui.zoom", json!({"x": at.x, "y": at.y, "factor": 4.0, "steps": 1}));
    assert!(handle(&mut a, &ctx, &req).is_none(), "the reply waits for delivery");
    let _ = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192)); // the hover move is its own batch
    assert!(a.synthetic_batch.is_none());
    assert_eq!(a.synthetic.len(), 1, "the zoom event is next");

    let mut raw = raw_input(800.0, 600.0, 8192);
    raw.events.push(host_key(egui::Key::F24, true));
    a.raw_input_hook(&ctx, &mut raw);
    assert!(a.synthetic_batch.as_ref().is_some_and(|b| b.iter().any(|e| matches!(e, egui::Event::Zoom(_)))));
    a.advance_test_clock(Duration::from_secs(16));
    let mut out = ctx.run_ui(raw, |ui| {
        a.logic(ui.ctx());
        a.ui(ui);
    });
    out.textures_delta.clear();

    let canceled = reply.try_recv().expect("one cancellation reply");
    assert_eq!(canceled["ok"], false, "{canceled}");
    assert_eq!(view(&a), before, "the expired pinch must not zoom");
    assert!(ctx.input(|i| i.keys_down.contains(&egui::Key::F24)), "a host key pressed in the same pass survives");
    assert!(
        ctx.input(|i| i.raw.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::F24, pressed: true, .. }))),
        "the host key event is kept in the pass input"
    );
    assert!(!ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Zoom(_)))), "no zoom event reaches widgets or egui's aggregates");
    assert!(!a.cancel_synthetic_scroll);

    let mut release = raw_input(800.0, 600.0, 8192);
    release.events.push(host_key(egui::Key::F24, false));
    let _ = frame(&mut a, &ctx, release);
    assert!(!ctx.input(|i| i.keys_down.contains(&egui::Key::F24)));
    for _ in 0..4 {
        let _ = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192));
    }
    assert_eq!(view(&a), before, "a later frame must not replay the expired pinch");

    let (req, reply) = ControlRequest::new("ui.zoom", json!({"x": at.x, "y": at.y, "factor": 4.0, "steps": 1}));
    assert!(handle(&mut a, &ctx, &req).is_none());
    let _ = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192)); // hover move
    deliver_gestures(&mut a, &ctx);
    assert_eq!(reply.try_recv().unwrap()["ok"], true);
    let after = view(&a);
    assert!((after.0 * 4.0 / before.0 - 1.0).abs() < 1e-4, "a later zoom works once: {} -> {}", before.0, after.0);
    for _ in 0..4 {
        let _ = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192));
    }
    assert!((view(&a).0 - after.0).abs() < 1e-9, "the later zoom is not replayed");
}

/// After a drawn pass canceled an expired wheel, the next real wheel notch is often byte-for-byte
/// identical to the injected one. Retaining that run would make the next hook's fallback strip
/// the genuine host event. The real notch must still scroll.
#[test]
fn a_fresh_identical_host_wheel_after_a_drawn_expiry_is_not_swallowed() {
    let (mut a, ctx) = (app(), egui::Context::default());
    a.advance_test_clock(Duration::ZERO);
    warm(&mut a, &ctx);
    a.run("track.new", json!({"count": 20, "format": "stereo"})).unwrap();
    warm(&mut a, &ctx);
    let at = timeline_center(&a);
    let before = view(&a);

    let (req, reply) = ControlRequest::new("ui.scroll", json!({"x": at.x, "y": at.y, "dy": -30.0, "unit": "line", "steps": 1}));
    assert!(handle(&mut a, &ctx, &req).is_none());
    let _ = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192)); // hover move
    let mut raw = raw_input(800.0, 600.0, 8192);
    a.raw_input_hook(&ctx, &mut raw);
    assert!(a.synthetic_batch.as_ref().is_some_and(|b| b.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. }))));

    a.advance_test_clock(Duration::from_secs(16));
    let mut out = ctx.run_ui(raw, |ui| {
        a.logic(ui.ctx());
        a.ui(ui);
    });
    out.textures_delta.clear();
    assert_eq!(reply.try_recv().expect("one cancellation reply")["ok"], false);
    assert_eq!(view(&a), before, "the expired wheel did not scroll");
    assert!(a.injected_events.is_empty(), "the drawn pass consumed the retained injected run");

    // A genuine host notch with the exact same fields is delivered, not mistaken for that run.
    let mut host = raw_input(800.0, 600.0, 8192);
    host.events.push(host_line_wheel(-30.0));
    let _ = frame(&mut a, &ctx, host);
    for _ in 0..12 {
        let _ = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192));
    }
    assert_ne!(view(&a), before, "the fresh host wheel still scrolls after the expiry");
}

/// The replay snapshot starts from the pre-injection `InputState`, so a key the host held from an
/// earlier pass must remain down even though this pass's raw input has no new key event.
#[test]
fn a_host_key_held_across_passes_survives_a_replayed_scroll_cancel() {
    let (mut a, ctx) = (app(), egui::Context::default());
    a.advance_test_clock(Duration::ZERO);
    warm(&mut a, &ctx);
    a.run("track.new", json!({"count": 20, "format": "stereo"})).unwrap();
    warm(&mut a, &ctx);
    let at = timeline_center(&a);
    let before = view(&a);

    let mut held = raw_input(800.0, 600.0, 8192);
    held.events.push(host_key(egui::Key::F24, true));
    let _ = frame(&mut a, &ctx, held);
    assert!(ctx.input(|i| i.key_down(egui::Key::F24)), "the host key is held before the scroll pass");

    let (req, reply) = ControlRequest::new("ui.scroll", json!({"x": at.x, "y": at.y, "dy": -30.0, "unit": "line", "steps": 1}));
    assert!(handle(&mut a, &ctx, &req).is_none());
    let _ = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192)); // hover move
    let mut raw = raw_input(800.0, 600.0, 8192); // no key event: F24 is still held
    a.raw_input_hook(&ctx, &mut raw);
    assert!(a.synthetic_batch.as_ref().is_some_and(|b| b.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. }))));

    a.advance_test_clock(Duration::from_secs(16));
    let mut out = ctx.run_ui(raw, |ui| {
        a.logic(ui.ctx());
        a.ui(ui);
    });
    out.textures_delta.clear();
    assert_eq!(reply.try_recv().expect("one cancellation reply")["ok"], false);
    assert_eq!(view(&a), before, "the expired wheel did not scroll");
    assert!(ctx.input(|i| i.key_down(egui::Key::F24)), "the held host key survives the replay");

    let mut release = raw_input(800.0, 600.0, 8192);
    release.events.push(host_key(egui::Key::F24, false));
    let _ = frame(&mut a, &ctx, release);
    assert!(!ctx.input(|i| i.key_down(egui::Key::F24)), "the release still applies");
}

/// A scripted wheel that a widget consumed before the acknowledgement leaves the batch pending.
/// If the deadline then crosses in a discarded rerun, replaying the first pass's host raw would
/// dispatch its real Cmd+Z a second time. The later pass must not replay that input.
#[test]
fn an_expiry_in_a_discarded_pass_does_not_replay_host_events() {
    let (mut a, ctx) = (app(), egui::Context::default());
    a.advance_test_clock(Duration::ZERO);
    warm(&mut a, &ctx);
    a.run("track.new", json!({"count": 1, "format": "stereo"})).unwrap();
    warm(&mut a, &ctx);
    let depth = a.engine.undo_history().len();
    assert!(depth > 0, "the test needs an undoable edit");
    let at = timeline_center(&a);

    let (req, reply) = ControlRequest::new("ui.scroll", json!({"x": at.x, "y": at.y, "dy": -30.0, "unit": "line", "steps": 1}));
    assert!(handle(&mut a, &ctx, &req).is_none());
    let _ = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192)); // hover move
    let mut raw = raw_input(800.0, 600.0, 8192);
    let command = egui::Modifiers { command: true, ..egui::Modifiers::NONE };
    raw.events.push(egui::Event::Key { key: egui::Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: command });
    raw.events.push(egui::Event::Key { key: egui::Key::Z, physical_key: None, pressed: false, repeat: false, modifiers: command });
    a.raw_input_hook(&ctx, &mut raw);
    assert!(a.synthetic_batch.as_ref().is_some_and(|b| b.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. }))));

    let mut passes = 0;
    let mut out = ctx.run_ui(raw, |ui| {
        passes += 1;
        if passes == 2 {
            // The deadline crosses only on the rerun, after the first pass ran its host shortcut.
            a.advance_test_clock(Duration::from_secs(16));
        }
        a.logic(ui.ctx());
        if passes == 1 {
            // Another widget consumed the injected wheel from the pass input, so the batch is
            // still pending when the first pass asks for a discard.
            ui.ctx().input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::MouseWheel { .. })));
            ui.ctx().request_discard("discard the pass whose wheel another widget consumed");
        }
        a.ui(ui);
    });
    out.textures_delta.clear();
    assert!(passes >= 2, "the regression must actually force a discarded pass");
    assert_eq!(reply.try_recv().expect("one cancellation reply")["ok"], false);
    assert_eq!(a.engine.undo_history().len(), depth - 1, "the real Cmd+Z ran once, not again on the rerun");
    assert!(a.injected_events.is_empty());
    assert!(a.input_before_scroll.is_none());
    assert!(a.synthetic_batch.is_none());
    assert!(
        !ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. } | egui::Event::Key { key: egui::Key::Z, .. }))),
        "the rerun neither replays the wheel nor the first pass's host key"
    );
}

/// An expired track drag cancels without reordering tracks or dispatching a drop; a fresh drag still drops
/// normally.
#[test]
fn a_timed_out_track_drag_cancels_without_dropping_and_the_next_drag_works() {
    let (mut a, ctx) = (app(), egui::Context::default());
    a.advance_test_clock(Duration::ZERO);
    warm(&mut a, &ctx);
    let before: Vec<_> = a.engine.session().tracks.iter().map(|track| track.id).collect();
    let first = a.edit_layout.rows[0].1;
    let third = a.edit_layout.rows[2].1;
    let grab = pos2(first[0] + 40.0, first[1] + 12.0);
    let release = pos2(grab.x, third[1]);
    let params = json!({"x": grab.x, "y": grab.y, "to_x": release.x, "to_y": release.y, "steps": 2});
    let (req, reply) = ControlRequest::new("ui.drag", params.clone());
    assert!(handle(&mut a, &ctx, &req).is_none());
    for _ in 0..3 {
        // move, press, first drag motion; final motion/release remain queued
        let _ = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192));
    }
    assert!(ctx.input(|i| i.pointer.primary_down()));
    assert!(ctx.dragged_id().is_some(), "the real name plate is being dragged");
    a.advance_test_clock(Duration::from_secs(16));
    let _ = ctx.run_logic(&raw_input(800.0, 600.0, 8192), |ctx| a.logic(ctx));
    assert_eq!(reply.try_recv().unwrap()["ok"], false);
    let _ = frame(&mut a, &ctx, raw_input(800.0, 600.0, 8192));
    assert!(!ctx.input(|i| i.pointer.any_down() || i.pointer.any_released()));
    assert!(ctx.dragged_id().is_none() && ctx.drag_stopped_id().is_none());
    assert!(a.gesture.is_none());
    assert_eq!(a.engine.session().tracks.iter().map(|track| track.id).collect::<Vec<_>>(), before, "canceling must not reorder tracks");

    let (req, reply) = ControlRequest::new("ui.drag", params);
    assert!(handle(&mut a, &ctx, &req).is_none());
    deliver_gestures(&mut a, &ctx);
    assert_eq!(reply.try_recv().unwrap()["ok"], true);
    let mut expected = before;
    expected.swap(0, 1);
    assert_eq!(a.engine.session().tracks.iter().map(|track| track.id).collect::<Vec<_>>(), expected, "a completed drag still drops normally");
    assert!(!ctx.input(|i| i.pointer.any_down()));
}

/// Wheel scrolls use line units and report `wheel`; point scrolls use trackpad phases and report `trackpad`, both
/// only after delivery.
#[test]
fn scroll_reports_the_wheel_and_trackpad_sources_after_delivery() {
    let (mut a, ctx) = (app(), egui::Context::default());
    warm(&mut a, &ctx);
    let (req, reply) = ControlRequest::new("ui.scroll", json!({"x": 10.0, "y": 10.0, "dy": -30.0, "unit": "line", "steps": 2}));
    assert!(handle(&mut a, &ctx, &req).is_none(), "the reply waits for delivery");
    assert!(
        a.synthetic.iter().all(|e| matches!(e, egui::Event::PointerMoved(_) | egui::Event::MouseWheel { source: egui::MouseWheelSource::Wheel, .. }))
    );
    deliver_gestures(&mut a, &ctx);
    let v = reply.try_recv().expect("wheel scroll replies after its events were applied");
    assert_eq!(v["result"]["source"], "wheel");

    let (req, reply) = ControlRequest::new("ui.scroll", json!({"x": 10.0, "y": 10.0, "dy": -30.0, "steps": 2}));
    assert!(handle(&mut a, &ctx, &req).is_none(), "the reply waits for delivery");
    assert!(a.synthetic.iter().any(|e| matches!(e, egui::Event::MouseWheel { source: egui::MouseWheelSource::Trackpad, .. })));
    deliver_gestures(&mut a, &ctx);
    let v = reply.try_recv().expect("trackpad scroll replies after its events were applied");
    assert_eq!(v["result"]["source"], "trackpad");
}
