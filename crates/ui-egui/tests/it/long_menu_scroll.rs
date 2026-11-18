//! A main menu taller than the window scrolls, so its last items can be reached (#1482).

use egui::{Event, Modifiers, MouseWheelUnit, PointerButton, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soundcraft_ui_egui::{Services, SoundApp, menus};

const SIZE: egui::Vec2 = vec2(1200.0, 420.0);

fn step(h: &mut Harness<'_, SoundApp>) {
    let mut raw = std::mem::take(h.input_mut());
    let ctx = h.ctx.clone();
    h.state_mut().raw_input_hook(&ctx, &mut raw);
    *h.input_mut() = raw;
    h.step();
}

#[test]
fn a_long_main_menu_scrolls_to_its_last_item() {
    let app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let mut h = Harness::builder().with_size(SIZE).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..4 {
        step(&mut h);
    }
    let options = menus::tree().into_iter().find(|r| r.path == "Options").unwrap();
    // The menu must scroll all the way to its last entry, a leaf like "Low Latency Monitoring".
    let last = options.children.last().unwrap().label.clone();

    let at = h.get_by_label("Options").rect().center();
    let s = &mut h.state_mut().synthetic;
    s.push(Event::PointerMoved(at));
    s.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    s.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    for _ in 0..6 {
        step(&mut h);
    }
    let bottom = |h: &Harness<'_, SoundApp>| h.get_by_label_contains(&last).rect().bottom();
    assert!(bottom(&h) > SIZE.y, "the Options menu is taller than the window, so `{last}` starts out of view");

    // Over the top of the open menu, which drops down below its title.
    h.state_mut().synthetic.push(Event::PointerMoved(at + vec2(80.0, 120.0)));
    step(&mut h);
    for _ in 0..40 {
        h.state_mut().synthetic.push(Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: vec2(0.0, -3.0),
            modifiers: Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
        });
        step(&mut h);
    }
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(bottom(&h) <= SIZE.y, "scrolling the menu brings `{last}` into view (bottom at {})", bottom(&h));
}
