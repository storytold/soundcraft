use super::{Color32, Rect, SoundApp, Tokens, TrackId, Ui, json, pos2, vec2};

/// An insert's menu: a search field that takes typing at once, then (until something is typed)
/// Bypass and removal and the plugins by category and format.
pub fn plugin_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId, slot: usize, occupied: bool) {
    let search = MenuSearch::show(ui, egui::Id::unique(("plugin-search", id.0, slot)), "Search plugins…");
    if search.typing() {
        if let Some(plugin) = plugin_search(ui, &search, SlotKind::Insert) {
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
    if let Some(plugin) = hosted_menus(ui, SlotKind::Insert) {
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
        if let Some(plugin) = plugin_search(ui, &search, SlotKind::Instrument) {
            run_pick(app, ui, "mix.instrument", json!({"track": id.0, "plugin": plugin}));
        }
        return;
    }
    let current = app.engine.session().track(id).and_then(|t| t.instrument.clone());
    let mut failure: Option<String> = None;
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
            } else if let Some(why) = editor_blocked_reason(current.as_ref().map(|i| i.plugin.as_str()).unwrap_or_default()) {
                // This platform has no window for the plugin's view: say so rather than fail.
                ui.add_enabled(false, egui::Button::new("Open Plugin Editor")).on_disabled_hover_text(why);
            } else if ui.button("Open Plugin Editor").clicked() {
                match p.open_editor(id, slot) {
                    Ok(()) => ui.close(),
                    Err(e) => failure = Some(e),
                }
            }
        }
        if let Some(e) = failure {
            // The status line belongs to the Edit window; this menu may be the only thing the
            // user is looking at, so the reason is shown here too.
            let msg = format!("{}: {e}", info.name);
            app.ui.status = msg.clone();
            ui.label(egui::RichText::new(msg).color(Tokens::current().meter_red));
        }
        ui.separator();
    }
    let mut picked = None;
    ui.menu_button("Built-in", |ui| {
        for p in soundcraft_dsp::plugins().iter().filter(|p| SlotKind::Instrument.takes(p.is_instrument, p.midi_out, p.is_midi_processor)) {
            if ui.button(p.name).clicked() {
                picked = Some(p.id.to_string());
            }
        }
    });
    if let Some(p) = hosted_menus(ui, SlotKind::Instrument) {
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
fn plugin_search(ui: &mut Ui, search: &MenuSearch, slot: SlotKind) -> Option<String> {
    let mut built_in: Vec<&soundcraft_dsp::PluginInfo> = soundcraft_dsp::plugins()
        .iter()
        .copied()
        .filter(|p| slot.takes(p.is_instrument, p.midi_out, p.is_midi_processor) && search.matches(p.name))
        .collect();
    built_in.sort_by_cached_key(|p| p.name.to_lowercase());
    let (clap, vst3) = (soundcraft_clap_host::cached(), soundcraft_vst3_host::cached());
    let mut hosted: Vec<(MenuPlugin, &str)> = Vec::new();
    for d in clap.iter().flatten().filter(|d| slot.takes(d.is_instrument, d.midi_out, d.is_midi_processor)) {
        hosted.push((MenuPlugin { id: d.id.clone(), name: d.name.clone(), vendor: d.vendor.clone() }, "CLAP"));
    }
    // A VST3 whose MIDI output is not known yet is found by search in either slot; the command
    // that puts it there loads it and checks.
    for d in vst3.iter().flatten().filter(|d| slot.vst3_fit(d, soundcraft_vst3_host::loaded_info(&d.id)) != Vst3Fit::Hidden) {
        hosted.push((MenuPlugin { id: d.id.clone(), name: d.name.clone(), vendor: d.vendor.clone() }, "VST3"));
    }
    for d in soundcraft_au_host::scan().into_iter().filter(|d| slot.takes(d.is_instrument, d.midi_out, d.is_midi_processor)) {
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

/// Which slot a plugin menu fills: an audio insert, or the instrument slot — the one slot the
/// mixer feeds MIDI to, where a synth or a MIDI generator (note/chord generator, arpeggiator)
/// belongs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SlotKind {
    Insert,
    Instrument,
}

impl SlotKind {
    /// True when a plugin (built-in or hosted) belongs in this slot's menu. An insert carries
    /// audio only, so an instrument or a MIDI generator dropped into one would never see its MIDI;
    /// both belong in the instrument slot, the one slot the mixer feeds MIDI to.
    pub fn takes(self, is_instrument: bool, midi_out: bool, is_midi_processor: bool) -> bool {
        match self {
            SlotKind::Insert => !is_instrument && !midi_out && !is_midi_processor,
            SlotKind::Instrument => is_instrument || midi_out,
        }
    }

    /// Where a scanned VST3 plugin goes in this slot's menu. Once its type has been loaded
    /// (`loaded`), the runtime capability decides: an event output bus makes it a MIDI generator.
    /// Before that only the scan's sub-categories are known, and many MIDI generators declare no
    /// more than `Fx`. Such a plain effect stays an audio insert, and the instrument slot offers it
    /// apart, as a plugin whose MIDI output is not known yet: `mix.instrument` loads and checks it,
    /// after which it is listed where it belongs.
    pub fn vst3_fit(self, d: &soundcraft_vst3_host::Vst3Descriptor, loaded: Option<&soundcraft_dsp::PluginInfo>) -> Vst3Fit {
        if let Some(i) = loaded {
            return if self.takes(i.is_instrument, i.midi_out, i.is_midi_processor) { Vst3Fit::Listed } else { Vst3Fit::Hidden };
        }
        if self.takes(d.is_instrument, d.midi_out, d.is_midi_processor) {
            Vst3Fit::Listed
        } else if self == SlotKind::Instrument && SlotKind::Insert.takes(d.is_instrument, d.midi_out, d.is_midi_processor) {
            Vst3Fit::Unverified
        } else {
            Vst3Fit::Hidden
        }
    }
}

/// Where a scanned VST3 plugin shows in a slot's menu (see [`SlotKind::vst3_fit`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vst3Fit {
    /// Listed with the slot's plugins.
    Listed,
    /// A plain effect not loaded yet, which may still turn out to generate MIDI: offered to the
    /// instrument slot in a submenu of its own.
    Unverified,
    /// Not offered.
    Hidden,
}

/// The CLAP, VST3 and (on macOS) Audio Units submenus, of effects or of MIDI-capable plugins;
/// returns the id picked. Each format is scanned the first time its submenu opens, then cached by
/// its host.
fn hosted_menus(ui: &mut Ui, slot: SlotKind) -> Option<String> {
    let mut picked = None;
    let (clap_empty, vst3_empty, au_empty) = match slot {
        SlotKind::Insert => ("No CLAP audio effects found", "No VST3 audio effects found", "No Audio Unit effects found"),
        SlotKind::Instrument => ("No CLAP instruments found", "No VST3 instruments found", "No Audio Unit instruments found"),
    };
    ui.menu_button("CLAP", |ui| {
        let list = soundcraft_clap_host::scan().into_iter().filter(|d| slot.takes(d.is_instrument, d.midi_out, d.is_midi_processor));
        if let Some(p) = vendor_menus(ui, list.map(|d| MenuPlugin { id: d.id, name: d.name, vendor: d.vendor }).collect(), clap_empty) {
            picked = Some(p);
        }
    });
    ui.menu_button("VST3", |ui| {
        let (mut listed, mut unverified) = (Vec::new(), Vec::new());
        for d in soundcraft_vst3_host::scan() {
            let fit = slot.vst3_fit(&d, soundcraft_vst3_host::loaded_info(&d.id));
            let p = MenuPlugin { id: d.id, name: d.name, vendor: d.vendor };
            match fit {
                Vst3Fit::Listed => listed.push(p),
                Vst3Fit::Unverified => unverified.push(p),
                Vst3Fit::Hidden => {}
            }
        }
        if let Some(p) = vendor_menus(ui, listed, vst3_empty) {
            picked = Some(p);
        }
        if !unverified.is_empty() {
            // A MIDI generator that declares itself only as an effect is found here until it has
            // been loaded once; picking one loads it and checks for an event output.
            ui.separator();
            let more = ui.menu_button("Other VST3 Effects (MIDI Output Unchecked)", |ui| vendor_menus(ui, unverified, ""));
            if let Some(Some(p)) = more.inner {
                picked = Some(p);
            }
            more.response.on_hover_text(
                "Plugins whose categories name no instrument or MIDI generator. Picking one loads it: a plugin with a MIDI output becomes the instrument, any other is refused.",
            );
        }
    });
    if cfg!(target_os = "macos") {
        ui.menu_button("Audio Units", |ui| {
            let list = soundcraft_au_host::scan().into_iter().filter(|d| slot.takes(d.is_instrument, d.midi_out, d.is_midi_processor));
            if let Some(p) = vendor_menus(ui, list.map(|d| MenuPlugin { id: d.id, name: d.name, vendor: d.vendor }).collect(), au_empty) {
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

/// Why this plugin's own editor cannot be opened here, when it cannot. VST3 editor windows are
/// hosted on macOS, Windows and X11 (Linux and the BSDs); on any other platform (Android, the web)
/// there is no window to put the plugin's view into, so the button says so instead of failing
/// silently when it is pressed.
pub fn editor_blocked_reason(plugin: &str) -> Option<&'static str> {
    if plugin.starts_with(soundcraft_vst3_host::ID_PREFIX) && !soundcraft_vst3_host::editors_supported() {
        Some("VST3 plugin editors cannot be hosted on this platform (macOS, Windows and X11 only)")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{SlotKind, editor_blocked_reason};

    #[test]
    fn the_editor_button_says_why_it_cannot_open() {
        // A VST3 plugin's editor window is hosted on macOS, Windows and, where X11 is
        // available, on Linux and the BSDs. Where it is not, the button says so instead of
        // failing silently when it is pressed.
        let vst3 = "vst3:0123456789ABCDEFfedcba9876543210";
        if soundcraft_vst3_host::editors_supported() {
            assert!(editor_blocked_reason(vst3).is_none());
        } else {
            assert!(editor_blocked_reason(vst3).is_some());
        }
        // A built-in plugin has no editor of its own, and CLAP editors are hosted separately.
        assert!(editor_blocked_reason("eq_7band").is_none());
        assert!(editor_blocked_reason("clap:com.vendor.plugin").is_none());
    }

    #[test]
    fn midi_capable_plugins_are_offered_only_where_the_mixer_feeds_midi() {
        // The instrument slot is the one slot that sees MIDI, so it takes synths and MIDI
        // generators alike; an audio insert takes neither, which is why a note generator such as
        // InstaComposer used to be unselectable at all.
        assert!(SlotKind::Instrument.takes(true, false, false), "a synth in the instrument slot");
        assert!(SlotKind::Instrument.takes(false, true, true), "a MIDI generator with supported output");
        assert!(!SlotKind::Instrument.takes(false, false, true), "unsupported MIDI output is not offered as a generator");
        assert!(!SlotKind::Instrument.takes(false, false, false), "a reverb is not an instrument");
        assert!(SlotKind::Insert.takes(false, false, false), "a reverb as an insert");
        assert!(!SlotKind::Insert.takes(true, false, false), "a synth is not an audio insert");
        assert!(!SlotKind::Insert.takes(false, true, false), "a MIDI generator is not an audio insert");
        assert!(!SlotKind::Insert.takes(false, false, true), "an unsupported MIDI processor is not an audio insert");
    }

    fn vst3(subs: &[&str]) -> soundcraft_vst3_host::Vst3Descriptor {
        let subs: Vec<String> = subs.iter().map(|s| s.to_string()).collect();
        soundcraft_vst3_host::Vst3Descriptor {
            id: "vst3:0123456789ABCDEF0123456789ABCDEF".into(),
            class_id: "0123456789ABCDEF0123456789ABCDEF".into(),
            name: "Plugin".into(),
            vendor: "Vendor".into(),
            version: String::new(),
            sdk_version: String::new(),
            category: soundcraft_vst3_host::scan::category_from_sub_categories(&subs),
            is_instrument: soundcraft_vst3_host::scan::is_instrument(&subs),
            is_midi_processor: soundcraft_vst3_host::scan::is_midi_processor(&subs),
            midi_out: soundcraft_vst3_host::scan::midi_out(&subs),
            sub_categories: subs,
            path: std::path::PathBuf::new(),
        }
    }

    fn loaded(is_instrument: bool, midi_out: bool) -> soundcraft_dsp::PluginInfo {
        soundcraft_dsp::PluginInfo {
            id: "vst3:0123456789ABCDEF0123456789ABCDEF",
            name: "Plugin",
            short_name: "Plugin",
            category: soundcraft_dsp::Category::Other,
            params: &[],
            is_instrument,
            is_midi_processor: false,
            midi_out,
            audiosuite: false,
        }
    }

    #[test]
    fn a_vst3_generator_labelled_only_fx_can_reach_the_instrument_slot() {
        use super::Vst3Fit::{Hidden, Listed, Unverified};
        let fx = vst3(&["Fx"]);
        // Not loaded yet: an effect as an insert, and offered apart to the instrument slot,
        // because only loading it shows whether it has an event output.
        assert_eq!(SlotKind::Insert.vst3_fit(&fx, None), Listed);
        assert_eq!(SlotKind::Instrument.vst3_fit(&fx, None), Unverified);
        // Loaded, with an event output: a MIDI generator, never an audio insert.
        assert_eq!(SlotKind::Instrument.vst3_fit(&fx, Some(&loaded(false, true))), Listed);
        assert_eq!(SlotKind::Insert.vst3_fit(&fx, Some(&loaded(false, true))), Hidden);
        // Loaded, without one: a plain effect again.
        assert_eq!(SlotKind::Instrument.vst3_fit(&fx, Some(&loaded(false, false))), Hidden);
        assert_eq!(SlotKind::Insert.vst3_fit(&fx, Some(&loaded(false, false))), Listed);
        // Declared instruments and generators are listed straight away.
        assert_eq!(SlotKind::Instrument.vst3_fit(&vst3(&["Instrument", "Synth"]), None), Listed);
        assert_eq!(SlotKind::Instrument.vst3_fit(&vst3(&["Fx", "Arpeggiator"]), None), Listed);
        assert_eq!(SlotKind::Insert.vst3_fit(&vst3(&["Fx", "Arpeggiator"]), None), Hidden);
    }
}
