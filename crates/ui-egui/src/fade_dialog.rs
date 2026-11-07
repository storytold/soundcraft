//! The Fade In, Fade Out and Crossfade dialogs: what Edit › Fades › Create… shows for one fade or
//! crossfade in the edit selection, and what double-clicking a fade or crossfade opens. Shape and
//! slope are drawn live over the clips' waveforms; OK is one `edit.fades_create`.

use crate::dialogs::FadeTarget;
use crate::fade_plot;
use crate::theme::Tokens;
use egui::{Align, Layout, RichText, Ui};
use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_engine::edit::{FadeSpot, fade_spots};
use soundcraft_model::{Fade, FadeShape, Session, TrackId};
use soundcraft_time::Range;

/// Which fade a dialog is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    In,
    Out,
    Crossfade,
}

/// The shape group's three choices. A preset curve names the model shape behind it.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Shape {
    Standard,
    SCurve,
    Preset(FadeShape),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Slope {
    EqualPower,
    EqualGain,
}

/// A crossfade's Link Out / In: how the out side follows the in side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Link {
    EqualPower,
    EqualGain,
    None,
}

impl Link {
    fn slope(self) -> Option<Slope> {
        match self {
            Link::EqualPower => Some(Slope::EqualPower),
            Link::EqualGain => Some(Slope::EqualGain),
            Link::None => None,
        }
    }
}

/// One side of a fade as the dialog shows it: shape, slope and the bend a fade came with.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Side {
    pub shape: Shape,
    pub slope: Slope,
    /// The fade's `curvature`: kept while its preset curve stays chosen, 0 once anything else is picked.
    pub bend: f32,
}

impl Side {
    pub const FACTORY: Side = Side { shape: Shape::Standard, slope: Slope::EqualPower, bend: 0.0 };

    /// How a model fade comes up in the dialog. A bent fade, and Exponential and Logarithmic, are preset curves.
    fn of(f: Fade) -> Side {
        let bend = if f.curvature.is_finite() { f.curvature.clamp(-1.0, 1.0) } else { 0.0 };
        let (shape, slope) = match f.shape {
            FadeShape::Linear => (Shape::Standard, Slope::EqualGain),
            FadeShape::EqualPower => (Shape::Standard, Slope::EqualPower),
            FadeShape::SCurve => (Shape::SCurve, Slope::EqualGain),
            FadeShape::SCurvePower => (Shape::SCurve, Slope::EqualPower),
            p @ (FadeShape::Exponential | FadeShape::Logarithmic) => (Shape::Preset(p), Slope::EqualPower),
        };
        if bend != 0.0 { Side { shape: Shape::Preset(f.shape), slope, bend } } else { Side { shape, slope, bend } }
    }

    /// The model shape this side stands for.
    fn model_shape(self) -> FadeShape {
        match (self.shape, self.slope) {
            (Shape::Preset(p), _) => p,
            (Shape::Standard, Slope::EqualGain) => FadeShape::Linear,
            (Shape::Standard, Slope::EqualPower) => FadeShape::EqualPower,
            (Shape::SCurve, Slope::EqualGain) => FadeShape::SCurve,
            (Shape::SCurve, Slope::EqualPower) => FadeShape::SCurvePower,
        }
    }

    /// The fade this side makes; its length is the command's.
    pub fn fade(self) -> Fade {
        let curvature = if self.bend.is_finite() { self.bend.clamp(-1.0, 1.0) } else { 0.0 };
        Fade { len: 0, shape: self.model_shape(), curvature }
    }

    fn is_preset(self) -> bool {
        matches!(self.shape, Shape::Preset(_))
    }

    /// The side with a bend that is a number in range: slots come from a preferences file anyone can edit.
    fn sane(self) -> Side {
        Side { bend: if self.bend.is_finite() { self.bend.clamp(-1.0, 1.0) } else { 0.0 }, ..self }
    }
}

/// Everything the dialog edits. Fade In uses `input`, Fade Out `output`, a crossfade all three.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    pub input: Side,
    pub output: Side,
    pub link: Link,
}

impl Settings {
    pub const FACTORY: Settings = Settings { input: Side::FACTORY, output: Side::FACTORY, link: Link::EqualPower };

    /// A crossfade's link applied: linked, the out side mirrors the in side and takes the link's slope.
    // ponytail: our preset curves have no linked complement, so a preset curve on a linked side goes back to Standard.
    fn linked(mut self) -> Settings {
        if let Some(slope) = self.link.slope() {
            let shape = if self.input.is_preset() { Shape::Standard } else { self.input.shape };
            self.input = Side { shape, slope, bend: 0.0 };
            self.output = self.input;
        }
        self
    }

    /// Only what a dialog of `kind` edits; the rest at its factory value.
    fn of_kind(self, kind: Kind) -> Settings {
        match kind {
            Kind::In => Settings { output: Side::FACTORY, link: Link::EqualPower, ..self },
            Kind::Out => Settings { input: Side::FACTORY, link: Link::EqualPower, ..self },
            Kind::Crossfade => self,
        }
    }

    /// Every bend taken off: a bent fade comes back as the plain shape it bends.
    fn unbent(self) -> Settings {
        let plain = |s: Side| if s.bend == 0.0 { s } else { Side::of(Fade { len: 0, shape: s.model_shape(), curvature: 0.0 }) };
        Settings { input: plain(self.input), output: plain(self.output), ..self }
    }

    /// `of_kind` with what a stored preset says laid over `self`.
    fn with(self, kind: Kind, stored: Settings) -> Settings {
        let stored = Settings { input: stored.input.sane(), output: stored.output.sane(), ..stored };
        let merged = match kind {
            Kind::In => Settings { input: stored.input, ..self },
            Kind::Out => Settings { output: stored.output, ..self },
            Kind::Crossfade => stored,
        };
        if kind == Kind::Crossfade { merged.linked() } else { merged }
    }
}

const SLOTS: usize = 5;
/// The width of a dialog's content: the sketch's 460 and 580 less the window's own frame.
const FADE_WIDTH: f32 = 428.0;
const CROSSFADE_WIDTH: f32 = 548.0;

/// The five preset slots of each dialog, kept with the interface preferences.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FadePresets {
    #[serde(deserialize_with = "lenient_slots")]
    fade_in: [Option<Settings>; SLOTS],
    #[serde(deserialize_with = "lenient_slots")]
    fade_out: [Option<Settings>; SLOTS],
    #[serde(deserialize_with = "lenient_slots")]
    crossfade: [Option<Settings>; SLOTS],
}

/// The slots of one dialog as the preferences file has them. A slot that cannot be read (a hand-edited file, a shape a
/// later version added) comes up empty, and anything that is not a list of slots is five empty ones: the rest of the
/// preferences must still load.
fn lenient_slots<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<[Option<Settings>; SLOTS], D::Error> {
    let saved = <Value as serde::Deserialize>::deserialize(deserializer)?;
    let mut slots = [None; SLOTS];
    for (slot, value) in slots.iter_mut().zip(saved.as_array().into_iter().flatten()) {
        *slot = serde_json::from_value::<Option<Settings>>(value.clone()).ok().flatten();
    }
    Ok(slots)
}

impl FadePresets {
    /// Slot `n` (0 to 4) of the dialog of `kind`.
    pub fn slot(&self, kind: Kind, n: usize) -> Option<Settings> {
        match kind {
            Kind::In => &self.fade_in,
            Kind::Out => &self.fade_out,
            Kind::Crossfade => &self.crossfade,
        }
        .get(n)
        .copied()
        .flatten()
    }

    fn slots(&mut self, kind: Kind) -> &mut [Option<Settings>; SLOTS] {
        match kind {
            Kind::In => &mut self.fade_in,
            Kind::Out => &mut self.fade_out,
            Kind::Crossfade => &mut self.crossfade,
        }
    }
}

/// What the graph shows besides the curves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Curves,
    Lanes,
    Overlaid,
    Combined,
}

impl View {
    pub const ALL: [(View, &'static str); 4] = [
        (View::Curves, "Curves only"),
        (View::Lanes, "Waveforms, one lane each"),
        (View::Overlaid, "Waveforms overlaid"),
        (View::Combined, "Combined waveform"),
    ];
}

/// Where the clips of a [`FadeSpot`] lie (the second only for a crossfade): the dialog's graph is laid out from them.
type SpotClips = (Range, Option<Range>);

fn spot_clips(s: &Session, spot: FadeSpot) -> Option<SpotClips> {
    let at = |c| s.find_clip(c).map(|(_, c)| c.range());
    match spot {
        FadeSpot::In(c) | FadeSpot::Out(c) => Some((at(c)?, None)),
        FadeSpot::Cross(a, b) => Some((at(a)?, Some(at(b)?))),
    }
}

/// The state of one open dialog.
#[derive(Debug, Clone)]
pub struct FadeEditor {
    target: FadeTarget,
    /// Set when the dialog came from an edit selection with no fade there yet: OK makes the fade over this range, if the
    /// selection still covers exactly this one fade and its clips lie where they did.
    selection: Option<(TrackId, Range, FadeSpot, SpotClips)>,
    /// The clips behind the graph: the outgoing one (A, top lane) and the incoming one (B, bottom lane),
    /// and the stretch of timeline the graph spans.
    pub(crate) a: Option<soundcraft_model::ClipId>,
    pub(crate) b: Option<soundcraft_model::ClipId>,
    pub(crate) span: Range,
    pub(crate) now: Settings,
    original: Settings,
    pub(crate) compare: bool,
    pub(crate) view: View,
    pub(crate) zoom: f32,
}

impl FadeEditor {
    fn new(target: FadeTarget, span: Range, (a, b): (Option<soundcraft_model::ClipId>, Option<soundcraft_model::ClipId>), now: Settings) -> Self {
        let now = if matches!(target, FadeTarget::Crossfade { .. }) { now.linked() } else { now };
        FadeEditor { target, selection: None, a, b, span, now, original: now, compare: false, view: View::Lanes, zoom: 1.0 }
    }

    /// The dialog for the fade or crossfade `target` aims at, open on what it has now. None when the clips are gone.
    pub fn for_target(s: &Session, target: FadeTarget) -> Option<FadeEditor> {
        match target {
            FadeTarget::Single { clip, fade_in } => {
                let (_, c) = s.find_clip(clip)?;
                let fade = if fade_in { c.fade_in } else { c.fade_out };
                let (span, side) = if fade_in {
                    (Range::new(c.start, c.start.saturating_add(fade.len)), Side::of(fade))
                } else {
                    (Range::new(c.end().saturating_sub(fade.len), c.end()), Side::of(fade))
                };
                let now = if fade_in { Settings { input: side, ..Settings::FACTORY } } else { Settings { output: side, ..Settings::FACTORY } };
                Some(FadeEditor::new(target, span, if fade_in { (None, Some(clip)) } else { (Some(clip), None) }, now))
            }
            FadeTarget::Crossfade { later } => {
                let (track, b) = s.find_clip(later)?;
                let a = crate::edit_window::crossfade_partner(s.track(track)?, b).filter(|a| a.end() <= b.end())?;
                let (input, output) = (Side::of(b.fade_in), Side::of(a.fade_out));
                Some(FadeEditor::new(
                    target,
                    Range::new(b.start, a.end()),
                    (Some(a.id), Some(later)),
                    Settings { input, output, link: infer_link(b.fade_in, a.fade_out) },
                ))
            }
        }
    }

    /// Edit › Fades › Create… with an edit selection that comes down to one fade on one track: its dialog, open
    /// on the factory settings. None for anything else (the simple dialog is for those).
    pub fn for_selection(e: &Engine) -> Option<FadeEditor> {
        let s = e.session();
        let range = s.edit.selection;
        let mut found: Option<(TrackId, FadeSpot)> = None;
        for track in e.target_tracks() {
            for spot in fade_spots(s, track, range) {
                if found.replace((track, spot)).is_some() {
                    return None;
                }
            }
        }
        let (track, spot) = found?;
        let (target, span, clips) = match spot {
            FadeSpot::In(c) => (FadeTarget::Single { clip: c, fade_in: true }, Range::new(s.find_clip(c)?.1.start, range.end), (None, Some(c))),
            FadeSpot::Out(c) => (FadeTarget::Single { clip: c, fade_in: false }, Range::new(range.start, s.find_clip(c)?.1.end()), (Some(c), None)),
            FadeSpot::Cross(a, b) => (FadeTarget::Crossfade { later: b }, range, (Some(a), Some(b))),
        };
        let mut editor = FadeEditor::new(target, span, clips, Settings::FACTORY);
        editor.selection = Some((track, range, spot, spot_clips(s, spot)?));
        Some(editor)
    }

    pub fn kind(&self) -> Kind {
        match self.target {
            FadeTarget::Single { fade_in: true, .. } => Kind::In,
            FadeTarget::Single { fade_in: false, .. } => Kind::Out,
            FadeTarget::Crossfade { .. } => Kind::Crossfade,
        }
    }

    pub fn title(&self) -> &'static str {
        self.target.label()
    }

    /// The name `ui.state` gives the open dialog.
    pub fn dialog_name(&self) -> &'static str {
        match self.kind() {
            Kind::In => "fade_in",
            Kind::Out => "fade_out",
            Kind::Crossfade => "crossfade",
        }
    }

    pub fn target(&self) -> FadeTarget {
        self.target
    }

    /// What the dialog has now: the settings OK applies.
    pub fn settings(&self) -> Settings {
        self.now
    }

    /// The settings the controls and the graph show: the original ones while COMPARE is on.
    pub fn shown(&self) -> Settings {
        if self.compare { self.original } else { self.now }
    }

    /// A preset slot recalled into this dialog. Over a selection OK makes the fades with the range form, which has no
    /// bend to give, and a crossfade's command has one bend for both sides, never a recalled one; so a bend the slot
    /// holds is dropped here rather than shown and then not applied.
    fn recall(&mut self, kind: Kind, stored: Settings) {
        let unbent = self.selection.is_some() || self.kind() == Kind::Crossfade;
        self.now = self.now.with(kind, if unbent { stored.unbent() } else { stored });
    }

    /// The command OK runs, or None when there is nothing to change.
    pub fn command(&self, s: &Session) -> Option<(String, Value)> {
        let (kind, now) = (self.kind(), self.now);
        let (input, output) = (now.input.fade(), now.output.fade());
        let name = |f: Fade| f.shape.label();
        if let Some((track, range, spot, clips)) = self.selection {
            // The dialog does not hold the Edit window: if the clips moved since it opened, the range may no longer make
            // this one fade, or make it at another place or length, and what it would make is not what the dialog shows.
            if fade_spots(s, track, range) != [spot] || spot_clips(s, spot) != Some(clips) {
                return None;
            }
            let mut params = json!({"tracks": [track.0], "start": range.start, "end": range.end});
            if kind != Kind::Out {
                params["in_shape"] = json!(name(input));
            }
            if kind != Kind::In {
                params["out_shape"] = json!(name(output));
            }
            return Some(("edit.fades_create".into(), params));
        }
        let same = |old: Fade, new: Fade| old.len > 0 && old.shape == new.shape && old.curvature == new.curvature;
        let params = match self.target {
            FadeTarget::Single { clip, fade_in } => {
                let (old, new) = s.find_clip(clip).map(|(_, c)| if fade_in { (c.fade_in, input) } else { (c.fade_out, output) })?;
                // The fade has gone since the dialog opened, or is what the dialog shows.
                if old.len <= 0 || same(old, new) {
                    return None;
                }
                json!({"clip": clip.0, "side": if fade_in { "in" } else { "out" }, "length": old.len, "shape": name(new), "curvature": new.curvature})
            }
            FadeTarget::Crossfade { later } => {
                let (track, b) = s.find_clip(later)?;
                let a = crate::edit_window::crossfade_partner(s.track(track)?, b).filter(|a| a.end() <= b.end())?;
                // Another earlier clip since the dialog opened: its fade-out is not the one the dialog shows.
                if self.a != Some(a.id) || (same(b.fade_in, input) && same(a.fade_out, output)) {
                    return None;
                }
                let mut params = json!({"clip": later.0, "side": "crossfade", "in_shape": name(input), "out_shape": name(output)});
                // ponytail: the command takes one bend for both fades, and a fade whose shape stays keeps its own bend unless
                // one is given. So a bend is given only to take it off a fade that keeps its shape; a bent fade left alone next
                // to it loses its bend too. Per-side curvature in the command is the way out.
                let unbend = |old: Fade, new: Fade| old.shape == new.shape && old.curvature != new.curvature;
                if unbend(b.fade_in, input) || unbend(a.fade_out, output) {
                    params["curvature"] = json!(0.0);
                }
                params
            }
        };
        Some(("edit.fades_create".into(), params))
    }
}

/// A crossfade opens linked when both fades are the same shape, unbent, and one that has a linked complement.
fn infer_link(fade_in: Fade, fade_out: Fade) -> Link {
    if fade_in.shape != fade_out.shape || fade_in.curvature != 0.0 || fade_out.curvature != 0.0 {
        return Link::None;
    }
    match fade_in.shape {
        FadeShape::Linear | FadeShape::SCurve => Link::EqualGain,
        FadeShape::EqualPower | FadeShape::SCurvePower => Link::EqualPower,
        FadeShape::Exponential | FadeShape::Logarithmic => Link::None,
    }
}

/// The dialog's body. Returns true when OK was pressed (or Enter); Cancel and Esc are the caller's.
pub fn ui(ui: &mut Ui, ed: &mut FadeEditor, s: &Session, presets: &mut FadePresets, enter: bool) -> bool {
    let kind = ed.kind();
    let t = Tokens::current();
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
    ui.set_width(if kind == Kind::Crossfade { CROSSFADE_WIDTH } else { FADE_WIDTH });
    presets_bar(ui, ed, presets);
    let editable = !ed.compare;
    let mut edited = ed.shown();
    egui::Frame::NONE.fill(t.panel_bg).stroke(egui::Stroke::new(1.0, t.border)).corner_radius(4.0).inner_margin(egui::Margin::same(10)).show(
        ui,
        |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 10.0);
            fade_plot::work_area(ui, ed, s);
            groups(ui, kind, &mut edited, editable);
        },
    );
    if editable && edited != ed.now {
        ed.now = if kind == Kind::Crossfade { edited.linked() } else { edited };
        // The graph above was drawn before the groups below changed: draw it again with the change.
        ui.ctx().request_repaint();
    }
    let mut ok = enter && ui.ctx().memory(|m| m.focused()).is_none();
    ui.separator();
    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
        ok |= ui.add(egui::Button::new(RichText::new("OK").strong()).fill(t.accent)).clicked();
        if ui.button("Cancel").clicked() {
            crate::dialogs::request_cancel(ui.ctx());
        }
    });
    ok
}

/// Presets: five slots, the factory-default menu and COMPARE.
fn presets_bar(ui: &mut Ui, ed: &mut FadeEditor, presets: &mut FadePresets) {
    let (kind, editable) = (ed.kind(), !ed.compare);
    let t = Tokens::current();
    let group = |ui: &mut Ui, add: &mut dyn FnMut(&mut Ui)| {
        egui::Frame::NONE
            .fill(t.toolbar_group)
            .stroke(egui::Stroke::new(1.0, t.group_border))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(6, 3))
            .show(ui, |ui| ui.horizontal(|ui| add(ui)));
    };
    egui::Frame::NONE.fill(t.toolbar_bg).corner_radius(3.0).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            group(ui, &mut |ui| {
                ui.label(RichText::new("Presets:").color(t.text_dim));
                ui.spacing_mut().item_spacing.x = 0.0;
                for (n, slot) in presets.slots(kind).iter_mut().enumerate() {
                    let selected = *slot == Some(ed.now.of_kind(kind));
                    if !fade_plot::slot_button(ui, n, slot.is_some(), selected, editable).clicked() {
                        continue;
                    }
                    // Cmd-click stores what the dialog has now; a click recalls what the slot has.
                    if ui.input(|i| i.modifiers.command) {
                        *slot = Some(ed.now.of_kind(kind));
                    } else if let Some(stored) = *slot {
                        ed.recall(kind, stored);
                    }
                }
            });
            group(ui, &mut |ui| {
                ui.add_enabled_ui(editable, |ui| {
                    let combo = craft_ui::ComboBox::from_id_salt("fade_presets").selected_text("<factory default>").width(150.0);
                    let menu = combo.show_ui(ui, |ui| ui.selectable_label(false, "<factory default>").clicked());
                    if menu.inner == Some(true) {
                        ed.now = ed.now.with(kind, Settings::FACTORY);
                    }
                    if !editable {
                        menu.response.on_disabled_hover_text(fade_plot::COMPARING);
                    }
                });
            });
            group(ui, &mut |ui| {
                if fade_plot::toggle_button(ui, "COMPARE", ed.compare, fade_plot::COMPARE_TIP).clicked() {
                    ed.compare = !ed.compare;
                }
            });
        });
    });
}

const FOLLOWS: &str = "Out follows In while Link Out / In is on. Choose None to set it separately.";
const PRESET_LINK: &str = "Preset curves have no linked partner. Choose None under Link Out / In to use one.";
const PRESET_SLOPE: &str = "A preset curve fixes the slope. Choose Standard or S-Curve to set it.";

/// The shape and slope or link groups under the graph.
fn groups(ui: &mut Ui, kind: Kind, set: &mut Settings, editable: bool) {
    let off = (!editable).then_some(fade_plot::COMPARING);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        match kind {
            Kind::In | Kind::Out => {
                let (title, side) = if kind == Kind::In { ("In Shape", &mut set.input) } else { ("Out Shape", &mut set.output) };
                panel(ui, 150.0, title, |ui| shape_group(ui, side, kind == Kind::Out, off, None));
                panel(ui, 150.0, "Slope", |ui| {
                    let why = off.or_else(|| side.is_preset().then_some(PRESET_SLOPE));
                    for (slope, label) in [(Slope::EqualPower, "Equal Power"), (Slope::EqualGain, "Equal Gain")] {
                        if radio(ui, label, side.slope == slope, why) {
                            side.slope = slope;
                        }
                    }
                });
            }
            Kind::Crossfade => {
                // Three groups side by side inside the content's margins.
                let width = ((CROSSFADE_WIDTH - 20.0 - 16.0) / 3.0).floor();
                let linked = set.link != Link::None;
                panel(ui, width, "In Shape", |ui| shape_group(ui, &mut set.input, false, off, linked.then_some(PRESET_LINK)));
                panel(ui, width, "Link Out / In", |ui| {
                    for (link, label) in [(Link::EqualPower, "Equal Power"), (Link::EqualGain, "Equal Gain"), (Link::None, "None")] {
                        if radio(ui, label, set.link == link, off) {
                            set.link = link;
                        }
                    }
                });
                panel(ui, width, "Out Shape", |ui| shape_group(ui, &mut set.output, true, off.or(linked.then_some(FOLLOWS)), None));
            }
        }
    });
}

/// A group with a bold caption over its choices.
fn panel(ui: &mut Ui, width: f32, title: &str, body: impl FnOnce(&mut Ui)) {
    let t = Tokens::current();
    egui::Frame::NONE.fill(t.strip_bg).stroke(egui::Stroke::new(1.0, t.border)).corner_radius(4.0).show(ui, |ui| {
        ui.set_width(width - 2.0);
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
        ui.vertical(|ui| {
            let top = egui::CornerRadius { nw: 3, ne: 3, sw: 0, se: 0 };
            egui::Frame::NONE.fill(t.strip_section).corner_radius(top).inner_margin(egui::Margin::symmetric(8, 3)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(title).font(crate::theme::bold(13.0)).color(t.section_label));
            });
            let y = ui.cursor().min.y;
            ui.painter().hline(ui.max_rect().x_range(), y, egui::Stroke::new(1.0, t.border));
            egui::Frame::NONE.inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
                body(ui);
                // As tall as the shape group (two rows and the 30 pt preset curve), so groups side by side line up.
                ui.allocate_space(egui::vec2(0.0, (78.0 - ui.min_rect().height()).max(0.0)));
            });
        });
    });
}

/// Standard, S-Curve and the preset curve with its menu. `off` greys the whole group, `no_preset` only the preset curve.
fn shape_group(ui: &mut Ui, side: &mut Side, out: bool, off: Option<&str>, no_preset: Option<&str>) {
    for (shape, label) in [(Shape::Standard, "Standard"), (Shape::SCurve, "S-Curve")] {
        if radio(ui, label, side.shape == shape, off) {
            *side = Side { shape, bend: 0.0, ..*side };
        }
    }
    let why = off.or(no_preset);
    ui.horizontal(|ui| {
        let picked = fade_plot::preset_choice(ui, *side, out, why);
        if let Some(shape) = picked {
            *side = Side { shape: Shape::Preset(shape), bend: if side.shape == Shape::Preset(shape) { side.bend } else { 0.0 }, ..*side };
        }
    });
}

/// A radio button; greyed with `why` on hover when it can't be used. True when clicked.
fn radio(ui: &mut Ui, label: &str, on: bool, why: Option<&str>) -> bool {
    let resp = ui.add_enabled(why.is_none(), egui::RadioButton::new(on, label));
    match why {
        Some(why) => {
            resp.on_disabled_hover_text(why);
            false
        }
        None => resp.clicked(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{Clip, ClipId, SourceId};

    fn fade(shape: FadeShape, curvature: f32) -> Fade {
        Fade { len: 100, shape, curvature }
    }

    #[test]
    fn shape_and_slope_map_onto_the_four_standard_shapes_and_back() {
        let side = |shape, slope| Side { shape, slope, bend: 0.0 };
        for (shape, slope, model) in [
            (Shape::Standard, Slope::EqualGain, FadeShape::Linear),
            (Shape::Standard, Slope::EqualPower, FadeShape::EqualPower),
            (Shape::SCurve, Slope::EqualGain, FadeShape::SCurve),
            (Shape::SCurve, Slope::EqualPower, FadeShape::SCurvePower),
        ] {
            assert_eq!(side(shape, slope).fade().shape, model);
            assert_eq!(Side::of(fade(model, 0.0)), side(shape, slope));
        }
        for preset in [FadeShape::Exponential, FadeShape::Logarithmic] {
            assert_eq!(Side::of(fade(preset, 0.0)).shape, Shape::Preset(preset));
        }
    }

    #[test]
    fn every_fade_comes_up_in_the_dialog_and_goes_back_the_same_and_a_bent_one_is_a_preset_curve() {
        for shape in FadeShape::ALL {
            for curvature in [0.0, 0.3, -1.0, 1.0] {
                let side = Side::of(fade(shape, curvature));
                let back = side.fade();
                assert_eq!((back.shape, back.curvature), (shape, curvature), "{shape:?} {curvature}");
                assert_eq!(side.is_preset(), curvature != 0.0 || matches!(shape, FadeShape::Exponential | FadeShape::Logarithmic));
                // The thumbnail is the real curve.
                assert_eq!(back.gain(0.4), fade(shape, curvature).gain(0.4));
            }
        }
        // Hostile bends (a hand-edited preferences file) come out as something drawable and commandable.
        for bend in [f32::NAN, f32::INFINITY, 7.0, -7.0] {
            let c = Side { bend, ..Side::FACTORY }.fade().curvature;
            assert!(c.is_finite() && (-1.0..=1.0).contains(&c), "{bend}");
        }
    }

    #[test]
    fn a_crossfade_opens_linked_only_when_both_fades_match_and_have_a_linked_partner() {
        let link = |a, b| infer_link(fade(a, 0.0), fade(b, 0.0));
        assert_eq!(link(FadeShape::Linear, FadeShape::Linear), Link::EqualGain);
        assert_eq!(link(FadeShape::SCurve, FadeShape::SCurve), Link::EqualGain);
        assert_eq!(link(FadeShape::EqualPower, FadeShape::EqualPower), Link::EqualPower);
        assert_eq!(link(FadeShape::SCurvePower, FadeShape::SCurvePower), Link::EqualPower);
        assert_eq!(link(FadeShape::Linear, FadeShape::EqualPower), Link::None);
        assert_eq!(link(FadeShape::Exponential, FadeShape::Exponential), Link::None);
        assert_eq!(infer_link(fade(FadeShape::Linear, 0.2), fade(FadeShape::Linear, 0.0)), Link::None);
        assert_eq!(infer_link(fade(FadeShape::Linear, 0.0), fade(FadeShape::Linear, -0.2)), Link::None);
    }

    #[test]
    fn linking_makes_out_follow_in_and_takes_a_preset_curve_back_to_standard() {
        let preset = Side { shape: Shape::Preset(FadeShape::Logarithmic), slope: Slope::EqualPower, bend: 0.4 };
        let set = Settings { input: preset, output: Side::FACTORY, link: Link::EqualGain }.linked();
        assert_eq!(set.input, Side { shape: Shape::Standard, slope: Slope::EqualGain, bend: 0.0 });
        assert_eq!(set.output, set.input);
        assert_eq!((set.input.fade().shape, set.output.fade().shape), (FadeShape::Linear, FadeShape::Linear));
        // Unlinked, each side is its own and keeps its own preset curve.
        let apart = Settings { input: preset, output: Side::FACTORY, link: Link::None };
        assert_eq!(apart.linked(), apart);
    }

    #[test]
    fn a_preset_slot_holds_only_what_its_dialog_edits() {
        let s = Side { shape: Shape::SCurve, slope: Slope::EqualGain, bend: 0.0 };
        let all = Settings { input: s, output: s, link: Link::EqualGain };
        assert_eq!(all.of_kind(Kind::In), Settings { output: Side::FACTORY, link: Link::EqualPower, ..all });
        assert_eq!(all.of_kind(Kind::Out), Settings { input: Side::FACTORY, link: Link::EqualPower, ..all });
        assert_eq!(all.of_kind(Kind::Crossfade), all);
        // Recalling a Fade In slot leaves the Out side alone.
        let mine = Settings { output: Side { shape: Shape::SCurve, ..Side::FACTORY }, ..Settings::FACTORY };
        assert_eq!(mine.with(Kind::In, all).output, mine.output);
        assert_eq!(mine.with(Kind::In, all).input, s);
        let mut presets = FadePresets::default();
        presets.slots(Kind::Out)[3] = Some(all);
        assert_eq!((presets.slot(Kind::Out, 3), presets.slot(Kind::In, 3), presets.slot(Kind::Out, 9)), (Some(all), None, None));
        // A slot with a bend that is not a number (a hand-edited preferences file) recalls as an unbent side.
        let hostile = Settings { input: Side { bend: f32::NAN, ..s }, ..all };
        assert_eq!(Settings::FACTORY.with(Kind::In, hostile).input.bend, 0.0);
        // Old preferences without the slots load empty; the slots survive a round trip.
        assert_eq!(serde_json::from_str::<FadePresets>("{}").ok(), Some(FadePresets::default()));
        assert_eq!(serde_json::from_str::<FadePresets>(&serde_json::to_string(&presets).unwrap_or_default()).ok(), Some(presets));
    }

    #[test]
    fn a_slot_that_cannot_be_read_is_an_empty_slot_and_the_rest_of_the_preferences_load() {
        let good = serde_json::to_value(Settings::FACTORY).unwrap();
        let mut bad_shape = good.clone();
        bad_shape["input"]["shape"] = json!("NoSuchShape");
        let saved = json!({"fade_in": [good, bad_shape, good, {"bogus": 1}, null], "fade_out": "garbage", "crossfade": [good, 7]});
        let presets: FadePresets = serde_json::from_value(saved.clone()).unwrap();
        let slot = |kind, n| presets.slot(kind, n);
        assert_eq!((slot(Kind::In, 0), slot(Kind::In, 2)), (Some(Settings::FACTORY), Some(Settings::FACTORY)));
        assert_eq!((slot(Kind::In, 1), slot(Kind::In, 3), slot(Kind::In, 4)), (None, None, None));
        assert_eq!(slot(Kind::Out, 0), None, "not a list of slots: all empty");
        assert_eq!((slot(Kind::Crossfade, 0), slot(Kind::Crossfade, 1)), (Some(Settings::FACTORY), None));
        // Inside the whole interface state, the preferences load with the slots that could be read.
        let ui: crate::UiState = serde_json::from_value(json!({"fade_presets": saved})).unwrap();
        assert_eq!(ui.fade_presets, presets);
    }

    #[test]
    fn a_slot_recalled_where_its_bend_could_not_be_applied_comes_back_unbent() {
        let (s, _, b) = crossfade_session();
        let mut ed = FadeEditor::for_target(&s, FadeTarget::Single { clip: b, fade_in: true }).unwrap();
        let slot = |shape, slope, bend| Settings { input: Side { shape, slope, bend }, ..Settings::FACTORY };
        let bent_linear = slot(Shape::Preset(FadeShape::Linear), Slope::EqualGain, 0.4);
        // On an existing fade a bend is applied, so it is recalled.
        ed.recall(Kind::In, bent_linear);
        assert_eq!(ed.now.input.bend, 0.4);
        // Over a selection it is not: a bent Linear comes back as Standard with Equal Gain, a preset curve unbent.
        ed.selection = Some((TrackId(1), Range::new(0, 100), FadeSpot::In(b), (Range::new(1000, 3000), None)));
        ed.recall(Kind::In, bent_linear);
        assert_eq!(ed.now.input, Side { shape: Shape::Standard, slope: Slope::EqualGain, bend: 0.0 });
        ed.recall(Kind::In, slot(Shape::Preset(FadeShape::Exponential), Slope::EqualPower, -0.3));
        assert_eq!(ed.now.input, Side { shape: Shape::Preset(FadeShape::Exponential), slope: Slope::EqualPower, bend: 0.0 });
        ed.recall(Kind::In, slot(Shape::SCurve, Slope::EqualGain, f32::NAN));
        assert_eq!(ed.now.input, Side { shape: Shape::SCurve, slope: Slope::EqualGain, bend: 0.0 });
        // A crossfade's command has one bend for both sides and never a recalled one: it comes back unbent there too.
        let mut ed = FadeEditor::for_target(&s, FadeTarget::Crossfade { later: b }).unwrap();
        let bent = Side { shape: Shape::Preset(FadeShape::Linear), slope: Slope::EqualGain, bend: -0.4 };
        ed.recall(Kind::Crossfade, Settings { input: bent, output: bent, link: Link::None });
        let plain = Side { shape: Shape::Standard, slope: Slope::EqualGain, bend: 0.0 };
        assert_eq!((ed.now.input, ed.now.output, ed.now.link), (plain, plain, Link::None));
        assert_eq!(ed.command(&s), None, "the crossfade is two plain Linear fades already: the bend is not something to apply");
    }

    /// Clip A on 0..1200 and clip B on 1000..3000: a crossfade over 1000..1200, both fades Linear and 200 long.
    fn crossfade_session() -> (Session, ClipId, ClipId) {
        let mut s = Session::default();
        let t = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, None);
        let (a, b) = (s.new_clip_id(), s.new_clip_id());
        let (mut ca, mut cb) = (Clip::audio(a, "a", SourceId(1), 0, 0, 1200), Clip::audio(b, "b", SourceId(1), 0, 1000, 2000));
        ca.fade_out = fade(FadeShape::Linear, 0.0);
        cb.fade_in = fade(FadeShape::Linear, 0.0);
        ca.fade_out.len = 200;
        cb.fade_in.len = 200;
        if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
            pl.clips = vec![ca, cb];
        }
        (s, a, b)
    }

    #[test]
    fn a_crossfade_dialog_opens_on_the_pair_and_a_command_only_says_what_changed() {
        let (s, a, b) = crossfade_session();
        let mut ed = FadeEditor::for_target(&s, FadeTarget::Crossfade { later: b }).unwrap();
        assert_eq!((ed.kind(), ed.a, ed.b, ed.span), (Kind::Crossfade, Some(a), Some(b), Range::new(1000, 1200)));
        assert_eq!(ed.settings().link, Link::EqualGain, "two Linear fades: Equal Gain");
        assert_eq!(ed.command(&s), None, "nothing changed");
        // S-Curve with the link on: both sides take the variant the link says.
        ed.now.input.shape = Shape::SCurve;
        ed.now = ed.now.linked();
        let (cmd, p) = ed.command(&s).unwrap();
        assert_eq!(cmd, "edit.fades_create");
        assert_eq!(p, json!({"clip": b.0, "side": "crossfade", "in_shape": "S-Curve", "out_shape": "S-Curve"}));
        ed.now.link = Link::EqualPower;
        ed.now = ed.now.linked();
        let (_, p) = ed.command(&s).unwrap();
        assert_eq!((&p["in_shape"], &p["out_shape"]), (&json!("S-Curve Equal Power"), &json!("S-Curve Equal Power")));
        // Link None: the two sides are independent.
        ed.now.link = Link::None;
        ed.now.output = Side { shape: Shape::Preset(FadeShape::Exponential), ..Side::FACTORY };
        let (_, p) = ed.command(&s).unwrap();
        assert_eq!((&p["in_shape"], &p["out_shape"]), (&json!("S-Curve Equal Power"), &json!("Exponential")));
        assert_eq!(p.get("curvature"), None, "no bend to take off");
    }

    #[test]
    fn a_bent_crossfade_keeps_its_bend_until_a_side_asks_for_none() {
        let (mut s, _, b) = crossfade_session();
        if let Some(c) = s.find_clip_mut(b) {
            c.fade_in.curvature = 0.4;
        }
        let mut ed = FadeEditor::for_target(&s, FadeTarget::Crossfade { later: b }).unwrap();
        assert_eq!(ed.settings().link, Link::None, "a bent fade has no linked partner");
        assert_eq!(ed.settings().input.shape, Shape::Preset(FadeShape::Linear));
        assert_eq!(ed.command(&s), None);
        // Standard on the bent side: same Linear shape, bend off, which the command must say.
        ed.now.input = Side { shape: Shape::Standard, slope: Slope::EqualGain, bend: 0.0 };
        let (_, p) = ed.command(&s).unwrap();
        assert_eq!((&p["in_shape"], p.get("curvature")), (&json!("Linear"), Some(&json!(0.0))));
    }

    #[test]
    fn a_crossfade_command_does_nothing_once_the_earlier_clip_runs_past_the_later_one() {
        let (mut s, a, b) = crossfade_session();
        let mut ed = FadeEditor::for_target(&s, FadeTarget::Crossfade { later: b }).unwrap();
        ed.now.input.shape = Shape::SCurve;
        ed.now = ed.now.linked();
        assert!(ed.command(&s).is_some());
        // The engine does not treat such a pair as a crossfade, and neither does the dialog (as when it opens).
        if let Some(c) = s.find_clip_mut(a) {
            c.length = 5000;
        }
        assert_eq!(ed.command(&s), None);
    }

    #[test]
    fn a_crossfade_command_does_nothing_once_another_clip_leads_into_the_later_one() {
        let (mut s, a, b) = crossfade_session();
        let mut ed = FadeEditor::for_target(&s, FadeTarget::Crossfade { later: b }).unwrap();
        ed.now.input.shape = Shape::SCurve;
        ed.now = ed.now.linked();
        assert!(ed.command(&s).is_some());
        // A gives way to C, over the same stretch: the engine would bend C's fade-out, which the dialog never showed.
        let c = s.new_clip_id();
        if let Some(pl) = s.tracks.first_mut().and_then(|t| t.playlist_mut()) {
            for clip in pl.clips.iter_mut().filter(|clip| clip.id == a) {
                clip.id = c;
            }
        }
        assert_eq!(ed.command(&s), None);
    }

    #[test]
    fn a_single_fade_command_keeps_the_length_and_does_nothing_when_nothing_changed() {
        let (mut s, a, b) = crossfade_session();
        for (clip, fade_in) in [(b, true), (a, false)] {
            let mut ed = FadeEditor::for_target(&s, FadeTarget::Single { clip, fade_in }).unwrap();
            assert_eq!(ed.kind(), if fade_in { Kind::In } else { Kind::Out });
            assert_eq!(ed.command(&s), None);
            let side = if fade_in { &mut ed.now.input } else { &mut ed.now.output };
            *side = Side { shape: Shape::SCurve, slope: Slope::EqualPower, bend: 0.0 };
            let (_, p) = ed.command(&s).unwrap();
            let want =
                json!({"clip": clip.0, "side": if fade_in { "in" } else { "out" }, "length": 200, "shape": "S-Curve Equal Power", "curvature": 0.0});
            assert_eq!(p, want);
        }
        // The fade has gone since the dialog opened: nothing to change.
        let ed = FadeEditor::for_target(&s, FadeTarget::Single { clip: b, fade_in: true }).unwrap();
        if let Some(c) = s.find_clip_mut(b) {
            c.fade_in = Fade::default();
        }
        let mut gone = ed.clone();
        gone.now.input.shape = Shape::SCurve;
        assert_eq!(gone.command(&s), None);
        assert!(FadeEditor::for_target(&s, FadeTarget::Single { clip: ClipId(999), fade_in: true }).is_none());
    }

    #[test]
    fn the_graph_spans_the_fade_and_the_clips_behind_it() {
        let (s, a, b) = crossfade_session();
        let fade_in = FadeEditor::for_target(&s, FadeTarget::Single { clip: b, fade_in: true }).unwrap();
        assert_eq!((fade_in.a, fade_in.b, fade_in.span), (None, Some(b), Range::new(1000, 1200)));
        let fade_out = FadeEditor::for_target(&s, FadeTarget::Single { clip: a, fade_in: false }).unwrap();
        assert_eq!((fade_out.a, fade_out.b, fade_out.span), (Some(a), None, Range::new(1000, 1200)));
    }
}
