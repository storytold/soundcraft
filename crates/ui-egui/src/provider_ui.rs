//! UI integration for the optional external file capability.
use crate::SoundApp;
use serde_json::{Value, json};

pub fn document_id(app: &SoundApp) -> u64 {
    app.doc_epoch
}
pub fn choose(app: &mut SoundApp, id: &str) -> Option<Result<Value, String>> {
    let host = app.services.file_providers.clone();
    let doc = document_id(app);
    if !host.enabled() {
        if matches!(id, "session.save" | "session.save_as") && (host.bound(doc) || host.pending_for(doc)) {
            return Some(Err("Re-enable the provider before saving this linked or pending project; use Save a Copy for a device download".into()));
        }
        return None;
    }
    if matches!(id, "session.new" | "session.open" | "session.close" | "session.revert" | "app.quit") && host.busy() {
        return Some(Err("Finish or cancel the pending provider operation before replacing the project".into()));
    }
    if !matches!(id, "session.save" | "session.save_as") {
        return None;
    }
    let name = host.filename(doc).unwrap_or_else(|| suggested_name(&app.engine.session().name, "scraftz"));
    Some(
        host.choose_request(doc, app.engine.revision, name, id == "session.save_as")
            .map(|request| json!({"pending":"fileProvider","requestId":request})),
    )
}

/// A close/open/quit continuation belongs to one interactive save request and its actual capture.
#[derive(Clone, Debug)]
pub(crate) struct AfterSave {
    doc: u64,
    request: String,
    captured_revision: Option<u64>,
    next: (String, Value),
}

#[cfg(test)]
pub(crate) fn close_capture_identity(app: &SoundApp) -> Option<(&str, u64, Option<u64>)> {
    app.dialogs.provider_after_save.as_ref().map(|after| (after.request.as_str(), after.doc, after.captured_revision))
}

/// Save from the Save Changes prompt uses exactly the same provider chooser as menu Save.
pub(crate) fn save_before_replacing(app: &mut SoundApp, next: &(String, Value)) -> Option<Result<Value, String>> {
    let result = choose(app, "session.save")?;
    Some(result.and_then(|value| {
        let request = value["requestId"].as_str().ok_or("Finish the pending provider save before starting another close operation")?.to_owned();
        app.dialogs.provider_after_save = Some(AfterSave { doc: document_id(app), request, captured_revision: None, next: next.clone() });
        Ok(value)
    }))
}

/// Called only after committing plugin state and capturing the session that will be encoded.
pub fn record_close_capture(app: &mut SoundApp, request: &str, doc: u64, revision: u64) {
    if let Some(after) = app.dialogs.provider_after_save.as_mut().filter(|after| after.doc == doc && after.request == request) {
        after.captured_revision = Some(revision);
    }
}

/// Consume the runtime's exact captured result. Failed delivery releases its checkpoint/close
/// continuation through this same path, without acknowledging persistence or committing drafts.
pub fn apply_result(
    app: &mut SoundApp,
    capture: &crate::file_providers::Capture,
    resource: String,
    status: &str,
    snapshot: Option<std::sync::Arc<soundcraft_model::Session>>,
    ctx: &egui::Context,
) {
    let mut durable = false;
    if status == "saved"
        && capture.doc == document_id(app)
        && let Some(snapshot) = snapshot
    {
        app.services.file_providers.bind(capture, resource);
        app.engine.acknowledge_external_save(snapshot);
        // The provider now owns the saved checkpoint. Revert cannot read its old device copy.
        app.engine.path = None;
        durable = true;
    } else if status == "failed" {
        app.ui.status = "Provider delivery/save failed. Your project and changes are still open.".into();
    }
    finish_close_capture(app, capture, durable, ctx);
}

/// Only an exact, durable checkpoint can continue the original close/open/quit action.
pub fn finish_close_capture(app: &mut SoundApp, capture: &crate::file_providers::Capture, durable: bool, ctx: &egui::Context) {
    let matches =
        app.dialogs.provider_after_save.as_ref().is_some_and(|after| after.doc == capture.doc && after.captured_revision == Some(capture.revision));
    if !matches {
        return;
    }
    let Some(after) = app.dialogs.provider_after_save.take() else { return };
    let ready = durable && document_id(app) == after.doc && app.engine.revision == capture.revision && can_replace_session(app, ctx);
    if !ready {
        crate::dialogs::discard_provider_close(app);
        return;
    }
    if let Err(error) = crate::dialogs::resume_provider_close(app, &after.next.0, after.next.1) {
        crate::dialogs::discard_provider_close(app);
        app.ui.status = error;
    }
}

/// A cancelled chooser, failed encoding or device download cannot leave a stale close action.
pub fn discard_finished_close_capture(app: &mut SoundApp) {
    let finished = app
        .dialogs
        .provider_after_save
        .as_ref()
        .is_some_and(|after| after.doc != document_id(app) || !app.services.file_providers.pending_for(after.doc));
    if finished {
        app.dialogs.provider_after_save = None;
        crate::dialogs::discard_provider_close(app);
    }
}

pub fn publish(app: &SoundApp) {
    let host = &app.services.file_providers;
    if !host.connected() {
        return;
    }
    let doc = document_id(app);
    host.publish_documents(json!([{"id":doc.to_string(),"revision":app.engine.revision,"name":app.engine.session().name,"dirty":app.has_unsaved_changes(),"provider":host.bound(doc)}]),&[doc]);
}

/// Provider recent files are separate from device paths and remain account-scoped.
pub fn recent(app: &SoundApp, ui: &mut egui::Ui) {
    app.services.file_providers.recent_ui(ui);
}

/// Install a fully decoded provider session after every asset has been validated.
pub fn install_session(app: &mut SoundApp, session: soundcraft_model::Session, ctx: &egui::Context) -> Result<(), String> {
    if !can_replace_session(app, ctx) {
        return Err("Save changes and finish pending work before opening a provider session".into());
    }
    app.stop_play();
    app.engine.replace_session(session);
    app.engine.path = None;
    app.document_replaced();
    Ok(())
}

/// Provider opens and saved-project continuations share the same loss-prevention boundary.
pub fn can_replace_session(app: &SoundApp, ctx: &egui::Context) -> bool {
    !app.has_unsaved_changes()
        && app.gesture.is_none()
        && !app.pointer_down
        // Logic receives provider replies before widgets apply this pass's raw input.
        && !ctx.input(|input| !input.events.is_empty() || !input.raw.dropped_files.is_empty() || input.pointer.any_down())
        && !app.engine.transport.recording
        && !app.imports.busy()
        && app.bounce.is_none()
        && app.dialogs.open.is_none()
        && !app.services.file_providers.pending_for(document_id(app))
}

/// Safe suggested filename; project names may contain path separators or be long UTF-8.
pub fn suggested_name(stem: &str, extension: &str) -> String {
    let mut name = String::new();
    for c in stem.chars() {
        if name.len() + c.len_utf8() > 230 {
            break;
        }
        name.push(if c.is_control() || matches!(c, '/' | '\\') { '-' } else { c });
    }
    if name.trim().is_empty() {
        name = "Untitled".into();
    }
    format!("{name}.{extension}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    #[test]
    fn suggestion_is_bounded_utf8_and_not_a_device_path() {
        let name = suggested_name(&"á/\\nube".repeat(100), "scraftz");
        assert!(name.len() <= 255);
        assert!(!name.contains(['/', '\\']));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod provider_routing_tests {
    use super::*;

    #[test]
    fn disabled_linked_or_pending_save_cannot_fall_through_to_device_saving() {
        for linked in [false, true] {
            let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
            app.engine.execute("track.new", &json!({})).unwrap();
            let host = app.services.file_providers.clone();
            host.set_session("0123456789abcdef0123456789abcdef".into()).unwrap();
            host.connect("test".into(), "Provider".into(), "account".into()).unwrap();
            let doc = document_id(&app);
            if linked {
                host.bind(&crate::file_providers::Capture { doc, revision: 1, name: "Linked.scraftz".into(), save_as: false }, "linked".into());
            } else {
                host.choose(doc, app.engine.revision, "Pending.scraftz".into(), false).unwrap();
                host.take_events();
            }
            host.set_enabled(false).unwrap();
            let session = app.engine.session_arc();
            let revision = app.engine.revision;
            for command in ["session.save", "session.save_as"] {
                crate::menus::invoke_menu(&mut app, command, "File > Save Session");
                assert!(app.ui.status.contains("Re-enable the provider"));
                assert!(app.engine.is_dirty());
                assert_eq!(app.engine.revision, revision);
                assert!(std::sync::Arc::ptr_eq(&session, &app.engine.session_arc()));
                assert!(app.engine.path.is_none());
                assert!(app.dialogs.open.is_none(), "rejected Save must not open the native path prompt");
                assert_eq!(host.bound(doc), linked);
                assert_eq!(host.pending_for(doc), !linked);
                assert!(host.take_events().is_empty(), "rejected Save must deliver neither a chooser nor bytes");
            }
            host.set_enabled(true).unwrap();
            publish(&app);
            assert_eq!(host.snapshot()["documents"][0]["dirty"], true);
            assert_eq!(host.snapshot()["documents"][0]["provider"], linked);
        }
    }

    #[test]
    fn disabled_unbound_document_keeps_legacy_save_routing() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let host = app.services.file_providers.clone();
        host.set_session("0123456789abcdef0123456789abcdef".into()).unwrap();
        host.connect("test".into(), "Provider".into(), "account".into()).unwrap();
        host.set_enabled(false).unwrap();
        assert!(choose(&mut app, "session.save").is_none());
        assert!(choose(&mut app, "session.save_copy").is_none());
        assert!(host.take_events().is_empty());
    }

    #[test]
    fn default_off_keeps_original_save_and_registration_only_emits_a_choice() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        assert!(choose(&mut app, "session.save").is_none());
        let host = app.services.file_providers.clone();
        host.set_session("0123456789abcdef0123456789abcdef".into()).unwrap();
        host.connect("test".into(), "Provider".into(), "account".into()).unwrap();
        let before = app.engine.revision;
        let chosen = choose(&mut app, "session.save").unwrap().unwrap();
        let events = host.take_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].metadata["type"], "saveDialog");
        assert_eq!(chosen["requestId"], events[0].metadata["requestId"]);
        assert!(chosen["requestId"].as_str().is_some());
        let repeated = choose(&mut app, "session.save").unwrap().unwrap();
        assert_eq!(repeated["requestId"], chosen["requestId"]);
        assert!(host.take_events().is_empty(), "an existing chooser must not emit a second dialog");
        assert_eq!(app.engine.revision, before, "a choice must not mutate document state");
    }

    #[test]
    fn close_continuation_uses_the_actual_live_chooser_request() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let host = app.services.file_providers.clone();
        host.set_session("0123456789abcdef0123456789abcdef".into()).unwrap();
        host.connect("test".into(), "Provider".into(), "account".into()).unwrap();
        let chosen = choose(&mut app, "session.save").unwrap().unwrap();
        let response = save_before_replacing(&mut app, &("app.quit".into(), json!({}))).unwrap().unwrap();
        let after = app.dialogs.provider_after_save.as_ref().unwrap();
        assert_eq!(after.request, chosen["requestId"].as_str().unwrap());
        assert_eq!(response["requestId"], chosen["requestId"]);
        assert_eq!(after.doc, app.doc_epoch);
        assert_eq!(after.captured_revision, None);
        let events = host.take_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].metadata["requestId"], chosen["requestId"]);
    }

    #[test]
    fn an_existing_snapshot_cannot_acquire_a_new_close_continuation() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let host = app.services.file_providers.clone();
        host.set_session("0123456789abcdef0123456789abcdef".into()).unwrap();
        host.connect("test".into(), "Provider".into(), "account".into()).unwrap();
        let chosen = choose(&mut app, "session.save").unwrap().unwrap();
        let request = chosen["requestId"].as_str().unwrap();
        host.take_events();
        host.resolve_dialog(request, "provider", "Saved.scraftz").unwrap();
        host.take_commands();
        let capture =
            crate::file_providers::Capture { doc: app.doc_epoch, revision: app.engine.revision, name: "Saved.scraftz".into(), save_as: false };
        host.capture(request, capture, vec![1]).unwrap();
        host.take_events();
        app.run("track.new", json!({"count":1,"format":"mono"})).unwrap();
        assert!(save_before_replacing(&mut app, &("app.quit".into(), json!({}))).unwrap().is_err());
        assert!(app.dialogs.provider_after_save.is_none());
        assert!(host.take_events().is_empty(), "coalescing an in-flight snapshot must not emit a fresh chooser");
        assert!(!app.quit_requested);
    }

    #[test]
    fn durable_provider_save_detaches_the_device_revert_path() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let host = app.services.file_providers.clone();
        host.set_session("0123456789abcdef0123456789abcdef".into()).unwrap();
        host.connect("test".into(), "Provider".into(), "account".into()).unwrap();
        let old_path = "Old device copy.scraft".to_owned();
        app.engine.path = Some(old_path.clone());
        app.run("track.new", json!({"count":1,"format":"mono"})).unwrap();
        let snapshot = app.engine.session_arc();
        let capture =
            crate::file_providers::Capture { doc: app.doc_epoch, revision: app.engine.revision, name: "Provider.scraftz".into(), save_as: true };
        let ctx = egui::Context::default();

        apply_result(&mut app, &capture, String::new(), "failed", Some(snapshot.clone()), &ctx);
        assert_eq!(app.engine.path.as_deref(), Some(old_path.as_str()));
        app.run("track.new", json!({"count":1,"format":"mono"})).unwrap();
        let edited = app.engine.session_arc();
        apply_result(&mut app, &capture, "resource".into(), "saved", Some(snapshot), &ctx);

        assert!(host.bound(app.doc_epoch));
        assert!(app.engine.path.is_none());
        assert!(app.engine.is_dirty(), "newer edits must stay dirty after the provider acknowledges the earlier snapshot");
        assert!(app.run("session.revert", json!({})).unwrap_err().contains("the session has not been saved"));
        assert!(std::sync::Arc::ptr_eq(&edited, &app.engine.session_arc()));
    }

    #[test]
    fn provider_open_preserves_pending_recording_jobs_and_editor_drafts() {
        for pending in ["dirty", "plugin", "gesture", "pointer", "recording", "import", "bounce", "dialog", "save"] {
            let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
            app.run("track.new", json!({"count":1,"format":"mono"})).unwrap();
            app.engine.acknowledge_external_save(app.engine.session_arc());
            let ctx = egui::Context::default();
            assert!(can_replace_session(&app, &ctx), "the fixture starts with a replaceable clean session");
            let directory = std::env::temp_dir().join(format!("soundcraft-provider-open-{}-{pending}", std::process::id()));
            // create_dir refuses an existing path: cleanup below only owns this fixture's directory.
            std::fs::create_dir(&directory).unwrap();
            let track = app.engine.session().tracks[0].id;
            match pending {
                "dirty" => {
                    app.run("track.new", json!({"count":1,"format":"mono"})).unwrap();
                }
                "plugin" => app.editor_states.push((track, 0)),
                "gesture" => app.gesture = Some(crate::Gesture::Fader { track, start_db: 0.0 }),
                "pointer" => app.pointer_down = true,
                "recording" => app.engine.transport.recording = true,
                "import" => crate::import::start(&mut app, json!({"path":"queued-audio.wav"})),
                "bounce" => {
                    let path = directory.join("pending.wav");
                    app.bounce = Some(soundcraft_engine::cmd::bounce_job(&app.engine, &json!({"path":path})).unwrap());
                }
                "dialog" => {
                    app.dialogs.open = Some(crate::dialogs::Dialog::NewSession { name: "Pending dialog".into(), sample_rate: 48000, demo: false });
                }
                "save" => {
                    let host = &app.services.file_providers;
                    host.set_session("0123456789abcdef0123456789abcdef".into()).unwrap();
                    host.connect("test".into(), "Provider".into(), "account".into()).unwrap();
                    host.choose(app.doc_epoch, app.engine.revision, "Pending.scraftz".into(), false).unwrap();
                }
                _ => panic!("unexpected pending fixture case: {pending}"),
            }
            let original = app.engine.session_arc();
            let epoch = app.doc_epoch;
            assert!(!can_replace_session(&app, &ctx), "{pending} must block replacement");
            let incoming = soundcraft_model::Session { name: "Incoming provider session".into(), ..Default::default() };
            assert!(install_session(&mut app, incoming, &ctx).is_err(), "{pending}");
            assert!(std::sync::Arc::ptr_eq(&original, &app.engine.session_arc()), "{pending}: the current document was replaced or mutated");
            assert_eq!(app.doc_epoch, epoch, "{pending}");
            match pending {
                "plugin" => assert_eq!(app.editor_states, vec![(track, 0)]),
                "gesture" => assert!(app.gesture.is_some()),
                "pointer" => assert!(app.pointer_down),
                "recording" => assert!(app.engine.transport.recording, "Open must not stop/finalize the pending take"),
                "import" => assert!(app.imports.busy()),
                "bounce" => assert!(app.bounce.is_some()),
                "dialog" => assert!(app.dialogs.open.is_some()),
                "save" => assert!(app.services.file_providers.pending_for(epoch)),
                _ => assert!(app.engine.is_dirty()),
            }
            // Bounce's Drop removes its unfinished part file before removing the empty directory.
            drop(app);
            std::fs::remove_dir(directory).unwrap();
        }
    }

    #[test]
    fn provider_open_admits_only_a_clean_idle_session_and_preserves_current_frame_input() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let ctx = egui::Context::default();
        assert!(can_replace_session(&app, &ctx));
        let epoch = app.doc_epoch;
        let incoming = soundcraft_model::Session { name: "Incoming provider session".into(), ..Default::default() };
        install_session(&mut app, incoming, &ctx).unwrap();
        assert_eq!(app.engine.session().name, "Incoming provider session");
        assert_ne!(app.doc_epoch, epoch);
        assert!(!app.has_unsaved_changes());
        let original = app.engine.session_arc();
        let epoch = app.doc_epoch;
        let input = egui::RawInput { events: vec![egui::Event::Text("Unconsumed edit".into())], ..Default::default() };
        let mut checked = false;
        let mut output = ctx.run_ui(input, |ui| {
            if checked {
                return;
            }
            checked = true;
            assert!(!can_replace_session(&app, ui.ctx()));
            assert!(install_session(&mut app, soundcraft_model::Session::default(), ui.ctx()).is_err());
            assert!(std::sync::Arc::ptr_eq(&original, &app.engine.session_arc()));
            assert_eq!(app.doc_epoch, epoch);
            app.run("track.new", json!({"count":1,"format":"mono"})).unwrap();
            assert!(app.engine.is_dirty(), "the following UI pass must retain its admitted edit");
        });
        output.textures_delta.clear();
        assert!(checked, "the fixture must exercise the pre-UI replacement boundary");
        assert_eq!(app.engine.session().name, "Incoming provider session");
        assert!(app.engine.is_dirty());
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod delivery_recovery_tests {
    use super::*;
    use crate::file_providers::{Capture, Command, Host};
    use soundcraft_engine::{Engine, io};
    use std::sync::Arc;

    fn audio_app() -> SoundApp {
        let mut engine = Engine::default();
        let audio = craft_audio::AudioBuffer { sample_rate: engine.session().sample_rate.hz(), channels: vec![vec![0.0, 0.25, -0.5, 0.75]] };
        let options = craft_audio::EncodeOptions::new(craft_audio::EncodeFormat::Wav).with_bit_depth(craft_audio::BitDepth::Float32);
        let wav = craft_audio::encode(&audio, &options).unwrap();
        io::import_audio_bytes(&mut engine, "Original.wav", &wav, None, None, 0).unwrap();
        SoundApp::new(engine, None, crate::Services::default())
    }

    fn connect(app: &SoundApp) -> Host {
        let host = app.services.file_providers.clone();
        host.set_session("0123456789abcdef0123456789abcdef".into()).unwrap();
        host.connect("test".into(), "Provider".into(), "account".into()).unwrap();
        host
    }

    fn capture_next(app: &mut SoundApp, host: &Host) -> (String, Capture, Arc<soundcraft_model::Session>, Vec<u8>) {
        let mut commands = host.take_commands();
        assert_eq!(commands.len(), 1);
        let Command::Save { request, doc, destination, name, save_as } = commands.remove(0) else { panic!("expected actual Save command") };
        assert_eq!(destination, "provider");
        assert_eq!(doc, document_id(app));
        app.capture_plugin_states();
        let capture = Capture { doc, revision: app.engine.revision, name, save_as };
        record_close_capture(app, &request, doc, capture.revision);
        let snapshot = app.engine.session_arc();
        let bytes = io::session_provider_bundle(&snapshot).unwrap();
        host.capture(&request, capture.clone(), bytes.clone()).unwrap();
        (request, capture, snapshot, bytes)
    }

    fn assert_audio_bundle(bytes: &[u8], expected: &soundcraft_model::Session) {
        assert!(bytes.starts_with(b"PK\x03\x04"));
        let mut decoded = Engine::default();
        assert!(io::open_provider_session_bytes(&mut decoded, "Recovery.scraftz", bytes).unwrap().is_empty());
        assert_eq!(decoded.session().tracks.len(), expected.tracks.len());
        assert_eq!(decoded.session().sources.len(), expected.sources.len());
        for source in &expected.sources {
            let original = expected.pool.get(source.id).unwrap();
            let restored = decoded.session().pool.get(source.id).unwrap();
            assert_eq!(restored.buffer.sample_rate, original.buffer.sample_rate);
            assert_eq!(restored.buffer.channels, original.buffer.channels, "native bundle must retain every source sample");
        }
    }

    #[test]
    fn delivery_failure_reaches_app_consumer_keeps_audio_drafts_and_close_open_until_retry() {
        let mut app = audio_app();
        let host = connect(&app);
        let ctx = egui::Context::default();
        let value = save_before_replacing(&mut app, &("app.quit".into(), json!({}))).unwrap().unwrap();
        let request = value["requestId"].as_str().unwrap().to_string();
        host.take_events();
        host.resolve_dialog(&request, "provider", "Recovery.scraftz").unwrap();
        let (_, capture, snapshot, bytes) = capture_next(&mut app, &host);
        assert_audio_bundle(&bytes, &snapshot);
        app.run("track.new", json!({"count":1,"format":"mono"})).unwrap();
        let revision = app.engine.revision;
        assert_ne!(revision, capture.revision);
        let current = app.engine.session_arc();
        let draft = (app.engine.session().tracks[0].id, 0);
        app.editor_states.push(draft);
        assert!(host.publish_events_with(|_, _| Err("injected copy failure"), |_| {}));
        assert!(host.pending_for(capture.doc));
        let mut commands = host.take_commands();
        assert_eq!(commands.len(), 1);
        let Command::Result { capture: failed, resource, status } = commands.remove(0) else {
            panic!("failure must reach the actual Result consumer")
        };
        assert_eq!((failed.doc, failed.revision, status.as_str()), (capture.doc, capture.revision, "failed"));
        apply_result(&mut app, &failed, resource, &status, Some(snapshot), &ctx);
        assert!(close_capture_identity(&app).is_none());
        assert!(!app.quit_requested);
        assert!(!host.pending_for(capture.doc));
        assert!(!host.bound(capture.doc));
        assert!(app.engine.is_dirty());
        assert_eq!(app.engine.revision, revision);
        assert!(Arc::ptr_eq(&current, &app.engine.session_arc()));
        assert_eq!(app.editor_states, vec![draft]);
        assert!(host.result(request, "saved".into(), "late".into()).is_err());

        app.editor_states.clear(); // Explicitly finish this test's outstanding editor-state draft.
        choose(&mut app, "session.save").unwrap().unwrap();
        let retry = host.take_events().remove(0).metadata["requestId"].as_str().unwrap().to_string();
        host.resolve_dialog(&retry, "provider", "Recovery.scraftz").unwrap();
        let (_, captured, snapshot, bytes) = capture_next(&mut app, &host);
        assert_audio_bundle(&bytes, &snapshot);
        assert_eq!(snapshot.tracks.len(), current.tracks.len());
        host.publish_events_with(|_, _| Ok::<(), ()>(()), |_| {});
        host.result(retry, "saved".into(), "recovered".into()).unwrap();
        let Command::Result { capture, resource, status } = host.take_commands().remove(0) else { panic!("expected retry Result") };
        apply_result(&mut app, &capture, resource, &status, Some(snapshot), &ctx);
        assert!(!app.engine.is_dirty());
        assert!(host.bound(captured.doc));
        assert_eq!(host.filename(captured.doc).as_deref(), Some("Recovery.scraftz"));
        assert!(app.engine.path.is_none());
    }

    #[test]
    fn accepted_reentrant_checkpoint_preserves_later_edits_and_plugin_state_draft() {
        let mut app = audio_app();
        let host = connect(&app);
        choose(&mut app, "session.save").unwrap().unwrap();
        let request = host.take_events().remove(0).metadata["requestId"].as_str().unwrap().to_string();
        host.resolve_dialog(&request, "provider", "Captured.scraftz").unwrap();
        let (_, capture, snapshot, bytes) = capture_next(&mut app, &host);
        assert_audio_bundle(&bytes, &snapshot);
        app.run("track.new", json!({"count":1,"format":"mono"})).unwrap();
        let current = app.engine.session_arc();
        let revision = app.engine.revision;
        let draft = (app.engine.session().tracks[0].id, 0);
        app.editor_states.push(draft);
        assert!(host.publish_events_with(
            |_, _| {
                host.result(request.clone(), "saved".into(), "resource".into()).unwrap();
                Err("dispatch threw after callback acceptance")
            },
            |_| {},
        ));
        let mut commands = host.take_commands();
        assert_eq!(commands.len(), 1, "accepted completion cannot also enqueue a failed capture");
        let Command::Result { capture: accepted, resource, status } = commands.remove(0) else { panic!("expected accepted Result") };
        assert_eq!(status, "saved");
        apply_result(&mut app, &accepted, resource, &status, Some(snapshot), &egui::Context::default());
        assert!(host.bound(capture.doc));
        assert!(app.engine.is_dirty());
        assert!(app.has_unsaved_changes());
        assert_eq!(app.engine.revision, revision);
        assert!(Arc::ptr_eq(&current, &app.engine.session_arc()));
        assert_eq!(app.editor_states, vec![draft]);
    }

    #[test]
    fn an_ack_for_clean_engine_state_cannot_clean_a_plugin_draft_or_resume_close() {
        let mut app = audio_app();
        let host = connect(&app);
        let ctx = egui::Context::default();
        let value = save_before_replacing(&mut app, &("app.quit".into(), json!({}))).unwrap().unwrap();
        let request = value["requestId"].as_str().unwrap().to_string();
        host.take_events();
        host.resolve_dialog(&request, "provider", "Captured.scraftz").unwrap();
        let (_, capture, snapshot, _) = capture_next(&mut app, &host);
        host.publish_events_with(|_, _| Ok::<(), ()>(()), |_| {});
        let draft = (app.engine.session().tracks[0].id, 0);
        app.editor_states.push(draft);
        host.result(request, "saved".into(), "resource".into()).unwrap();
        let Command::Result { capture: accepted, resource, status } = host.take_commands().remove(0) else { panic!("expected saved Result") };
        apply_result(&mut app, &accepted, resource, &status, Some(snapshot), &ctx);
        assert!(!app.engine.is_dirty(), "the captured engine snapshot really was acknowledged");
        assert!(app.has_unsaved_changes(), "uncommitted editor state remains unsaved");
        assert_eq!(app.editor_states, vec![draft]);
        assert_eq!(app.engine.revision, capture.revision);
        assert!(!app.quit_requested);
        assert!(close_capture_identity(&app).is_none());
        publish(&app);
        assert_eq!(host.snapshot()["documents"][0]["dirty"], true);
    }
}
