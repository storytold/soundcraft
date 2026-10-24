//! One or more independent New Tracks rows, submitted as a single engine command.

use egui::{Event, Key, TextStyle, Ui, vec2};
use serde_json::{Value, json};
use soundcraft_model::{ChannelFormat, TrackKind};

const MAX_TRACKS: usize = 128;
const NAME_WIDTH: f32 = 260.0;
const ROW_BUTTON_WIDTH: f32 = 22.0;

#[derive(Debug, Clone)]
pub struct NewTrackRow {
    key: u64,
    pub count: u32,
    pub format: String,
    pub kind: String,
    pub timebase: String,
    pub name: String,
}

impl Default for NewTrackRow {
    fn default() -> Self {
        Self { key: 0, count: 1, format: "Mono".into(), kind: "audio".into(), timebase: "samples".into(), name: "Audio".into() }
    }
}

impl NewTrackRow {
    fn params(&self) -> Value {
        json!({"count": self.count, "format": self.format, "kind": self.kind, "timebase": self.timebase, "name": self.name})
    }
}

#[derive(Debug, Clone)]
pub struct NewTracks {
    pub rows: Vec<NewTrackRow>,
    active: usize,
    next_key: u64,
    focus_count: bool,
}

impl Default for NewTracks {
    fn default() -> Self {
        Self { rows: vec![NewTrackRow::default()], active: 0, next_key: 1, focus_count: true }
    }
}

impl NewTracks {
    pub fn active_row(&self) -> Option<&NewTrackRow> {
        self.rows.get(self.active)
    }

    fn insert_after(&mut self, index: usize) {
        if self.rows.len() >= MAX_TRACKS || self.rows.get(index).is_none() {
            return;
        }
        let Some(next) = self.next_key.checked_add(1) else { return };
        self.rows.insert(index + 1, NewTrackRow { key: self.next_key, ..NewTrackRow::default() });
        self.next_key = next;
        self.active = index + 1;
        self.focus_count = true;
    }

    fn remove(&mut self, index: usize) {
        if self.rows.len() <= 1 || self.rows.get(index).is_none() {
            return;
        }
        self.rows.remove(index);
        if index == self.active {
            self.active = index.min(self.rows.len().saturating_sub(1));
            self.focus_count = true;
        } else if index < self.active {
            self.active = self.active.saturating_sub(1);
        }
    }

    fn keys(&mut self, ctx: &egui::Context) {
        let mac = ctx.os() == egui::os::OperatingSystem::Mac;
        ctx.input_mut(|i| {
            i.events.retain(|event| {
                let Event::Key { key, pressed: true, modifiers: m, .. } = event else { return true };
                if !m.command {
                    return true;
                }
                if m.shift {
                    match key {
                        Key::ArrowDown => self.insert_after(self.active),
                        Key::ArrowUp => self.remove(self.active),
                        _ => return true,
                    }
                    return false;
                }
                if m.alt && m.ctrl && mac {
                    return true;
                }
                let Some(row) = self.rows.get_mut(self.active) else { return true };
                let forward = matches!(key, Key::ArrowRight | Key::ArrowDown);
                match key {
                    Key::ArrowLeft | Key::ArrowRight if !m.alt => {
                        if has_format(&row.kind) {
                            let index = ChannelFormat::ALL.iter().position(|f| f.label() == row.format).unwrap_or(0);
                            if let Some(next) = ChannelFormat::ALL.get(cycle_index(index, ChannelFormat::ALL.len(), forward)) {
                                row.format = next.label().to_string();
                            }
                        }
                    }
                    Key::ArrowUp | Key::ArrowDown if m.alt => {
                        if has_timebase(&row.kind) {
                            row.timebase = if row.timebase == "ticks" { "samples" } else { "ticks" }.to_string();
                        }
                    }
                    Key::ArrowUp | Key::ArrowDown => {
                        let index = TrackKind::ALL.iter().position(|k| k.id() == row.kind).unwrap_or(0);
                        if let Some(next) = TrackKind::ALL.get(cycle_index(index, TrackKind::ALL.len(), forward)) {
                            set_new_track_kind(&mut row.kind, &mut row.name, *next);
                        }
                    }
                    _ => return true,
                }
                false
            });
        });
    }

    pub(crate) fn show(&mut self, ui: &mut Ui, enter: bool) -> Option<Value> {
        self.keys(ui.ctx());
        let widths = RowWidths::new(ui);
        ui.set_min_width(widths.total(ui));
        // The old row must not consume text on the first frame after adding a new one.
        if self.focus_count && ui.is_enabled() && !ui.is_sizing_pass() {
            ui.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
        }
        let mut insert = None;
        let mut remove = None;
        let len = self.rows.len();
        egui::ScrollArea::vertical().id_salt("new_tracks_rows").max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
            for (index, row) in self.rows.iter_mut().enumerate() {
                // Explicit row IDs keep editing state with the row when rows above it are removed.
                let id = egui::Id::new(("new_tracks_row", row.key));
                let rect = egui::Rect::from_min_size(ui.available_rect_before_wrap().min, vec2(ui.available_width(), ui.spacing().interact_size.y));
                ui.scope_builder(egui::UiBuilder::new().id(id).max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
                    let label = ui.label("Create");
                    let counter = ui
                        .scope_builder(egui::UiBuilder::new().id(id.with("count")), |ui| {
                            ui.spacing_mut().interact_size.x = widths.count;
                            if self.focus_count && index == self.active && ui.is_enabled() && !ui.is_sizing_pass() {
                                ui.memory_mut(|m| m.request_focus(ui.next_auto_id()));
                                self.focus_count = false;
                            }
                            ui.add(egui::DragValue::new(&mut row.count).speed(1.0).range(1..=128)).labelled_by(label.id)
                        })
                        .inner;
                    row.count = row.count.clamp(1, 128);
                    if counter.has_focus() || counter.clicked() {
                        self.active = index;
                    }
                    if counter.gained_focus() {
                        counter.scroll_to_me(Some(egui::Align::Center));
                    }
                    ui.label("new");
                    let format = if has_format(&row.kind) {
                        Some(
                            egui::ComboBox::from_id_salt("nt_fmt")
                                .width(widths.format)
                                .selected_text(&row.format)
                                .show_ui(ui, |ui| {
                                    for f in ChannelFormat::ALL {
                                        ui.selectable_value(&mut row.format, f.label().to_string(), f.label());
                                    }
                                })
                                .response,
                        )
                    } else {
                        ui.allocate_space(vec2(widths.format, ui.spacing().interact_size.y));
                        None
                    };
                    let kind = egui::ComboBox::from_id_salt("nt_kind")
                        .width(widths.kind)
                        .selected_text(TrackKind::from_id(&row.kind).map_or("Audio Track", |k| k.label()))
                        .show_ui(ui, |ui| {
                            for k in TrackKind::ALL {
                                if ui.selectable_label(row.kind == k.id(), k.label()).clicked() {
                                    set_new_track_kind(&mut row.kind, &mut row.name, k);
                                }
                            }
                        })
                        .response;
                    let timebase = if has_timebase(&row.kind) {
                        ui.label("in");
                        Some(
                            egui::ComboBox::from_id_salt("nt_tb")
                                .width(widths.timebase)
                                .selected_text(if row.timebase == "ticks" { "Ticks" } else { "Samples" })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut row.timebase, "samples".to_string(), "Samples");
                                    ui.selectable_value(&mut row.timebase, "ticks".to_string(), "Ticks");
                                })
                                .response,
                        )
                    } else {
                        ui.allocate_space(vec2(text_width(ui, "in"), ui.spacing().interact_size.y));
                        ui.allocate_space(vec2(widths.timebase, ui.spacing().interact_size.y));
                        None
                    };
                    let label = ui.label("Name:");
                    let name = ui.add(egui::TextEdit::singleline(&mut row.name).id(id.with("name")).desired_width(NAME_WIDTH)).labelled_by(label.id);
                    if format.iter().chain(timebase.iter()).chain([&kind, &name]).any(|r| r.clicked() || r.has_focus()) {
                        self.active = index;
                    }
                    let size = vec2(ROW_BUTTON_WIDTH, ui.spacing().interact_size.y);
                    if ui.add_enabled(len > 1, egui::Button::new("−").min_size(size)).on_hover_text("Remove this row").clicked() {
                        remove = Some(index);
                    }
                    if ui.add_enabled(len < MAX_TRACKS, egui::Button::new("+").min_size(size)).on_hover_text("Add a row below").clicked() {
                        insert = Some(index);
                    }
                });
            }
        });
        if let Some(index) = remove {
            self.active = index;
            self.remove(index);
            ui.ctx().request_repaint();
        } else if let Some(index) = insert {
            self.insert_after(index);
            ui.ctx().request_repaint();
        }
        let total = self.rows.iter().fold(0_u32, |total, row| total.saturating_add(row.count));
        let can_create = !self.rows.is_empty() && total <= MAX_TRACKS as u32;
        if !can_create {
            ui.colored_label(ui.visuals().error_fg_color, "Create up to 128 tracks at a time.");
        }
        if crate::dialogs::buttons_enabled(ui, "Create", enter, can_create) {
            Some(json!({"rows": self.rows.iter().map(NewTrackRow::params).collect::<Vec<_>>()}))
        } else {
            None
        }
    }
}

/// Measure all choices with the current UI font, rather than the selected label.
struct RowWidths {
    count: f32,
    format: f32,
    kind: f32,
    timebase: f32,
}

impl RowWidths {
    fn new(ui: &Ui) -> Self {
        Self {
            count: (styled_text_width(ui, "128", &ui.style().drag_value_text_style) + 2.0 * ui.spacing().button_padding.x)
                .ceil()
                .max(ui.spacing().interact_size.x),
            format: combo_width(ui, ChannelFormat::ALL.iter().map(|f| f.label())),
            kind: combo_width(ui, TrackKind::ALL.iter().map(|k| k.label())),
            timebase: combo_width(ui, ["Samples", "Ticks"].into_iter()),
        }
    }

    fn total(&self, ui: &Ui) -> f32 {
        let labels: f32 = ["Create", "new", "in", "Name:"].iter().map(|s| text_width(ui, s)).sum();
        labels + self.count + self.format + self.kind + self.timebase + NAME_WIDTH + 2.0 * ROW_BUTTON_WIDTH + 10.0 * ui.spacing().item_spacing.x + 8.0
    }
}

fn text_width(ui: &Ui, text: &str) -> f32 {
    styled_text_width(ui, text, &TextStyle::Button)
}

fn styled_text_width(ui: &Ui, text: &str, style: &TextStyle) -> f32 {
    let font = style.resolve(ui.style());
    ui.fonts_mut(|f| f.layout_no_wrap(text.to_string(), font, ui.visuals().text_color()).size().x)
}

fn combo_width<'a>(ui: &Ui, labels: impl Iterator<Item = &'a str>) -> f32 {
    let longest = labels.map(|s| text_width(ui, s)).fold(0.0, f32::max);
    (longest + ui.spacing().icon_width + ui.spacing().icon_spacing + 2.0 * ui.spacing().button_padding.x + 2.0).ceil()
}

pub(crate) fn set_new_track_kind(kind: &mut String, name: &mut String, next: TrackKind) {
    if TrackKind::from_id(kind).is_some_and(|old| name == old.default_name()) {
        *name = next.default_name().to_string();
    }
    *kind = next.id().to_string();
}

fn has_format(kind: &str) -> bool {
    !matches!(TrackKind::from_id(kind), Some(TrackKind::Midi | TrackKind::Vca | TrackKind::Video))
}

fn has_timebase(kind: &str) -> bool {
    !matches!(TrackKind::from_id(kind), Some(TrackKind::Video))
}

fn cycle_index(index: usize, len: usize, forward: bool) -> usize {
    if forward {
        if index.saturating_add(1) < len { index.saturating_add(1) } else { 0 }
    } else {
        index.checked_sub(1).unwrap_or(len.saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Services, SoundApp, dialogs::Dialog};
    use egui::{Modifiers, accesskit::Role};
    use egui_kittest::{Harness, kittest::Queryable};

    fn harness() -> Harness<'static, SoundApp> {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        assert!(app.dialogs.open_for_command(&app.engine, "track.new"));
        let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(ui.ctx());
                app.ui(ui);
            },
            app,
        );
        h.ctx.set_os(egui::os::OperatingSystem::Mac);
        h.run_steps(4);
        h
    }

    fn tracks<'a>(h: &'a Harness<'_, SoundApp>) -> &'a NewTracks {
        let Some(Dialog::NewTracks(tracks)) = &h.state().dialogs.open else { panic!("missing New Tracks dialog") };
        tracks
    }

    fn tracks_mut<'a>(h: &'a mut Harness<'_, SoundApp>) -> &'a mut NewTracks {
        let Some(Dialog::NewTracks(tracks)) = &mut h.state_mut().dialogs.open else { panic!("missing New Tracks dialog") };
        tracks
    }

    fn chord(h: &mut Harness<SoundApp>, m: Modifiers, key: Key) {
        h.key_press_modifiers(m, key);
        // A centered window needs another frame to settle after its row count changes.
        h.run_steps(4);
    }

    #[test]
    fn every_choice_keeps_the_same_field_widths_and_name_position() {
        let mut h = harness();
        let format = h.get_by_value("Mono").rect();
        let kind = h.get_by_value("Audio Track").rect();
        let timebase = h.get_by_value("Samples").rect();
        let name = h.get_by_role_and_label(Role::TextInput, "Name:").rect();
        assert!(name.width() >= NAME_WIDTH);
        for f in ChannelFormat::ALL {
            tracks_mut(&mut h).rows[0].format = f.label().to_string();
            h.run_steps(2);
            assert_eq!(h.get_by_value(f.label()).rect().width(), format.width(), "{}", f.label());
            assert_eq!(h.get_by_role_and_label(Role::TextInput, "Name:").rect(), name);
        }
        for k in TrackKind::ALL {
            tracks_mut(&mut h).rows[0].kind = k.id().to_string();
            h.run_steps(2);
            assert_eq!(h.get_by_value(k.label()).rect().width(), kind.width(), "{}", k.label());
            let current_name = h.get_by_role_and_label(Role::TextInput, "Name:").rect();
            assert_eq!((current_name.min.x, current_name.width()), (name.min.x, name.width()));
        }
        tracks_mut(&mut h).rows[0].kind = "audio".to_string();
        tracks_mut(&mut h).rows[0].timebase = "ticks".to_string();
        tracks_mut(&mut h).rows[0].count = 128;
        h.run_steps(2);
        assert_eq!(h.get_by_value("Ticks").rect().width(), timebase.width());
        assert_eq!(h.get_by_role_and_label(Role::TextInput, "Name:").rect(), name);
        h.ctx.global_style_mut(|s| s.drag_value_text_style = TextStyle::Heading);
        tracks_mut(&mut h).rows[0].count = 1;
        h.run_steps(4);
        let larger_font_name = h.get_by_role_and_label(Role::TextInput, "Name:").rect();
        tracks_mut(&mut h).rows[0].count = 128;
        h.run_steps(2);
        assert_eq!(h.get_by_role_and_label(Role::TextInput, "Name:").rect(), larger_font_name);
    }

    #[test]
    fn inapplicable_fields_are_hidden_without_moving_the_other_columns() {
        let mut h = harness();
        let name = h.get_by_role_and_label(Role::TextInput, "Name:").rect();
        let all_fields = h.get_all_by_role(Role::ComboBox).count();

        for kind in [TrackKind::Midi, TrackKind::Vca] {
            tracks_mut(&mut h).rows[0].kind = kind.id().to_string();
            h.run_steps(2);
            assert_eq!(h.get_all_by_role(Role::ComboBox).count(), all_fields - 1, "{}", kind.label());
            let current_name = h.get_by_role_and_label(Role::TextInput, "Name:").rect();
            assert_eq!((current_name.min.x, current_name.width()), (name.min.x, name.width()));
        }

        tracks_mut(&mut h).rows[0].kind = TrackKind::Video.id().to_string();
        h.run_steps(2);
        assert_eq!(h.get_all_by_role(Role::ComboBox).count(), all_fields - 2);
        let current_name = h.get_by_role_and_label(Role::TextInput, "Name:").rect();
        assert_eq!((current_name.min.x, current_name.width()), (name.min.x, name.width()));
    }

    #[test]
    fn both_requested_row_chords_focus_the_new_count_and_preserve_other_rows() {
        for alt in [false, true] {
            let mut h = harness();
            let row_chord = Modifiers { command: true, mac_cmd: true, ctrl: true, shift: true, alt };
            tracks_mut(&mut h).rows[0].name = "Dialogue".into();
            chord(&mut h, row_chord, Key::ArrowDown);
            assert_eq!(tracks(&h).rows.len(), 2);
            assert_eq!(tracks(&h).active, 1);
            h.event(Event::Text("7".into()));
            h.run_steps(2);
            assert_eq!(tracks(&h).rows[1].count, 7);
            assert_eq!(tracks(&h).rows[0].count, 1);
            assert_eq!(tracks(&h).rows[0].name, "Dialogue");
            chord(&mut h, Modifiers { command: true, mac_cmd: true, ..Default::default() }, Key::ArrowDown);
            assert_eq!(tracks(&h).rows[1].kind, "aux");
            assert_eq!(tracks(&h).rows[0].kind, "audio");
            chord(&mut h, row_chord, Key::ArrowUp);
            assert_eq!(tracks(&h).rows.len(), 1);
            assert_eq!(tracks(&h).rows[0].name, "Dialogue");
            chord(&mut h, row_chord, Key::ArrowUp);
            assert_eq!(tracks(&h).rows.len(), 1, "the last row cannot be removed");
            assert!(h.state().engine.session().tracks.is_empty());
        }
    }

    #[test]
    fn plus_and_minus_buttons_operate_on_their_own_row() {
        let mut h = harness();
        h.get_by_role_and_label(Role::Button, "+").click();
        h.run_steps(4);
        let names: Vec<_> = h.get_all_by_role_and_label(Role::TextInput, "Name:").map(|n| n.rect()).collect();
        assert_eq!(names.len(), 2);
        assert_eq!(names[0].min.x, names[1].min.x);
        assert_eq!(names[0].width(), names[1].width());
        assert!(names[1].min.y - names[0].min.y <= 32.0, "rows must stay compact: {names:?}");
        tracks_mut(&mut h).rows[0].name = "Dialogue".into();
        tracks_mut(&mut h).rows[1].name = "Effects".into();
        h.get_all_by_role_and_label(Role::Button, "+").next().unwrap().click();
        h.run_steps(4);
        assert_eq!(tracks(&h).rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Dialogue", "Audio", "Effects"]);
        h.get_all_by_role_and_label(Role::Button, "−").nth(1).unwrap().click();
        h.run_steps(4);
        assert_eq!(tracks(&h).rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Dialogue", "Effects"]);
    }

    #[test]
    fn removing_an_earlier_row_keeps_editing_state_with_the_surviving_row() {
        let mut h = harness();
        chord(&mut h, Modifiers { command: true, mac_cmd: true, shift: true, ctrl: true, ..Default::default() }, Key::ArrowDown);
        h.event(Event::Text("42".into()));
        h.run_steps(2);
        let focused = h.ctx.memory(|m| m.focused());
        let key = tracks(&h).rows[1].key;
        tracks_mut(&mut h).remove(0);
        h.run_steps(2);
        assert_eq!(tracks(&h).rows[0].key, key);
        assert_eq!(tracks(&h).rows[0].count, 42);
        assert_eq!(h.ctx.memory(|m| m.focused()), focused);
        chord(&mut h, Modifiers::NONE, Key::ArrowUp);
        assert_eq!(tracks(&h).rows[0].count, 43);
    }

    #[test]
    fn clicking_a_different_row_targets_its_chords_and_keeps_its_custom_name() {
        let mut h = harness();
        chord(&mut h, Modifiers { command: true, mac_cmd: true, shift: true, ctrl: true, ..Default::default() }, Key::ArrowDown);
        tracks_mut(&mut h).rows[0].name = "Dialogue".into();
        h.get_all_by_role_and_label(Role::TextInput, "Name:").next().unwrap().click();
        h.run_steps(2);
        let cmd = Modifiers { command: true, mac_cmd: true, ..Default::default() };
        chord(&mut h, cmd, Key::ArrowDown);
        chord(&mut h, cmd, Key::ArrowRight);
        chord(&mut h, Modifiers { alt: true, ..cmd }, Key::ArrowUp);
        let rows = &tracks(&h).rows;
        assert_eq!(
            (&rows[0].kind, &rows[0].format, &rows[0].timebase, &rows[0].name),
            (&"aux".to_string(), &"Stereo".to_string(), &"ticks".to_string(), &"Dialogue".to_string())
        );
        assert_eq!((&rows[1].kind, &rows[1].format, &rows[1].timebase), (&"audio".to_string(), &"Mono".to_string(), &"samples".to_string()));
        assert_eq!((rows[0].count, rows[1].count), (1, 1));
    }

    #[test]
    fn create_submits_every_row_and_undo_removes_the_whole_creation() {
        let mut h = harness();
        tracks_mut(&mut h).rows[0].count = 2;
        chord(&mut h, Modifiers { command: true, mac_cmd: true, shift: true, ctrl: true, ..Default::default() }, Key::ArrowDown);
        let row = &mut tracks_mut(&mut h).rows[1];
        row.kind = "aux".into();
        row.format = "Stereo".into();
        row.name = "Reverb".into();
        chord(&mut h, Modifiers::NONE, Key::Enter);
        assert!(h.state().dialogs.open.is_none());
        assert_eq!(
            h.state().engine.session().tracks.iter().map(|t| t.kind).collect::<Vec<_>>(),
            [TrackKind::Audio, TrackKind::Audio, TrackKind::Aux]
        );
        assert_eq!(h.state().engine.session().tracks[2].name, "Reverb");
        assert!(h.state().engine.session().markers.is_empty());
        assert!(h.state_mut().engine.undo());
        assert!(h.state().engine.session().tracks.is_empty());
        assert!(!h.state().engine.can_undo());
    }

    #[test]
    fn too_many_tracks_disable_create_but_leave_cancel_available() {
        let mut h = harness();
        tracks_mut(&mut h).rows[0].count = 128;
        chord(&mut h, Modifiers { command: true, mac_cmd: true, shift: true, ctrl: true, ..Default::default() }, Key::ArrowDown);
        chord(&mut h, Modifiers::NONE, Key::Enter);
        assert!(h.state().dialogs.open.is_some());
        assert!(h.state().engine.session().tracks.is_empty());
        h.get_by_role_and_label(Role::Button, "Cancel").click();
        h.run_steps(2);
        assert!(h.state().dialogs.open.is_none());
    }
}
