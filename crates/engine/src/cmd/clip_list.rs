//! Clip List commands.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::SourceId;
use std::collections::BTreeSet;

const CLEAR: &str = "clip.clear";

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "clip.clear",
        "Clear Clips",
        [],
        Some("Cmd+Shift+B"),
        "{clips?: [id], sources?: [id], with_clips?: bool} — removes clips and audio files from the session (files stay on disk); default: the Clip List selection. A file still used on the timeline needs `with_clips: true`, which removes those clips too",
        has_list_selection,
        clear
    )]
}

fn has_list_selection(e: &Engine) -> std::result::Result<(), String> {
    let ed = &e.session().edit;
    if ed.selected_clips.is_empty() && ed.selected_sources.is_empty() { Err("select clips or files in the Clip List".into()) } else { Ok(()) }
}

fn ids<'a, T: serde::Deserialize<'a>>(p: &'a Value, key: &str) -> Result<Option<T>> {
    p.get(key).map(|v| T::deserialize(v).map_err(|_| bad(CLEAR, format!("`{key}` must be an array of ids")))).transpose()
}

fn clear(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session();
    let (clips, sources): (BTreeSet<ClipId>, BTreeSet<SourceId>) = match (ids(p, "clips")?, ids(p, "sources")?) {
        (None, None) => (s.edit.selected_clips.iter().copied().collect(), s.edit.selected_sources.iter().copied().collect()),
        (c, f) => (c.unwrap_or_default(), f.unwrap_or_default()),
    };
    let known: BTreeSet<ClipId> = s.all_clips().map(|c| c.id).collect();
    if let Some(c) = clips.iter().find(|c| !known.contains(c)) {
        return Err(bad(CLEAR, format!("no clip with id {c}")));
    }
    let files = sources.iter().map(|f| s.source(*f).ok_or_else(|| bad(CLEAR, format!("no audio file with id {f}")))).collect::<Result<Vec<_>>>()?;
    if clips.is_empty() && files.is_empty() {
        return Err(bad(CLEAR, "nothing to clear: pass `clips` or `sources`, or select them in the Clip List"));
    }
    if !bool_or(p, "with_clips", false)
        && let Some((f, n)) = files.iter().map(|f| (f, s.clips_using(f.id).filter(|c| !clips.contains(&c.id)).count())).find(|(_, n)| *n > 0)
    {
        let plural = if n == 1 { "" } else { "s" };
        return Err(bad(CLEAR, format!("`{}` is used by {n} clip{plural} on the timeline; pass `with_clips: true` to remove them too", f.name)));
    }
    let gone: BTreeSet<ClipId> = clips.iter().copied().chain(sources.iter().flat_map(|f| s.clips_using(*f).map(|c| c.id))).collect();
    let s = e.session_mut();
    for pl in s.tracks.iter_mut().flat_map(|t| t.playlists.iter_mut()) {
        pl.clips.retain(|c| !gone.contains(&c.id));
    }
    s.sources.retain(|f| !sources.contains(&f.id));
    for f in &sources {
        s.pool.remove(*f);
    }
    s.edit.selected_clips.retain(|c| !gone.contains(c));
    s.edit.selected_sources.retain(|f| !sources.contains(f));
    Ok(json!({"clips": gone.len(), "sources": sources.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_audio_io::{AudioBuffer, FileFormat};

    fn kick(e: &Engine) -> (SourceId, Vec<ClipId>) {
        let s = e.session();
        let f = s.sources.iter().find(|x| x.name == "Kick").map(|x| x.id).unwrap();
        (f, s.clips_using(f).map(|c| c.id).collect())
    }

    #[test]
    fn clearing_a_clip_takes_it_off_the_timeline_and_keeps_its_file() {
        let mut e = crate::demo::demo_engine();
        let (f, clips) = kick(&e);
        let r = e.execute("clip.clear", &json!({"clips": [clips[0]]})).unwrap();
        assert_eq!(r, json!({"clips": 1, "sources": 0}));
        assert!(e.session().find_clip(clips[0]).is_none());
        assert!(e.session().source(f).is_some());
        assert!(e.undo());
        assert!(e.session().find_clip(clips[0]).is_some());
    }

    #[test]
    fn a_file_in_use_stays_unless_its_clips_go_too() {
        let mut e = crate::demo::demo_engine();
        e.execute("track.playlist_duplicate", &json!({"track": "Kick"})).unwrap();
        let (f, clips) = kick(&e);
        assert_eq!(clips.len(), 6);
        let err = e.execute("clip.clear", &json!({"sources": [f]})).unwrap_err();
        assert!(err.to_string().contains("used by 6 clips"), "{err}");
        let r = e.execute("clip.clear", &json!({"sources": [f], "with_clips": true})).unwrap();
        assert_eq!(r, json!({"clips": 6, "sources": 1}));
        let s = e.session();
        assert!(s.source(f).is_none() && !s.pool.contains(f));
        assert_eq!(s.clips_using(f).count(), 0);
        assert_eq!(e.undo_label(), Some("Clear Clips"));
        assert!(e.undo());
        assert!(e.session().pool.contains(f));
        assert_eq!(e.session().clips_using(f).count(), 6);
    }

    #[test]
    fn listing_a_files_clips_too_is_enough() {
        let mut e = crate::demo::demo_engine();
        let (f, clips) = kick(&e);
        let r = e.execute("clip.clear", &json!({"clips": clips, "sources": [f]})).unwrap();
        assert_eq!(r, json!({"clips": 3, "sources": 1}));
    }

    #[test]
    fn the_clip_list_selection_is_the_default_target() {
        let mut e = crate::demo::demo_engine();
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 4_800]] };
        let f = crate::io::add_source(e.session_mut(), "Spare", buf, None, FileFormat::Wav);
        let (_, clips) = kick(&e);
        e.execute("edit.select", &json!({"clips": [clips[0]]})).unwrap();
        let r = e.execute("edit.select", &json!({"sources": [f, f, 999_999]})).unwrap();
        assert_eq!(r["sources"], json!([f]));
        assert_eq!(r["clips"], json!([]));
        let r = e.execute("edit.select", &json!({"clips": [clips[0]]})).unwrap();
        assert_eq!(r["sources"], json!([]));
        e.execute("clip.clear", &json!({})).unwrap();
        assert!(e.session().find_clip(clips[0]).is_none());
        assert!(e.session().edit.selected_clips.is_empty());
        e.execute("edit.select", &json!({"sources": [f]})).unwrap();
        e.execute("clip.clear", &json!({})).unwrap();
        assert!(e.session().source(f).is_none());
        assert!(e.session().edit.selected_sources.is_empty());
    }

    #[test]
    fn bad_targets_are_errors() {
        let mut e = crate::demo::demo_engine();
        for p in [
            json!({}),
            json!({"clips": []}),
            json!({"clips": [999_999]}),
            json!({"sources": [999_999]}),
            json!({"sources": "Kick"}),
            json!({"clips": [-1]}),
        ] {
            assert!(e.execute("clip.clear", &p).is_err(), "{p}");
        }
    }
}
