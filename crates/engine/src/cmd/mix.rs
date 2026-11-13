//! Mixer, insert, send and automation commands.
use super::{
    CommandSpec, always, bad, bool_or, f32_or, has_range, has_selection, has_tracks, i64_or, param_id, position_param, range_param, str_param,
    tracks_required,
};

use crate::{Engine, EngineError, Result, cmd};
use serde_json::{Value, json};
use soundcraft_model::{AutoParam, AutomationMode, INSERT_SLOTS, Insert, SEND_SLOTS, SendSlot, Session, TrackId};

/// Commands for mixer controls, with live input toggles kept outside document history.
pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("mix.volume", "Set Volume", [], None, "{tracks?, db: -144..12 | delta_db}", has_selection, volume),
        cmd!("mix.pan", "Set Pan", [], None, "{tracks?, pan: -1..1, index?: 0|1}", has_selection, pan),
        cmd!("mix.mute", "Mute", [], Some("Shift+M"), "{tracks?, value?: bool}", has_selection, |e, p| toggle(
            e,
            p,
            "mix.mute",
            |s, t, v| set_grouped(s, t, "mute", |tr| tr.mixer.mute = v),
            |tr| tr.mixer.mute
        )),
        cmd!("mix.solo", "Solo", [], Some("Shift+S"), "{tracks?, value?: bool, exclusive?: bool}", has_selection, solo),
        cmd!("mix.solo_safe", "Solo Safe", [], None, "{tracks?, value?: bool}", has_selection, |e, p| toggle(
            e,
            p,
            "mix.solo_safe",
            |s, t, v| {
                if let Some(tr) = s.track_mut(t) {
                    tr.mixer.solo_safe = v
                }
            },
            |tr| tr.mixer.solo_safe
        )),
        cmd!("mix.clear_solos", "Clear All Solos", [], None, "{}", always, |e, _| {
            for t in &mut e.session_mut().tracks {
                t.mixer.solo = false
            }
            Ok(json!({}))
        }),
        cmd!("mix.clear_mutes", "Clear All Mutes", [], None, "{}", always, |e, _| {
            for t in &mut e.session_mut().tracks {
                t.mixer.mute = false
            }
            Ok(json!({}))
        }),
        cmd!(noundo "mix.record_arm", "Record Enable", [], Some("Shift+R"), "{tracks?, value?: bool}", has_selection, |e, p| toggle_runtime(
            e,
            p,
            "mix.record_arm",
            |s, t, v| set_grouped(s, t, "record", |tr| if tr.kind.has_playlist() {
                tr.mixer.record_arm = v
            }),
            |tr| tr.mixer.record_arm
        )),
        cmd!(noundo "mix.input_monitor", "Input Monitoring", [], Some("Shift+I"), "{tracks?, value?: bool}", has_selection, |e, p| toggle_runtime(
            e,
            p,
            "mix.input_monitor",
            |s, t, v| {
                if let Some(tr) = s.track_mut(t) {
                    tr.mixer.input_monitor = v
                }
            },
            |tr| tr.mixer.input_monitor
        )),
        cmd!("mix.phase_invert", "Phase Invert", [], None, "{tracks?, value?: bool}", has_selection, |e, p| toggle(
            e,
            p,
            "mix.phase_invert",
            |s, t, v| {
                if let Some(tr) = s.track_mut(t) {
                    tr.mixer.phase_invert = v
                }
            },
            |tr| tr.mixer.phase_invert
        )),
        cmd!("mix.trim", "Input Trim", [], None, "{tracks?, db}", has_selection, |e, p| {
            let tracks = tracks_required(e, "mix.trim", p)?;
            let db = (p.get("db").and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| bad("mix.trim", "`db` (number) required"))? as f32)
                .clamp(-144.0, 24.0);
            let s = e.session_mut();
            for t in tracks {
                if let Some(tr) = s.track_mut(t) {
                    tr.mixer.trim_db = db
                }
            }
            Ok(json!({"db": db}))
        }),
        cmd!(
            "mix.automation_mode",
            "Automation Mode",
            [],
            None,
            "{tracks?, mode: off|read|touch|latch|touch/latch|write|trim}",
            has_selection,
            automation_mode
        ),
        cmd!(
            "mix.insert",
            "Insert Plugin",
            [],
            None,
            "{track?, slot?: 0..9 | a..j, plugin: id, params?: {id: value}}. value: number, or text: a Toggle/Choice label or the plugin's own text, e.g. \"29\"; text that needs the plugin's own parsing instantiates it temporarily. Unknown ids and invalid values are errors.",
            has_selection,
            insert
        ),
        cmd!("mix.insert_remove", "Remove Insert", [], None, "{track?, slot}", has_selection, insert_remove),
        cmd!("mix.insert_bypass", "Bypass Insert", [], None, "{track?, slot, value?: bool}", has_selection, insert_bypass),
        cmd!(
            "mix.insert_param",
            "Set Plugin Parameter",
            [],
            None,
            "{track?, slot: 0..9 | a..j | \"instrument\", param: id (string or number), value: number, or text: a Toggle/Choice label or the plugin's own text, e.g. \"29\"; text that needs the plugin's own parsing instantiates it temporarily}",
            has_selection,
            insert_param
        ),
        cmd!(
            "mix.insert_params",
            "Set Plugin Parameters",
            [],
            None,
            "{track?, slot: 0..9 | a..j | \"instrument\", params: {id: value}, preset?: name}. value: number, or text: a Toggle/Choice label or the plugin's own text, e.g. \"29\"; text that needs the plugin's own parsing instantiates it temporarily. Unknown ids and invalid values are errors; nothing is stored if any key fails.",
            has_selection,
            insert_params
        ),
        cmd!("mix.insert_move", "Move Insert", [], None, "{track?, from, to}", has_selection, insert_move),
        cmd!(
            "mix.send",
            "Assign Send",
            [],
            None,
            "{track?, slot?: 0..9|a..j, bus: name, level_db?: -144..12, default -144 (silent; kept when re-targeting), pre_fader?: false}",
            has_selection,
            send
        ),
        cmd!("mix.send_remove", "Remove Send", [], None, "{track?, slot}", has_selection, |e, p| {
            let (t, slot) = track_slot(e, p, "mix.send_remove", SEND_SLOTS)?;
            if let Some(tr) = e.session_mut().track_mut(t)
                && let Some(s) = tr.mixer.sends.get_mut(slot)
            {
                *s = None
            }
            Ok(json!({}))
        }),
        cmd!("mix.send_level", "Send Level", [], None, "{track?, slot, db?, pan?, mute?, pre_fader?}", has_selection, send_level),
        cmd!("mix.new_bus", "New Bus", [], None, "{name, format?: stereo}", always, |e, p| {
            let name = str_param(p, "name").filter(|n| !n.trim().is_empty()).ok_or_else(|| bad("mix.new_bus", "`name` required"))?.to_string();
            let fmt = str_param(p, "format").and_then(soundcraft_model::ChannelFormat::from_id).unwrap_or(soundcraft_model::ChannelFormat::Stereo);
            let id = e.session_mut().add_bus(&name, fmt);
            Ok(json!({"bus": id}))
        }),
        cmd!(
            "automation.set_point",
            "Add Automation Breakpoint",
            [],
            None,
            "{track?, param: volume|pan|mute|send_a_level|plugin:a:<id>|plugin:instrument:<id>, at, value, from?}",
            has_selection,
            set_point
        ),
        cmd!("automation.write_range", "Write Automation", [], None, "{track?, param, start?, end?, value}", has_selection, write_range),
        cmd!("automation.clear", "Clear Automation", [], None, "{track?, param?, start?, end?}", has_selection, clear_lane),
        cmd!("automation.thin", "Thin", ["Edit", "Automation"], Some("Cmd+Alt+T"), "{tracks?, tolerance?: 0.1}", has_selection, |e, p| thin(
            e, p, false
        )),
        cmd!("automation.thin_all", "Thin All", ["Edit", "Automation"], None, "{tolerance?: 0.1}", has_tracks, |e, p| thin(e, p, true)),
        cmd!("automation.write_to_current", "Write to Current", ["Edit", "Automation"], Some("Cmd+/"), "{tracks?}", has_range, |e, p| write_current(
            e, p, false
        )),
        cmd!("automation.write_to_all", "Write to All Enabled", ["Edit", "Automation"], Some("Cmd+Alt+/"), "{tracks?}", has_range, |e, p| {
            write_current(e, p, true)
        }),
        cmd!(
            "automation.volume_to_clip_gain",
            "Convert Volume Automation to Clip Gain",
            ["Edit", "Automation"],
            None,
            "{tracks?}",
            has_range,
            vol_to_clip_gain
        ),
        cmd!(
            "automation.clip_gain_to_volume",
            "Convert Clip Gain to Volume Automation",
            ["Edit", "Automation"],
            None,
            "{tracks?}",
            has_range,
            clip_gain_to_vol
        ),
        cmd!(
            "automation.copy_to_send",
            "Copy to Send...",
            ["Edit", "Automation"],
            Some("Cmd+Alt+H"),
            "{tracks?, send: slot}",
            has_selection,
            copy_to_send
        ),
    ]
}

/// `t` plus the members of its active mix groups that share `attr`.
fn grouped_ids(s: &Session, t: TrackId, attr: &str) -> Vec<TrackId> {
    let mut ids = vec![t];
    for g in &s.groups {
        if g.active && g.mix && g.members.contains(&t) && g.attributes.iter().any(|a| a == attr) {
            for m in &g.members {
                if !ids.contains(m) {
                    ids.push(*m);
                }
            }
        }
    }
    ids
}

/// Apply a change to `t` and its active mix-group siblings.
fn set_grouped(s: &mut Session, t: TrackId, attr: &str, f: impl Fn(&mut soundcraft_model::Track)) {
    for id in grouped_ids(s, t, attr) {
        if let Some(tr) = s.track_mut(id) {
            f(tr);
        }
    }
}

fn toggle(e: &mut Engine, p: &Value, cmd: &str, set: fn(&mut Session, TrackId, bool), get: fn(&soundcraft_model::Track) -> bool) -> Result<Value> {
    let tracks = tracks_required(e, cmd, p)?;
    let s = e.session_mut();
    let v = p.get("value").and_then(Value::as_bool).unwrap_or_else(|| !tracks.iter().all(|t| s.track(*t).is_some_and(get)));
    for t in &tracks {
        set(s, *t, v);
    }
    Ok(json!({"value": v}))
}

/// Toggle live input flags without marking the saved document as changed.
fn toggle_runtime(
    e: &mut Engine,
    p: &Value,
    cmd: &str,
    set: fn(&mut Session, TrackId, bool),
    get: fn(&soundcraft_model::Track) -> bool,
) -> Result<Value> {
    let tracks = tracks_required(e, cmd, p)?;
    let s = e.runtime_session_mut();
    let v = p.get("value").and_then(Value::as_bool).unwrap_or_else(|| !tracks.iter().all(|t| s.track(*t).is_some_and(get)));
    for t in &tracks {
        set(s, *t, v);
    }
    Ok(json!({"value": v}))
}

fn volume(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "mix.volume", p)?;
    let abs = p.get("db").and_then(Value::as_f64);
    let delta = p.get("delta_db").and_then(Value::as_f64);
    if abs.is_none() && delta.is_none() {
        return Err(bad("mix.volume", "`db` or `delta_db` required"));
    }
    let s = e.session_mut();
    // The fader stops at -144 and +12 dB. The reply says when a value was held there: +15 dB used
    // to play at +12 with nothing to show it.
    let clamped = std::cell::Cell::new(false);
    let fader = |db: f32| {
        if !(-144.0..=12.0).contains(&db) {
            clamped.set(true);
        }
        db.clamp(-144.0, 12.0)
    };
    // Tracks already moved as a group sibling of an earlier target must not move again.
    let mut done: Vec<TrackId> = Vec::new();
    for t in &tracks {
        if done.contains(t) {
            continue;
        }
        done.extend(grouped_ids(s, *t, "volume"));
        let cur = s.track(*t).map_or(0.0, |tr| tr.mixer.volume_db);
        let new = match (abs, delta) {
            (Some(a), _) => a as f32,
            (None, Some(d)) => cur + d as f32,
            _ => cur,
        };
        let new = if new.is_finite() { fader(new) } else { cur };
        let d = new - cur;
        set_grouped(s, *t, "volume", |tr| tr.mixer.volume_db = if tr.id == *t { new } else { fader(tr.mixer.volume_db + d) });
    }
    // Every requested track at its final volume, also one that moved as a group sibling of an
    // earlier target and was skipped above.
    let volumes: Vec<Value> = tracks.iter().filter_map(|t| s.track(*t)).map(|tr| json!({"track": tr.id.0, "db": tr.mixer.volume_db})).collect();
    Ok(json!({"tracks": tracks.len(), "volumes": volumes, "clamped": clamped.get()}))
}

fn pan(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "mix.pan", p)?;
    let v = (p.get("pan").and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| bad("mix.pan", "`pan` (number) required"))? as f32)
        .clamp(-1.0, 1.0);
    let idx = i64_or(p, "index", 0).clamp(0, 1) as usize;
    let s = e.session_mut();
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t)
            && let Some(slot) = tr.mixer.pan.get_mut(idx)
        {
            *slot = v;
        }
    }
    Ok(json!({"pan": v}))
}

fn solo(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "mix.solo", p)?;
    let exclusive = bool_or(p, "exclusive", e.session().edit.solo_mode.contains("xor"));
    let s = e.session_mut();
    let v = p.get("value").and_then(Value::as_bool).unwrap_or_else(|| !tracks.iter().all(|t| s.track(*t).is_some_and(|tr| tr.mixer.solo)));
    if exclusive && v {
        for tr in &mut s.tracks {
            tr.mixer.solo = false;
        }
    }
    for t in &tracks {
        set_grouped(s, *t, "solo", |tr| tr.mixer.solo = v);
    }
    Ok(json!({"value": v}))
}

fn automation_mode(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "mix.automation_mode", p)?;
    let m = str_param(p, "mode").and_then(AutomationMode::from_id).ok_or_else(|| bad("mix.automation_mode", "unknown mode"))?;
    let s = e.session_mut();
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            tr.mixer.automation_mode = m;
        }
    }
    Ok(json!({"mode": m}))
}

fn slot_of(p: &Value, key: &str, max: usize) -> Option<usize> {
    let v = p.get(key)?;
    let i = if let Some(n) = v.as_u64() {
        n as usize
    } else {
        let s = v.as_str()?.trim().to_ascii_lowercase();
        let c = s.chars().next()?;
        if s.len() == 1 && c.is_ascii_lowercase() { (c as u8 - b'a') as usize } else { s.parse().ok()? }
    };
    (i < max).then_some(i)
}

fn slot_error(cmd: &str, max: usize) -> EngineError {
    bad(cmd, format!("`slot` must be 0..{} or a..{}", max - 1, (b'a' + max as u8 - 1) as char))
}

fn track_slot(e: &Engine, p: &Value, cmd: &str, max: usize) -> Result<(TrackId, usize)> {
    let t = tracks_required(e, cmd, p)?.first().copied().ok_or_else(|| bad(cmd, "no track"))?;
    let slot = slot_of(p, "slot", max).ok_or_else(|| slot_error(cmd, max))?;
    Ok((t, slot))
}

/// The plugin a parameter command addresses: an insert slot, or the instrument of an
/// instrument or MIDI track (`slot: "instrument"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PluginSlot {
    Insert(usize),
    Instrument,
}

pub(super) fn track_plugin_slot(e: &Engine, p: &Value, cmd: &str) -> Result<(TrackId, PluginSlot)> {
    if p.get("slot").and_then(Value::as_str).is_some_and(|s| s.trim().eq_ignore_ascii_case("instrument")) {
        let t = tracks_required(e, cmd, p)?.first().copied().ok_or_else(|| bad(cmd, "no track"))?;
        return Ok((t, PluginSlot::Instrument));
    }
    track_slot(e, p, cmd, INSERT_SLOTS).map(|(t, i)| (t, PluginSlot::Insert(i)))
}

fn plugin_mut<'a>(s: &'a mut Session, t: TrackId, slot: PluginSlot, cmd: &str) -> Result<&'a mut Insert> {
    let tr = s.track_mut(t).ok_or_else(|| bad(cmd, "no track"))?;
    match slot {
        PluginSlot::Insert(i) => tr.mixer.inserts.get_mut(i).and_then(Option::as_mut).ok_or_else(|| bad(cmd, "empty slot")),
        PluginSlot::Instrument => tr.instrument.as_mut().ok_or_else(|| bad(cmd, "the track has no instrument")),
    }
}

fn plugin_ref<'a>(s: &'a Session, t: TrackId, slot: PluginSlot, cmd: &str) -> Result<&'a Insert> {
    let tr = s.track(t).ok_or_else(|| bad(cmd, "no track"))?;
    match slot {
        PluginSlot::Insert(i) => tr.mixer.inserts.get(i).and_then(Option::as_ref).ok_or_else(|| bad(cmd, "empty slot")),
        PluginSlot::Instrument => tr.instrument.as_ref().ok_or_else(|| bad(cmd, "the track has no instrument")),
    }
}

/// An optional `slot`: absent (or null) means "first free"; a present but invalid value is
/// an error, never silently treated as absent.
fn optional_slot(p: &Value, cmd: &str, max: usize) -> Result<Option<usize>> {
    match p.get("slot") {
        None | Some(Value::Null) => Ok(None),
        Some(_) => slot_of(p, "slot", max).map(Some).ok_or_else(|| slot_error(cmd, max)),
    }
}

/// Text longer than this never reaches plugin FFI. The cap is in Unicode scalar values.
const MAX_PARAM_TEXT_CHARS: usize = 256;

pub(super) struct ParameterValueResolver {
    cmd: &'static str,
    insert: Insert,
    sample_rate: f32,
    instance: Option<Box<dyn soundcraft_dsp::Plugin>>,
    tried_instance: bool,
}

impl ParameterValueResolver {
    pub(super) fn new(cmd: &'static str, insert: &Insert, sample_rate: f32) -> Self {
        Self { cmd, insert: insert.clone(), sample_rate, instance: None, tried_instance: false }
    }

    /// One temporary instance per resolver. Created only when a string needs the plugin's parser,
    /// then reused for every later string in the same command. Stored parameters are applied first.
    fn instance(&mut self) -> Result<&mut (dyn soundcraft_dsp::Plugin + 'static)> {
        if !self.tried_instance {
            self.tried_instance = true;
            let state = self.insert.state_bytes();
            if let Some(mut created) = soundcraft_mix::create_instance(&self.insert.plugin, state.as_deref(), self.sample_rate, 512, 2) {
                for (id, value) in &self.insert.params {
                    created.plugin.set_param(id, *value);
                }
                self.instance = Some(created.plugin);
            }
        }
        let cmd = self.cmd;
        let plugin = self.insert.plugin.clone();
        self.instance.as_deref_mut().ok_or_else(move || bad(cmd, format!("could not instantiate plugin `{plugin}` to parse parameter text")))
    }

    /// Built-in processors have no text of their own (they keep the trait's `None`), so a
    /// temporary instance of one would only cost time: their strings go to the plain-number
    /// fallback. Hosted plugins, and an instance already at hand, are asked.
    fn uses_plugin_text(&self) -> bool {
        self.instance.is_some() || soundcraft_mix::is_third_party(&self.insert.plugin)
    }

    /// Every command stores values the same way: clamped, toggles and choices rounded.
    fn store(&self, param: &soundcraft_dsp::ParamInfo, value: f32) -> f32 {
        param.clamp(value)
    }

    pub(super) fn resolve(&mut self, param: &soundcraft_dsp::ParamInfo, value: &Value) -> Result<f32> {
        if let Some(number) = value.as_f64().filter(|number| number.is_finite()) {
            return Ok(self.store(param, number as f32));
        }
        let text = value.as_str().ok_or_else(|| bad(self.cmd, format!("`{}` must be a finite number or string", param.id)))?;
        let trimmed = text.trim();
        if let Some(labelled) = labelled_value(param, trimmed) {
            return Ok(self.store(param, labelled));
        }
        // Cap before any FFI. Label matching above does not call the plugin.
        if text_too_long(text) {
            return Err(self.invalid_text(param, text, false));
        }
        // The plugin's parser is not a guarantee: some units report success and return 0 for
        // any string. Keep the value only when the plugin's own text for it agrees with what
        // was typed, and refuse a number outside the displayed range instead of clamping it.
        if self.uses_plugin_text()
            && let Some(parsed) = self.instance()?.value_from_text(param.id, text).filter(|parsed| parsed.is_finite())
        {
            return self.accept_plugin_text(param, text, parsed);
        }
        if !soundcraft_mix::is_third_party(&self.insert.plugin)
            && let Ok(parsed) = trimmed.parse::<f32>()
            && parsed.is_finite()
        {
            return Ok(self.store(param, parsed));
        }
        Err(self.invalid_text(param, text, true))
    }

    fn invalid_text(&mut self, param: &soundcraft_dsp::ParamInfo, text: &str, allow_instance: bool) -> EngineError {
        let examples = self.examples(param, allow_instance);
        let shown = shown_text(text);
        let id = param.id;
        bad(self.cmd, format!("`{shown}` is not a value of `{id}`; examples: {examples}"))
    }

    /// `value_from_text` succeeded. Accept `parsed` only when `text` round-trips through the
    /// plugin's display, or report a range error when `text` is a number outside that display.
    fn accept_plugin_text(&mut self, param: &soundcraft_dsp::ParamInfo, text: &str, parsed: f32) -> Result<f32> {
        let rendered = self.rendered_text(param, parsed)?;
        let min_text = self.rendered_text(param, param.min)?;
        let max_text = self.rendered_text(param, param.max)?;
        if text_outside_display_range(text, &min_text, &max_text) {
            let shown = shown_text(text);
            let (min_text, max_text) = (shown_text(&min_text), shown_text(&max_text));
            return Err(bad(self.cmd, format!("`{shown}` is out of range for `{}` ({min_text} … {max_text})", param.id)));
        }
        if !plugin_text_matches(text, &rendered) {
            return Err(self.invalid_text(param, text, true));
        }
        Ok(self.store(param, parsed))
    }

    /// Plugin display for `value`, or [`soundcraft_dsp::ParamInfo::format`] when the plugin has none.
    fn rendered_text(&mut self, param: &soundcraft_dsp::ParamInfo, value: f32) -> Result<String> {
        Ok(match self.instance()?.value_text(param.id, value) {
            Some(text) => text,
            None => param.format(value),
        })
    }

    /// Display text at min, default and max: the plugin's own text when an instance already
    /// exists or this failure is allowed to create one, otherwise SoundCraft's formatting.
    fn examples(&mut self, param: &soundcraft_dsp::ParamInfo, allow_instance: bool) -> String {
        let min = param.min;
        let default = param.default;
        let max = param.max;
        if ((allow_instance && self.uses_plugin_text()) || self.instance.is_some())
            && let Ok(plugin) = self.instance()
        {
            let min = plugin.value_text(param.id, min).map_or_else(|| param.format(min), |text| shown_text(&text));
            let default = plugin.value_text(param.id, default).map_or_else(|| param.format(default), |text| shown_text(&text));
            let max = plugin.value_text(param.id, max).map_or_else(|| param.format(max), |text| shown_text(&text));
            return format!("{min}, {default}, {max}");
        }
        format!("{}, {}, {}", param.format(min), param.format(default), param.format(max))
    }

    /// Test seam: the next string parse uses `instance` instead of loading the insert's plugin.
    #[cfg(test)]
    fn with_instance(cmd: &'static str, insert: &Insert, sample_rate: f32, instance: Box<dyn soundcraft_dsp::Plugin>) -> Self {
        Self { cmd, insert: insert.clone(), sample_rate, instance: Some(instance), tried_instance: true }
    }
}

/// Toggle labels are Off/On (and true/false). Choice labels are the parameter's `choices`.
fn labelled_value(param: &soundcraft_dsp::ParamInfo, text: &str) -> Option<f32> {
    match param.unit {
        soundcraft_dsp::Unit::Toggle => {
            if text.eq_ignore_ascii_case("off") || text.eq_ignore_ascii_case("false") {
                Some(0.0)
            } else if text.eq_ignore_ascii_case("on") || text.eq_ignore_ascii_case("true") {
                Some(1.0)
            } else {
                None
            }
        }
        soundcraft_dsp::Unit::Choice => param.choices.iter().position(|choice| text.eq_ignore_ascii_case(choice)).map(|index| index as f32),
        _ => None,
    }
}

fn text_too_long(text: &str) -> bool {
    text.chars().nth(MAX_PARAM_TEXT_CHARS).is_some()
}

/// Text as it may appear in an error or a response: typed input or a plugin's own display.
/// At most [`MAX_PARAM_TEXT_CHARS`] characters (then `...`), and control characters such as
/// newlines and escapes become U+FFFD so one string cannot fake extra log lines or terminal output.
pub(super) fn shown_text(text: &str) -> String {
    let mut shown = String::new();
    let mut truncated = false;
    for (i, ch) in text.chars().enumerate() {
        if i >= MAX_PARAM_TEXT_CHARS {
            truncated = true;
            break;
        }
        shown.push(if ch.is_control() { char::REPLACEMENT_CHARACTER } else { ch });
    }
    if truncated {
        shown.push_str("...");
    }
    shown
}

/// A number written at the start of `text`: optional sign (`+`, `-` or Unicode minus U+2212),
/// digits and one decimal point. `step` is the place of the last written digit (1, 0.1, …).
struct LeadingNumber {
    value: f64,
    step: f64,
    /// Bytes consumed in the trimmed input.
    end: usize,
}

fn split_first(text: &str) -> Option<(char, &str)> {
    let mut chars = text.chars();
    Some((chars.next()?, chars.as_str()))
}

fn decimal_step(frac_digits: u32) -> f64 {
    let mut step = 1.0_f64;
    for _ in 0..frac_digits.min(12) {
        step /= 10.0;
    }
    step
}

fn leading_float(text: &str) -> Option<LeadingNumber> {
    let s = text.trim();
    let mut rest = s;
    let mut buf = String::new();
    if let Some((c, next)) = split_first(rest)
        && (c == '+' || c == '-' || c == '\u{2212}')
    {
        rest = next;
        buf.push(if c == '\u{2212}' { '-' } else { c });
    }
    let mut saw_digit = false;
    let mut saw_dot = false;
    let mut frac: u32 = 0;
    while let Some((c, next)) = split_first(rest) {
        if c.is_ascii_digit() {
            rest = next;
            buf.push(c);
            saw_digit = true;
            if saw_dot {
                frac = frac.saturating_add(1);
            }
        } else if c == '.' && !saw_dot {
            rest = next;
            buf.push('.');
            saw_dot = true;
        } else {
            break;
        }
    }
    if !saw_digit {
        return None;
    }
    if buf.ends_with('.') {
        buf.pop();
    }
    let value = buf.parse::<f64>().ok().filter(|value| value.is_finite())?;
    let step = if frac == 0 { 1.0 } else { decimal_step(frac) };
    Some(LeadingNumber { value, step, end: s.len().saturating_sub(rest.len()) })
}

/// The whole trimmed string is one number, not a number plus a unit.
fn entire_number(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    let lead = leading_float(trimmed)?;
    (lead.end == trimmed.len()).then_some(lead.value)
}

/// Trim, Unicode-lowercase and collapse inner whitespace.
fn normalize_param_text(text: &str) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    let mut started = false;
    for ch in text.trim().chars() {
        if ch.is_whitespace() {
            if started {
                pending_space = true;
            }
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        for lower in ch.to_lowercase() {
            out.push(lower);
        }
        started = true;
    }
    out
}

/// `rendered` is `text` plus one trailing unit or label (`29` vs `29 ms`, `50` vs `50%`).
fn display_has_one_extra_suffix(text: &str, rendered: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let Some(rest) = rendered.strip_prefix(text) else {
        return false;
    };
    let suffix = rest.trim_start();
    if suffix.is_empty() || suffix.chars().any(char::is_whitespace) {
        return false;
    }
    let sep = rest.len().saturating_sub(suffix.len());
    if sep > 1 {
        return false;
    }
    match suffix.chars().next() {
        Some(c) if c.is_ascii_digit() || c == '.' => false,
        Some(_) => true,
        None => false,
    }
}

fn numbers_agree(input: f64, display: f64, display_step: f64) -> bool {
    let diff = (input - display).abs();
    // Half the last displayed digit, a 0.5% relative slack, and a floor for tiny values.
    let tolerance = 1e-6_f64.max(0.5 * display_step.max(0.0)).max(0.005 * display.abs());
    diff <= tolerance
}

/// True when the plugin's display `rendered` is a faithful reading of `text`.
fn plugin_text_matches(text: &str, rendered: &str) -> bool {
    let normalized = normalize_param_text(text);
    let shown = normalize_param_text(rendered);
    if normalized == shown || display_has_one_extra_suffix(&normalized, &shown) {
        return true;
    }
    match (leading_float(text), leading_float(rendered)) {
        (Some(input), Some(display)) => numbers_agree(input.value, display.value, display.step),
        _ => false,
    }
}

/// `text` is a bare number outside the numeric display at the parameter's endpoints.
/// The slack is half the endpoint's last digit (so `30.4` is still inside a `30` that has no
/// decimals) and not the round-trip's relative tolerance, which would widen the legal range.
fn text_outside_display_range(text: &str, min_text: &str, max_text: &str) -> bool {
    let Some(number) = entire_number(text) else {
        return false;
    };
    let (Some(min_n), Some(max_n)) = (leading_float(min_text), leading_float(max_text)) else {
        return false;
    };
    let (lo, lo_step, hi, hi_step) = if min_n.value <= max_n.value {
        (min_n.value, min_n.step, max_n.value, max_n.step)
    } else {
        (max_n.value, max_n.step, min_n.value, min_n.step)
    };
    let lo_tol = 1e-6_f64.max(0.5 * lo_step.max(0.0));
    let hi_tol = 1e-6_f64.max(0.5 * hi_step.max(0.0));
    number < lo - lo_tol || number > hi + hi_tol
}

fn insert(e: &mut Engine, p: &Value) -> Result<Value> {
    let plugin = str_param(p, "plugin").ok_or_else(|| bad("mix.insert", "`plugin` required"))?.to_string();
    let info = super::clap::plugin_info(&plugin).ok_or_else(|| bad("mix.insert", format!("unknown plugin `{plugin}`")))?;
    let t = tracks_required(e, "mix.insert", p)?.first().copied().ok_or_else(|| bad("mix.insert", "no track"))?;
    let explicit = optional_slot(p, "mix.insert", INSERT_SLOTS)?;
    let rate = e.session().sample_rate.as_f64() as f32;
    let tr = e.session().track(t).ok_or_else(|| bad("mix.insert", "no track"))?;
    let slot =
        explicit.or_else(|| tr.mixer.inserts.iter().position(Option::is_none)).ok_or_else(|| bad("mix.insert", "all insert slots are in use"))?;
    let mut ins = Insert::new(info.id);
    for pi in info.params {
        ins.params.insert(pi.id.to_string(), pi.default);
    }
    if let Some(params) = p.get("params") {
        let obj = params.as_object().ok_or_else(|| bad("mix.insert", "`params` must be an object"))?;
        let mut resolver = ParameterValueResolver::new("mix.insert", &ins, rate);
        for (k, v) in obj {
            let Some(pi) = info.params.iter().find(|x| x.id == k) else {
                return Err(bad("mix.insert", format!("`{plugin}` has no parameter `{k}`")));
            };
            let val = resolver.resolve(pi, v)?;
            ins.params.insert(k.clone(), val);
        }
    }
    let s = e.session_mut();
    let tr = s.track_mut(t).ok_or_else(|| bad("mix.insert", "no track"))?;
    if let Some(sl) = tr.mixer.inserts.get_mut(slot) {
        *sl = Some(ins);
    }
    Ok(json!({"track": t, "slot": slot, "plugin": info.id}))
}

fn insert_remove(e: &mut Engine, p: &Value) -> Result<Value> {
    let (t, slot) = track_slot(e, p, "mix.insert_remove", INSERT_SLOTS)?;
    if let Some(tr) = e.session_mut().track_mut(t)
        && let Some(s) = tr.mixer.inserts.get_mut(slot)
    {
        *s = None;
    }
    Ok(json!({}))
}

fn insert_bypass(e: &mut Engine, p: &Value) -> Result<Value> {
    let (t, slot) = track_slot(e, p, "mix.insert_bypass", INSERT_SLOTS)?;
    let val = p.get("value").and_then(Value::as_bool);
    let s = e.session_mut();
    let ins = s
        .track_mut(t)
        .and_then(|tr| tr.mixer.inserts.get_mut(slot))
        .and_then(Option::as_mut)
        .ok_or_else(|| bad("mix.insert_bypass", "empty slot"))?;
    ins.bypass = val.unwrap_or(!ins.bypass);
    Ok(json!({"bypass": ins.bypass}))
}

fn insert_param(e: &mut Engine, p: &Value) -> Result<Value> {
    let (t, slot) = track_plugin_slot(e, p, "mix.insert_param")?;
    let param = param_id("mix.insert_param", p, "param")?;
    let value = p.get("value").ok_or_else(|| bad("mix.insert_param", "`value` must be a finite number or string"))?;
    let ins = plugin_ref(e.session(), t, slot, "mix.insert_param")?.clone();
    let info = super::clap::plugin_info(&ins.plugin).ok_or_else(|| bad("mix.insert_param", format!("unknown plugin `{}`", ins.plugin)))?;
    let pi =
        info.params.iter().find(|x| x.id == param).ok_or_else(|| bad("mix.insert_param", format!("`{}` has no parameter `{param}`", info.id)))?;
    let mut resolver = ParameterValueResolver::new("mix.insert_param", &ins, e.session().sample_rate.as_f64() as f32);
    let v = resolver.resolve(pi, value)?;
    let ins = plugin_mut(e.session_mut(), t, slot, "mix.insert_param")?;
    ins.params.insert(param, v);
    Ok(json!({"value": v}))
}

/// Apply many parameters at once (preset recall); values are clamped. Unknown ids and
/// invalid values are errors, not silent no-ops: a typo must not look like success.
fn insert_params(e: &mut Engine, p: &Value) -> Result<Value> {
    let (t, slot) = track_plugin_slot(e, p, "mix.insert_params")?;
    let obj = p.get("params").and_then(Value::as_object).cloned().ok_or_else(|| bad("mix.insert_params", "`params` must be an object"))?;
    let preset = str_param(p, "preset").map(str::to_string);
    let ins = plugin_ref(e.session(), t, slot, "mix.insert_params")?.clone();
    let info = super::clap::plugin_info(&ins.plugin).ok_or_else(|| bad("mix.insert_params", format!("unknown plugin `{}`", ins.plugin)))?;
    let mut unknown: Vec<String> = Vec::new();
    for k in obj.keys() {
        if !info.params.iter().any(|x| x.id == k.as_str()) {
            unknown.push(k.clone());
        }
    }
    if !unknown.is_empty() {
        unknown.sort_unstable();
        return Err(bad("mix.insert_params", format!("`{}` has no parameter(s): {}", info.id, unknown.join(", "))));
    }
    let mut resolver = ParameterValueResolver::new("mix.insert_params", &ins, e.session().sample_rate.as_f64() as f32);
    let mut values = Vec::with_capacity(obj.len());
    for (k, value) in obj {
        let Some(pi) = info.params.iter().find(|x| x.id == k) else { continue };
        values.push((k, resolver.resolve(pi, &value)?));
    }
    let n = values.len();
    let ins = plugin_mut(e.session_mut(), t, slot, "mix.insert_params")?;
    for (k, value) in values {
        ins.params.insert(k, value);
    }
    if let Some(name) = preset {
        ins.preset = name;
    }
    Ok(json!({"set": n}))
}

fn insert_move(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = tracks_required(e, "mix.insert_move", p)?.first().copied().ok_or_else(|| bad("mix.insert_move", "no track"))?;
    let from = slot_of(p, "from", INSERT_SLOTS).ok_or_else(|| bad("mix.insert_move", "`from` slot"))?;
    let to = slot_of(p, "to", INSERT_SLOTS).ok_or_else(|| bad("mix.insert_move", "`to` slot"))?;
    if let Some(tr) = e.session_mut().track_mut(t) {
        tr.mixer.inserts.swap(from, to);
    }
    Ok(json!({}))
}

fn send(e: &mut Engine, p: &Value) -> Result<Value> {
    let bus = str_param(p, "bus").ok_or_else(|| bad("mix.send", "`bus` required"))?.to_string();
    let t = tracks_required(e, "mix.send", p)?.first().copied().ok_or_else(|| bad("mix.send", "no track"))?;
    let explicit = optional_slot(p, "mix.send", SEND_SLOTS)?;
    let s = e.session_mut();
    let bid = s.bus_by_name(&bus).map(|b| b.id).ok_or_else(|| bad("mix.send", format!("no bus named \"{bus}\"")))?;
    let tr = s.track_mut(t).ok_or_else(|| bad("mix.send", "no track"))?;
    let slot = explicit.or_else(|| tr.mixer.sends.iter().position(Option::is_none)).ok_or_else(|| bad("mix.send", "all send slots are in use"))?;
    // Re-targeting an existing send keeps its level, pan, mute and pre/post setting.
    let mut sl = match tr.mixer.sends.get(slot).cloned().flatten() {
        Some(old) => SendSlot { target: soundcraft_model::Route::Bus(bid), ..old },
        None => SendSlot::new(soundcraft_model::Route::Bus(bid)),
    };
    sl.level_db = f32_or(p, "level_db", sl.level_db).clamp(-144.0, 12.0);
    sl.pre_fader = bool_or(p, "pre_fader", sl.pre_fader);
    if let Some(x) = tr.mixer.sends.get_mut(slot) {
        *x = Some(sl);
    }
    Ok(json!({"slot": slot, "bus": bid}))
}

fn send_level(e: &mut Engine, p: &Value) -> Result<Value> {
    let (t, slot) = track_slot(e, p, "mix.send_level", SEND_SLOTS)?;
    let s = e.session_mut();
    let sl = s
        .track_mut(t)
        .and_then(|tr| tr.mixer.sends.get_mut(slot))
        .and_then(Option::as_mut)
        .ok_or_else(|| bad("mix.send_level", "empty send slot"))?;
    if let Some(db) = p.get("db").and_then(Value::as_f64) {
        sl.level_db = (db as f32).clamp(-144.0, 12.0);
    }
    if let Some(v) = p.get("pan").and_then(Value::as_f64) {
        sl.pan = (v as f32).clamp(-1.0, 1.0);
    }
    if let Some(m) = p.get("mute").and_then(Value::as_bool) {
        sl.mute = m;
    }
    if let Some(m) = p.get("pre_fader").and_then(Value::as_bool) {
        sl.pre_fader = m;
    }
    Ok(json!({"send": sl.clone()}))
}

fn lane_param(p: &Value, cmd: &str) -> Result<AutoParam> {
    str_param(p, "param").map_or(Ok(AutoParam::Volume), |s| AutoParam::parse(s).ok_or_else(|| bad(cmd, format!("unknown automation param `{s}`"))))
}

/// With `from` (a write pass's previous point), the points between `from` and `at` are removed first.
fn set_point(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "automation.set_point", p)?;
    let param = lane_param(p, "automation.set_point")?;
    let at = position_param(e, "automation.set_point", p, "at")?.unwrap_or(e.session().edit.selection.start).max(0);
    let from = position_param(e, "automation.set_point", p, "from")?;
    let v = p.get("value").and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| bad("automation.set_point", "`value` required"))? as f32;
    let s = e.session_mut();
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            let lane = tr.lane_mut(&param);
            if let Some(from) = from {
                lane.clear_range(from.saturating_add(1), at);
            }
            lane.set_point(at, v);
        }
    }
    Ok(json!({"at": at, "value": v}))
}

fn write_range(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "automation.write_range", p)?;
    let param = lane_param(p, "automation.write_range")?;
    let r = range_param(e, "automation.write_range", p)?;
    let v = p.get("value").and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| bad("automation.write_range", "`value` required"))? as f32;
    let s = e.session_mut();
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            let def = current_value(tr, &param);
            tr.lane_mut(&param).write_range(r.start, r.end, v, def);
        }
    }
    Ok(json!({}))
}

/// The static (non-automated) value of a parameter on a track.
pub(crate) fn current_value(tr: &soundcraft_model::Track, param: &AutoParam) -> f32 {
    match param {
        AutoParam::Volume => tr.mixer.volume_db,
        AutoParam::Pan(i) => tr.mixer.pan.get(usize::from(*i)).copied().unwrap_or(0.0),
        AutoParam::Mute => f32::from(u8::from(tr.mixer.mute)),
        AutoParam::SendLevel(i) => tr.mixer.sends.get(usize::from(*i)).and_then(Option::as_ref).map_or(-144.0, |s| s.level_db),
        AutoParam::SendPan(i) => tr.mixer.sends.get(usize::from(*i)).and_then(Option::as_ref).map_or(0.0, |s| s.pan),
        AutoParam::SendMute(i) => tr.mixer.sends.get(usize::from(*i)).and_then(Option::as_ref).map_or(0.0, |s| f32::from(u8::from(s.mute))),
        // A parameter the session holds no value for (a hosted instrument made by `track.new`)
        // starts from the plugin's default, not 0.
        AutoParam::Plugin { slot, param } => tr.plugin_at(*slot).map_or(0.0, |i| {
            i.params
                .get(param)
                .copied()
                .or_else(|| super::clap::plugin_info(&i.plugin).and_then(|info| info.param(param)).map(|pi| pi.default))
                .unwrap_or(0.0)
        }),
    }
}

fn clear_lane(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "automation.clear", p)?;
    let param = if str_param(p, "param").is_some() { Some(lane_param(p, "automation.clear")?) } else { None };
    let r = range_param(e, "automation.clear", p)?;
    let (a, b) = if r.is_empty() { (i64::MIN, i64::MAX) } else { (r.start, r.end) };
    let s = e.session_mut();
    let mut n = 0;
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            for l in &mut tr.automation {
                if param.as_ref().is_none_or(|x| x == &l.param) {
                    n += l.clear_range(a, b);
                }
            }
        }
    }
    Ok(json!({"removed": n}))
}

fn thin(e: &mut Engine, p: &Value, all: bool) -> Result<Value> {
    let tracks = if all { e.session().tracks.iter().map(|t| t.id).collect() } else { tracks_required(e, "automation.thin", p)? };
    let tol = f32_or(p, "tolerance", 0.1).clamp(0.0, 100.0);
    let s = e.session_mut();
    let mut n = 0;
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            for l in &mut tr.automation {
                n += l.thin(tol);
            }
        }
    }
    Ok(json!({"removed": n}))
}

fn write_current(e: &mut Engine, p: &Value, all: bool) -> Result<Value> {
    let tracks = tracks_required(e, "automation.write_to_current", p)?;
    let r = range_param(e, "automation.write_to_current", p)?;
    let s = e.session_mut();
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            let params: Vec<AutoParam> = if all {
                let mut v = vec![AutoParam::Volume, AutoParam::Pan(0), AutoParam::Mute];
                v.extend(tr.automation.iter().map(|l| l.param.clone()));
                v.sort();
                v.dedup();
                v
            } else {
                vec![AutoParam::parse(&tr.view).unwrap_or(AutoParam::Volume)]
            };
            for param in params {
                let v = current_value(tr, &param);
                tr.lane_mut(&param).write_range(r.start, r.end, v, v);
            }
        }
    }
    Ok(json!({}))
}

/// Clip-gain envelope value in dB. An empty envelope contributes nothing, and the last point holds.
#[cfg(test)]
fn env_db(env: &[(i64, f32)], at: i64) -> f32 {
    let mut prev: Option<(i64, f32)> = None;
    for &(t, v) in env {
        if t >= at {
            return match prev {
                Some((pt, pv)) if t > pt => pv + (v - pv) * ((at - pt) as f32 / (t - pt) as f32),
                _ => v,
            };
        }
        prev = Some((t, v));
    }
    prev.map_or(0.0, |(_, v)| v)
}

/// Repeated lookups in one clip-gain envelope, each O(log n) instead of a walk from its start.
/// Gives exactly what walking the points in stored order gives, sorted or not: the first point
/// at or after `at` is the first whose running maximum position reaches `at`.
struct EnvLookup<'a> {
    env: &'a [(i64, f32)],
    /// Largest position among `env[..=i]`.
    reach: Vec<i64>,
}

impl<'a> EnvLookup<'a> {
    fn new(env: &'a [(i64, f32)]) -> Self {
        let reach = env
            .iter()
            .scan(i64::MIN, |m, &(t, _)| {
                *m = (*m).max(t);
                Some(*m)
            })
            .collect();
        EnvLookup { env, reach }
    }

    /// Value in dB at `at` (see `env_db`).
    fn db(&self, at: i64) -> f32 {
        let i = self.reach.partition_point(|&m| m < at);
        let Some(&(t, v)) = self.env.get(i) else { return self.env.last().map_or(0.0, |&(_, v)| v) };
        match i.checked_sub(1).and_then(|p| self.env.get(p)) {
            Some(&(pt, pv)) if t > pt => pv + (v - pv) * ((at - pt) as f32 / (t - pt) as f32),
            _ => v,
        }
    }
}

/// Fold the volume `lane` into the gain envelope of clip `c` over the selection `r`.
fn bake_volume_into_clip(c: &mut soundcraft_model::Clip, lane: &soundcraft_model::AutomationLane, base: f32, r: soundcraft_time::Range) {
    let delta = |at: i64| lane.value_at(at, base) - base;
    let sel0 = r.start.max(c.start);
    let sel1 = r.end.min(c.end());
    if sel1 <= sel0 {
        return;
    }
    let rel = |at: i64| at - c.start;
    let old_env = std::mem::take(&mut c.gain_env);
    let env = EnvLookup::new(&old_env);
    let env_db = |at: i64| env.db(at);
    let rel0 = rel(sel0);
    let rel1 = rel(sel1);
    // Later writes win, so the selection anchors replace any point they land on.
    let mut pts: Vec<(i64, f32)> = old_env.iter().copied().filter(|&(at, _)| at < rel0.saturating_sub(1) || (sel1 < c.end() && at > rel1)).collect();
    if rel0 > 0 {
        pts.push((rel0 - 1, env_db(rel0 - 1)));
    }
    pts.push((rel0, env_db(rel0) + delta(sel0)));
    for pt in lane.points.iter().filter(|pt| pt.at > sel0 && pt.at < sel1) {
        let at = rel(pt.at);
        pts.push((at, env_db(at) + delta(pt.at)));
    }
    // The clip's own gain points inside the selection keep their shape, offset by the lane.
    for &(at, _) in old_env.iter().filter(|&&(at, _)| at > rel0 && at < rel1) {
        pts.push((at, env_db(at) + delta(c.start.saturating_add(at))));
    }
    if sel1 - 1 > sel0 {
        let at = rel(sel1 - 1);
        pts.push((at, env_db(at) + delta(sel1 - 1)));
    }
    if sel1 < c.end() {
        pts.push((rel1, env_db(rel1)));
    }
    pts.sort_by_key(|p| p.0);
    let mut baked = Vec::new();
    for p in pts {
        if baked.last().is_some_and(|q: &(i64, f32)| q.0 == p.0) {
            baked.pop();
        }
        baked.push(p);
    }
    c.gain_env = baked;
}

/// Clear the volume lane over `r` when the clips all sit inside it, else hold the fader there.
fn reset_volume_lane(tr: &mut soundcraft_model::Track, lane: &soundcraft_model::AutomationLane, base: f32, r: soundcraft_time::Range) {
    // Clearing the lane is only safe when no audio plays outside the selection; otherwise the
    // held edge values outside would fall back to the fader level.
    let clips_inside = tr.clips().iter().all(|c| c.start >= r.start && c.end() <= r.end);
    let covers_all = clips_inside && lane.points.iter().all(|pt| pt.at >= r.start && pt.at < r.end);
    let lane_m = tr.lane_mut(&AutoParam::Volume);
    if covers_all {
        lane_m.clear_range(r.start, r.end);
    } else {
        // Hold the fader across the selection and anchor the samples on either side, so the
        // curve outside does not interpolate through the gap or keep the old level inside.
        let before = (r.start > 0).then(|| lane.value_at(r.start - 1, base));
        let after = lane.value_at(r.end, base);
        lane_m.clear_range(r.start, r.end);
        if let Some(v) = before {
            lane_m.set_point(r.start - 1, v);
        }
        lane_m.set_point(r.start, base);
        if r.end - 1 > r.start {
            lane_m.set_point(r.end - 1, base);
        }
        lane_m.set_point(r.end, after);
    }
}

fn vol_to_clip_gain(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "automation.volume_to_clip_gain", p)?;
    let r = range_param(e, "automation.volume_to_clip_gain", p)?;
    let s = e.session_mut();
    for t in &tracks {
        let Some(tr) = s.track_mut(*t) else { continue };
        let Some(lane) = tr.lane(&AutoParam::Volume).cloned().filter(|l| !l.points.is_empty()) else { continue };
        let base = tr.mixer.volume_db;
        if let Some(pl) = tr.playlist_mut() {
            for c in pl.clips.iter_mut().filter(|c| c.range().overlaps(&r)) {
                bake_volume_into_clip(c, &lane, base, r);
            }
        }
        reset_volume_lane(tr, &lane, base, r);
    }
    Ok(json!({}))
}

fn clip_gain_to_vol(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "automation.clip_gain_to_volume", p)?;
    let r = range_param(e, "automation.clip_gain_to_volume", p)?;
    let s = e.session_mut();
    for t in &tracks {
        let Some(tr) = s.track_mut(*t) else { continue };
        let base = tr.mixer.volume_db;
        // An existing lane overrides the fader, so the clip gain rides on top of the lane value.
        let old = tr.lane(&AutoParam::Volume).cloned();
        let level = |at: i64| old.as_ref().map_or(base, |l| l.value_at(at, base));
        let mut pins = Vec::new();
        let mut pts = Vec::new();
        if let Some(pl) = tr.playlist_mut() {
            for c in pl.clips.iter_mut().filter(|c| c.range().overlaps(&r)) {
                // Pin the unchanged level just outside the clip so the neighbouring segments keep their shape.
                if c.start > 0 {
                    pins.push((c.start - 1, level(c.start - 1)));
                }
                pins.push((c.end(), level(c.end())));
                // The clip edges always get a point, so the envelope's first and last values hold out to them.
                let mut ats = vec![c.start, c.end() - 1];
                ats.extend(c.gain_env.iter().map(|&(o, _)| c.start + o).filter(|&at| at > c.start && at < c.end() - 1));
                // The lane's own points inside the clip carry the clip gain too.
                if let Some(l) = &old {
                    ats.extend(l.points.iter().map(|pt| pt.at).filter(|&at| at > c.start && at < c.end() - 1));
                }
                let env = EnvLookup::new(&c.gain_env);
                for at in ats {
                    pts.push((at, level(at) + c.gain_db + env.db(at - c.start)));
                }
                c.gain_db = 0.0;
                c.gain_env.clear();
            }
        }
        let lane = tr.lane_mut(&AutoParam::Volume);
        // Pins first, so a neighbouring clip's converted points win where they meet.
        for (at, v) in pins.into_iter().chain(pts) {
            lane.set_point(at, v);
        }
    }
    Ok(json!({}))
}

fn copy_to_send(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "automation.copy_to_send", p)?;
    // No `send` means Send A (the menu item passes none); a given one must be valid.
    let slot = match p.get("send") {
        None | Some(Value::Null) => 0,
        Some(_) => slot_of(p, "send", SEND_SLOTS).ok_or_else(|| {
            bad("automation.copy_to_send", format!("`send` must be 0..{} or a..{}", SEND_SLOTS - 1, (b'a' + SEND_SLOTS as u8 - 1) as char))
        })?,
    } as u8;
    let s = e.session_mut();
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            let pts = tr.lane(&AutoParam::Volume).map(|l| l.points.clone()).unwrap_or_default();
            let v = tr.mixer.volume_db;
            let lane = tr.lane_mut(&AutoParam::SendLevel(slot));
            if pts.is_empty() {
                lane.set_point(0, v);
            }
            for pt in pts {
                lane.set_point(pt.at, pt.value);
            }
        }
    }
    Ok(json!({}))
}

#[cfg(test)]
mod tests;
