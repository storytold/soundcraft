use super::{Color32, Rect, SoundApp, TrackId, Ui, json, pos2, vec2};

/// An insert's menu: a search field that takes typing at once, then (until something is typed)
/// Bypass and removal and the plugins by category and format.
pub fn plugin_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId, slot: usize, occupied: bool) {
    let search = MenuSearch::show(ui, egui::Id::unique(("plugin-search", id.0, slot)), "Search plugins…");
    if search.typing() {
        if let Some(plugin) = plugin_search(ui, &search, false) {
            pick_insert(app, ui, id, slot, &plugin);
        }
        return;
    }
    if occupied {
        if ui.button("Bypass").clicked() {
            run_pick(app, ui, "mix.insert_bypass", json!({"track": id.0, "slot": slot}));
        }
        if ui.button("no insert").clicked() {
            run_pick(app, ui, "mix.insert_remove", json!({"track": id.0, "slot": slot}));
        }
        ui.separator();
    }
    let mut cats: Vec<soundcraft_dsp::Category> = Vec::new();
    for p in soundcraft_dsp::plugins() {
        if !p.is_instrument && !cats.contains(&p.category) {
            cats.push(p.category);
        }
    }
    for c in cats {
        ui.menu_button(format!("{c:?}"), |ui| {
            for p in soundcraft_dsp::plugins().iter().filter(|p| p.category == c && !p.is_instrument) {
                if ui.button(p.name).clicked() {
                    pick_insert(app, ui, id, slot, p.id);
                }
            }
        });
    }
    if let Some(plugin) = hosted_menus(ui, false) {
        pick_insert(app, ui, id, slot, &plugin);
    }
}

/// The instrument picker of an instrument track: a search field that takes typing at once, then
/// (until something is typed) its instrument's editor and the built-in, CLAP, VST3 and Audio Units
/// instruments.
pub fn instrument_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId) {
    let slot = soundcraft_mix::INSTRUMENT_SLOT;
    let search = MenuSearch::show(ui, egui::Id::unique(("instrument-search", id.0)), "Search instruments…");
    if search.typing() {
        if let Some(plugin) = plugin_search(ui, &search, true) {
            run_pick(app, ui, "mix.instrument", json!({"track": id.0, "plugin": plugin}));
        }
        return;
    }
    let current = app.engine.session().track(id).and_then(|t| t.instrument.clone());
    if let Some(info) = current.as_ref().and_then(|i| plugin_info(&i.plugin)) {
        ui.label(egui::RichText::new(info.name).strong());
        if current.as_ref().is_some_and(|i| !soundcraft_mix::is_third_party(&i.plugin)) && ui.button("Edit Parameters…").clicked() {
            if !app.ui.plugin_windows.contains(&(id, slot)) {
                app.ui.plugin_windows.push((id, slot));
            }
            ui.close();
        }
        if let Some(p) = &app.player
            && p.has_editor(id, slot)
        {
            if p.editor_open(id, slot) {
                if ui.button("Close Plugin Editor").clicked() {
                    p.close_editor(id, slot);
                    ui.close();
                }
            } else if ui.button("Open Plugin Editor").clicked() {
                if let Err(e) = p.open_editor(id, slot) {
                    app.ui.status = format!("{}: {e}", info.name);
                }
                ui.close();
            }
        }
        ui.separator();
    }
    let mut picked = None;
    ui.menu_button("Built-in", |ui| {
        for p in soundcraft_dsp::plugins().iter().filter(|p| p.is_instrument) {
            if ui.button(p.name).clicked() {
                picked = Some(p.id.to_string());
            }
        }
    });
    if let Some(p) = hosted_menus(ui, true) {
        picked = Some(p);
    }
    if let Some(plugin) = picked {
        run_pick(app, ui, "mix.instrument", json!({"track": id.0, "plugin": plugin}));
    }
}

/// Inserts the plugin picked in an insert's menu and, as DAWs do, opens it once the audio engine has
/// it (`SoundApp::open_inserted`).
fn pick_insert(app: &mut SoundApp, ui: &Ui, id: TrackId, slot: usize, plugin: &str) {
    match app.run("mix.insert", json!({"track": id.0, "slot": slot, "plugin": plugin})) {
        Ok(_) => app.pending_open = Some((id, slot)),
        Err(e) => app.ui.status = e,
    }
    ui.close();
}

/// Runs a menu's pick, shows its error in the status line and closes the menu.
fn run_pick(app: &mut SoundApp, ui: &Ui, cmd: &str, params: serde_json::Value) {
    if let Err(e) = app.run(cmd, params) {
        app.ui.status = e;
    }
    ui.close();
}

/// The plugins whose name or vendor holds the search text: the built-in ones, then Audio Units
/// (listing them creates no instances) and whichever of CLAP and VST3 have been scanned, with a
/// button to scan each of the others. Returns the id picked.
fn plugin_search(ui: &mut Ui, search: &MenuSearch, instruments: bool) -> Option<String> {
    let mut built_in: Vec<&soundcraft_dsp::PluginInfo> =
        soundcraft_dsp::plugins().iter().copied().filter(|p| p.is_instrument == instruments && search.matches(p.name)).collect();
    built_in.sort_by_cached_key(|p| p.name.to_lowercase());
    let (clap, vst3) = (soundcraft_clap_host::cached(), soundcraft_vst3_host::cached());
    let mut hosted: Vec<(MenuPlugin, &str)> = Vec::new();
    for d in clap.iter().flatten().filter(|d| d.is_instrument == instruments) {
        hosted.push((MenuPlugin { id: d.id.clone(), name: d.name.clone(), vendor: d.vendor.clone() }, "CLAP"));
    }
    for d in vst3.iter().flatten().filter(|d| d.is_instrument == instruments) {
        hosted.push((MenuPlugin { id: d.id.clone(), name: d.name.clone(), vendor: d.vendor.clone() }, "VST3"));
    }
    for d in soundcraft_au_host::scan().into_iter().filter(|d| d.is_instrument == instruments) {
        hosted.push((MenuPlugin { id: d.id, name: d.name, vendor: d.vendor }, "AU"));
    }
    hosted.retain(|(p, _)| search.matches(&p.name) || search.matches(&p.vendor));
    hosted.sort_by_cached_key(|(p, format)| (p.name.to_lowercase(), *format));
    let n = built_in.len() + hosted.len();
    let picked = search.rows(ui, n, 320.0, |ui, i, highlighted| match built_in.get(i) {
        Some(p) => menu_row(ui, highlighted, p.name),
        None => {
            let (p, format) = &hosted[i - built_in.len()];
            let label = if p.vendor.is_empty() { format!("{} ({format})", p.name) } else { format!("{} — {} ({format})", p.name, p.vendor) };
            menu_row(ui, highlighted, &label)
        }
    });
    if n == 0 {
        ui.weak("No matches");
    }
    if clap.is_none() || vst3.is_none() {
        ui.separator();
        ui.horizontal(|ui| {
            if clap.is_none() && ui.small_button("Scan CLAP").clicked() {
                soundcraft_clap_host::scan();
            }
            if vst3.is_none() && ui.small_button("Scan VST3").clicked() {
                soundcraft_vst3_host::scan();
            }
        });
    }
    picked.map(|i| built_in.get(i).map_or_else(|| hosted[i - built_in.len()].0.id.clone(), |p| p.id.to_string()))
}

/// A picker menu's search field. It has the keyboard as soon as the menu opens and starts empty
/// each time; Up/Down move the highlight through the matches and Enter picks it.
pub(super) struct MenuSearch {
    id: egui::Id,
    query: String,
    highlight: usize,
    keys: [bool; 3],
}

impl MenuSearch {
    pub(super) fn show(ui: &mut Ui, id: egui::Id, hint: &str) -> Self {
        let pass = ui.ctx().cumulative_pass_nr();
        // Kept only while the menu stays open: one not shown last pass has just opened.
        let kept = ui.data(|d| d.get_temp::<(String, usize, u64)>(id)).filter(|s| s.2 + 1 >= pass);
        let opened = kept.is_none();
        let (mut text, mut highlight, _) = kept.unwrap_or_default();
        let field = ui.add(egui::TextEdit::singleline(&mut text).id(id.with("field")).hint_text(hint).return_key(None));
        if opened {
            field.request_focus();
        }
        if field.changed() {
            highlight = 0;
        }
        // egui's focus, as typing uses: `has_focus` also wants the window focused, which scripted input lacks.
        let focused = ui.memory(|m| m.has_focus(field.id));
        let keys = [egui::Key::ArrowUp, egui::Key::ArrowDown, egui::Key::Enter].map(|k| focused && ui.input(|i| i.key_pressed(k)));
        ui.data_mut(|d| d.insert_temp(id, (text.clone(), highlight, pass)));
        Self { id, query: text.trim().to_lowercase(), highlight, keys }
    }

    pub(super) fn typing(&self) -> bool {
        !self.query.is_empty()
    }

    pub(super) fn matches(&self, text: &str) -> bool {
        text.to_lowercase().contains(&self.query)
    }

    /// `n` matches in a scrolling list, each drawn by `row` (told whether it is highlighted);
    /// returns the one clicked, or the highlighted one on Enter.
    pub(super) fn rows(&self, ui: &mut Ui, n: usize, max_height: f32, row: impl FnMut(&mut Ui, usize, bool) -> bool) -> Option<usize> {
        let row_h = ui.spacing().interact_size.y;
        egui::ScrollArea::vertical()
            .id_salt(self.id.with("rows"))
            .max_height(max_height)
            .show_rows(ui, row_h, n, |ui, range| self.list_rows(ui, range, n, row))
            .inner
    }

    /// The rows of `range` out of `n` matches, drawn into a list that scrolls and virtualises
    /// itself (a shared dropdown menu's list); `rows` is the same list in a scroll area of its own.
    pub(super) fn list_rows(
        &self,
        ui: &mut Ui,
        range: std::ops::Range<usize>,
        n: usize,
        mut row: impl FnMut(&mut Ui, usize, bool) -> bool,
    ) -> Option<usize> {
        let [up, down, enter] = self.keys;
        let typing = self.typing();
        let highlight = match (up, down) {
            (_, true) => self.highlight + 1,
            (true, _) => self.highlight.saturating_sub(1),
            _ => self.highlight,
        }
        .min(n.saturating_sub(1));
        ui.data_mut(|d| d.get_temp_mut_or_default::<(String, usize, u64)>(self.id).1 = highlight);
        let mut picked = (typing && enter && n > 0).then_some(highlight);
        let row_h = ui.spacing().interact_size.y;
        let step = row_h + ui.spacing().item_spacing.y;
        if typing && (up || down) {
            let top = ui.max_rect().top() + (highlight as f32 - range.start as f32) * step;
            ui.scroll_to_rect(Rect::from_min_size(pos2(ui.max_rect().left(), top), vec2(1.0, row_h)), None);
        }
        for i in range {
            if row(ui, i, typing && i == highlight) {
                picked = Some(i);
            }
        }
        picked
    }
}

pub(super) fn menu_row(ui: &mut Ui, selected: bool, text: &str) -> bool {
    let text = if selected { egui::RichText::new(text).color(Color32::WHITE) } else { egui::RichText::new(text) };
    ui.add(egui::Button::selectable(selected, text).min_size(vec2(0.0, ui.spacing().interact_size.y))).clicked()
}

/// A hosted plugin as the plugin menus list it.
struct MenuPlugin {
    id: String,
    name: String,
    vendor: String,
}

/// The CLAP, VST3 and (on macOS) Audio Units submenus, of effects or of instruments; returns the
/// id picked. Each format is scanned the first time its submenu opens, then cached by its host.
fn hosted_menus(ui: &mut Ui, instruments: bool) -> Option<String> {
    let mut picked = None;
    ui.menu_button("CLAP", |ui| {
        let list = soundcraft_clap_host::scan().into_iter().filter(|d| d.is_instrument == instruments);
        if let Some(p) = vendor_menus(ui, list.map(|d| MenuPlugin { id: d.id, name: d.name, vendor: d.vendor }).collect(), "No CLAP plugins found") {
            picked = Some(p);
        }
    });
    ui.menu_button("VST3", |ui| {
        let list = soundcraft_vst3_host::scan().into_iter().filter(|d| d.is_instrument == instruments);
        if let Some(p) = vendor_menus(ui, list.map(|d| MenuPlugin { id: d.id, name: d.name, vendor: d.vendor }).collect(), "No VST3 plugins found") {
            picked = Some(p);
        }
    });
    if cfg!(target_os = "macos") {
        ui.menu_button("Audio Units", |ui| {
            let list = soundcraft_au_host::scan().into_iter().filter(|d| d.is_instrument == instruments);
            if let Some(p) = vendor_menus(ui, list.map(|d| MenuPlugin { id: d.id, name: d.name, vendor: d.vendor }).collect(), "No Audio Units found")
            {
                picked = Some(p);
            }
        });
    }
    picked
}

/// One submenu per vendor (as Pro Tools lists plug-ins by manufacturer), each list scrolling
/// within the screen so that every plugin stays reachable however many are installed.
fn vendor_menus(ui: &mut Ui, mut list: Vec<MenuPlugin>, none: &str) -> Option<String> {
    if list.is_empty() {
        ui.label(none);
        return None;
    }
    list.sort_by_cached_key(|p| (p.vendor.to_lowercase(), p.name.to_lowercase()));
    let mut vendors: Vec<&str> = list.iter().map(|p| p.vendor.as_str()).collect();
    vendors.dedup();
    let max_h = (ui.ctx().content_rect().height() - 80.0).max(160.0);
    let mut picked = None;
    egui::ScrollArea::vertical().id_salt("vendors").max_height(max_h).show(ui, |ui| {
        for v in vendors {
            ui.menu_button(if v.is_empty() { "Unknown vendor" } else { v }, |ui| {
                egui::ScrollArea::vertical().id_salt("plugins").max_height(max_h).show(ui, |ui| {
                    for p in list.iter().filter(|p| p.vendor == v) {
                        if ui.button(&p.name).clicked() {
                            picked = Some(p.id.clone());
                        }
                    }
                });
            });
        }
    });
    picked
}

/// A built-in plugin's description, else a hosted CLAP (`clap:<id>`), VST3 (`vst3:<class id>`) or
/// Audio Unit (`au:<type>:<subtype>:<manufacturer>`) plugin's.
pub fn plugin_info(id: &str) -> Option<&'static soundcraft_dsp::PluginInfo> {
    soundcraft_dsp::plugin_info(id)
        .or_else(|| soundcraft_clap_host::plugin_info(id))
        .or_else(|| soundcraft_vst3_host::plugin_info(id))
        .or_else(|| soundcraft_au_host::plugin_info(id))
}
