//! UI integration for the optional external file capability.
use crate::SoundApp;
use serde_json::{Value, json};

pub fn document_id(app: &SoundApp) -> u64 {
    app.doc_epoch
}
pub fn choose(app: &mut SoundApp, id: &str) -> Option<Result<Value, String>> {
    let host = app.services.file_providers.clone();
    if !host.enabled() {
        return None;
    }
    let doc = document_id(app);
    if matches!(id, "session.new" | "session.open" | "session.close" | "session.revert" | "app.quit") && host.busy() {
        return Some(Err("Finish or cancel the pending provider operation before replacing the project".into()));
    }
    if !matches!(id, "session.save" | "session.save_as") {
        return None;
    }
    let name = host.filename(doc).unwrap_or_else(|| suggested_name(&app.engine.session().name, "scraftz"));
    Some(host.choose(doc, app.engine.revision, name, id == "session.save_as").map(|request| json!({"pending":"fileProvider","requestId":request})))
}

/// A close/open/quit continuation belongs to one interactive save request and its actual capture.
#[derive(Clone, Debug)]
pub(crate) struct AfterSave {
    doc: u64,
    request: String,
    captured_revision: Option<u64>,
    next: (String, Value),
}

/// Save from the Save Changes prompt uses exactly the same provider chooser as menu Save.
pub(crate) fn save_before_replacing(app: &mut SoundApp, next: &(String, Value)) -> Option<Result<Value, String>> {
    let result = choose(app, "session.save")?;
    Some(result.and_then(|value| {
        let request = value["requestId"].as_str().ok_or("The provider save did not create a request")?.to_owned();
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

/// Only an exact, durable checkpoint can continue the original close/open/quit action.
pub fn finish_close_capture(app: &mut SoundApp, capture: &crate::file_providers::Capture, durable: bool, ctx: &egui::Context) {
    let matches =
        app.dialogs.provider_after_save.as_ref().is_some_and(|after| after.doc == capture.doc && after.captured_revision == Some(capture.revision));
    if !matches {
        return;
    }
    let Some(after) = app.dialogs.provider_after_save.take() else { return };
    let ready = durable
        && document_id(app) == after.doc
        && app.engine.revision == capture.revision
        && !app.has_unsaved_changes()
        && app.gesture.is_none()
        && !app.pointer_down
        // Logic receives the ACK before widgets apply this pass's raw input. Keep the
        // durable checkpoint, but never replace its document underneath admitted input.
        && !ctx.input(|input| !input.events.is_empty() || !input.raw.dropped_files.is_empty() || input.pointer.any_down())
        && !app.engine.transport.recording
        && !app.imports.busy()
        && app.bounce.is_none()
        && app.dialogs.open.is_none();
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
    host.publish_documents(json!([{"id":doc.to_string(),"revision":app.engine.revision,"name":app.engine.session().name,"dirty":app.engine.is_dirty(),"provider":host.bound(doc)}]),&[doc]);
}

/// Provider recent files are separate from device paths and remain account-scoped.
pub fn recent(app: &SoundApp, ui: &mut egui::Ui) {
    app.services.file_providers.recent_ui(ui);
}

/// Install a fully decoded provider session after every asset has been validated.
pub fn install_session(app: &mut SoundApp, session: soundcraft_model::Session) {
    app.stop_play();
    app.engine.replace_session(session);
    app.engine.path = None;
    app.document_replaced();
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
    fn default_off_keeps_original_save_and_registration_only_emits_a_choice() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        assert!(choose(&mut app, "session.save").is_none());
        let host = app.services.file_providers.clone();
        host.set_session("0123456789abcdef0123456789abcdef".into()).unwrap();
        host.connect("test".into(), "Provider".into(), "account".into()).unwrap();
        let before = app.engine.revision;
        assert!(choose(&mut app, "session.save").unwrap().is_ok());
        let events = host.take_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].metadata["type"], "saveDialog");
        assert_eq!(app.engine.revision, before, "a choice must not mutate document state");
    }
}
