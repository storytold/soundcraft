//! SoundCraft's end of the `craft-ui` cloud contract: what only the application can answer about
//! its own document, and nothing about any provider (there isn't one wired in here; see the
//! module docs on [`crate::dialogs::Dialog::Cloud`]).

use super::SoundApp;
use craft_cloud::EntryKind;
use craft_ui::cloud::{CloudApp, CloudFiles, Destination, DocumentKind};

/// SoundCraft has one session document per window, so the cloud pages' `uid` is always this one
/// constant rather than an identity the engine tracks. It stands for "the session that is open."
const DOC: u64 = 0;

/// What a SoundCraft document is, for the cloud Open and Save As pages: a `.scraft` bundle, same
/// extension whichever Craft or cloud location it was opened from.
const SESSION_KIND: DocumentKind = DocumentKind { extensions: &["scraft"], mimes: &[], noun: "session", default_extension: "scraft" };

impl CloudApp for SoundApp {
    fn cloud(&self) -> &CloudFiles {
        &self.cloud
    }

    fn cloud_mut(&mut self) -> &mut CloudFiles {
        &mut self.cloud
    }

    fn kind(&self) -> DocumentKind {
        SESSION_KIND
    }

    fn active(&self) -> Option<u64> {
        Some(DOC)
    }

    fn title(&self, _uid: u64) -> String {
        self.engine.session().name.clone()
    }

    /// Bytes arrived from the cloud. Replaces the open session the same way a session the browser
    /// read does (`soundcraft_engine::io::open_cloud_bytes`), except that call adopts no local
    /// path: a cloud document has nowhere on this machine's disk that it is "at", so it must not
    /// gain a File > Open Recent entry or have a later plain Save write its bundle name to disk.
    fn open_bytes(&mut self, name: &str, bytes: Vec<u8>) -> Result<Option<u64>, String> {
        let missing = soundcraft_engine::io::open_cloud_bytes(&mut self.engine, name, &bytes)
            .map_err(|error| crate::localization::error_text(&error.to_string()))?;
        self.document_replaced();
        if !missing.is_empty() {
            self.ui.status = crate::localized_format!("{missing} missing media after opening from the cloud", missing = missing.len());
        }
        Ok(Some(DOC))
    }

    /// The session as bytes to send to `into`. The session this encodes is a clone with its name
    /// taken from the destination's file name, same as a local Save As.
    ///
    /// `session_bundle` also returns a session with its source paths rewritten to point inside
    /// the bundle it just built; that rewrite is for something that can open and play back that
    /// exact bundle file. This method discards it and leaves `self.engine`'s own session alone:
    /// the local audio files it already points at have not moved, and no bundle was written to
    /// this disk for them to point into instead.
    fn encode(&mut self, _uid: u64, into: Destination<'_>) -> Result<Vec<u8>, String> {
        let mut session = self.engine.session().clone();
        if let Some(stem) = std::path::Path::new(into.name).file_stem().and_then(std::ffi::OsStr::to_str).filter(|stem| !stem.is_empty()) {
            session.name = stem.to_owned();
        }
        let (bytes, _rewritten_for_the_bundle) =
            soundcraft_engine::io::session_bundle(&session).map_err(|error| crate::localization::error_text(&error.to_string()))?;
        Ok(bytes)
    }

    /// Nothing is held per document identity beyond the one open session, which `open_bytes`
    /// already replaced before this would be called.
    fn close(&mut self, _uid: u64) {}

    /// The write landed. The document's dirty flag is content-based (`Engine::is_dirty`), so a
    /// save that started here and is still on the wire must not be reported clean until this is
    /// called, which `craft_ui::cloud` does only once `Event::Saved(Ok(_))` actually arrives.
    fn mark_saved(&mut self, _uid: u64) {
        self.engine.mark_clean();
    }

    fn sharing(&mut self) -> &mut craft_ui::share::ShareDialog {
        &mut self.sharing
    }

    fn presence(&mut self) -> &mut craft_ui::presence::PresenceBar {
        &mut self.presence
    }

    fn paint_icon(&self, painter: &egui::Painter, rect: egui::Rect, kind: EntryKind, colour: egui::Color32) {
        crate::icons::draw(painter, rect, if kind == EntryKind::Folder { "cloud_folder" } else { "cloud_document" }, colour);
    }

    /// Queued rather than opened directly: this method takes `&self`, with no egui context to
    /// open a tab from. The next UI pass drains it where one is in hand (`SoundApp::ui`), the same
    /// queue-now-open-later shape `updates_ui` already uses for its own "visit this page" links.
    fn open_url(&self, url: &str) {
        self.pending_cloud_url.set(Some(url.to_owned()));
    }

    fn say(&mut self, text: String) {
        self.ui.status = text;
    }

    fn toast(&mut self, text: String) {
        self.ui.status = text;
    }

    /// A refused save answered with "Save as a copy" jumps straight to the cloud Save As page: the
    /// conflict was on a cloud document, so the copy belongs there too, not on a local-file prompt
    /// the user never asked for.
    fn go_to_save_as(&mut self) {
        self.dialogs.set(crate::dialogs::Dialog::Cloud { mode: crate::dialogs::CloudMode::SaveAs });
    }

    fn app_name(&self) -> &str {
        "SoundCraft"
    }

    fn tr(&self, text: &str) -> String {
        crate::localization::text(text).to_owned()
    }

    fn save_note(&self) -> Option<String> {
        Some(crate::localization::text("Saves a SoundCraft session (.scraft).").to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Services, SoundApp};
    use craft_cloud::Entry;
    use craft_cloud_local::LocalBackend;

    fn app() -> SoundApp {
        SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default())
    }

    fn pumped(app: &mut SoundApp) {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| craft_ui::cloud::pump(app, ui.ctx()));
        output.textures_delta.clear();
    }

    /// Pumps until `done` answers true, bounded. The reference backend answers every job on its
    /// own thread, so a single pump right after submitting one can race the answer.
    fn pump_until(app: &mut SoundApp, done: impl Fn(&SoundApp) -> bool) {
        for _ in 0..200 {
            pumped(app);
            if done(app) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        pumped(app);
    }

    fn connected(app: &mut SoundApp) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        app.cloud_mut().connect(LocalBackend::new(dir.path().to_path_buf()));
        pumped(app);
        dir
    }

    /// With no cloud backend connected (the real production state: nothing in `storytold` answers
    /// `CloudBackend` yet), every cloud surface this app offers stays hidden.
    #[test]
    fn with_no_backend_connected_nothing_cloud_shaped_is_offered() {
        let mut app = app();
        assert!(!craft_ui::cloud::available(&app));
        assert!(!app.sharing().connected());
    }

    /// Opening from the cloud never adopts a local path or a recents entry: `engine.path` is the
    /// only thing recents tracking (`note_recent_session`) watches, and it must stay as it was.
    #[test]
    fn opening_from_the_cloud_never_adopts_a_path_or_a_recent_entry() {
        let mut app = app();
        let dir = connected(&mut app);
        // A document for the fixture folder to list, written through the same `encode` the cloud
        // Save As page would call, so the bytes this test opens are exactly what this app saves.
        let bytes = app.encode(DOC, Destination::new("Song.scraft")).unwrap();
        std::fs::write(dir.path().join("Song.scraft"), &bytes).unwrap();

        craft_ui::cloud::browse(&mut app, "");
        pump_until(&mut app, |app| app.cloud().mine.loaded);
        let found = app.cloud().mine.entries.iter().find(|e| e.name == "Song.scraft").cloned().expect("the file just written is listed");
        let before_path = app.engine.path.clone();
        let before_recents = app.ui.recent_sessions.clone();

        craft_ui::cloud::open_entry(&mut app, found, None);
        pump_until(&mut app, |app| app.cloud().busy.is_none() && app.cloud().origins.contains_key(&DOC));
        app.note_recent_session();
        assert_eq!(app.engine.path, before_path, "a cloud open must not adopt a local path");
        assert_eq!(app.ui.recent_sessions, before_recents, "a cloud open must not gain a recents entry");
    }

    /// A save still on the wire must not read as saved: `is_dirty` is false only once the mock
    /// backend's write actually completes and `pump` delivers it, which is what calls
    /// `mark_saved`. A host that marked clean when the request was merely sent (rather than when
    /// it landed) would let a quit-time prompt skip a save that was never written.
    #[test]
    fn a_save_in_flight_is_not_reported_clean_until_it_completes() {
        let mut app = app();
        let _dir = connected(&mut app);
        app.engine.session_mut().name = "Changed".into();
        assert!(app.engine.is_dirty());

        craft_ui::cloud::save_as(&mut app, "", "Song.scraft").unwrap();
        assert!(app.engine.is_dirty(), "the write has not landed yet");

        // The reference backend answers on its own thread; pump until the completion lands.
        pump_until(&mut app, |app| app.cloud().busy.is_none());
        assert!(!app.engine.is_dirty(), "the write has now landed and mark_saved ran");
        assert!(app.cloud().origins.contains_key(&DOC));
    }

    /// A conflicted save answered with "Save as a copy" lands on the cloud Save As page, not a
    /// local-file prompt: `go_to_save_as` is reached only when the conflict itself was on a cloud
    /// document.
    #[test]
    fn save_as_a_copy_after_a_conflict_opens_the_cloud_save_page() {
        let mut app = app();
        connected(&mut app);
        app.cloud_mut().conflict = Some(craft_ui::cloud::Conflict {
            uid: DOC,
            entry: Entry {
                id: "x".into(),
                room: "x".into(),
                name: "Song.scraft".into(),
                path: "/Song.scraft".into(),
                kind: EntryKind::File,
                size: None,
                modified: None,
                mime: None,
                etag: None,
                writable: true,
                shared_by: None,
            },
        });
        // `conflict_modal`'s "Save as a copy" button runs exactly this call; exercised directly
        // here rather than through a painted frame, which a bare `Context` cannot provide.
        app.go_to_save_as();
        assert!(matches!(app.dialogs.open, Some(crate::dialogs::Dialog::Cloud { mode: crate::dialogs::CloudMode::SaveAs })));
    }
}
