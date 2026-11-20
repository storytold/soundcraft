//! `window.soundcraft`: the page command API (`craft-web-api`, storytold/craft-libs#413) over the
//! app's own control channel, the one the desktop control server drives (`docs/control-protocol.md`).
//!
//! The crate owns the JavaScript side (`request`, `execute`, `commands`, `docInspect`, `uiInspect`,
//! `screenshot`, as promises with a timeout). This module is SoundCraft's part: a [`Request`] that
//! queues a [`ControlRequest`] for the app, which answers it on its next frame, and waits for the
//! reply. SoundCraft's control request has no abandon state, so a call the page gave up on still runs
//! when the app reaches it, as it does for a desktop client that timed out.

use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError, TrySendError, sync_channel};

use craft_web_api::methods::Methods;
use craft_web_api::page::{PageApi, Request};
use serde_json::{Value, json};
use soundcraft_ui_egui::ControlRequest;
use wasm_bindgen_futures::JsFuture;

/// The app id in `window.<app>`.
const APP: &str = "soundcraft";

/// How often a waiting call looks for the app's reply, in milliseconds (about a frame).
const POLL_MS: i32 = 16;

/// How many control requests may wait for the app at once. `craft-web-api` already refuses calls past its own
/// in-flight limit; this bounds the app's queue too, so a flood of calls can't grow it without limit.
const QUEUE: usize = 64;

/// Put `window.soundcraft` on the page and return the queue the app drains each frame (hand it to
/// `WordApp::with_control`). An existing `window.soundcraft` keeps the functions it already has.
pub fn install(ctx: egui::Context) -> Receiver<ControlRequest> {
    let (queue, requests) = sync_channel::<ControlRequest>(QUEUE);
    let request: Request = Rc::new(move |method, params| {
        let (control, reply) = ControlRequest::new(method, params);
        if let Err(refused) = queue.try_send(control) {
            let error = match refused {
                TrySendError::Full(_) => format!("soundcraft is busy: {QUEUE} requests are already waiting for the app"),
                TrySendError::Disconnected(_) => "the app is not running".to_string(),
            };
            return Box::pin(async move { json!({"ok": false, "error": error}) });
        }
        ctx.request_repaint();
        Box::pin(wait(reply))
    });
    if let Err(e) = Rc::new(PageApi::new(APP, Methods::default(), request)).install() {
        log::warn!("couldn't install window.{APP}: {e:?}");
    }
    requests
}

/// The app's reply, polled about once a frame.
async fn wait(reply: Receiver<Value>) -> Value {
    loop {
        match reply.try_recv() {
            Ok(reply) => return reply,
            Err(TryRecvError::Empty) => {
                if !sleep(POLL_MS).await {
                    return json!({"ok": false, "error": "this page has no timer to wait for the app with"});
                }
            }
            Err(TryRecvError::Disconnected) => return json!({"ok": false, "error": "the app dropped the request"}),
        }
    }
}

async fn sleep(ms: i32) -> bool {
    // `false` when this page has no timer to wait with (no window, or the timer is refused): the caller ends the
    // call with an error instead of going round again at once.
    let timer = std::cell::Cell::new(false);
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let set = web_sys::window().is_some_and(|w| w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms).is_ok());
        timer.set(set);
        if !set {
            let _ = resolve.call0(&wasm_bindgen::JsValue::UNDEFINED);
        }
    });
    let _ = JsFuture::from(promise).await;
    timer.get()
}
