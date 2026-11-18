//! Command plug-ins: WebAssembly modules that act on the open session through the same commands as
//! the menus, the CLI, the control channel and MCP, inside the sandbox of the shared
//! `craft-plugin` crate (RFC-0046). They are not audio plug-ins: those are hosted by the CLAP,
//! VST3 and Audio Unit crates and run in the mixer. A command plug-in never runs on the audio
//! thread; it edits the session document through commands only, as one undo step.
//!
//! This module is SoundCraft's side of it: which commands a plug-in may call ([`policy`],
//! [`refusal`]), one undo step per run named after the plug-in ([`run`], rolled back when the run
//! fails), read-only access, the transport, the clipboard and live inputs checked after every call
//! (`host`), each plug-in's settings (kept with the app's preferences by the UI) and the installed
//! plug-ins (the user's `Plug-ins` folder in the settings directory, or this session only). The
//! `plugin.*` commands are in `cmd::command_plugins`. See `docs/plugins.md`.

mod host;
mod install;
mod runtime;
#[cfg(test)]
mod tests;

use std::sync::OnceLock;

pub use craft_plugin;
use craft_plugin::{API_VERSION, AppInfo, Instance, KIND_COMMAND, Limits, Manifest, Policy, RunOutput, UiChange};
use serde_json::{Map, Value, json};

use crate::cmd::bad;
use crate::{Engine, EngineError, Result};
use host::{EngineHost, Mark};
pub use install::{Candidate, catalog_packages_with, download, offer, update_for_with};
pub(crate) use install::{check_catalog_url, hold, install, obtain, remove, request_key, review, take_held};
pub use runtime::{Fetcher, InstalledPlugin, PluginPrefs, Runtime};

/// SoundCraft as manifests name it: command plug-ins of every shared ABI version, and only
/// manifests that name `soundcraft` in `apps`.
pub const APP: AppInfo<'static> = AppInfo {
    id: "soundcraft",
    version: env!("CARGO_PKG_VERSION"),
    api: 1..=API_VERSION,
    kinds: &[KIND_COMMAND],
    accept_legacy: false,
    kind_api: &[],
};

/// What one plug-in may keep in its settings (keys and JSON values, in bytes), and its longest key.
pub const MAX_SETTINGS_BYTES: usize = 64 * 1024;
pub const MAX_SETTING_KEY: usize = 128;
/// What every plug-in's settings take up together: they are saved in the app's preferences file,
/// which has a size limit of its own.
pub const MAX_ALL_SETTINGS_BYTES: usize = 1024 * 1024;
/// Plug-ins whose settings are kept (more are dropped when the preferences are read).
pub const MAX_SETTINGS_PLUGINS: usize = 64;
/// Catalogs a person may add.
pub const MAX_CATALOGS: usize = 32;
/// Messages a run may show (`notify`), and their length in characters.
const MAX_NOTES: usize = 20;
const MAX_NOTE_CHARS: usize = 1000;
/// Least instruction budget `plugin.run {fuel}` may ask for.
const MIN_FUEL: u64 = 1000;

/// The only commands a command plug-in may call: everything else is refused (`*` in [`policy`]),
/// so a command added to SoundCraft later is refused until someone decides it is safe here.
///
/// What is on it edits the open session's document (tracks, clips, playlists, automation, MIDI,
/// markers, tempo and meter, the mixer's built-in processors, sends and routing inside the
/// session), selects, and reads (queries). What is not, and why:
/// - the transport, playback and recording (`transport.*` but its status, `edit.play_timeline`,
///   `edit.tab` and `markers.recall`/`next`/`previous`, which locate the playhead,
///   `mix.record_arm`, `mix.input_monitor`, `event.step_input`, `event.retrospective_record`,
///   `event.all_notes_off`, `track.set_record_to_input_only`): they reach the audio engine and
///   its callback, which a run never touches;
/// - devices and the machine (`setup.*` but the session's own settings, `event.input_quantize`,
///   `event.midi_track_offsets`), options and the app's behaviour (`options.*`, `edit.mode`,
///   `edit.tool`, `edit.shuffle_lock`, `edit.snap_to_grid`), the view (`view.*`, `track.scroll_to`);
/// - other sessions and files on disk (`session.*` but the queries, `file.*`, track presets,
///   sample files, relinking movies, a hosted plug-in's saved state) and the audio plug-in scan
///   (`engine.*` but `engine.commands`);
/// - the undo history (`edit.undo`, `edit.redo`, `engine.history`) and the clipboard, which is the
///   person's (`edit.copy*` but the playlist copies, `edit.cut*`, `edit.paste*`);
/// - other command plug-ins (`plugin.*`) and the app's windows.
///
/// Hosted (third-party) audio plug-ins stay out even through allowed commands ([`refusal`]).
pub const ALLOWED: &[&str] = &[
    // reading
    "session.inspect",
    "session.format_time",
    "session.audio_health",
    "transport.status",
    "engine.commands",
    "clip.inspect",
    "track.inspect",
    "midi.notes",
    "video.list",
    "view.flags",
    "edit.strip_silence_preview",
    "event.midi_ops_window",
    "event.tempo_ops_window",
    "event.time_ops_window",
    // the session's own settings
    "setup.session",
    "setup.current_timecode",
    // selecting
    "edit.select",
    "edit.select_all",
    "edit.select_none",
    "edit.restore_last_selection",
    "edit.double_selection",
    "edit.halve_selection",
    "edit.extend_selection_down",
    "edit.extend_selection_up",
    "edit.extend_down_to_members",
    "edit.extend_up_to_members",
    "edit.remove_selection_bottom",
    "edit.remove_selection_top",
    "edit.move_selection_left",
    "edit.move_selection_right",
    "edit.selection_to_timeline",
    "edit.timeline_to_selection",
    "edit.marker_lane_extend_down",
    "edit.marker_lane_extend_up",
    "edit.marker_lane_move_down",
    "edit.marker_lane_move_up",
    "event.select_split_notes",
    // editing clips and selections
    "edit.clear",
    "edit.clear_all_automation",
    "edit.clear_clip_effects",
    "edit.clear_clip_gain",
    "edit.clear_pan_automation",
    "edit.clear_plugin_automation",
    "edit.consolidate",
    "edit.copy_to_duplicate_playlist",
    "edit.copy_to_new_playlist",
    "edit.copy_to_target_playlist",
    "edit.move_to_duplicate_playlist",
    "edit.move_to_new_playlist",
    "edit.move_to_target_playlist",
    "edit.duplicate",
    "edit.duplicate_extend",
    "edit.fade_to_end",
    "edit.fade_to_start",
    "edit.fades_create",
    "edit.fades_delete",
    "edit.heal",
    "edit.insert_silence",
    "edit.move_clips",
    "edit.mute_clips",
    "edit.nudge",
    "edit.repeat",
    "edit.separate",
    "edit.separate_at_transients",
    "edit.separate_on_grid",
    "edit.shift",
    "edit.snap_next",
    "edit.snap_previous",
    "edit.space_clips",
    "edit.strip_silence",
    "edit.tce_to_timeline",
    "edit.trim_clip",
    "edit.trim_end_to_insertion",
    "edit.trim_start_to_insertion",
    "edit.trim_to_file_boundaries",
    "edit.trim_to_fill_selection",
    "edit.trim_to_selection",
    "edit.automation_coalesce_clip_gain_to_volume",
    "edit.automation_coalesce_volume_to_clip_gain",
    "edit.automation_glide_pan",
    "edit.automation_glide_to_all",
    "edit.automation_trim_to_all",
    "clip.auto_rename",
    "clip.bring_to_front",
    "clip.capture",
    "clip.clear",
    "clip.color",
    "clip.conform_to_tempo",
    "clip.edit_lock",
    "clip.effects_bypass",
    "clip.effects_render",
    "clip.effects_set",
    "clip.elastic_properties",
    "clip.gain",
    "clip.gain_bypass",
    "clip.gain_nudge_down",
    "clip.gain_nudge_up",
    "clip.gain_render",
    "clip.group",
    "clip.identify_sync_point",
    "clip.loop",
    "clip.place_source",
    "clip.quantize_to_grid",
    "clip.rating",
    "clip.regroup",
    "clip.remove_pitch_shift",
    "clip.remove_sync_point",
    "clip.remove_warp",
    "clip.rename",
    "clip.send_to_back",
    "clip.time_lock",
    "clip.ungroup",
    "clip.ungroup_all",
    "clip.unloop",
    "audiosuite.process",
    // automation
    "automation.clear",
    "automation.clip_gain_to_volume",
    "automation.copy_to_send",
    "automation.set_point",
    "automation.thin",
    "automation.thin_all",
    "automation.volume_to_clip_gain",
    "automation.write_range",
    "automation.write_to_all",
    "automation.write_to_current",
    // MIDI, tempo, meter and key
    "midi.new_clip",
    "midi.note_add",
    "midi.note_delete",
    "midi.note_edit",
    "midi.notes_move",
    "event.add_key_change",
    "event.beat_detective",
    "event.change_duration",
    "event.change_velocity",
    "event.cut_time",
    "event.extract_chords",
    "event.flatten_performance",
    "event.identify_beat",
    "event.insert_time",
    "event.meter",
    "event.midi_note",
    "event.midi_rtp",
    "event.move_song_start",
    "event.quantize",
    "event.remove_duplicate_notes",
    "event.renumber_bars",
    "event.restore_performance",
    "event.tempo",
    "event.tempo_constant",
    "event.tempo_linear",
    "event.tempo_parabolic",
    "event.tempo_remove",
    "event.tempo_s_curve",
    "event.tempo_scale",
    "event.tempo_stretch",
    "event.transpose",
    // markers
    "markers.add",
    "markers.delete",
    "markers.edit",
    // the mixer (built-in processors only: see `refusal`)
    "mix.automation_mode",
    "mix.clear_mutes",
    "mix.clear_solos",
    "mix.insert",
    "mix.insert_bypass",
    "mix.insert_move",
    "mix.insert_param",
    "mix.insert_params",
    "mix.insert_remove",
    "mix.instrument",
    "mix.instrument_param",
    "mix.mute",
    "mix.new_bus",
    "mix.pan",
    "mix.phase_invert",
    "mix.sampler_clear",
    "mix.send",
    "mix.send_level",
    "mix.send_remove",
    "mix.send_surround_pan",
    "mix.solo",
    "mix.solo_safe",
    "mix.surround_pan",
    "mix.trim",
    "mix.volume",
    // tracks
    "track.bounce",
    "track.bypass_delay",
    "track.bypass_dynamics",
    "track.bypass_eq",
    "track.bypass_inserts",
    "track.bypass_inserts_ae",
    "track.bypass_inserts_fj",
    "track.bypass_modulation",
    "track.bypass_reverb",
    "track.change_width",
    "track.clear_trim_automation",
    "track.coalesce_trim",
    "track.coalesce_vca",
    "track.color",
    "track.comments",
    "track.commit",
    "track.convert_aux_to_folder",
    "track.create_click",
    "track.delete",
    "track.designate_target_playlist",
    "track.duplicate",
    "track.elastic",
    "track.extract_midi",
    "track.folder_toggle",
    "track.freeze",
    "track.group",
    "track.group_modify",
    "track.group_toggle",
    "track.height",
    "track.height_down",
    "track.height_up",
    "track.hide",
    "track.input",
    "track.make_inactive",
    "track.move",
    "track.move_to_folder",
    "track.move_to_new_folder",
    "track.mute_sends",
    "track.mute_sends_ae",
    "track.mute_sends_fj",
    "track.new",
    "track.object",
    "track.output",
    "track.pin",
    "track.playlist_delete_unused",
    "track.playlist_duplicate",
    "track.playlist_new",
    "track.playlist_promote",
    "track.playlist_select",
    "track.remove_from_folder",
    "track.rename",
    "track.show_target_playlist",
    "track.split_into_mono",
    "track.timebase",
    "track.toggle_recent_playlist",
    "track.trim_automation_write",
    "track.ungroup",
    "track.vca",
    "track.view",
    "track.write_midi_rtp",
    // video
    "video.offset",
];

/// Edits one run may make. A run keeps one copy of the session as it started (put back when the
/// run fails) and records one undo step, whatever it does; this bounds the work one run does on
/// the editing thread. A plug-in with more to do asks to be run again.
pub const MAX_RUN_EDITS: u64 = 1000;

/// Parameter names that name a file or folder on disk, at any depth, when their value is text (or
/// a list holding text).
const FILE_KEYS: &[&str] = &["path", "paths", "file", "files", "dir", "folder", "directory", "destination", "location"];

/// Parameter names that pick an audio plug-in or instrument (`mix.insert`, `mix.instrument`,
/// `track.new` rows): a hosted one (`clap:`, `vst3:`, `au:`) would load native code from disk.
const PROCESSOR_KEYS: &[&str] = &["plugin", "instrument"];
/// The id prefixes of hosted (third-party) audio plug-ins.
const HOSTED_PREFIXES: &[&str] = &["clap:", "vst3:", "au:"];

/// The rules every plug-in run follows: [`craft_plugin::ALWAYS_DENIED`], and everything refused
/// but [`ALLOWED`].
pub fn policy() -> &'static Policy {
    static POLICY: OnceLock<Policy> = OnceLock::new();
    POLICY.get_or_init(|| Policy::new(&["*"], ALLOWED))
}

/// Why plug-in `m` may not call `command` with `params`, before it runs: a refused command, one
/// outside its manifest's `permissions.commands`, parameters that name a file, or a hosted audio
/// plug-in.
pub fn refusal(m: &Manifest, command: &str, params: &Value) -> Option<String> {
    if let Err(why) = craft_plugin::check_call(m, policy(), command) {
        return Some(why);
    }
    if names_a_file(params, 0) {
        let keys = FILE_KEYS.iter().map(|k| format!("`{k}`")).collect::<Vec<_>>().join(", ");
        return Some(format!("command plug-ins can't name files (`{command}` was given one of {keys}): use what is already in the session"));
    }
    if names_a_hosted_processor(params, 0) {
        return Some(format!(
            "command plug-ins can't load hosted audio plug-ins (`{command}` was given a `clap:`, `vst3:` or `au:` id): use SoundCraft's built-in processors"
        ));
    }
    None
}

/// Whether `v` holds one of [`FILE_KEYS`] with text (or a list holding text), at any depth.
fn names_a_file(v: &Value, depth: usize) -> bool {
    let text = |v: &Value| v.is_string() || v.as_array().is_some_and(|a| a.iter().any(Value::is_string));
    // serde_json refuses nesting past 128, so nothing deeper reaches here
    depth < 128
        && match v {
            Value::Object(m) => m.iter().any(|(k, v)| (FILE_KEYS.contains(&k.as_str()) && text(v)) || names_a_file(v, depth + 1)),
            Value::Array(a) => a.iter().any(|v| names_a_file(v, depth + 1)),
            _ => false,
        }
}

/// Whether `v` names a hosted audio plug-in under one of [`PROCESSOR_KEYS`], at any depth.
fn names_a_hosted_processor(v: &Value, depth: usize) -> bool {
    let hosted = |v: &Value| v.as_str().is_some_and(|id| HOSTED_PREFIXES.iter().any(|p| id.trim_start().to_ascii_lowercase().starts_with(p)));
    depth < 128
        && match v {
            Value::Object(m) => m.iter().any(|(k, v)| (PROCESSOR_KEYS.contains(&k.as_str()) && hosted(v)) || names_a_hosted_processor(v, depth + 1)),
            Value::Array(a) => a.iter().any(|v| names_a_hosted_processor(v, depth + 1)),
            _ => false,
        }
}

/// A plug-in error as the engine reports it.
fn failed(name: &str, e: impl std::fmt::Display) -> EngineError {
    EngineError::Plugin(format!("{name}: {e}"))
}

/// Store (`Some`) or forget (`None`) plug-in `id`'s setting `key` in `all`, within
/// [`MAX_SETTINGS_BYTES`], [`MAX_SETTING_KEY`], [`MAX_SETTINGS_PLUGINS`] and
/// [`MAX_ALL_SETTINGS_BYTES`].
pub fn set_setting(
    all: &mut std::collections::BTreeMap<String, Map<String, Value>>,
    id: &str,
    key: &str,
    value: Option<&str>,
) -> std::result::Result<(), String> {
    let Some(text) = value else {
        if let Some(mine) = all.get_mut(id) {
            mine.remove(key);
            if mine.is_empty() {
                all.remove(id);
            }
        }
        return Ok(());
    };
    if !runtime::valid_setting_key(key) {
        return Err(format!("a setting's key must be 1–{MAX_SETTING_KEY} characters without control characters"));
    }
    let too_big = || format!("a plug-in's settings are limited to {} KB", MAX_SETTINGS_BYTES / 1024);
    if key.len().saturating_add(text.len()) > MAX_SETTINGS_BYTES {
        return Err(too_big());
    }
    let v: Value = serde_json::from_str(text).map_err(|e| format!("a setting must be JSON: {e}"))?;
    let entry = key.len().saturating_add(v.to_string().len());
    let others = all.get(id).map_or(0, |mine| runtime::settings_size(mine.iter().filter(|(k, _)| k.as_str() != key)));
    if others.saturating_add(entry) > MAX_SETTINGS_BYTES {
        return Err(too_big());
    }
    let everyone_else: usize =
        all.iter().filter(|(other, _)| other.as_str() != id).map(|(_, m)| runtime::settings_size(m.iter())).fold(0, usize::saturating_add);
    if everyone_else.saturating_add(others).saturating_add(entry) > MAX_ALL_SETTINGS_BYTES {
        return Err(format!("the plug-ins' settings together are limited to {} KB", MAX_ALL_SETTINGS_BYTES / 1024));
    }
    if !all.contains_key(id) && all.len() >= MAX_SETTINGS_PLUGINS {
        return Err(format!("settings are kept for at most {MAX_SETTINGS_PLUGINS} plug-ins"));
    }
    all.entry(id.to_string()).or_default().insert(key.to_string(), v);
    Ok(())
}

// ---------- running ----------

/// The selection as a plug-in sees it: the selected tracks and clips (ids) and the edit
/// selection's start and end (samples).
fn selection_json(e: &Engine) -> Value {
    let ed = &e.session().edit;
    json!({
        "tracks": ed.selected_tracks.iter().map(|t| t.0).collect::<Vec<_>>(),
        "clips": ed.selected_clips.iter().map(|c| c.0).collect::<Vec<_>>(),
        "start": ed.selection.start,
        "end": ed.selection.end,
    })
}

/// Run plug-in `id`'s `command` (default: its first) with `params` on the open session, as one
/// undo step named after the plug-in. A run that fails (a trap, an exhausted budget, a refusal it
/// passes on, a non-zero status) leaves the session, the selection, the undo history, the
/// clipboard and the transport as they were.
pub fn run(e: &mut Engine, id: &str, command: Option<&str>, params: &Value, fuel: Option<u64>) -> Result<Value> {
    let cmd = "plugin.run";
    if e.transport.recording {
        return Err(EngineError::Disabled(cmd.into(), "stop recording first: command plug-ins don't run while a take is recorded".into()));
    }
    let p = e.command_plugins.get(id).cloned().ok_or_else(|| bad(cmd, format!("no command plug-in `{id}` is installed (see plugin.list)")))?;
    let m = &p.manifest;
    let api = m.target(&APP).map_err(|err| bad(cmd, format!("{} can't run here: {err}", m.name)))?.api();
    let c = m.command(command).ok_or_else(|| bad(cmd, format!("{} has no command `{}`", m.name, command.unwrap_or_default())))?;
    let params = c.resolve_params(params).map_err(|err| bad(cmd, err))?;
    let mut limits = Limits::default();
    if let Some(f) = fuel {
        limits.fuel = f.clamp(MIN_FUEL, limits.fuel);
    }
    let mut instance = Instance::new(&p.wasm, &limits).map_err(|err| failed(&m.name, err))?;
    let input = json!({"api": api, "command": c.id, "params": params, "selection": selection_json(e)}).to_string();
    e.command_plugins.last = Some((m.id.clone(), c.id.clone(), params));
    let label = if m.commands.len() > 1 { format!("{}: {}", m.name, c.label.trim_end_matches(['…', '.'])) } else { m.name.clone() };

    // The calls are recorded as one step below, and are not the person's own journal entries
    // (the journal gets this `plugin.run`). While the run lasts, a call's own undo step never
    // pushes the person's oldest one out of the history (the host drops it after the call).
    let mark = Mark::take(e);
    let journal = std::mem::take(&mut e.journal);
    let limit = e.history_limit;
    e.history_limit = usize::MAX;
    let ran = {
        let mut host = EngineHost::new(e, m);
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| instance.call(&input, &mut host)));
        let (notes, recorded) = host.finish();
        let out: std::result::Result<RunOutput, String> = match ran {
            Ok(Ok(out)) => Ok(out),
            Ok(Err(err)) => Err(err.to_string()),
            Err(_) => Err("internal error (see the log); the run did not complete".into()),
        };
        out.map(|out| (out, notes, recorded))
    };
    e.journal = journal;
    e.history_limit = limit;
    let (out, notes, recorded) = match ran {
        Ok(done) => done,
        Err(why) => {
            mark.restore(e);
            e.set_history_limit(limit);
            return Err(failed(&m.name, why));
        }
    };
    mark.collapse(e, &label, recorded);
    e.set_history_limit(limit);
    for note in &notes {
        e.message(format!("{}: {note}", m.name));
    }
    let output = serde_json::from_str::<Value>(&out.output).unwrap_or(Value::String(out.output));
    // Panels (`ui_show`) aren't shown in SoundCraft yet: say so rather than drop it silently
    let panel = matches!(out.ui, Some(UiChange::Show(_))).then_some("SoundCraft doesn't show command plug-in panels yet");
    Ok(
        json!({"plugin": m.id, "command": c.id, "output": output, "notes": notes, "logs": out.logs, "calls": out.calls, "fuel": out.fuel, "panel": panel}),
    )
}
