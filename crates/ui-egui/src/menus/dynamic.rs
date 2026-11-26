//! The menu bar's submenus whose entries change while the app runs, File › Open Recent and
//! Track › Instrument, as data for a host that draws its own menu bar: the macOS system menu
//! (`apps/soundcraft/src/native_menu.rs`). The egui menu bar draws the same submenus itself
//! (`menus::menu_bar`); both act through the same commands.
use crate::SoundApp;
use crate::localization::text;
use serde_json::json;
use soundcraft_model::TrackId;

/// The submenus, each shown right after the menu leaf at `after`, in the order [`submenus`]
/// returns them.
pub const SUBMENUS: [Submenu; 2] = [Submenu { after: "File > Open", label: "Open Recent" }, Submenu { after: "Track > New...", label: "Instrument" }];

/// Where one of [`SUBMENUS`] goes and its English label (a translation key).
#[derive(Clone, Copy, Debug)]
pub struct Submenu {
    pub after: &'static str,
    pub label: &'static str,
}

/// One entry of a submenu.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// A leaf; `action` is `None` for a heading that only shows text.
    Item {
        label: String,
        enabled: bool,
        checked: bool,
        action: Option<Action>,
    },
    Submenu {
        label: String,
        entries: Vec<Entry>,
    },
    Separator,
}

impl Entry {
    pub(crate) fn item(label: &str, action: Action) -> Self {
        Self::Item { label: label.to_string(), enabled: true, checked: false, action: Some(action) }
    }

    pub(crate) fn disabled(label: &str) -> Self {
        Self::Item { label: label.to_string(), enabled: false, checked: false, action: None }
    }

    fn submenu(label: &str, entries: Vec<Entry>) -> Self {
        Self::Submenu { label: label.to_string(), entries }
    }
}

/// What picking an entry does; [`run`] carries it out.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    OpenRecent(String),
    ClearRecent,
    /// `document` is the session the track belongs to (`SoundApp::doc_epoch`): a session opened
    /// since can reuse the track's id.
    SetInstrument {
        document: u64,
        track: TrackId,
        plugin: String,
    },
    /// Opens a built-in instrument's parameter window.
    EditInstrument {
        document: u64,
        track: TrackId,
    },
    ScanClap,
    ScanVst3,
}

/// What one of [`SUBMENUS`] shows: its title in the interface language, whether it is enabled,
/// and its entries.
#[derive(Clone, Debug, PartialEq)]
pub struct Contents {
    pub title: String,
    pub enabled: bool,
    pub entries: Vec<Entry>,
}

/// Each of [`SUBMENUS`], in the interface language. A host calls this outside
/// [`SoundApp::logic`], whose language scope has ended by then, so it enters its own.
pub fn submenus(app: &mut SoundApp) -> [Contents; 2] {
    let _language = crate::localization::enter(app.ui.language);
    // The egui menu bar disables every entry while Preferences is open; so does this one.
    let open = !app.ui.show_preferences;
    let recent = crate::recent::entries(app);
    let (has_track, instrument) = instrument(app);
    let [r, i] = SUBMENUS.map(|d| text(d.label).to_string());
    [Contents { title: r, enabled: open, entries: recent }, Contents { title: i, enabled: open && has_track, entries: instrument }]
}

/// Carries out a picked entry; a failure goes to the status line, as from the egui menus.
///
/// A host's menu can be up to one refresh behind the app, so the pick is checked against the
/// app as it is now. While Preferences is open nothing runs, as for every other menu entry then
/// disabled. An instrument entry names the session and track that were selected when the submenu
/// was built; if either has changed, the pick is refused rather than applied to a track the user
/// no longer has selected, and the status line says so.
pub fn run(app: &mut SoundApp, action: &Action) {
    if app.ui.show_preferences {
        return;
    }
    let _language = crate::localization::enter(app.ui.language);
    if let Action::SetInstrument { document, track, .. } | Action::EditInstrument { document, track } = action
        && (*document != app.doc_epoch || super::instrument_track(app.engine.session()) != Some(*track))
    {
        app.ui.status = text("The selected track changed: choose the instrument again").to_string();
        return;
    }
    match action {
        Action::OpenRecent(path) => crate::recent::open_session(app, path),
        Action::ClearRecent => crate::recent::clear(app),
        Action::SetInstrument { track, plugin, .. } => {
            if let Err(e) = app.run("mix.instrument", json!({"track": track.0, "plugin": plugin})) {
                app.ui.status = e;
            }
        }
        Action::EditInstrument { track, .. } => {
            let w = (*track, soundcraft_mix::INSTRUMENT_SLOT);
            if !app.ui.plugin_windows.contains(&w) {
                app.ui.plugin_windows.push(w);
            }
        }
        Action::ScanClap => {
            soundcraft_clap_host::scan();
        }
        Action::ScanVst3 => {
            soundcraft_vst3_host::scan();
        }
    }
}

/// Track › Instrument for the selected instrument track: its instrument's name (and, for a
/// built-in one, its parameters), then the built-in and hosted instruments, the current one
/// checked. Disabled and empty with no instrument track selected.
fn instrument(app: &SoundApp) -> (bool, Vec<Entry>) {
    let Some(track) = super::instrument_track(app.engine.session()) else {
        return (false, Vec::new());
    };
    let document = app.doc_epoch;
    let current = app.engine.session().track(track).and_then(|t| t.instrument.as_ref()).map(|i| i.plugin.clone());
    let mut out = Vec::new();
    if let Some(cur) = current.as_deref()
        && let Some(info) = crate::mix_window::plugin_info(cur)
    {
        out.push(Entry::disabled(info.name));
        if !soundcraft_mix::is_third_party(cur) {
            out.push(Entry::item(text("Edit Parameters…"), Action::EditInstrument { document, track }));
        }
        out.push(Entry::Separator);
    }
    let pick = |id: &str, name: &str| Entry::Item {
        label: name.to_string(),
        enabled: true,
        checked: current.as_deref() == Some(id),
        action: Some(Action::SetInstrument { document, track, plugin: id.to_string() }),
    };
    let built_in = soundcraft_dsp::plugins().iter().filter(|p| p.is_instrument).map(|p| pick(p.id, p.name)).collect();
    out.push(Entry::submenu(text("Built-in"), built_in));
    // CLAP and VST3 scans load every plugin binary, so they run only when asked; Audio Units are
    // listed without creating instances, as the egui menu does.
    let clap = soundcraft_clap_host::cached()
        .map(|l| l.into_iter().filter(|d| d.is_instrument).map(|d| Hosted { id: d.id, name: d.name, vendor: d.vendor }).collect());
    out.push(hosted(text("CLAP"), clap, Some((text("Scan CLAP"), Action::ScanClap)), text("No CLAP plugins found"), &pick));
    let vst3 = soundcraft_vst3_host::cached()
        .map(|l| l.into_iter().filter(|d| d.is_instrument).map(|d| Hosted { id: d.id, name: d.name, vendor: d.vendor }).collect());
    out.push(hosted(text("VST3"), vst3, Some((text("Scan VST3"), Action::ScanVst3)), text("No VST3 plugins found"), &pick));
    if cfg!(target_os = "macos") {
        let au =
            soundcraft_au_host::scan().into_iter().filter(|d| d.is_instrument).map(|d| Hosted { id: d.id, name: d.name, vendor: d.vendor }).collect();
        out.push(hosted(text("Audio Units"), Some(au), None, text("No Audio Units found"), &pick));
    }
    (true, out)
}

/// A hosted instrument as the submenu lists it.
struct Hosted {
    id: String,
    name: String,
    vendor: String,
}

/// One plugin format's submenu: a scan entry until it has been scanned, then one submenu per
/// vendor, as the egui menu lists them.
fn hosted(label: &str, list: Option<Vec<Hosted>>, scan: Option<(&str, Action)>, none: &str, pick: &dyn Fn(&str, &str) -> Entry) -> Entry {
    let entries = match (list, scan) {
        (None, Some((scan_label, action))) => vec![Entry::item(scan_label, action)],
        (None, None) => vec![Entry::disabled(none)],
        (Some(list), _) if list.is_empty() => vec![Entry::disabled(none)],
        (Some(mut list), _) => {
            list.sort_by_cached_key(|p| (p.vendor.to_lowercase(), p.name.to_lowercase()));
            let mut vendors: Vec<Entry> = Vec::new();
            let mut last: Option<&str> = None;
            for p in &list {
                let row = pick(&p.id, &p.name);
                match vendors.last_mut() {
                    Some(Entry::Submenu { entries, .. }) if last == Some(p.vendor.as_str()) => entries.push(row),
                    _ => {
                        let v = if p.vendor.is_empty() { text("Unknown vendor") } else { p.vendor.as_str() };
                        vendors.push(Entry::submenu(v, vec![row]));
                        last = Some(p.vendor.as_str());
                    }
                }
            }
            vendors
        }
    };
    Entry::submenu(label, entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;

    fn app() -> SoundApp {
        SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default())
    }

    /// The entries that set an instrument, anywhere in `entries`.
    fn picks(entries: &[Entry]) -> Vec<(&str, bool, &Action)> {
        let mut out = Vec::new();
        for e in entries {
            match e {
                Entry::Item { label, checked, action: Some(a @ Action::SetInstrument { .. }), .. } => out.push((label.as_str(), *checked, a)),
                Entry::Submenu { entries, .. } => out.extend(picks(entries)),
                _ => {}
            }
        }
        out
    }

    #[test]
    fn each_submenu_follows_an_entry_of_the_menu_tree() {
        fn has(nodes: &[crate::menus::MenuNode], path: &str) -> bool {
            nodes.iter().any(|n| n.path == path || has(&n.children, path))
        }
        let tree = crate::menus::tree();
        for d in SUBMENUS {
            assert!(has(&tree, d.after), "`{}` has no `{}` to follow", d.label, d.after);
        }
    }

    #[test]
    fn instrument_submenu_needs_a_selected_instrument_track() {
        let mut app = app();
        let audio = app.run("track.new", json!({"kind": "audio"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.run("edit.select", json!({"tracks": [audio]})).unwrap();
        let [_, Contents { enabled, entries, .. }] = submenus(&mut app);
        assert!(!enabled && entries.is_empty());
        app.ui.show_preferences = true;
        let inst = app.run("track.new", json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.run("edit.select", json!({"tracks": [inst]})).unwrap();
        assert!(!submenus(&mut app)[1].enabled, "disabled while Preferences is open, as the egui menu bar is");
    }

    #[test]
    fn picking_the_sampler_makes_it_the_tracks_instrument() {
        let mut app = app();
        let inst = app.run("track.new", json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.run("edit.select", json!({"tracks": [inst]})).unwrap();
        let [_, Contents { enabled, entries, .. }] = submenus(&mut app);
        assert!(enabled);
        let before: Vec<_> = picks(&entries).into_iter().filter(|p| p.1).map(|p| p.0.to_string()).collect();
        assert_eq!(before.len(), 1, "exactly the current instrument is checked");
        let sampler = picks(&entries).into_iter().find(|p| p.0 == "Sampler").map(|p| p.2.clone()).expect("Built-in lists the Sampler");
        assert!(!before.contains(&"Sampler".to_string()));
        run(&mut app, &sampler);
        let track = app.engine.session().tracks.iter().find(|t| t.id.0 == inst).unwrap();
        assert_eq!(track.instrument.as_ref().map(|i| i.plugin.as_str()), Some("sampler"));
        let [_, Contents { entries, .. }] = submenus(&mut app);
        let now: Vec<_> = picks(&entries).into_iter().filter(|p| p.1).map(|p| p.0).collect();
        assert_eq!(now, ["Sampler"], "the check follows the pick");
    }

    #[test]
    fn open_recent_lists_sessions_and_clears() {
        let mut app = app();
        let [Contents { enabled, entries, .. }, _] = submenus(&mut app);
        assert!(enabled);
        assert!(matches!(entries.as_slice(), [Entry::Item { enabled: false, action: None, .. }]), "only a disabled note");
        app.ui.recent_sessions = vec!["/nowhere/Song.scraft".into()];
        let [Contents { entries, .. }, _] = submenus(&mut app);
        assert!(
            entries
                .iter()
                .any(|e| matches!(e, Entry::Item { action: Some(Action::OpenRecent(p)), enabled: false, .. } if p == "/nowhere/Song.scraft"))
        );
        run(&mut app, &Action::ClearRecent);
        assert!(app.ui.recent_sessions.is_empty());
    }

    #[test]
    fn a_session_that_reappears_is_enabled_once_the_check_expires() {
        let mut app = app();
        let dir = std::env::temp_dir().join(format!("sc-recent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Back.scraft");
        let _ = std::fs::remove_file(&path);
        app.ui.recent_sessions = vec![path.to_string_lossy().into_owned()];
        let enabled = |app: &mut SoundApp| {
            let [Contents { entries, .. }, _] = submenus(app);
            entries.iter().any(|e| matches!(e, Entry::Item { action: Some(Action::OpenRecent(_)), enabled: true, .. }))
        };
        app.advance_test_clock(std::time::Duration::ZERO);
        assert!(!enabled(&mut app), "missing, so disabled");
        std::fs::write(&path, b"").unwrap();
        // The app's clock moves on its own: egui's input time stands still while the window is hidden.
        app.advance_test_clock(std::time::Duration::from_secs(60));
        assert!(enabled(&mut app), "re-checked once the app clock has moved past the check's lifetime");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_instrument_pick_for_a_track_no_longer_selected_is_refused() {
        let mut app = app();
        let new = |app: &mut SoundApp| app.run("track.new", json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        let (a, b) = (new(&mut app), new(&mut app));
        app.run("edit.select", json!({"tracks": [a]})).unwrap();
        let [_, Contents { entries, .. }] = submenus(&mut app);
        let sampler = picks(&entries).into_iter().find(|p| p.0 == "Sampler").map(|p| p.2.clone()).unwrap();
        let instrument =
            |app: &SoundApp, t: u64| app.engine.session().tracks.iter().find(|x| x.id.0 == t).and_then(|x| x.instrument.clone()).map(|i| i.plugin);
        let before = (instrument(&app, a), instrument(&app, b));
        app.run("edit.select", json!({"tracks": [b]})).unwrap();
        app.ui.status.clear();
        run(&mut app, &sampler);
        assert_eq!((instrument(&app, a), instrument(&app, b)), before, "neither track changed");
        assert!(!app.ui.status.is_empty(), "the refusal is reported");
        app.run("edit.select", json!({"tracks": [a]})).unwrap();
        run(&mut app, &sampler);
        assert_eq!(instrument(&app, a).as_deref(), Some("sampler"), "applied while the selection still matches");
    }

    #[test]
    fn an_instrument_pick_from_a_replaced_session_is_refused() {
        let mut app = app();
        let first = app.run("track.new", json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.run("edit.select", json!({"tracks": [first]})).unwrap();
        let [_, Contents { entries, .. }] = submenus(&mut app);
        let sampler = picks(&entries).into_iter().find(|p| p.0 == "Sampler").map(|p| p.2.clone()).unwrap();
        app.run("session.new", json!({})).unwrap();
        let second = app.run("track.new", json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        assert_eq!(second, first, "the new session reuses the id, so only the session tells them apart");
        app.run("edit.select", json!({"tracks": [second]})).unwrap();
        let plugin =
            |app: &SoundApp| app.engine.session().tracks.iter().find(|x| x.id.0 == second).and_then(|x| x.instrument.clone()).map(|i| i.plugin);
        let before = plugin(&app);
        app.ui.status.clear();
        run(&mut app, &sampler);
        assert_eq!(plugin(&app), before, "the new session's track is untouched");
        assert!(!app.ui.status.is_empty(), "the refusal is reported");
    }

    #[test]
    fn nothing_runs_while_preferences_is_open() {
        let mut app = app();
        app.ui.recent_sessions = vec!["/nowhere/Song.scraft".into()];
        app.ui.show_preferences = true;
        run(&mut app, &Action::ClearRecent);
        assert_eq!(app.ui.recent_sessions.len(), 1, "a pick from a menu one refresh behind is ignored");
        app.ui.show_preferences = false;
        run(&mut app, &Action::ClearRecent);
        assert!(app.ui.recent_sessions.is_empty());
    }

    #[test]
    fn submenus_follow_the_interface_language() {
        let mut app = app();
        let english = submenus(&mut app);
        app.ui.language = crate::localization::Language::PortugueseBrazil;
        let other = submenus(&mut app);
        for (e, o) in english.iter().zip(&other) {
            assert_ne!(e.title, o.title, "the title is translated");
        }
        assert_ne!(english[0].entries, other[0].entries, "so are the entries (here: No Recent Sessions)");
    }
}
