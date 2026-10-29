use super::plugin_menu::{MenuSearch, menu_row};
use super::{Route, SoundApp, Track, TrackId, Ui, json};

pub(crate) fn route_name(app: &SoundApp, r: &Route) -> String {
    match r {
        Route::None => "no output".into(),
        Route::Main => app.engine.session().outputs.first().map_or_else(|| "Out 1-2".into(), |o| o.name.clone()),
        Route::Bus(b) => app.engine.session().bus(*b).map_or_else(|| "bus?".into(), |b| b.name.clone()),
        Route::Hardware(h) => h.clone(),
    }
}

pub(crate) fn input_name(app: &SoundApp, track: &Track) -> String {
    if track.kind.has_playlist() && track.mixer.input == Route::None { "In 1".to_string() } else { route_name(app, &track.mixer.input) }
}

/// The input or output menu under `resp`. It closes only on a click outside, so its bus search
/// field can be typed in.
pub(crate) fn route_popup(app: &mut SoundApp, resp: &egui::Response, id: TrackId, input: bool) {
    egui::Popup::menu(resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| route_menu(app, ui, id, input));
}

/// Input or output menu: no route and buses on both, plus the main output (outputs) or inputs 1-8 (inputs).
fn route_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId, input: bool) {
    let key = if input { "input" } else { "output" };
    let cmd = if input { "track.input" } else { "track.output" };
    if ui.button(if input { "no input" } else { "no output" }).clicked() {
        let _ = app.run(cmd, json!({"track": id.0, key: "none"}));
        ui.close();
    }
    let main_name = app.engine.session().outputs.first().map_or_else(|| "Out 1-2".to_string(), |o| o.name.clone());
    if !input && ui.button(main_name).clicked() {
        let _ = app.run(cmd, json!({"track": id.0, key: "main"}));
        ui.close();
    }
    if input {
        for i in 1..=8 {
            if ui.button(format!("In {i}")).clicked() {
                let _ = app.run(cmd, json!({"track": id.0, key: format!("In {i}")}));
                ui.close();
            }
        }
    }
    ui.separator();
    let busses: Vec<String> = app.engine.session().busses.iter().map(|b| b.name.clone()).collect();
    ui.strong("Bus");
    let search = MenuSearch::show(ui, egui::Id::unique(("route-bus-search", id.0, input)), "Search buses…");
    let visible: Vec<&String> = busses.iter().filter(|b| search.matches(b)).collect();
    if let Some(i) = search.rows(ui, visible.len(), 420.0, |ui, i, highlighted| menu_row(ui, highlighted, visible[i])) {
        let _ = app.run(cmd, json!({"track": id.0, key: visible[i]}));
        ui.close();
    }
    if ui.button("new bus...").clicked() {
        let mut n = busses.len() + 1;
        while busses.contains(&format!("Bus {n}")) || app.engine.session().track_by_name(&format!("Bus {n}")).is_some() {
            n += 1;
        }
        let _ = app.run(cmd, json!({"track": id.0, key: format!("Bus {n}")}));
        ui.close();
    }
}
