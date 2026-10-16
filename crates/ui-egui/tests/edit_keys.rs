use egui::os::OperatingSystem;
use egui::{Event, Key, Modifiers, OutputCommand};
use serde_json::json;
use soundcraft_ui_egui::{MainWindow, Services, SoundApp, shortcuts};

const MAC_CMD: Modifiers = Modifiers { mac_cmd: true, command: true, ..Modifiers::NONE };

fn send(ctx: &egui::Context, app: &mut SoundApp, event: Event, modifiers: Modifiers) -> Vec<OutputCommand> {
    let input = egui::RawInput { events: vec![Event::ModifiersChanged(modifiers), event], ..Default::default() };
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx());
        shortcuts::handle(app, ui.ctx());
    });
    out.textures_delta.clear();
    out.platform_output.commands
}

fn demo() -> SoundApp {
    SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default())
}

fn kick_clips(app: &SoundApp) -> usize {
    app.engine.session().track_by_name("Kick").unwrap().clips().len()
}

fn select_first_kick_clip(app: &mut SoundApp) {
    let first = app.engine.session().track_by_name("Kick").unwrap().clips()[0].id.0;
    app.run("edit.select", json!({"clips": [first]})).unwrap();
}

/// The commands an event ran; the system clipboard must stay untouched.
fn ran(ctx: &egui::Context, app: &mut SoundApp, event: Event, modifiers: Modifiers) -> Vec<String> {
    let before = app.engine.journal.len();
    let out = send(ctx, app, event, modifiers);
    assert!(!out.iter().any(|c| matches!(c, OutputCommand::CopyText(_))));
    app.engine.journal[before..].iter().map(|(id, _)| id.clone()).collect()
}

#[test]
fn the_desktop_clipboard_keys_cut_copy_and_paste_clips() {
    let pc_ctrl = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    let v = |pressed, modifiers| Event::Key { key: Key::V, physical_key: None, pressed, repeat: false, modifiers };
    let none: [&str; 0] = [];
    for (os, cmd) in [
        (OperatingSystem::Mac, MAC_CMD),
        (OperatingSystem::Windows, pc_ctrl),
        (OperatingSystem::Windows, Modifiers::SHIFT),
        (OperatingSystem::Nix, pc_ctrl),
    ] {
        let (mut app, ctx) = (demo(), egui::Context::default());
        ctx.set_os(os);
        let clips = kick_clips(&app);
        select_first_kick_clip(&mut app);
        assert_eq!(ran(&ctx, &mut app, Event::Copy, cmd), ["edit.copy"], "{os:?} {cmd:?}");
        app.run("edit.select", json!({"tracks": ["Kick"], "start": 0})).unwrap();
        assert_eq!(ran(&ctx, &mut app, Event::Paste("text".into()), cmd), ["edit.paste"], "{os:?} {cmd:?}");
        assert_eq!(kick_clips(&app), clips + 1, "{os:?} {cmd:?}");
        if cmd.command {
            assert_eq!(ran(&ctx, &mut app, v(false, cmd), cmd), none, "{os:?}: the release of a Cmd+V that pasted");
            assert_eq!(ran(&ctx, &mut app, v(false, cmd), cmd), ["edit.paste"], "{os:?}: Cmd+V with no text on the clipboard");
            assert_eq!(ran(&ctx, &mut app, v(false, Modifiers::NONE), Modifiers::NONE), ["edit.paste"], "{os:?}: Cmd let go first");
            send(&ctx, &mut app, v(true, Modifiers::NONE), Modifiers::NONE);
            assert_eq!(ran(&ctx, &mut app, v(false, Modifiers::NONE), Modifiers::NONE), none, "{os:?}: the release of a V that arrived");
        }
        select_first_kick_clip(&mut app);
        assert_eq!(ran(&ctx, &mut app, Event::Cut, cmd), ["edit.cut"], "{os:?} {cmd:?}");
    }
}

#[test]
fn delete_and_backspace_clear_the_selected_clip_unless_midi_notes_take_them() {
    for key in [Key::Delete, Key::Backspace] {
        for (window, editor, notes, cleared) in [
            (MainWindow::Edit, false, vec![], true),
            (MainWindow::Edit, false, vec![0], true),
            (MainWindow::Edit, true, vec![], true),
            (MainWindow::Edit, true, vec![0], false),
            (MainWindow::Mix, false, vec![], false),
        ] {
            let mut app = demo();
            let clips = kick_clips(&app);
            select_first_kick_clip(&mut app);
            (app.ui.window, app.ui.show_midi_editor, app.midi.selected) = (window, editor, notes);
            let event = Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
            send(&egui::Context::default(), &mut app, event, Modifiers::NONE);
            assert_eq!(kick_clips(&app), clips - usize::from(cleared), "{key:?} {window:?}, editor open: {editor}, notes: {:?}", app.midi.selected);
        }
    }
}
