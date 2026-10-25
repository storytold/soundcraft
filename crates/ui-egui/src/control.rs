//! The JSON control channel's UI side. The app's TCP server turns each request line into a
//! [`ControlRequest`]; they are handled on the UI thread here, so every command an agent sends
//! goes through exactly the same code as a click.
//!
//! Methods:
//! - `engine.execute {command, params}` (alias `command`): run any command programmatically (no dialogs)
//! - `engine.commands {filter?}`, `engine.parity`, `session.inspect {detail?}`, `ui.inspect`
//! - `ui.menu.list`, `ui.menu.invoke {path: "Track > New..." | id}` (menu-style: may open dialogs)
//! - `ui.click {x,y,button?,count?,mods?}`, `ui.move {x,y}`, `ui.drag {x,y,to_x,to_y,steps?}`
//! - `ui.scroll {x,y,dx?,dy?,unit?,steps?,mods?}` (trackpad swipe or wheel), `ui.zoom {x,y,factor,steps?}` (pinch)
//! - `ui.key {key, cmd?, shift?, alt?, ctrl?}`, `ui.text {text}`
//! - `ui.screenshot {path?}` (PNG; base64 when no path), `ui.set {...UiState fields}`
//! - `app.quit`
//!
//! Scripted-input methods reply only after a UI pass applied their events, and later requests wait
//! behind that gesture. The connection's one-request-at-a-time reply order therefore also orders an
//! inspect, command or screenshot after the gesture it follows.

use crate::SoundApp;
use craft_ui::capture::CaptureSlot;
use serde_json::{Value, json};
use std::sync::mpsc::{Receiver, Sender, channel};
use web_time::Instant;

pub struct ControlRequest {
    pub method: String,
    pub params: Value,
    pub reply: Sender<Value>,
}

impl ControlRequest {
    pub fn new(method: impl Into<String>, params: Value) -> (Self, Receiver<Value>) {
        let (tx, rx) = channel();
        (ControlRequest { method: method.into(), params, reply: tx }, rx)
    }
}

/// One admitted `ui.screenshot` request. The app owns the strong completion slot; the callback the
/// renderer receives holds only a weak reference to it (see `craft_ui::capture`).
pub(crate) struct PendingShot {
    token: u64,
    /// Taken once the single reply (success, timeout, cancellation or failure) was sent.
    reply: Option<Sender<Value>>,
    path: Option<String>,
    /// Geometry the request was admitted with; a change before completion cancels and retries.
    geometry: CaptureGeometry,
    /// Session identity at admission. A different document never receives these pixels.
    document: u64,
    deadline: Instant,
    retries: u32,
    /// Bytes the captured RGBA image may occupy while it is pending or completed but undrained.
    image_bytes: usize,
    /// Worst-case encode peak reserved for this request (image, PNG buffer and base64 string).
    reserved_bytes: usize,
    slot: Option<CaptureSlot>,
    issued: bool,
}

impl PendingShot {
    /// Send the single reply if it has not been taken; later completions are ignored.
    fn reply(&mut self, v: Value) {
        if let Some(tx) = self.reply.take() {
            let _ = tx.send(v);
        }
    }

    /// Release the capture slot and send one error reply.
    fn fail(&mut self, e: impl std::fmt::Display) {
        self.slot = None;
        self.reply(err(e));
    }
}

/// The viewport geometry a screenshot is bound to. Captured from the host's raw input for every
/// host frame (including logic-only ticks), so a hidden resize is seen without a stale UI input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CaptureGeometry {
    pub points: egui::Vec2,
    pub pixels_per_point: f32,
    pub max_texture_side: usize,
    pub focused: bool,
}

impl CaptureGeometry {
    /// Geometry from a host raw-input frame, or `None` when the viewport has no finite, positive size. A missing
    /// texture limit falls back to the last one egui saw instead of inventing a smaller cap.
    pub(crate) fn from_raw(ctx: &egui::Context, raw: &egui::RawInput) -> Option<Self> {
        let viewport = raw.viewports.get(&raw.viewport_id);
        let points = viewport.and_then(|v| v.inner_rect).or(raw.screen_rect)?.size();
        if !(points.x.is_finite() && points.y.is_finite()) || points.x <= 0.0 || points.y <= 0.0 {
            return None;
        }
        let native = viewport.and_then(|v| v.native_pixels_per_point).unwrap_or(1.0);
        Some(Self {
            points,
            pixels_per_point: (native * ctx.zoom_factor()).max(0.01),
            // A host that omits the limit still has it in the last input egui interpreted; using the
            // context value avoids inventing a 2048 cap that would reject a larger valid surface.
            max_texture_side: raw.max_texture_side.unwrap_or_else(|| ctx.input(|i| i.max_texture_side)),
            focused: raw.focused,
        })
    }

    /// Geometry from the context's current viewport, used outside a raw-input frame with the same scaling and
    /// texture-limit rules.
    pub(crate) fn from_context(ctx: &egui::Context) -> Self {
        let viewport = ctx.input(|i| i.viewport().clone());
        let points = viewport.inner_rect.map_or_else(|| ctx.viewport_rect().size(), |r| r.size());
        let native = viewport.native_pixels_per_point.unwrap_or(1.0);
        Self {
            points,
            pixels_per_point: (native * ctx.zoom_factor()).max(0.01),
            max_texture_side: ctx.input(|i| i.max_texture_side),
            focused: ctx.input(|i| i.focused),
        }
    }

    /// The physical pixel size the capture is expected to have: the viewport points at the current
    /// scale, clamped by the renderer's texture limit, with checked arithmetic.
    fn expected_size(self) -> Option<[usize; 2]> {
        let side = self.max_texture_side.max(1);
        let axis = |points: f32| -> Option<usize> {
            let px = (points * self.pixels_per_point).round();
            if !px.is_finite() || px <= 0.0 {
                return None;
            }
            Some((px as usize).min(side))
        };
        Some([axis(self.points.x)?, axis(self.points.y)?])
    }

    /// Bytes for the captured RGBA image at the expected size, with checked arithmetic.
    fn reserved_bytes(self) -> Option<usize> {
        let [w, h] = self.expected_size()?;
        w.checked_mul(h)?.checked_mul(std::mem::size_of::<egui::Color32>())
    }

    /// The worst-case memory needed to encode this image, not just to hold its pixels.
    ///
    /// Encoding builds a raw RGBA copy and a PNG buffer while the captured image is still alive;
    /// the base64 reply form then keeps the PNG and builds its 4/3-size text. Reserving only the
    /// image made the documented aggregate a retained-image budget rather than a peak bound.
    fn encode_peak_bytes(self, base64_reply: bool) -> Option<usize> {
        let image = self.reserved_bytes()?;
        let [_, height] = self.expected_size()?;
        // PNG adds one filter byte per row and chunk/frame overhead; this also allows for a
        // worst-case incompressible deflate stream (stored blocks are far below 1/16 expansion).
        let png = image.checked_add(image / 16)?.checked_add(height)?.checked_add(4096)?;
        let png_phase = image.checked_add(image)?.checked_add(png)?;
        if base64_reply {
            let b64 = png.div_ceil(3).checked_mul(4)?;
            let b64_phase = image.checked_add(png)?.checked_add(b64)?;
            Some(png_phase.max(b64_phase))
        } else {
            Some(png_phase)
        }
    }

    /// The most bytes an image [`check_image`] accepts for this geometry: one pixel larger on each
    /// axis, the same tolerance the dimension check uses. Reserving the exact size but rejecting an
    /// image the staleness rule accepts would fail a legitimate DPI/surface rounding by one pixel.
    fn tolerated_bytes(self) -> Option<usize> {
        let [w, h] = self.expected_size()?;
        w.checked_add(1)?.checked_mul(h.checked_add(1)?)?.checked_mul(std::mem::size_of::<egui::Color32>())
    }
}

/// Why a completed image is rejected. A stale image, or one whose dimensions no longer match the
/// current viewport, is retried; a malformed image for the current geometry is reported.
enum ImageProblem {
    Stale,
    Malformed(String),
}

/// The smallest uniform scale a returned image may have relative to the capture-time viewport
/// before it is treated as truncated rather than a legitimately reduced `surface_fit` surface.
/// The expected size is already clamped by `max_texture_side`, so a real surface stays near it;
/// this only rejects a partial read (a 1x1 or 2x1 image of a normal window is far below it).
const MIN_CAPTURE_SCALE: f32 = 0.1;

/// Whether the returned pixels belong to this request's logical viewport. `surface_fit` may render a
/// smaller surface than the window, so a uniformly reduced image is accepted: the effective scale
/// is derived from the image and the capture-time logical size instead of requiring an exact match.
fn check_image(img: &egui::ColorImage, expected: [usize; 2]) -> Result<(), ImageProblem> {
    let [w, h] = img.size;
    if w == 0 || h == 0 {
        return Err(ImageProblem::Malformed("screenshot has zero size".into()));
    }
    let pixels = w.checked_mul(h).ok_or_else(|| ImageProblem::Malformed("screenshot dimensions overflow".into()))?;
    if img.pixels.len() != pixels {
        return Err(ImageProblem::Malformed("screenshot pixel count does not match its dimensions".into()));
    }
    let (ew, eh) = (expected[0].max(1), expected[1].max(1));
    if w > ew + 1 || h > eh + 1 {
        return Err(ImageProblem::Stale);
    }
    let scale_x = w as f32 / ew as f32;
    let scale_y = h as f32 / eh as f32;
    if (scale_x - scale_y).abs() > 0.02 {
        return Err(ImageProblem::Stale);
    }
    // A 1x1 or otherwise truncated read has a plausible aspect ratio but is not a screenshot of the
    // viewport; it must not be written and replied as a successful capture.
    if scale_x < MIN_CAPTURE_SCALE || scale_y < MIN_CAPTURE_SCALE {
        return Err(ImageProblem::Malformed(format!("screenshot is much smaller than its {ew}x{eh} viewport")));
    }
    Ok(())
}

/// Re-reserve a changed window's image size, rejecting a growth past the per-image or aggregate
/// bound. `others` is the bytes reserved by the other outstanding requests.
fn rebudget(shot: &mut PendingShot, geometry: CaptureGeometry, others: usize) -> Result<(), String> {
    let Some(image_bytes) = geometry.reserved_bytes() else {
        return Err("screenshot unavailable: the window has no capturable size yet".into());
    };
    if image_bytes > MAX_SHOT_BYTES {
        return Err(format!("screenshot too large: {image_bytes} bytes over the {MAX_SHOT_BYTES}-byte per-image limit"));
    }
    let Some(bytes) = geometry.encode_peak_bytes(shot.path.is_none()) else {
        return Err("screenshot unavailable: the window has no capturable size yet".into());
    };
    let total = others.saturating_add(bytes);
    if total > MAX_SHOT_TOTAL_BYTES {
        return Err(format!("screenshot too large: {total} bytes of encoding peak over the {MAX_SHOT_TOTAL_BYTES}-byte budget"));
    }
    shot.image_bytes = image_bytes;
    shot.reserved_bytes = bytes;
    Ok(())
}

const MAX_PENDING_SHOTS: usize = 8;
const MAX_IN_FLIGHT_SHOTS: usize = 4;
const MAX_SHOT_BYTES: usize = 64 << 20;
const MAX_SHOT_TOTAL_BYTES: usize = 192 << 20;
/// A scripted gesture gets this long to reach a UI pass before its request chain is canceled.
const GESTURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const MAX_SHOT_RETRIES: u32 = 2;
const SHOT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
/// The most scripted events one client may leave waiting for the UI. A drag or scroll request is
/// at most a few hundred events (its `steps` are capped), so this is far above a real gesture and
/// still keeps a client that outruns the UI from growing the queue without bound.
const MAX_SYNTHETIC_EVENTS: usize = 4096;
/// The most text one `ui.text` request may type. Input-derived strings are otherwise bounded only
/// by the transport's ~96 MiB request-line limit.
const MAX_CONTROL_TEXT_BYTES: usize = 64 << 10;
/// The most bytes a screenshot output path may hold; a path is copied into the pending request.
const MAX_SHOT_PATH_BYTES: usize = 4096;

/// Queue scripted input, refusing a request that would grow the pending event queue past
/// [`MAX_SYNTHETIC_EVENTS`]. Each request is small; the cap bounds a client that sends many.
fn queue_synthetic(app: &mut SoundApp, reply: Sender<Value>, response: Value, events: impl IntoIterator<Item = egui::Event>) -> Result<(), String> {
    let events: Vec<egui::Event> = events.into_iter().collect();
    if app.synthetic.len().saturating_add(events.len()) > MAX_SYNTHETIC_EVENTS {
        return Err(format!("synthetic input queue full: at most {MAX_SYNTHETIC_EVENTS} events may wait"));
    }
    app.gesture_acks.push_back(GestureAck { remaining: events.len(), deadline: app.clock_now() + GESTURE_TIMEOUT, reply: Some(reply), response });
    app.synthetic.extend(events);
    Ok(())
}

/// Queue a gesture and return no immediate reply: [`synthetic_delivered`] sends the result once
/// a UI pass has applied every event. Only a queue-bound refusal replies here.
fn queue_reply(app: &mut SoundApp, req: &ControlRequest, events: Vec<egui::Event>, response: Value) -> Option<Value> {
    match queue_synthetic(app, req.reply.clone(), ok(response), events) {
        Ok(()) => None,
        Err(e) => Some(err(e)),
    }
}

/// A gesture's reply is held until a UI pass has applied all of its events. This is the ordering
/// barrier for the control channel: the connection thread sends one request at a time, so nothing
/// that follows a click/key/text (inspect, engine command or screenshot) can be admitted before
/// the gesture changed the UI.
pub(crate) struct GestureAck {
    remaining: usize,
    deadline: Instant,
    reply: Option<Sender<Value>>,
    response: Value,
}

/// True while a scripted gesture has events waiting or an applied batch still needs acknowledging.
pub(crate) fn gesture_pending(app: &SoundApp) -> bool {
    !app.gesture_acks.is_empty() || !app.synthetic.is_empty() || app.synthetic_batch.is_some()
}

/// Count one delivered batch against the front gesture. Batches never straddle requests (a key
/// press/release pair is taken together), so the first unfinished gesture owns it.
pub(crate) fn synthetic_delivered(app: &mut SoundApp, delivered: usize) {
    let mut left = delivered;
    while left > 0 {
        let Some(front) = app.gesture_acks.front_mut() else { break };
        let take = front.remaining.min(left);
        front.remaining -= take;
        left -= take;
        if front.remaining == 0 {
            if let Some(mut ack) = app.gesture_acks.pop_front()
                && let Some(tx) = ack.reply.take()
            {
                let _ = tx.send(ack.response);
            }
        } else {
            break;
        }
    }
}

/// A gesture that cannot reach a UI pass (a host that never paints) must not wedge every later
/// control request. Cancel the whole pending gesture chain: FIFO means no later request can run
/// while an earlier undelivered click is still waiting.
pub(crate) fn expire_gestures(app: &mut SoundApp) {
    let now = app.clock_now();
    if !app.gesture_acks.iter().any(|ack| now >= ack.deadline) {
        return;
    }
    // A batch that the hook already appended to a pass may still be sitting in that pass's
    // `InputState`. Mark it so `ui` drops it before shortcuts or widgets dispatch it.
    if !app.injected_events.is_empty() {
        let scroll = app
            .synthetic_batch
            .as_ref()
            .is_some_and(|batch| batch.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. } | egui::Event::Zoom(_))));
        let keys = app.synthetic_batch.as_ref().is_some_and(|batch| {
            batch
                .iter()
                .any(|e| matches!(e, egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_)))
        });
        if scroll {
            // Removing a wheel/zoom event alone would leave egui's per-pass aggregates and its
            // frame-to-frame wheel smoothing carrying the expired gesture. `ui` replays the host
            // input from the pre-injection state instead.
            app.cancel_synthetic_scroll = app.input_before_scroll.is_some();
        } else if keys {
            app.cancel_synthetic_keys = true;
        }
    }
    app.synthetic.clear();
    app.synthetic_batch = None;
    if std::mem::take(&mut app.synthetic_pointer_active) {
        // A consumed press cannot just lose its queued release: egui would remain down. A
        // synthetic release is also wrong (it may click/drop), so cancel before the next input
        // pass. Drop app-owned drag previews now, before drive_drag can commit one on !any_down.
        app.cancel_synthetic_pointer = true;
        app.pointer_down = false;
        app.gesture = None;
    }
    for mut ack in std::mem::take(&mut app.gesture_acks) {
        if let Some(tx) = ack.reply.take() {
            let _ = tx.send(err("synthetic input was not delivered"));
        }
    }
}

fn ok(v: Value) -> Value {
    json!({"ok": true, "result": v})
}

fn err(e: impl std::fmt::Display) -> Value {
    json!({"ok": false, "error": e.to_string()})
}

/// Run at most 65 queued control requests on the UI thread, stopping while a scripted gesture is undelivered so later
/// requests observe its result. A panicking handler still gets one error reply.
pub fn drain(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(rx) = app.control_rx.take() else { return };
    let mut n = 0;
    loop {
        // Do not let a request behind an undelivered gesture observe or mutate the state the
        // gesture is about to change. The request stays in the bounded transport queue.
        if gesture_pending(app) {
            break;
        }
        let Ok(req) = rx.try_recv() else { break };
        n += 1;
        // A handler that panics must not take the receiver with it: the UI pass catches the panic
        // one level up, and a dead control channel afterwards would be a second failure.
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handle(app, ctx, &req))) {
            Ok(Some(v)) => {
                let _ = req.reply.send(v);
            }
            Ok(None) => {}
            Err(_) => {
                let _ = req.reply.send(err("control request failed"));
            }
        }
        if n > 64 {
            break;
        }
    }
    app.control_rx = Some(rx);
    if n > 0 {
        ctx.request_repaint();
    }
}

fn key_from(name: &str) -> Option<egui::Key> {
    egui::Key::from_name(name).or(match name.to_ascii_lowercase().as_str() {
        "space" => Some(egui::Key::Space),
        "enter" | "return" => Some(egui::Key::Enter),
        "escape" | "esc" => Some(egui::Key::Escape),
        "tab" => Some(egui::Key::Tab),
        "home" => Some(egui::Key::Home),
        "end" => Some(egui::Key::End),
        "=" => Some(egui::Key::Equals),
        _ => None,
    })
}

fn mods(p: &Value) -> egui::Modifiers {
    let b = |k: &str| p.get(k).and_then(Value::as_bool).unwrap_or(false);
    let cmd = b("cmd") || b("command");
    egui::Modifiers {
        alt: b("alt"),
        ctrl: b("ctrl") || (cmd && !cfg!(target_os = "macos")),
        shift: b("shift"),
        mac_cmd: cmd && cfg!(target_os = "macos"),
        command: cmd,
    }
}

/// List engine commands plus UI commands, applying the same case-insensitive id/label filter to both.
fn command_list(app: &mut SoundApp, p: &Value) -> Value {
    let mut list = match app.engine.execute("engine.commands", p) {
        Ok(v) => v,
        Err(e) => return err(e),
    };
    // Same predicate as the engine's `engine.commands`: id or label substring, ASCII case-insensitive.
    let filter = p.get("filter").and_then(Value::as_str).map(str::to_ascii_lowercase);
    let matches =
        |id: &str, label: &str| filter.as_deref().is_none_or(|f| id.to_ascii_lowercase().contains(f) || label.to_ascii_lowercase().contains(f));
    if let Some(arr) = list.as_array_mut() {
        for (id, label, path, sc) in crate::menus::UI_COMMANDS.iter().filter(|c| matches(c.0, c.1)) {
            arr.push(json!({"id": id, "label": label, "menu": path, "shortcut": sc, "enabled": true, "params": "{}"}));
        }
    }
    ok(list)
}

/// Dispatch one control method to commands, inspection, menus or queued scripted input. Methods that need a UI pass
/// return `None` and reply later; everything else returns the reply for [`drain`] to send.
fn handle(app: &mut SoundApp, ctx: &egui::Context, req: &ControlRequest) -> Option<Value> {
    let p = &req.params;
    let f = |k: &str| p.get(k).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    let pos = egui::pos2(f("x"), f("y"));
    let steps = p.get("steps").and_then(Value::as_u64).unwrap_or(8).clamp(1, 200);
    Some(match req.method.as_str() {
        "engine.execute" | "command" => {
            let id = p.get("command").or_else(|| p.get("id")).and_then(Value::as_str).unwrap_or("");
            let params = p.get("params").cloned().unwrap_or(json!({}));
            match app.run(id, params) {
                Ok(v) => ok(v),
                Err(e) => err(e),
            }
        }
        "engine.commands" => command_list(app, p),
        "engine.parity" => ok(crate::menus::parity()),
        "session.inspect" | "document.inspect" => {
            ok(soundcraft_engine::inspect::session(&app.engine, p.get("detail").and_then(Value::as_str) == Some("full")))
        }
        "ui.inspect" => {
            let mut v = app.inspect(ctx);
            v["edit_layout"] = json!(app.edit_layout);
            ok(v)
        }
        "ui.menu.list" => ok(json!(
            soundcraft_engine::catalog::catalog()
                .iter()
                .map(|e| json!({"path": e, "command": soundcraft_engine::catalog::implemented_by(e, &crate::menus::ui_aliases())}))
                .collect::<Vec<_>>()
        )),
        "ui.menu.invoke" => {
            let path = p.get("path").and_then(Value::as_str).unwrap_or("");
            let id = p
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| soundcraft_engine::catalog::implemented_by(path, &crate::menus::ui_aliases()));
            match id {
                Some(id) => {
                    crate::menus::invoke_menu(app, &id, path);
                    ok(json!({"invoked": id, "dialog": app.dialogs.open_name()}))
                }
                None => err(format!("menu item `{path}` is not implemented yet")),
            }
        }
        "ui.set" => match app.run("ui.set", p.clone()) {
            Ok(v) => ok(v),
            Err(e) => err(e),
        },
        "ui.move" => {
            let events = vec![egui::Event::PointerMoved(pos)];
            return queue_reply(app, req, events, json!({}));
        }
        "ui.click" => {
            let button = match p.get("button").and_then(Value::as_str) {
                Some("right") | Some("secondary") => egui::PointerButton::Secondary,
                _ => egui::PointerButton::Primary,
            };
            let m = mods(p);
            let count = p.get("count").and_then(Value::as_u64).unwrap_or(1).clamp(1, 3);
            let mut events = vec![egui::Event::PointerMoved(pos)];
            for _ in 0..count {
                events.push(egui::Event::PointerButton { pos, button, pressed: true, modifiers: m });
                events.push(egui::Event::PointerButton { pos, button, pressed: false, modifiers: m });
            }
            return queue_reply(app, req, events, json!({}));
        }
        "ui.drag" => {
            let (a, b) = (pos, egui::pos2(f("to_x"), f("to_y")));
            let m = mods(p);
            let mut events = vec![
                egui::Event::PointerMoved(a),
                egui::Event::PointerButton { pos: a, button: egui::PointerButton::Primary, pressed: true, modifiers: m },
            ];
            for i in 1..=steps {
                let k = i as f32 / steps as f32;
                events.push(egui::Event::PointerMoved(a + (b - a) * k));
            }
            events.push(egui::Event::PointerButton { pos: b, button: egui::PointerButton::Primary, pressed: false, modifiers: m });
            return queue_reply(app, req, events, json!({}));
        }
        "ui.scroll" => {
            let line = p.get("unit").and_then(Value::as_str) == Some("line");
            let unit = if line { egui::MouseWheelUnit::Line } else { egui::MouseWheelUnit::Point };
            let d = |k: &str| f(k).clamp(-100_000.0, 100_000.0) / steps as f32;
            let (delta, modifiers) = (egui::vec2(d("dx"), d("dy")), mods(p));
            let source = if line { egui::MouseWheelSource::Wheel } else { egui::MouseWheelSource::Trackpad };
            let wheel = |phase, delta| egui::Event::MouseWheel { unit, delta, phase, source, modifiers };
            let mut events = vec![egui::Event::PointerMoved(pos)];
            events.extend((!line).then(|| wheel(egui::TouchPhase::Start, egui::Vec2::ZERO)));
            events.extend((0..steps).map(|_| wheel(egui::TouchPhase::Move, delta)));
            events.extend((!line).then(|| wheel(egui::TouchPhase::End, egui::Vec2::ZERO)));
            let source = if line { "wheel" } else { "trackpad" };
            return queue_reply(app, req, events, json!({"source": source}));
        }
        "ui.zoom" => {
            let Some(factor) = p.get("factor").and_then(Value::as_f64).filter(|v| *v > 0.0) else {
                return Some(err("ui.zoom needs a positive factor (> 1 zooms in)"));
            };
            let each = factor.clamp(1e-3, 1e3).powf(1.0 / steps as f64) as f32;
            let mut events = vec![egui::Event::PointerMoved(pos)];
            events.extend((0..steps).map(|_| egui::Event::Zoom(each)));
            return queue_reply(app, req, events, json!({}));
        }
        "ui.key" => {
            let name = p.get("key").and_then(Value::as_str).unwrap_or("");
            let Some(key) = key_from(name) else { return Some(err(format!("unknown key `{name}`"))) };
            let m = mods(p);
            let events = vec![
                egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: m },
                egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: m },
            ];
            return queue_reply(app, req, events, json!({}));
        }
        "ui.text" => {
            let t = p.get("text").and_then(Value::as_str).unwrap_or("");
            if t.len() > MAX_CONTROL_TEXT_BYTES {
                return Some(err(format!("ui.text is limited to {MAX_CONTROL_TEXT_BYTES} bytes")));
            }
            let events = vec![egui::Event::Text(t.to_string())];
            return queue_reply(app, req, events, json!({}));
        }
        "ui.screenshot" => match admit(app, ctx, req) {
            Ok(()) => return None,
            Err(e) => err(e),
        },
        "app.quit" => {
            app.quit_requested = true;
            ok(json!({}))
        }
        other => err(format!("unknown method `{other}`")),
    })
}

/// Admit a `ui.screenshot` request with a checked byte reservation.
///
/// The reply stays pending until [`drain_screenshots`] reports success, timeout, cancellation or a
/// failure. Admission is rejected before a capture is issued when the queue, the per-image bound or
/// the aggregate bound would be exceeded; a rejected request writes no file.
fn admit(app: &mut SoundApp, ctx: &egui::Context, req: &ControlRequest) -> Result<(), String> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    if app.pending_shots.len() >= MAX_PENDING_SHOTS {
        return Err("screenshot queue full".into());
    }
    let geometry = app.capture_geometry(ctx);
    let Some(image_bytes) = geometry.reserved_bytes() else {
        return Err("screenshot unavailable: the window has no capturable size yet".into());
    };
    if image_bytes > MAX_SHOT_BYTES {
        return Err(format!("screenshot too large: {image_bytes} bytes over the {MAX_SHOT_BYTES}-byte per-image limit"));
    }
    let path = req.params.get("path").and_then(Value::as_str);
    let Some(bytes) = geometry.encode_peak_bytes(path.is_none()) else {
        return Err("screenshot unavailable: the window has no capturable size yet".into());
    };
    let reserved: usize = app.pending_shots.iter().map(|s| s.reserved_bytes).sum();
    let total = reserved.saturating_add(bytes);
    if total > MAX_SHOT_TOTAL_BYTES {
        return Err(format!("screenshot too large: {total} bytes of encoding peak over the {MAX_SHOT_TOTAL_BYTES}-byte budget"));
    }
    if path.is_some_and(|p| p.len() > MAX_SHOT_PATH_BYTES) {
        return Err(format!("screenshot path longer than {MAX_SHOT_PATH_BYTES} bytes"));
    }
    let token = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    app.pending_shots.push(PendingShot {
        token,
        reply: Some(req.reply.clone()),
        path: path.map(str::to_string),
        geometry,
        document: app.doc_epoch,
        deadline: app.clock_now() + SHOT_TIMEOUT,
        retries: 0,
        image_bytes,
        reserved_bytes: bytes,
        slot: None,
        issued: false,
    });
    ctx.request_repaint();
    Ok(())
}

/// Issue admitted screenshots from a known UI pass: after layout, with fonts ready and only when the
/// pass will not be discarded. A request is bound to the geometry and document current at issuance;
/// one-shot callbacks go to the renderer, while the app keeps the strong completion slot.
pub fn issue_screenshots(app: &mut SoundApp, ctx: &egui::Context) {
    if app.pending_shots.is_empty() {
        return;
    }
    if ctx.will_discard() {
        ctx.request_repaint();
        return;
    }
    let now = app.clock_now();
    let geometry = app.capture_geometry(ctx);
    let document = app.doc_epoch;
    let mut in_flight = app.pending_shots.iter().filter(|s| s.issued).count();
    let mut remaining: usize = app.pending_shots.iter().map(|s| s.reserved_bytes).sum();
    let mut keep = Vec::with_capacity(app.pending_shots.len());
    for mut shot in std::mem::take(&mut app.pending_shots) {
        remaining = remaining.saturating_sub(shot.reserved_bytes);
        if shot.issued {
            keep.push(shot);
            continue;
        }
        if now >= shot.deadline {
            shot.fail("screenshot timed out");
            continue;
        }
        if shot.document != document {
            shot.fail("screenshot canceled: the session changed");
            continue;
        }
        if geometry.focused != shot.geometry.focused {
            shot.fail("screenshot canceled: focus changed");
            continue;
        }
        if in_flight >= MAX_IN_FLIGHT_SHOTS {
            keep.push(shot);
            continue;
        }
        if geometry != shot.geometry {
            // The window changed since admission. The retry is counted here and again only if the
            // completed image still belongs to different geometry.
            shot.retries = shot.retries.saturating_add(1);
            if shot.retries > MAX_SHOT_RETRIES {
                shot.fail("screenshot stale: the window kept changing");
                continue;
            }
            let others = keep.iter().map(|s: &PendingShot| s.reserved_bytes).sum::<usize>() + remaining;
            if let Err(e) = rebudget(&mut shot, geometry, others) {
                shot.fail(e);
                continue;
            }
            shot.geometry = geometry;
        }
        // Bound the retained image by this request's own reservation (plus the same one-pixel
        // tolerance the dimension check allows), not by the 64 MiB per-image ceiling: four small
        // requests must not each retain a full-size image and exceed the aggregate budget.
        let image_bound = shot.geometry.tolerated_bytes().unwrap_or(shot.image_bytes).min(MAX_SHOT_BYTES);
        let (slot, callback) = CaptureSlot::new(ctx, ctx.viewport_id(), image_bound);
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(callback));
        shot.slot = Some(slot);
        shot.issued = true;
        in_flight += 1;
        keep.push(shot);
    }
    app.pending_shots = keep;
    ctx.request_repaint();
}

/// Drain completed, cancelled, stale or timed-out requests. Runs in `logic`; it never writes a file
/// while holding a completion-slot lock and always sends exactly one reply per request.
pub fn drain_screenshots(app: &mut SoundApp, ctx: &egui::Context) {
    if app.pending_shots.is_empty() {
        return;
    }
    let now = app.clock_now();
    let geometry = app.capture_geometry(ctx);
    let document = app.doc_epoch;
    let mut remaining: usize = app.pending_shots.iter().map(|s| s.reserved_bytes).sum();
    let mut keep = Vec::with_capacity(app.pending_shots.len());
    for mut shot in std::mem::take(&mut app.pending_shots) {
        remaining = remaining.saturating_sub(shot.reserved_bytes);
        // The deadline is checked before any completed image is taken or written: a callback that
        // arrives too late must not create a file or a second reply.
        if now >= shot.deadline {
            shot.fail("screenshot timed out");
            continue;
        }
        if shot.document != document {
            shot.fail("screenshot canceled: the session changed");
            continue;
        }
        if geometry.focused != shot.geometry.focused {
            shot.fail("screenshot canceled: focus changed");
            continue;
        }
        let Some(slot) = shot.slot.take() else {
            keep.push(shot);
            continue;
        };
        let Some(result) = slot.take() else {
            shot.slot = Some(slot);
            keep.push(shot);
            continue;
        };
        let image = match result {
            Ok(image) => image,
            Err(e) => {
                shot.fail(e);
                continue;
            }
        };
        // The image is validated against the geometry that is current *now*, not the geometry it
        // was requested for. An image can be perfectly well-formed for the size at issuance and
        // still be stale; that is why the retry decision comes before writing or replying.
        let stale = geometry != shot.geometry || shot.geometry.expected_size().is_none();
        let expected = geometry.expected_size().unwrap_or([1, 1]);
        // The slot already enforced this issuance-time bound; check it again so a future slot
        // change cannot bypass it. A changed window is stale and retried before this matters.
        let tolerance = shot.geometry.tolerated_bytes().unwrap_or(shot.image_bytes).min(MAX_SHOT_BYTES);
        let image_bytes = image.size[0].checked_mul(image.size[1]).and_then(|p| p.checked_mul(std::mem::size_of::<egui::Color32>()));
        let problem = match image_bytes {
            None => Some(ImageProblem::Malformed("screenshot dimensions overflow".into())),
            Some(b) if b > tolerance => Some(ImageProblem::Malformed("screenshot too large: the image exceeded its reservation".into())),
            Some(_) => check_image(&image, expected).err(),
        };
        // An image rendered for earlier geometry is retried whatever its dimensions: a window that
        // grew can make an old image look truncated, and one that shrank can make it look oversized.
        // Only a bad image for the current geometry is a permanent failure.
        if stale || matches!(&problem, Some(ImageProblem::Stale)) {
            if shot.retries >= MAX_SHOT_RETRIES {
                shot.fail("screenshot stale: the window changed while it was captured");
            } else {
                let others = keep.iter().map(|s: &PendingShot| s.reserved_bytes).sum::<usize>() + remaining;
                match rebudget(&mut shot, geometry, others) {
                    Ok(()) => {
                        shot.retries = shot.retries.saturating_add(1);
                        shot.issued = false;
                        shot.geometry = geometry;
                        keep.push(shot);
                    }
                    Err(e) => shot.fail(e),
                }
            }
            continue;
        }
        if let Some(ImageProblem::Malformed(why)) = problem {
            shot.fail(why);
            continue;
        }
        shot.reply(encode(&image, shot.path.as_deref(), shot.token));
    }
    app.pending_shots = keep;
    if !app.pending_shots.is_empty() {
        ctx.request_repaint();
    }
}

/// Whether a hidden or logic-only tick must ask the host for a UI paint so an admitted capture can
/// be issued (or a settling one collected by a later pass).
pub fn screenshots_need_paint(app: &SoundApp) -> bool {
    app.pending_shots.iter().any(|s| !s.issued)
}

/// Requests retained by the app, for tests and diagnostics.
pub fn pending_shot_count(app: &SoundApp) -> usize {
    app.pending_shots.len()
}

/// Encode a color image as PNG, rejecting an inconsistent or overflowing pixel count and reserving the raw buffer
/// fallibly.
pub fn png_bytes(img: &egui::ColorImage) -> Result<Vec<u8>, String> {
    let [w, h] = img.size;
    let pixels = w.checked_mul(h).ok_or("image dimensions overflow")?;
    let len = pixels.checked_mul(4).ok_or("image dimensions overflow")?;
    if img.pixels.len() != pixels {
        return Err("bad image size".into());
    }
    let mut raw = Vec::new();
    raw.try_reserve_exact(len).map_err(|e| format!("cannot allocate {len} bytes for the screenshot: {e}"))?;
    for c in &img.pixels {
        raw.extend_from_slice(&c.to_array());
    }
    let buf = image::RgbaImage::from_raw(u32::try_from(w).map_err(|e| e.to_string())?, u32::try_from(h).map_err(|e| e.to_string())?, raw)
        .ok_or("bad image size")?;
    let mut out = std::io::Cursor::new(Vec::new());
    buf.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// Encode a completed screenshot, writing it to `path` or returning base64, and report the image size and request
/// token.
fn encode(img: &egui::ColorImage, path: Option<&str>, token: u64) -> Value {
    let bytes = match png_bytes(img) {
        Ok(b) => b,
        Err(e) => return err(e),
    };
    match path {
        Some(p) => match std::fs::write(p, &bytes) {
            Ok(()) => ok(json!({"request": token, "path": p, "width": img.size[0], "height": img.size[1]})),
            Err(e) => err(format!("{p}: {e}")),
        },
        None => ok(json!({"request": token, "png_base64": base64(&bytes), "width": img.size[0], "height": img.size[1]})),
    }
}

/// Minimal base64 (standard alphabet) to avoid another dependency.
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                let idx = ((n >> (18 - 6 * i)) & 63) as usize;
                out.push(char::from(T.get(idx).copied().unwrap_or(b'A')));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
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
            a.synthetic
                .iter()
                .all(|e| matches!(e, egui::Event::PointerMoved(_) | egui::Event::MouseWheel { source: egui::MouseWheelSource::Wheel, .. }))
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
}
