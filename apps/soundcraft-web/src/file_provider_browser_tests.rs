//! Original four-frame synthetic audio; real Window publisher and production Runtime consumer.
//! This owns the one temporary JS fault injection at a time and never opens an audio device.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use js_sys::{Array, Function, Object, Reflect, Uint8Array};
use serde_json::json;
use soundcraft_engine::{Engine, io};
use soundcraft_ui_egui::{Services, SoundApp, dialogs, file_providers::Host, fonts, menus, provider_ui};
use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

use super::{API, EVENT, error, install, publish};
use crate::provider_storage::Runtime;

wasm_bindgen_test_configure!(run_in_browser);

// Test-only recoverable exceptions, not real OOM or browser-swallowed listener exceptions.
// JS finally restores descriptors even if the actual Rust publisher callback throws.
#[wasm_bindgen(inline_js = r#"
export function soundcraftProviderCopyFailureForTest(callback) {
    const descriptor = Object.getOwnPropertyDescriptor(Uint8Array, 'from');
    const original = Uint8Array.from;
    let calls = 0;
    try {
        Object.defineProperty(Uint8Array, 'from', {
            configurable: true, writable: true,
            value() { calls += 1; throw new RangeError('injected SoundCraft byte copy failure'); }
        });
        callback();
    } finally {
        if (descriptor) Object.defineProperty(Uint8Array, 'from', descriptor);
        else Reflect.deleteProperty(Uint8Array, 'from');
    }
    return { calls, restored: Uint8Array.from === original };
}
export function soundcraftProviderRequestFailureForTest(callback) {
    const descriptor = Object.getOwnPropertyDescriptor(window, 'dispatchEvent');
    const original = window.dispatchEvent;
    let calls = 0;
    try {
        Object.defineProperty(window, 'dispatchEvent', {
            configurable: true, writable: true,
            value(event) {
                if (event.type === 'soundcraft-file-provider' &&
                    (event.detail.type === 'saveDialog' || event.detail.type === 'openRequest')) {
                    calls += 1;
                    throw new Error('injected SoundCraft request dispatch failure');
                }
                return original.call(this, event);
            }
        });
        callback();
    } finally {
        if (descriptor) Object.defineProperty(window, 'dispatchEvent', descriptor);
        else Reflect.deleteProperty(window, 'dispatchEvent');
    }
    return { calls, restored: window.dispatchEvent === original };
}
"#)]
extern "C" {
    #[wasm_bindgen(catch, js_name = soundcraftProviderCopyFailureForTest)]
    fn copy_failure(callback: &Function) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = soundcraftProviderRequestFailureForTest)]
    fn request_failure(callback: &Function) -> Result<JsValue, JsValue>;
}

struct Events {
    window: web_sys::Window,
    listener: Closure<dyn FnMut(web_sys::Event)>,
    details: Rc<RefCell<Vec<JsValue>>>,
    api: JsValue,
}

impl Events {
    fn new() -> Result<Self, JsValue> {
        let window = web_sys::window().ok_or_else(|| error("no test window"))?;
        if Reflect::has(&window, &API.into())? {
            return Err(error("fixture requires a fresh owned page without an existing provider API"));
        }
        let details = Rc::new(RefCell::new(Vec::new()));
        let received = details.clone();
        let listener = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            if let Some(event) = event.dyn_ref::<web_sys::CustomEvent>() {
                received.borrow_mut().push(event.detail());
            }
        });
        window.add_event_listener_with_callback(EVENT, listener.as_ref().unchecked_ref())?;
        Ok(Self { window, listener, details, api: JsValue::UNDEFINED })
    }

    fn matching(&self, kind: &str) -> Result<Vec<JsValue>, JsValue> {
        let mut matches = Vec::new();
        for detail in self.details.borrow().iter() {
            if field(detail, "type")? == kind {
                matches.push(detail.clone());
            }
        }
        Ok(matches)
    }
}

impl Drop for Events {
    fn drop(&mut self) {
        let _ = self.window.remove_event_listener_with_callback(EVENT, self.listener.as_ref().unchecked_ref());
        if Reflect::get(&self.window, &API.into()).is_ok_and(|current| Object::is(&current, &self.api)) {
            let _ = Reflect::delete_property(self.window.unchecked_ref::<Object>(), &API.into());
        }
    }
}

fn call(api: &JsValue, method: &str, args: &Array) -> Result<JsValue, JsValue> {
    Reflect::get(api, &method.into())?.dyn_into::<Function>()?.apply(api, args)
}

fn field(value: &JsValue, name: &str) -> Result<String, JsValue> {
    Reflect::get(value, &name.into())?.as_string().ok_or_else(|| error(format!("missing {name}")))
}

fn audio_engine() -> Result<Engine, JsValue> {
    let mut engine = Engine::default();
    let audio = craft_audio::AudioBuffer { sample_rate: engine.session().sample_rate.hz(), channels: vec![vec![0.0, 0.25, -0.5, 0.75]] };
    let options = craft_audio::EncodeOptions::new(craft_audio::EncodeFormat::Wav).with_bit_depth(craft_audio::BitDepth::Float32);
    let wav = craft_audio::encode(&audio, &options).map_err(error)?;
    io::import_audio_bytes(&mut engine, "Original.wav", &wav, None, None, 0).map_err(error)?;
    Ok(engine)
}

fn tick(state: &Rc<RefCell<(Runtime, SoundApp)>>, ctx: &egui::Context) {
    let mut state = state.borrow_mut();
    let (runtime, app) = &mut *state;
    // Exactly the production consumer invoked by WebShell after its UI-independent logic.
    runtime.tick(app, ctx);
}

fn dialog_frame(app: &mut SoundApp, ctx: &egui::Context, events: Vec<egui::Event>) {
    let input =
        egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0))), events, ..Default::default() };
    ctx.run_ui(input, |ui| dialogs::show(app, ui.ctx())).drop_without_applying_deltas();
}

fn close_via_provider(
    state: &Rc<RefCell<(Runtime, SoundApp)>>,
    ctx: &egui::Context,
    events: &Events,
    api: &JsValue,
    host: &Host,
    saved_bytes: &[u8],
) -> Result<(), JsValue> {
    let current_doc = {
        let app = &mut state.borrow_mut().1;
        app.run("track.new", json!({"count":1,"format":"mono"})).map_err(error)?;
        let doc = provider_ui::document_id(app);
        menus::invoke_menu(app, "session.close", "File > Close Session");
        assert_eq!(app.dialogs.open_name(), Some("save_changes"));
        for _ in 0..3 {
            dialog_frame(app, ctx, vec![]);
        }
        let enter = egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        dialog_frame(app, ctx, vec![enter]);
        assert!(app.dialogs.open_name().is_none());
        doc
    };
    tick(state, ctx);
    let requests = events.matching("saveDialog")?;
    assert_eq!(requests.len(), 4);
    let request = field(&requests[3], "requestId")?;
    call(api, "resolveSaveDialog", &Array::of3(&request.clone().into(), &"provider".into(), &"Recovery.scraftz".into()))?;
    // A clean input frame matches the WebShell's logic on a later animation frame.
    ctx.run_ui(egui::RawInput::default(), |ui| tick(state, ui.ctx())).drop_without_applying_deltas();
    let saves = events.matching("save")?;
    assert_eq!(saves.len(), 2);
    assert_eq!(field(&saves[1], "requestId")?, request);
    let close_bytes = Reflect::get(&saves[1], &"bytes".into())?.dyn_into::<Uint8Array>()?.to_vec();
    assert_ne!(close_bytes.as_slice(), saved_bytes, "the close continuation must capture the new edit");
    assert_eq!(provider_ui::document_id(&state.borrow().1), current_doc);
    assert!(state.borrow().1.engine.is_dirty());
    assert!(host.pending_for(current_doc));
    call(api, "saveResult", &Array::of3(&request.into(), &"saved".into(), &"recovered".into()))?;
    ctx.run_ui(egui::RawInput::default(), |ui| tick(state, ui.ctx())).drop_without_applying_deltas();
    let closed_doc = provider_ui::document_id(&state.borrow().1);
    assert_ne!(closed_doc, current_doc, "only the matching durable result may finish Close Session");
    assert!(state.borrow().1.engine.session().sources.is_empty());

    let read = call(api, "requestOpen", &Array::of1(&"recovered".into()))?;
    tick(state, ctx);
    call(api, "openResource", &Array::of3(&read, &"Recovery.scraftz".into(), &Uint8Array::from(close_bytes.as_slice()).into()))?;
    tick(state, ctx);
    let opened = events.matching("opened")?;
    assert_eq!(opened.len(), 2);
    assert_eq!(field(&opened[1], "resourceId")?, "recovered");
    let app = &state.borrow().1;
    assert_ne!(provider_ui::document_id(app), closed_doc);
    assert!(!app.engine.is_dirty());
    let source = app.engine.session().sources.first().ok_or_else(|| error("reopened session lost its source"))?;
    let restored = app.engine.session().pool.get(source.id).ok_or_else(|| error("reopened session lost its audio"))?;
    assert_eq!(restored.buffer.channels.as_slice(), [vec![0.0, 0.25, -0.5, 0.75]]);
    Ok(())
}

#[wasm_bindgen_test]
fn page_local_sound_provider_delivery_failure_preserves_audio_and_recovers() -> Result<(), JsValue> {
    let ctx = egui::Context::default();
    fonts::install(&ctx, false);
    let host = Host::default();
    let mut events = Events::new()?;
    // The fresh-page ownership guard is already armed, including partial installation errors.
    let installed = install(host.clone(), ctx.clone());
    events.api = Reflect::get(&events.window, &API.into())?;
    installed?;
    let api = events.api.clone();
    assert!(!host.enabled() && !host.connected());
    let mut app = SoundApp::new(audio_engine()?, None, Services { file_providers: host.clone(), ..Default::default() });
    menus::invoke_menu(&mut app, "session.save", "File > Save Session");
    assert!(matches!(&app.dialogs.open, Some(dialogs::Dialog::PathPrompt { cmd, .. }) if cmd == "session.save_as"));
    assert!(!host.busy());
    app.dialogs.open = None;
    let connection = call(&api, "registerProvider", &Array::of3(&"fixture".into(), &"Fixture storage".into(), &"account-a".into()))?;
    let connection_id = connection.as_string().ok_or_else(|| error("registration must return its actual connection ID"))?;
    let document = provider_ui::document_id(&app);
    let state = Rc::new(RefCell::new((Runtime::default(), app)));
    provider_ui::choose(&mut state.borrow_mut().1, "session.save").ok_or_else(|| error("provider chooser was not used"))?.map_err(error)?;
    publish(&host, &ctx);
    let dialogs = events.matching("saveDialog")?;
    assert_eq!(dialogs.len(), 1);
    let failed_request = field(&dialogs[0], "requestId")?;
    assert_eq!(field(&dialogs[0], "connectionId")?, connection_id);
    call(&api, "resolveSaveDialog", &Array::of3(&failed_request.clone().into(), &"provider".into(), &"Recovery.scraftz".into()))?;

    let capture_revision = state.borrow().1.engine.revision;
    let callback_state = state.clone();
    let callback_ctx = ctx.clone();
    let capture_and_publish = Closure::<dyn FnMut()>::new(move || tick(&callback_state, &callback_ctx));
    let fault = copy_failure(capture_and_publish.as_ref().unchecked_ref())?;
    assert_eq!(Reflect::get(&fault, &"calls".into())?.as_f64(), Some(1.0), "the production catch-enabled import must execute");
    assert_eq!(Reflect::get(&fault, &"restored".into())?.as_bool(), Some(true));
    assert!(events.matching("save")?.is_empty(), "failed copying must not emit a partial save payload");
    assert!(host.pending_for(document), "failed capture waits for the actual Runtime result consumer");
    let failed_saves = events.matching("saveFailed")?;
    assert_eq!(failed_saves.len(), 1);
    assert_eq!(field(&failed_saves[0], "requestId")?, failed_request);
    assert_eq!(field(&failed_saves[0], "connectionId")?, connection_id);
    assert!(call(&api, "saveResult", &Array::of3(&failed_request.into(), &"saved".into(), &"late".into())).is_err());
    state.borrow_mut().1.run("track.new", json!({"count":1,"format":"mono"})).map_err(error)?;
    let newer_revision = state.borrow().1.engine.revision;
    let current = state.borrow().1.engine.session_arc();
    assert_ne!(newer_revision, capture_revision);
    tick(&state, &ctx);
    {
        let state = state.borrow();
        assert!(!host.pending_for(document));
        assert!(!host.bound(document));
        assert!(state.1.engine.is_dirty());
        assert_eq!(state.1.engine.revision, newer_revision);
        assert!(Arc::ptr_eq(&current, &state.1.engine.session_arc()));
        assert!(state.1.engine.path.is_none());
    }
    assert_eq!(host.snapshot()["documents"][0]["dirty"], true);
    assert_eq!(host.snapshot()["documents"][0]["provider"], false);

    provider_ui::choose(&mut state.borrow_mut().1, "session.save").ok_or_else(|| error("retry chooser missing"))?.map_err(error)?;
    publish(&host, &ctx);
    let dialogs = events.matching("saveDialog")?;
    assert_eq!(dialogs.len(), 2);
    let retry = field(&dialogs[1], "requestId")?;
    assert_ne!(retry, field(&dialogs[0], "requestId")?);
    call(&api, "resolveSaveDialog", &Array::of3(&retry.clone().into(), &"provider".into(), &"Recovery.scraftz".into()))?;
    tick(&state, &ctx);
    let saves = events.matching("save")?;
    assert_eq!(saves.len(), 1);
    assert_eq!(field(&saves[0], "requestId")?, retry);
    assert_eq!(field(&saves[0], "connectionId")?, connection_id);
    assert_eq!(field(&saves[0], "name")?, "Recovery.scraftz");
    let bytes = Reflect::get(&saves[0], &"bytes".into())?.dyn_into::<Uint8Array>()?.to_vec();
    assert_eq!(Reflect::get(&saves[0], &"bytesLength".into())?.as_f64(), Some(bytes.len() as f64));
    assert_eq!(Reflect::get(&saves[0], &"revision".into())?.as_f64(), Some(newer_revision as f64));
    let mut decoded = Engine::default();
    assert!(io::open_provider_session_bytes(&mut decoded, "Recovery.scraftz", &bytes).map_err(error)?.is_empty());
    assert_eq!(decoded.session().tracks.len(), current.tracks.len());
    assert_eq!(decoded.session().sources.len(), current.sources.len());
    assert!(!current.sources.is_empty(), "round-trip must include real source audio");
    for source in &current.sources {
        let expected = current.pool.get(source.id).ok_or_else(|| error("source fixture audio missing"))?;
        let restored = decoded.session().pool.get(source.id).ok_or_else(|| error("decoded bundle audio missing"))?;
        assert_eq!(restored.buffer.sample_rate, expected.buffer.sample_rate);
        assert_eq!(restored.buffer.channels, expected.buffer.channels, "native WAV resources must preserve every sample");
    }
    assert!(state.borrow().1.engine.is_dirty(), "dispatch alone is not durable persistence");
    call(&api, "saveResult", &Array::of3(&retry.into(), &"saved".into(), &"recovered".into()))?;
    tick(&state, &ctx);
    assert!(!state.borrow().1.engine.is_dirty());
    assert!(host.bound(document));
    assert_eq!(host.snapshot()["documents"][0]["dirty"], false);

    // Throw from actual dispatchEvent, not from a listener whose exception browsers swallow.
    provider_ui::choose(&mut state.borrow_mut().1, "session.save_as").ok_or_else(|| error("failure chooser missing"))?.map_err(error)?;
    let read = call(&api, "requestOpen", &Array::of1(&"recovered".into()))?;
    let read_id = read.as_string().ok_or_else(|| error("read must return a request ID"))?;
    let publish_host = host.clone();
    let publish_ctx = ctx.clone();
    let publisher = Closure::<dyn FnMut()>::new(move || publish(&publish_host, &publish_ctx));
    let fault = request_failure(publisher.as_ref().unchecked_ref())?;
    assert_eq!(Reflect::get(&fault, &"calls".into())?.as_f64(), Some(2.0));
    assert_eq!(Reflect::get(&fault, &"restored".into())?.as_bool(), Some(true));
    let failed_reads = events.matching("openFailed")?;
    assert_eq!(failed_reads.len(), 1);
    assert_eq!(field(&failed_reads[0], "requestId")?, read_id);
    assert_eq!(field(&failed_reads[0], "resourceId")?, "recovered");
    let failed_dialogs = events.matching("saveFailed")?;
    assert_eq!(failed_dialogs.len(), 2, "capture delivery and unresolved chooser each publish one correlated failure");
    let rejected_dialog = field(&failed_dialogs[1], "requestId")?;
    assert_eq!(field(&failed_dialogs[1], "connectionId")?, connection_id);
    assert!(call(&api, "resolveSaveDialog", &Array::of3(&rejected_dialog.into(), &"provider".into(), &"Late.scraftz".into())).is_err());
    assert!(call(&api, "openResource", &Array::of3(&read, &"Late.scraftz".into(), &Uint8Array::from(bytes.as_slice()).into())).is_err());
    assert!(!host.busy());
    tick(&state, &ctx);
    assert_eq!(provider_ui::document_id(&state.borrow().1), document);
    assert!(Arc::ptr_eq(&current, &state.borrow().1.engine.session_arc()));

    // Both request types work again, and old account callbacks stay retired after reconnect.
    provider_ui::choose(&mut state.borrow_mut().1, "session.save").ok_or_else(|| error("later chooser missing"))?.map_err(error)?;
    publish(&host, &ctx);
    let dialogs = events.matching("saveDialog")?;
    assert_eq!(dialogs.len(), 3);
    let cancelled = field(&dialogs[2], "requestId")?;
    call(&api, "resolveSaveDialog", &Array::of3(&cancelled.into(), &"cancel".into(), &"".into()))?;
    let retry_read = call(&api, "requestOpen", &Array::of1(&"recovered".into()))?;
    publish(&host, &ctx);
    let reads = events.matching("openRequest")?;
    assert_eq!(reads.len(), 1);
    assert_eq!(Reflect::get(&reads[0], &"requestId".into())?, retry_read);
    call(&api, "openResource", &Array::of3(&retry_read.clone(), &"Recovery.scraftz".into(), &Uint8Array::from(bytes.as_slice()).into()))?;
    tick(&state, &ctx);
    let opened = events.matching("opened")?;
    assert_eq!(opened.len(), 1);
    assert_eq!(Reflect::get(&opened[0], &"requestId".into())?, retry_read);
    assert_eq!(field(&opened[0], "resourceId")?, "recovered");
    {
        let state = state.borrow();
        let admitted = provider_ui::document_id(&state.1);
        assert_ne!(admitted, document, "the real Runtime must admit the decoded provider session");
        assert_eq!(field(&opened[0], "documentId")?, admitted.to_string());
        assert_eq!(host.filename(admitted).as_deref(), Some("Recovery.scraftz"));
        assert!(!state.1.engine.is_dirty());
        assert!(state.1.engine.path.is_none());
        assert_eq!(state.1.engine.session().tracks.len(), current.tracks.len());
        for source in &current.sources {
            let expected = current.pool.get(source.id).ok_or_else(|| error("original source audio missing"))?;
            let actual = state.1.engine.session().pool.get(source.id).ok_or_else(|| error("admitted source audio missing"))?;
            assert_eq!(actual.buffer.sample_rate, expected.buffer.sample_rate);
            assert_eq!(actual.buffer.channels, expected.buffer.channels, "actual openResource admission restores native audio");
        }
    }
    close_via_provider(&state, &ctx, &events, &api, &host, &bytes)?;
    let cancelled_read = call(&api, "requestOpen", &Array::of1(&"recovered".into()))?;
    publish(&host, &ctx);
    call(&api, "cancelOpen", &Array::of2(&cancelled_read, &"explicit cancellation".into()))?;
    publish(&host, &ctx);
    assert!(!host.busy());
    assert_eq!(call(&api, "unregisterProvider", &Array::new())?.as_bool(), Some(true));
    let next = call(&api, "registerProvider", &Array::of3(&"fixture".into(), &"Fixture storage".into(), &"account-b".into()))?;
    assert_ne!(next, connection);
    assert!(call(&api, "setResources", &Array::of2(&connection, &Array::new().into())).is_err());
    assert_eq!(host.snapshot()["namespace"], "account-b");
    assert_eq!(call(&api, "unregisterProvider", &Array::new())?.as_bool(), Some(true));
    Ok(())
}
