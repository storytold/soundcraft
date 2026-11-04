//! Clip List › Auto Rename: the names that editing gives the clips it separates from a clip.
use super::{CommandSpec, bad};

use crate::{Engine, Result, cmd};
use serde::Deserialize;
use serde_json::{Value, json};
use soundcraft_model::ClipId;
use std::collections::BTreeSet;

const AUTO_RENAME: &str = "clip.auto_rename";

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "clip.auto_rename",
        "Auto Rename...",
        [],
        None,
        "{clips?: [id], name?, number?: 0..999999999, zeros?: 0..9, suffix?}: sets the names that editing gives the clips it separates from these clips (default: the Clip List's selected clips): the piece after each cut is named `name`, then `zeros` zeros, then a number counting up from `number` (default 1), then `suffix`, in one number sequence for all of them. Without a name, the clips keep their names when they are separated",
        has_clip_selection,
        auto_rename
    )]
}

fn has_clip_selection(e: &Engine) -> std::result::Result<(), String> {
    if e.session().edit.selected_clips.is_empty() { Err("select a clip in the Clip List".into()) } else { Ok(()) }
}

fn auto_rename(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session();
    let clips: BTreeSet<ClipId> = match p.get("clips") {
        None => s.edit.selected_clips.iter().copied().collect(),
        Some(v) => BTreeSet::deserialize(v).map_err(|_| bad(AUTO_RENAME, "`clips` must be an array of ids"))?,
    };
    if clips.is_empty() {
        return Err(bad(AUTO_RENAME, "select a clip in the Clip List, or pass `clips`"));
    }
    let known: BTreeSet<ClipId> = s.all_clips().map(|c| c.id).collect();
    if let Some(c) = clips.iter().find(|c| !known.contains(c)) {
        return Err(bad(AUTO_RENAME, format!("no clip with id {c}")));
    }
    let names = super::edit::strip_names(AUTO_RENAME, p)?;
    let s = e.session_mut();
    let key = names.map(|n| {
        let (key, id) = (s.alloc(), crate::new_scheme_id(s));
        s.auto_names.insert(key, soundcraft_model::AutoName { id, ..n.into() });
        key
    });
    for c in s.tracks.iter_mut().flat_map(|t| t.playlists.iter_mut()).flat_map(|pl| pl.clips.iter_mut()) {
        if clips.contains(&c.id) {
            c.auto_name = key;
        }
    }
    // Earlier schemes stay while the session is open: copies on the clipboard can still use them.
    Ok(json!({"clips": clips.len(), "named": key.is_some()}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first Kick clip: its track, id, start and length.
    fn first_kick(e: &Engine) -> (soundcraft_model::TrackId, ClipId, i64, i64) {
        let s = e.session();
        let t = s.track_by_name("Kick").unwrap();
        let c = &t.clips()[0];
        (t.id, c.id, c.start, c.length)
    }

    /// The names of the clips on `t` that lie inside `[start, end)`, in time order.
    fn names_in(e: &Engine, t: soundcraft_model::TrackId, start: i64, end: i64) -> Vec<String> {
        e.session().track(t).unwrap().clips().iter().filter(|c| c.start >= start && c.end() <= end).map(|c| c.name.clone()).collect()
    }

    #[test]
    fn separating_an_auto_renamed_clip_names_each_new_piece() {
        let mut e = crate::demo::demo_engine();
        let (t, id, start, len) = first_kick(&e);
        let old = e.session().find_clip(id).unwrap().1.name.clone();
        let r = e.execute("clip.auto_rename", &json!({"clips": [id], "name": "SFX", "number": 23, "zeros": 1, "suffix": ".Reel1"})).unwrap();
        assert_eq!(r, json!({"clips": 1, "named": true}));
        assert_eq!(e.undo_label(), Some("Auto Rename..."));
        assert_eq!(names_in(&e, t, start, start + len), [old.as_str()], "setting the scheme leaves the clip's name");
        // Cut the middle, then each half: every piece after a cut is a new clip with the next name.
        for at in [start + len / 2, start + len / 4, start + len * 3 / 4] {
            e.execute("edit.separate", &json!({"tracks": [t], "at": at})).unwrap();
        }
        assert_eq!(names_in(&e, t, start, start + len), [old.as_str(), "SFX024.Reel1", "SFX023.Reel1", "SFX025.Reel1"]);
        assert!(e.undo());
        assert_eq!(names_in(&e, t, start, start + len).len(), 3);
        assert_eq!(e.session().auto_names.values().next().map(|a| a.number), Some(25), "undo takes the number back");
    }

    #[test]
    fn auto_rename_uses_the_clip_list_selection_and_one_number_sequence() {
        let mut e = crate::demo::demo_engine();
        let s = e.session();
        let t = s.track_by_name("Kick").unwrap();
        let (track, a, b) = (t.id, t.clips()[0].clone(), t.clips()[1].clone());
        e.execute("edit.select", &json!({"clips": [a.id, b.id]})).unwrap();
        e.execute("clip.auto_rename", &json!({"name": "Hit"})).unwrap();
        e.execute("edit.separate", &json!({"tracks": [track], "at": b.start + b.length / 2})).unwrap();
        e.execute("edit.separate", &json!({"tracks": [track], "at": a.start + a.length / 2})).unwrap();
        assert_eq!(names_in(&e, track, b.start, b.end())[1], "Hit1");
        assert_eq!(names_in(&e, track, a.start, a.end())[1], "Hit2", "both clips count up one sequence");
        assert_eq!(e.session().auto_names.len(), 1);
    }

    #[test]
    fn auto_rename_without_a_name_removes_the_scheme() {
        let mut e = crate::demo::demo_engine();
        let (t, id, start, len) = first_kick(&e);
        let old = e.session().find_clip(id).unwrap().1.name.clone();
        e.execute("clip.auto_rename", &json!({"clips": [id], "name": "SFX"})).unwrap();
        let r = e.execute("clip.auto_rename", &json!({"clips": [id], "name": "  "})).unwrap();
        assert_eq!(r["named"], false);
        assert_eq!(e.session().find_clip(id).unwrap().1.auto_name, None);
        e.execute("edit.separate", &json!({"tracks": [t], "at": start + len / 2})).unwrap();
        assert_eq!(names_in(&e, t, start, start + len), [old.clone(), old]);
    }

    #[test]
    fn a_pasted_copy_keeps_the_scheme_it_was_copied_with() {
        let mut e = crate::demo::demo_engine();
        let (t, id, start, len) = first_kick(&e);
        let old = e.session().find_clip(id).unwrap().1.name.clone();
        e.execute("clip.auto_rename", &json!({"clips": [id], "name": "SFX"})).unwrap();
        e.execute("edit.copy", &json!({"tracks": [t], "start": start, "end": start + len})).unwrap();
        e.execute("clip.auto_rename", &json!({"clips": [id]})).unwrap();
        let at = e.session().content_end() + 48_000;
        e.execute("edit.paste", &json!({"tracks": [t], "at": at})).unwrap();
        e.execute("edit.separate", &json!({"tracks": [t], "at": at + len / 2})).unwrap();
        assert_eq!(names_in(&e, t, at, at + len)[1], "SFX1", "the copy's scheme outlives the original clip's");
        // Pasted into another session, the copy brings its scheme along and counts on from it.
        e.execute("edit.copy", &json!({"tracks": [t], "start": at, "end": at + len})).unwrap();
        e.replace_session(crate::demo::demo_engine().session().clone());
        let t = first_kick(&e).0;
        e.execute("edit.paste", &json!({"tracks": [t], "at": at})).unwrap();
        e.execute("edit.separate", &json!({"tracks": [t], "at": at + len / 4})).unwrap();
        assert_eq!(names_in(&e, t, at, at + len), [old.as_str(), "SFX2", "SFX1"]);
    }

    #[test]
    fn a_copy_keeps_its_scheme_when_undo_hands_the_key_to_another() {
        let mut e = crate::demo::demo_engine();
        let (t, id, start, len) = first_kick(&e);
        let bass = e.session().track_by_name("Bass").unwrap().clips()[0].id;
        e.execute("clip.auto_rename", &json!({"clips": [id], "name": "SFX"})).unwrap();
        e.execute("edit.copy", &json!({"tracks": [t], "start": start, "end": start + len})).unwrap();
        assert!(e.undo());
        // The same name under the key the undo freed is still another scheme.
        e.execute("clip.auto_rename", &json!({"clips": [bass], "name": "SFX", "number": 100})).unwrap();
        let at = e.session().content_end() + 48_000;
        e.execute("edit.paste", &json!({"tracks": [t], "at": at})).unwrap();
        e.execute("edit.separate", &json!({"tracks": [t], "at": at + len / 2})).unwrap();
        assert_eq!(names_in(&e, t, at, at + len)[1], "SFX1");
        let s = e.session();
        assert_eq!(s.auto_names.len(), 2);
        let key = s.find_clip(bass).unwrap().1.auto_name.unwrap();
        assert_eq!(s.auto_names[&key].number, 100, "the other scheme's count is untouched");
    }

    #[test]
    fn a_copy_keeps_its_scheme_when_another_session_holds_its_id_with_other_text() {
        let mut e = crate::demo::demo_engine();
        let (t, id, start, len) = first_kick(&e);
        e.execute("clip.auto_rename", &json!({"clips": [id], "name": "SFX"})).unwrap();
        let copied = e.session().auto_names.values().next().unwrap().id;
        e.execute("edit.copy", &json!({"tracks": [t], "start": start, "end": start + len})).unwrap();
        // Another session holds a scheme under the same id with another name.
        e.replace_session(crate::demo::demo_engine().session().clone());
        let bass = e.session().track_by_name("Bass").unwrap().clips()[0].id;
        let s = e.session_mut();
        let key = s.alloc();
        s.auto_names.insert(key, soundcraft_model::AutoName { name: "Other".into(), number: 100, zeros: 0, suffix: String::new(), id: copied });
        s.tracks
            .iter_mut()
            .flat_map(|t| t.playlists.iter_mut())
            .flat_map(|pl| pl.clips.iter_mut())
            .filter(|c| c.id == bass)
            .for_each(|c| c.auto_name = Some(key));
        let t = first_kick(&e).0;
        let at = e.session().content_end() + 48_000;
        e.execute("edit.paste", &json!({"tracks": [t], "at": at})).unwrap();
        e.execute("edit.separate", &json!({"tracks": [t], "at": at + len / 2})).unwrap();
        assert_eq!(names_in(&e, t, at, at + len)[1], "SFX1");
        let s = e.session();
        assert_eq!(s.auto_names.len(), 2);
        assert_eq!(s.auto_names[&key].number, 100, "the other scheme's count is untouched");
    }

    #[test]
    fn a_refused_paste_leaves_the_clipboard_as_it_was() {
        let mut e = crate::demo::demo_engine();
        let (t, id, start, len) = first_kick(&e);
        e.execute("clip.auto_rename", &json!({"clips": [id], "name": "SFX"})).unwrap();
        e.execute("edit.copy", &json!({"tracks": [t], "start": start, "end": start + len})).unwrap();
        // Without its scheme in the session, a paste puts the scheme back under a new key.
        e.session_mut().auto_names.clear();
        let copied = e.clipboard.clone();
        // A time-locked clip at the paste point leaves Shuffle no room.
        e.session_mut().find_clip_mut(id).unwrap().time_locked = true;
        e.session_mut().edit.edit_mode = soundcraft_model::EditMode::Shuffle;
        assert!(e.execute("edit.paste", &json!({"tracks": [t], "at": start})).is_err());
        assert_eq!(e.clipboard, copied);
        e.session_mut().edit.edit_mode = soundcraft_model::EditMode::Slip;
        let at = e.session().content_end() + 48_000;
        e.execute("edit.paste", &json!({"tracks": [t], "at": at})).unwrap();
        assert_ne!(e.clipboard, copied, "a paste that succeeds keeps the scheme's new key");
    }

    #[test]
    fn bad_auto_rename_params_are_refused() {
        let mut e = crate::demo::demo_engine();
        let (_, id, _, _) = first_kick(&e);
        for p in [
            json!({"name": "SFX"}),
            json!({"clips": [], "name": "SFX"}),
            json!({"clips": [999_999], "name": "SFX"}),
            json!({"clips": "Kick", "name": "SFX"}),
            json!({"clips": [id], "name": 5}),
            json!({"clips": [id], "name": "SFX", "zeros": 10}),
            json!({"clips": [id], "name": "SFX", "number": 1_000_000_000}),
            json!({"clips": [id], "number": 3}),
        ] {
            assert!(e.execute("clip.auto_rename", &p).is_err(), "{p}");
        }
        assert!(e.session().auto_names.is_empty());
        assert!(e.session().find_clip(id).unwrap().1.auto_name.is_none());
    }

    #[test]
    fn auto_rename_schemes_are_saved_and_repaired_on_open() {
        let mut e = crate::demo::demo_engine();
        let (t, id, start, len) = first_kick(&e);
        e.execute("clip.auto_rename", &json!({"clips": [id], "name": "SFX", "number": 7})).unwrap();
        let mut v: Value = serde_json::from_str(&e.session().to_json().unwrap()).unwrap();
        let key = *e.session().auto_names.keys().next().unwrap();
        v["auto_names"][key.to_string()]["zeros"] = json!(usize::MAX);
        v["auto_names"]["999999999"] = json!({"name": "Unused", "number": 1, "zeros": 0, "suffix": ""});
        let mut s = soundcraft_model::Session::from_json(&v.to_string()).unwrap();
        assert_eq!(s.auto_names.len(), 1, "a scheme no clip uses is dropped");
        assert_eq!(s.auto_names[&key].zeros, soundcraft_model::AutoName::MAX_ZEROS);
        assert!(s.next_id > key);
        crate::edit::separate_at(&mut s, t, start + len / 2);
        let names: Vec<String> =
            s.track(t).unwrap().clips().iter().filter(|c| c.start >= start && c.end() <= start + len).map(|c| c.name.clone()).collect();
        assert_eq!(names[1], "SFX0000000007");
    }
}
