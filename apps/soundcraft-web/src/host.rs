//! The `postMessage` bridge to a same-origin parent page (`?host=parent`), for pages that embed
//! SoundCraft in an `<iframe>` (such as a file manager). It follows GridCraft's and WordCraft's:
//!
//! - `soundcraft:ready` (to the parent): `{ version }`, the app is listening; send a file now.
//! - `soundcraft:open` (from the parent): `{ name, bytes }`, bytes an `ArrayBuffer` or
//!   `Uint8Array`. It is handled like a file dropped on the window: a session (`.scraftz` bundle
//!   or bare `.scraft`) opens, MIDI and audio import.
//! - `soundcraft:save` (to the parent): `{ name, bytes }` (a transferred `ArrayBuffer`) for every
//!   file SoundCraft would otherwise download: Save (the `<name>.scraftz` bundle, which marks the
//!   session saved), Save As, Save a Copy, bounces and the exports.
//! - `soundcraft:dirty` (to the parent): `{ dirty }` whenever unsaved changes appear or go away.
//! - `soundcraft:failed` (to the parent): `{ error }` when the app couldn't start, or when a
//!   `soundcraft:open` document (or the documents waiting to open) is over `MAX_OPEN_BYTES` (1 GiB,
//!   the session-bundle cap).
//!
//! The bridge is installed only for a same-origin parent, or for a parent on the origin named by
//! `?host_origin=` (a bare `scheme://host[:port]` origin); messages go to that exact origin and only
//! the parent window on it is heard. A cross-origin parent without `host_origin` gets no bridge:
//! saves download as usual. In host mode the page
//! starts on an empty session instead of the demo. SoundCraft has no user name, so there is no
//! `?author=`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;

thread_local! {
    /// The bridge the engine's delivery hook posts to (a plain `fn` can't capture it).
    static BRIDGE: RefCell<Option<Bridge>> = const { RefCell::new(None) };
}

/// Files the parent sent, waiting for the next frame: `(name, bytes)`.
pub type Received = Rc<RefCell<Vec<(String, Vec<u8>)>>>;

#[derive(Clone)]
pub struct Bridge {
    window: web_sys::Window,
    parent: web_sys::Window,
    origin: String,
    /// The unsaved-changes state last reported to the parent.
    reported: Rc<Cell<Option<bool>>>,
    pub received: Received,
}

impl Bridge {
    /// `None` when the page isn't framed, or the parent is cross-origin and not the origin
    /// `host_origin` allows (see [`crate::bridge::target_origin`]).
    pub fn new(host_origin: Option<&str>) -> Option<Self> {
        let window = web_sys::window()?;
        let parent = window.parent().ok().flatten()?;
        if js_sys::Object::is(&parent, &window) {
            return None;
        }
        let own = window.location().origin().ok()?;
        // Reading a cross-origin parent's location throws: treat that as cross-origin.
        let parent_origin = parent.location().origin().ok();
        let origin = crate::bridge::target_origin(&own, parent_origin.as_deref(), host_origin)?;
        Some(Self { window, parent, origin, reported: Rc::default(), received: Rc::default() })
    }

    /// Hear files from the parent and make [`deliver`] post to it.
    pub fn connect(&self, ctx: egui::Context) {
        self.listen(ctx);
        BRIDGE.with(|b| *b.borrow_mut() = Some(self.clone()));
    }

    /// Report unsaved changes (once a frame).
    pub fn report_dirty(&self, dirty: bool) {
        if self.reported.get() != Some(dirty) && self.send(&[("type", "soundcraft:dirty".into()), ("dirty", dirty.into())]) {
            self.reported.set(Some(dirty));
        }
    }

    pub fn post_ready(&self) {
        self.send(&[("type", "soundcraft:ready".into()), ("version", env!("CARGO_PKG_VERSION").into())]);
    }

    pub fn post_failed(&self, error: &str) {
        self.send(&[("type", "soundcraft:failed".into()), ("error", error.into())]);
    }

    fn post_save(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let buffer = js_sys::Uint8Array::from(bytes).buffer();
        let message = message(&[("type", "soundcraft:save".into()), ("name", name.into()), ("bytes", buffer.clone().into())])
            .ok_or_else(|| "couldn't build the message".to_string())?;
        self.parent
            .post_message_with_transfer(&message, &self.origin, &js_sys::Array::of1(&buffer))
            .map_err(|e| e.as_string().unwrap_or_else(|| format!("{e:?}")))
    }

    fn listen(&self, ctx: egui::Context) {
        let parent = self.parent.clone();
        let origin = self.origin.clone();
        let received = self.received.clone();
        let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            if event.origin() != origin || !event.source().is_some_and(|s| js_sys::Object::is(&s, &parent)) {
                return;
            }
            let data = event.data();
            if string(&data, "type").as_deref() != Some("soundcraft:open") {
                return;
            }
            let name = string(&data, "name").filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Session.scraftz".to_string());
            let Some(array) = byte_array(&data) else { return };
            // The cap covers this document plus any still waiting for the next frame.
            let queued: f64 = received.borrow().iter().map(|(_, b)| b.len() as f64).sum();
            if let Err(error) = crate::bridge::check_open(&name, queued, f64::from(array.byte_length()), MAX_OPEN_BYTES) {
                post_failed_to(&parent, &origin, &error);
                return;
            }
            let bytes = array.to_vec();
            received.borrow_mut().push((name, bytes));
            ctx.request_repaint();
        });
        if self.window.add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref()).is_ok() {
            // The listener lives as long as the page.
            on_message.forget();
        }
    }

    /// Post a message to the parent; false if it couldn't be sent.
    fn send(&self, fields: &[(&str, JsValue)]) -> bool {
        message(fields).is_some_and(|m| self.parent.post_message(&m, &self.origin).is_ok())
    }
}

/// The engine's delivery hook: the parent when connected, else `fallback` (the download).
pub fn deliver(name: &str, bytes: &[u8], fallback: fn(&str, &[u8]) -> Result<(), String>) -> Result<(), String> {
    match BRIDGE.with(|b| b.borrow().clone()) {
        Some(bridge) => bridge.post_save(name, bytes),
        None => fallback(name, bytes),
    }
}

fn message(fields: &[(&str, JsValue)]) -> Option<js_sys::Object> {
    let m = js_sys::Object::new();
    for (key, value) in fields {
        js_sys::Reflect::set(&m, &JsValue::from_str(key), value).ok()?;
    }
    Some(m)
}

fn string(data: &JsValue, key: &str) -> Option<String> {
    js_sys::Reflect::get(data, &JsValue::from_str(key)).ok()?.as_string()
}

/// `bytes` (an `ArrayBuffer` or `Uint8Array`) as a view, not yet copied.
fn byte_array(data: &JsValue) -> Option<js_sys::Uint8Array> {
    let bytes = js_sys::Reflect::get(data, &JsValue::from_str("bytes")).ok()?;
    if let Some(array) = bytes.dyn_ref::<js_sys::Uint8Array>() {
        Some(array.clone())
    } else if bytes.is_instance_of::<js_sys::ArrayBuffer>() {
        Some(js_sys::Uint8Array::new(&bytes))
    } else {
        None
    }
}

fn post_failed_to(parent: &web_sys::Window, origin: &str, error: &str) {
    if let Some(m) = message(&[("type", "soundcraft:failed".into()), ("error", error.into())]) {
        let _ = parent.post_message(&m, origin);
    }
}

/// Most bytes `soundcraft:open` takes: the same cap a session bundle may unpack to
/// ([`soundcraft_engine::io::MAX_BUNDLE_BYTES`]), checked before the bytes are copied.
pub(crate) const MAX_OPEN_BYTES: u64 = soundcraft_engine::io::MAX_BUNDLE_BYTES;
