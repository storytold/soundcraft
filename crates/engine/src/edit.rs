//! Editing algorithms on playlists, independent of the command layer.
//!
//! Model rules: clips on a playlist are sorted and do not overlap, except where two clips
//! overlap exactly over a crossfade (the earlier clip fades out while the later fades in).
//! Placing a clip overwrites whatever is under it (like the incumbent's Slip mode); Shuffle
//! mode also ripples later clips.

use soundcraft_model::{AutoParam, AutomationPoint, Clip, ClipContent, ClipId, Fade, FadeShape, Session, Track, TrackId};
use soundcraft_time::{Range, SampleRate, Samples, TempoMap};
use std::collections::{HashMap, HashSet};

/// Edit-state flag: the Edit cursor and selections snap to the grid in Shuffle, Slip and Spot.
pub const SNAP_TO_GRID: &str = "edit.snap_to_grid";
/// Edit-state flag: Shuffle mode can't be chosen.
pub const SHUFFLE_LOCK: &str = "edit.shuffle_lock";
/// Edit-state flag: the Grid mode last used was Relative, so choosing Grid again picks it.
pub const RELATIVE_GRID: &str = "edit.relative_grid";

/// Context needed to split MIDI clips in musical time.
#[derive(Clone, Copy)]
pub struct TimeCtx<'a> {
    pub tempo: &'a TempoMap,
    pub sr: SampleRate,
}

impl<'a> TimeCtx<'a> {
    pub fn of(s: &'a Session) -> Self {
        TimeCtx { tempo: &s.tempo, sr: s.sample_rate }
    }
    fn ticks(&self, s: Samples) -> i64 {
        self.tempo.samples_to_ticks(s, self.sr)
    }
}

/// Split `clip` at absolute position `at` (strictly inside). Returns (left, right) where right gets `new_id`.
pub fn split_clip(clip: &Clip, at: Samples, new_id: ClipId, t: TimeCtx<'_>) -> Option<(Clip, Clip)> {
    if at <= clip.start || at >= clip.end() {
        return None;
    }
    let mut left = clip.clone();
    let mut right = clip.clone();
    right.id = new_id;
    left.trim_end_to(at);
    right.trim_start_to(at);
    left.fade_out = Fade::default();
    right.fade_in = Fade::default();
    if let (ClipContent::Midi { sequence: ls }, ClipContent::Midi { sequence: rs }) = (&mut left.content, &mut right.content) {
        let boundary = t.ticks(at) - t.ticks(clip.start);
        ls.notes.retain(|n| n.start < boundary);
        ls.ctrls.retain(|c| c.tick < boundary);
        rs.notes.retain(|n| n.start >= boundary);
        rs.ctrls.retain(|c| c.tick >= boundary);
        for n in &mut rs.notes {
            n.start -= boundary;
        }
        for c in &mut rs.ctrls {
            c.tick -= boundary;
        }
    }
    Some((left, right))
}

/// Trim a MIDI clip's start: drop notes before the new start and re-anchor the rest.
fn rebase_midi_start(clip: &mut Clip, old_start: Samples, t: TimeCtx<'_>) {
    let delta = t.ticks(clip.start) - t.ticks(old_start);
    if let ClipContent::Midi { sequence } = &mut clip.content {
        if delta == 0 {
            return;
        }
        sequence.notes.retain(|n| n.start >= delta);
        sequence.ctrls.retain(|c| c.tick >= delta);
        for n in &mut sequence.notes {
            n.start -= delta;
        }
        for c in &mut sequence.ctrls {
            c.tick -= delta;
        }
    }
}

/// Split every clip on a track's active playlist that spans `at`. The piece before `at` keeps the
/// clip's name. The piece after it is a new clip, and takes the next name of the clip's Auto Rename
/// scheme when it has one.
pub fn separate_at(s: &mut Session, track: TrackId, at: Samples) -> usize {
    separate_at_if(s, track, at, |_| true)
}

/// [`separate_at`], giving the piece after `at` the next Auto Rename name only when `auto_name`
/// returns true for it. Any other piece keeps the clip's name, and its scheme does not count up.
pub fn separate_at_if(s: &mut Session, track: TrackId, at: Samples, auto_name: impl Fn(&Clip) -> bool) -> usize {
    let tc_tempo = s.tempo.clone();
    let sr = s.sample_rate;
    let t = TimeCtx { tempo: &tc_tempo, sr };
    let mut ids = Vec::new();
    let n_split = s.track(track).map_or(0, |tr| tr.clips().iter().filter(|c| at > c.start && at < c.end()).count());
    for _ in 0..n_split {
        ids.push(s.new_clip_id());
    }
    let mut schemes = std::mem::take(&mut s.auto_names);
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else {
        s.auto_names = schemes;
        return 0;
    };
    let mut out = Vec::with_capacity(pl.clips.len() + ids.len());
    let mut count = 0;
    for c in pl.clips.drain(..) {
        if at > c.start
            && at < c.end()
            && let Some(id) = ids.pop()
            && let Some((l, mut r)) = split_clip(&c, at, id, t)
        {
            if auto_name(&r) {
                take_auto_name(&mut schemes, &mut r);
            }
            out.push(l);
            out.push(r);
            count += 1;
            continue;
        }
        out.push(c);
    }
    pl.clips = out;
    pl.sort();
    s.auto_names = schemes;
    count
}

/// Gives `c` the next name of its Auto Rename scheme, when it has one, and counts the scheme up.
fn take_auto_name(schemes: &mut std::collections::BTreeMap<u64, soundcraft_model::AutoName>, c: &mut Clip) {
    if let Some(scheme) = c.auto_name.and_then(|k| schemes.get_mut(&k)) {
        c.name = crate::io::StripNames::from(&*scheme).clip_name(0);
        scheme.number = scheme.number.saturating_add(1);
    }
}

/// Gives each clip on a track's active playlist whose id is not in `old`, and for which `auto_name`
/// returns true, the next name of its Auto Rename scheme, in time order. Pieces cut with
/// [`separate_at_if`] and `|_| false` take their names here once the caller has removed the ones it
/// does not keep.
pub fn auto_name_new_clips(s: &mut Session, track: TrackId, old: &HashSet<ClipId>, auto_name: impl Fn(&Clip) -> bool) {
    let mut schemes = std::mem::take(&mut s.auto_names);
    if let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) {
        let mut new: Vec<&mut Clip> = pl.clips.iter_mut().filter(|c| !old.contains(&c.id) && auto_name(c)).collect();
        new.sort_by_key(|c| (c.start, c.id.0));
        for c in new {
            take_auto_name(&mut schemes, c);
        }
    }
    s.auto_names = schemes;
}

/// Remove material inside `range` on a track. In Shuffle mode later clips move left, up to the
/// first locked clip (see [`ripple`]). Returns the removed material as clips positioned relative
/// to `range.start`.
pub fn clear_range(s: &mut Session, track: TrackId, range: Range, shuffle: bool) -> Vec<Clip> {
    if range.is_empty() {
        return Vec::new();
    }
    separate_at(s, track, range.start);
    separate_at(s, track, range.end);
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return Vec::new() };
    let mut removed = Vec::new();
    pl.clips.retain(|c| {
        if c.start >= range.start && c.end() <= range.end {
            let mut r = c.clone();
            r.start -= range.start;
            removed.push(r);
            false
        } else {
            true
        }
    });
    if shuffle {
        let moved = ripple(s, track, range.end, -range.len());
        follow_automation(s, track, range.end, moved);
    }
    removed
}

/// Copy the material inside `range` (clips trimmed to the range), relative to `range.start`.
pub fn copy_range(s: &Session, track: TrackId, range: Range) -> Vec<Clip> {
    let t = TimeCtx::of(s);
    let Some(tr) = s.track(track) else { return Vec::new() };
    let mut out = Vec::new();
    for c in tr.clips() {
        let Some(isect) = c.range().intersect(&range) else { continue };
        let mut part = c.clone();
        if isect.start > part.start {
            let old = part.start;
            part.trim_start_to(isect.start);
            rebase_midi_start(&mut part, old, t);
            part.fade_in = Fade::default();
        }
        if isect.end < part.end() {
            part.trim_end_to(isect.end);
            part.fade_out = Fade::default();
        }
        part.start -= range.start;
        out.push(part);
    }
    out
}

/// Merge the MIDI inside `range` into one clip covering the whole range, gaps included, as it
/// sounds (muted clips stay silent). Returns false for tracks that aren't MIDI or instrument tracks.
pub fn consolidate_midi(s: &mut Session, track: TrackId, range: Range) -> bool {
    let Some(name) = s.track(track).filter(|t| t.kind.is_midi()).map(|t| t.name.clone()) else { return false };
    let mut merged = soundcraft_midi::Sequence::default();
    {
        let t = TimeCtx::of(s);
        let base = t.ticks(range.start);
        for part in copy_range(s, track, range).into_iter().filter(|c| !c.muted) {
            let at = t.ticks(range.start.saturating_add(part.start)).saturating_sub(base);
            let len = t.ticks(range.start.saturating_add(part.end())).saturating_sub(base).saturating_sub(at);
            let ClipContent::Midi { sequence } = part.content else { continue };
            for mut n in sequence.notes.into_iter().filter(|n| (0..len).contains(&n.start)) {
                n.length = n.length.min(len - n.start);
                n.start += at;
                merged.notes.push(n);
            }
            for mut c in sequence.ctrls.into_iter().filter(|c| (0..len).contains(&c.tick)) {
                c.tick += at;
                merged.ctrls.push(c);
            }
        }
    }
    merged.sort();
    let id = s.new_clip_id();
    place_clip(s, track, Clip::midi(id, name, range.start, range.len(), merged));
    true
}

/// Insert `clips` (positions relative to 0) at `at`. Overwrites what's underneath unless
/// `shuffle`, which first makes room for `length` (see [`make_room`], whose refusal it returns).
pub fn paste(s: &mut Session, track: TrackId, at: Samples, clips: &[Clip], length: Samples, shuffle: bool) -> Result<Vec<ClipId>, &'static str> {
    let length = length.max(clips.iter().map(Clip::end).max().unwrap_or(0));
    if shuffle {
        make_room(s, track, at, length)?;
    }
    let mut ids = Vec::new();
    for c in clips {
        let mut nc = c.clone();
        nc.id = s.new_clip_id();
        nc.start += at;
        ids.push(nc.id);
        place_clip(s, track, nc);
    }
    Ok(ids)
}

/// Place one clip, overwriting whatever it covers.
pub fn place_clip(s: &mut Session, track: TrackId, clip: Clip) {
    clear_range(s, track, clip.range(), false);
    if let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) {
        pl.clips.push(clip);
        pl.sort();
    }
}

/// Insert silence: shift material at/after `range.start` right by the range length (Insert Time;
/// Shuffle mode edits use [`make_room`]).
pub fn insert_silence(s: &mut Session, track: TrackId, range: Range) {
    if range.is_empty() {
        return;
    }
    separate_at(s, track, range.start);
    shift_from(s, track, range.start, range.len());
}

/// Move every clip that starts at or after `from` by `by`, locked or not.
pub fn shift_from(s: &mut Session, track: TrackId, from: Samples, by: Samples) {
    if let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) {
        for c in pl.clips.iter_mut().filter(|c| c.start >= from) {
            c.start = c.start.saturating_add(by);
        }
    }
}

/// Time- or edit-locked: the clip doesn't move, and Shuffle stops at it.
pub fn locked(c: &Clip) -> bool {
    c.time_locked || c.edit_locked
}

/// A clip's place on its track, for working out Shuffle layouts without touching the clips.
#[derive(Clone, Copy)]
struct Span {
    id: ClipId,
    start: Samples,
    end: Samples,
    locked: bool,
}

fn spans(clips: &[Clip]) -> Vec<Span> {
    clips.iter().map(|c| Span { id: c.id, start: c.start, end: c.end(), locked: locked(c) }).collect()
}

/// Shuffle mode's ripple: the spans from `from` on move by `by`, up to the first locked one, which
/// stays put with everything after it. A move left stops at the session start, keeping their
/// spacing. Returns how far they moved and where the lock is (`Samples::MAX` when none).
fn ripple_spans(spans: &mut [Span], from: Samples, by: Samples) -> (Samples, Samples) {
    let lock = spans.iter().filter(|c| c.locked && c.start >= from).map(|c| c.start).min().unwrap_or(Samples::MAX);
    let moves = |c: &Span| c.start >= from && c.start < lock;
    let floor = spans.iter().filter(|c| moves(c)).map(|c| c.start).min().unwrap_or(0).max(0);
    let by = by.max(-floor);
    for c in spans.iter_mut().filter(|c| moves(c)) {
        c.start = c.start.saturating_add(by);
        c.end = c.end.saturating_add(by);
    }
    (by, lock)
}

/// [`ripple_spans`] on a track's clips.
fn ripple(s: &mut Session, track: TrackId, from: Samples, by: Samples) -> (Samples, Samples) {
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return (0, Samples::MAX) };
    let mut sp = spans(&pl.clips);
    let moved = ripple_spans(&mut sp, from, by);
    for (c, n) in pl.clips.iter_mut().zip(&sp) {
        c.start = n.start;
    }
    pl.sort();
    moved
}

/// Automation Follows Edit for a Shuffle ripple (`moved` from [`ripple`]): the points from `from`
/// up to the lock move with the clips, and when material closes up the points in the span that
/// goes away go too.
fn follow_automation(s: &mut Session, track: TrackId, from: Samples, (by, lock): (Samples, Samples)) {
    if !s.edit.automation_follows_edit || by == 0 {
        return;
    }
    let Some(t) = s.track_mut(track) else { return };
    for l in &mut t.automation {
        if by < 0 {
            l.clear_range(from.saturating_add(by), from);
        }
        for p in l.points.iter_mut().filter(|p| p.at >= from && p.at < lock) {
            p.at = p.at.saturating_add(by);
        }
        l.points.sort_by_key(|p| p.at);
        l.points.dedup_by_key(|p| p.at);
    }
}

/// With Automation Follows Edit, take the points in `[start, end)` off a track, timed from `origin`.
fn take_automation(s: &mut Session, track: TrackId, start: Samples, end: Samples, origin: Samples) -> Vec<(AutoParam, AutomationPoint)> {
    let mut out = Vec::new();
    if !s.edit.automation_follows_edit || end <= start {
        return out;
    }
    let Some(t) = s.track_mut(track) else { return out };
    for l in &mut t.automation {
        let taken = l.points.iter().filter(|p| p.at >= start && p.at < end);
        out.extend(taken.map(|p| (l.param.clone(), AutomationPoint { at: p.at.saturating_sub(origin), value: p.value })));
        l.clear_range(start, end);
    }
    out
}

fn put_automation(s: &mut Session, track: TrackId, origin: Samples, points: &[(AutoParam, AutomationPoint)]) {
    let Some(t) = s.track_mut(track) else { return };
    for (param, p) in points {
        t.lane_mut(param).set_point(origin.saturating_add(p.at), p.value);
    }
}

/// How far Shuffle mode can push the material from `at` on before it runs into a locked clip
/// (`Samples::MAX` when none follows).
pub fn shuffle_room(s: &Session, track: TrackId, at: Samples) -> Samples {
    let clips = s.track(track).map(|t| t.clips()).unwrap_or_default();
    let Some(lock) = clips.iter().filter(|c| locked(c) && c.end() > at).map(|c| c.start).min() else { return Samples::MAX };
    let end = clips.iter().filter(|c| c.end() > at && c.start < lock).map(Clip::end).fold(at, Samples::max);
    lock.saturating_sub(end)
}

/// Why Shuffle can't make room: a locked clip follows too closely.
pub const NO_ROOM: &str = "not enough room before a locked clip";
/// Why Shuffle can't make room: the point is inside a crossfade, which can't be pulled apart.
pub const IN_CROSSFADE: &str = "can't make room inside a crossfade";

/// Shuffle mode's room for `len` at `at`: later clips (and with Automation Follows Edit their
/// automation) move right. Refused, with nothing changed, when a locked clip is in the way or
/// `at` is inside a crossfade.
pub fn make_room(s: &mut Session, track: TrackId, at: Samples, len: Samples) -> Result<(), &'static str> {
    if len <= 0 {
        return Ok(());
    }
    let clips = s.track(track).map(|t| t.clips()).unwrap_or_default();
    if clips.iter().filter(|c| c.start <= at && at < c.end()).count() > 1 {
        return Err(IN_CROSSFADE);
    }
    if shuffle_room(s, track, at) < len {
        return Err(NO_ROOM);
    }
    separate_at(s, track, at);
    let moved = ripple(s, track, at, len);
    follow_automation(s, track, at, moved);
    Ok(())
}

/// Trim clips to keep only material inside `range`.
pub fn trim_to(s: &mut Session, track: TrackId, range: Range) -> usize {
    let tempo = s.tempo.clone();
    let tc = TimeCtx { tempo: &tempo, sr: s.sample_rate };
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    let mut n = 0;
    for c in &mut pl.clips {
        if let Some(i) = c.range().intersect(&range)
            && i != c.range()
        {
            let old = c.start;
            c.trim_start_to(i.start);
            rebase_midi_start(c, old, tc);
            c.trim_end_to(i.end);
            n += 1;
        }
    }
    pl.sort();
    n
}

/// Trim the start of clips under `at` to `at` (Trim Start to Insertion).
pub fn trim_start_to(s: &mut Session, track: TrackId, at: Samples) -> usize {
    let tempo = s.tempo.clone();
    let tc = TimeCtx { tempo: &tempo, sr: s.sample_rate };
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    let mut n = 0;
    for c in &mut pl.clips {
        if c.range().contains(at) && at > c.start {
            let old = c.start;
            c.trim_start_to(at);
            rebase_midi_start(c, old, tc);
            n += 1;
        }
    }
    n
}

pub fn trim_end_to(s: &mut Session, track: TrackId, at: Samples) -> usize {
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    let mut n = 0;
    for c in &mut pl.clips {
        if at > c.start && at < c.end() {
            c.trim_end_to(at);
            n += 1;
        }
    }
    n
}

/// Source samples covered by `length` timeline samples of a clip with the given `stretch`.
fn source_len(length: Samples, stretch: f64) -> Samples {
    if stretch.is_finite() && stretch > 0.0 && (stretch - 1.0).abs() >= 1e-9 { (length as f64 / stretch).round() as Samples } else { length }
}

/// Heal separations inside `range`: merge neighbouring audio clips that continue the same file,
/// either touching (a split) or with an empty gap that matches the gap in the source (a cut).
pub fn heal(s: &mut Session, track: TrackId, range: Range) -> usize {
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    pl.sort();
    let mut out: Vec<Clip> = Vec::with_capacity(pl.clips.len());
    let mut healed = 0;
    for c in pl.clips.drain(..) {
        if let Some(prev) = out.last_mut() {
            let contiguous = c.start >= prev.end() && prev.end() >= range.start && c.start <= range.end;
            let gap = if contiguous { c.start.saturating_sub(prev.end()) } else { 0 };
            let same = match (&prev.content, &c.content) {
                (ClipContent::Audio { source: a, offset: oa }, ClipContent::Audio { source: b, offset: ob }) => {
                    a == b && oa.saturating_add(source_len(prev.length.saturating_add(gap), prev.stretch)) == *ob
                }
                _ => false,
            };
            let same_playback = prev.muted == c.muted && (prev.stretch - c.stretch).abs() < 1e-9 && (prev.gain_db - c.gain_db).abs() < 1e-6;
            if contiguous && same && same_playback {
                if !prev.gain_env.is_empty() || !c.gain_env.is_empty() {
                    join_gain_env(prev, &c);
                }
                prev.length = c.end().saturating_sub(prev.start);
                prev.fade_out = c.fade_out;
                healed += 1;
                continue;
            }
        }
        out.push(c);
    }
    pl.clips = out;
    healed
}

/// Give `l` the gain envelope of `l` followed by `r`, which starts at or after where `l` ends, so
/// every sample keeps its gain. Call it before `l.length` grows to cover `r`.
fn join_gain_env(l: &mut Clip, r: &Clip) {
    let cut = l.length;
    let seam = r.start.saturating_sub(l.start);
    let last = cut.saturating_sub(1);
    let (end_db, start_db) = (l.env_db_at(last), r.env_db_at(0));
    l.gain_env.retain(|&(at, _)| at < cut);
    l.gain_env.extend(r.gain_env.iter().filter(|&&(at, _)| at >= 0).map(|&(at, db)| (at.saturating_add(seam), db)));
    // Pin both sides of the seam where the joined points would interpolate across it.
    for (at, db) in [(seam, start_db), (last, end_db)] {
        if (l.env_db_at(at) - db).abs() > 1e-4 {
            let i = l.gain_env.partition_point(|&(t, _)| t < at);
            l.gain_env.insert(i, (at, db));
        }
    }
}

/// Move clips by `delta`, overwriting what they land on; `dest` sends whole tracks of them
/// elsewhere (see [`destinations`]).
pub fn move_clips(s: &mut Session, ids: &[ClipId], delta: Samples, dest: &[(TrackId, TrackId)]) -> usize {
    let ids: HashSet<ClipId> = ids.iter().copied().collect();
    let mut moving: Vec<(TrackId, Clip)> = Vec::new();
    for t in &mut s.tracks {
        let tid = t.id;
        if let Some(pl) = t.playlist_mut() {
            pl.clips.retain(|c| {
                if ids.contains(&c.id) && !locked(c) {
                    moving.push((tid, c.clone()));
                    false
                } else {
                    true
                }
            });
        }
    }
    let n = moving.len();
    let placed = moving.into_iter().map(|(tid, mut c)| {
        c.start = c.start.saturating_add(delta).max(0);
        (lands(dest, tid), c)
    });
    put_clips(s, placed.collect());
    n
}

/// Place moved clips, overwriting what they cover but not each other, so a crossfaded pair
/// moved together stays a crossfade.
fn put_clips(s: &mut Session, placed: Vec<(TrackId, Clip)>) {
    for (t, c) in &placed {
        clear_range(s, *t, c.range(), false);
    }
    for (t, c) in placed {
        if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
            pl.clips.push(c);
            pl.sort();
        }
    }
}

/// Whether the Edit window shows track `t`: not hidden, and not inside a closed folder at any level.
pub fn shown(s: &Session, t: &Track) -> bool {
    !t.hidden && !folder_collapsed(s, t)
}

/// Hidden because a folder at any level above this track is closed.
fn folder_collapsed(s: &Session, track: &Track) -> bool {
    let mut current = track.folder;
    for _ in 0..64 {
        let Some(folder) = current.and_then(|folder| s.track(folder)) else { return false };
        if !folder.folder_open {
            return true;
        }
        current = folder.folder;
    }
    false
}

/// Where the tracks of clips `ids` go when track `from` goes to `to`: each keeps its distance
/// from it among the tracks the Edit window shows, or among all tracks when a hidden one is
/// involved. None when one would land past either end, or on a track that can't hold its clips.
pub fn destinations(s: &Session, ids: &[ClipId], from: TrackId, to: TrackId) -> Option<Vec<(TrackId, TrackId)>> {
    let ids: HashSet<ClipId> = ids.iter().copied().collect();
    let holds = |t: &Track| t.clips().iter().any(|c| ids.contains(&c.id));
    let visible = s.tracks.iter().filter(|t| t.id == from || t.id == to || holds(t)).all(|t| shown(s, t));
    let rows: Vec<&Track> = s.tracks.iter().filter(|t| !visible || shown(s, t)).collect();
    let row = |id: TrackId| rows.iter().position(|t| t.id == id).and_then(|i| isize::try_from(i).ok());
    let shift = row(to)? - row(from)?;
    s.tracks
        .iter()
        .filter(|t| holds(t))
        .map(|t| {
            let d = rows.get(usize::try_from(row(t.id)? + shift).ok()?)?;
            (d.kind.has_playlist() && (d.kind == t.kind || d.kind.is_midi() && t.kind.is_midi())).then_some((t.id, d.id))
        })
        .collect()
}

/// The track that clips on `t` land on under a [`destinations`] map.
pub fn lands(dest: &[(TrackId, TrackId)], t: TrackId) -> TrackId {
    dest.iter().find(|(from, _)| *from == t).map_or(t, |(_, d)| *d)
}

/// The earliest of clips `ids` that can move (neither time- nor edit-locked).
pub fn first_movable<'a>(s: &'a Session, ids: &[ClipId]) -> Option<&'a Clip> {
    ids.iter().filter_map(|id| s.find_clip(*id)).map(|(_, c)| c).filter(|c| !locked(c)).min_by_key(|c| c.start)
}

/// One Shuffle lift: where the hole the clip leaves starts and how long it is, how far the clips
/// after it actually close up (none past a lock, none before the session start) and where the
/// first locked clip after it is.
#[derive(Clone, Copy)]
struct Lift {
    id: ClipId,
    start: Samples,
    from: Samples,
    shift: Samples,
    by: Samples,
    lock: Samples,
}

/// Lifting the movable `moving` clips off a track's `spans` the Shuffle way, right to left, in
/// one pass: the lifts in that order, and the spans that stay at the starts they close up to.
/// A hole starts where the clips before it end (a crossfade partner keeps its material) and
/// closes up to the next clip, so silence after the lifted clip stays. Clips are ordered by
/// start, then id, so of two stacked clips the later one counts as after the other.
fn lift_plan(mut sp: Vec<Span>, moving: &HashSet<ClipId>) -> (Vec<Lift>, Vec<Span>) {
    sp.sort_by_key(|c| (c.start, c.id));
    let lifted = |c: &Span| !c.locked && moving.contains(&c.id);
    let reach: Vec<Samples> = std::iter::once(Samples::MIN)
        .chain(sp.iter().scan(Samples::MIN, |far, c| {
            *far = (*far).max(c.end);
            Some(*far)
        }))
        .collect();
    let mut lifts = Vec::new();
    let (mut next, mut next_moves, mut lock) = (None, false, Samples::MAX);
    for (i, c) in sp.iter().enumerate().rev() {
        if !lifted(c) {
            (next, next_moves) = (Some(c.start), !c.locked);
            if c.locked {
                lock = c.start;
            }
            continue;
        }
        let from = reach.get(i).map_or(c.start, |far| c.start.max(*far));
        let shift = next.unwrap_or(c.end).min(c.end).saturating_sub(from).max(0);
        let by = if next_moves { -shift.min(next.unwrap_or(0).max(0)) } else { 0 };
        next = next.map(|n| n.saturating_add(by));
        lifts.push(Lift { id: c.id, start: c.start, from, shift, by, lock });
    }
    let (mut by, mut pending) = (0, lifts.iter().rev());
    let mut staying = Vec::with_capacity(sp.len() - lifts.len());
    for c in sp {
        if lifted(&c) {
            by += pending.next().map_or(0, |l| l.by);
        } else if c.locked {
            by = 0;
            staying.push(c);
        } else {
            staying.push(Span { start: c.start.saturating_add(by), end: c.end.saturating_add(by), ..c });
        }
    }
    (lifts, staying)
}

/// Where Shuffle can land material on `track` once the movable `moving` clips have left it (later
/// clips close each gap): the clip boundaries, never inside a clip or a crossfade, sorted. Empty
/// for an empty track.
pub fn shuffle_slots(s: &Session, track: TrackId, moving: &[ClipId]) -> Vec<Samples> {
    let clips = s.track(track).map(|t| t.clips()).unwrap_or_default();
    let (_, mut sp) = lift_plan(spans(clips), &moving.iter().copied().collect());
    sp.sort_by_key(|c| c.start);
    let reach: Vec<Samples> = sp
        .iter()
        .scan(Samples::MIN, |far, c| {
            *far = (*far).max(c.end);
            Some(*far)
        })
        .collect();
    let inside = |b: Samples| reach.get(sp.partition_point(|c| c.start < b).wrapping_sub(1)).is_some_and(|far| *far > b);
    let mut slots: Vec<Samples> = sp.iter().flat_map(|c| [c.start, c.end]).filter(|b| !inside(*b)).collect();
    slots.sort_unstable();
    slots.dedup();
    slots
}

/// The slot nearest `at` (the earlier one on a tie), or the session start when there are none.
pub fn nearest_slot(slots: &[Samples], at: Samples) -> Samples {
    let i = slots.partition_point(|b| *b < at);
    [i.checked_sub(1), Some(i)].into_iter().flatten().filter_map(|i| slots.get(i)).copied().min_by_key(|b| b.abs_diff(at)).unwrap_or(0)
}

/// Where Shuffle mode lands material dropped at `at` on `track` (see [`shuffle_slots`]).
pub fn shuffle_slot(s: &Session, track: TrackId, at: Samples, moving: &[ClipId]) -> Samples {
    nearest_slot(&shuffle_slots(s, track, moving), at)
}

/// Lift the movable `moving` clips off a track the Shuffle way (see [`lift_plan`]); with
/// Automation Follows Edit their automation leaves with them. Returns the clips, each with that
/// automation timed from its start.
fn lift_all(s: &mut Session, track: TrackId, moving: &HashSet<ClipId>) -> Vec<(Clip, Vec<(AutoParam, AutomationPoint)>)> {
    let Some(clips) = s.track(track).map(|t| spans(t.clips())) else { return Vec::new() };
    let (lifts, staying) = lift_plan(clips, moving);
    let mut points: HashMap<ClipId, Vec<(AutoParam, AutomationPoint)>> = HashMap::new();
    for l in &lifts {
        points.insert(l.id, take_automation(s, track, l.from, l.from.saturating_add(l.shift), l.start));
        follow_automation(s, track, l.from.saturating_add(l.shift), (l.by, l.lock));
    }
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return Vec::new() };
    let starts: HashMap<ClipId, Samples> = staying.iter().map(|c| (c.id, c.start)).collect();
    let (out, kept): (Vec<Clip>, Vec<Clip>) = std::mem::take(&mut pl.clips).into_iter().partition(|c| points.contains_key(&c.id));
    pl.clips = kept;
    for c in &mut pl.clips {
        if let Some(start) = starts.get(&c.id) {
            c.start = *start;
        }
    }
    pl.sort();
    out.into_iter()
        .map(|c| {
            let p = points.remove(&c.id).unwrap_or_default();
            (c, p)
        })
        .collect()
}

/// The movable clips `ids` grouped by the track they land on (per `dest`): each destination and
/// the start of its earliest clip, which Shuffle lands on a slot; the others keep their distance.
pub fn shuffle_groups(s: &Session, ids: &[ClipId], dest: &[(TrackId, TrackId)]) -> Vec<(TrackId, Samples)> {
    let ids: HashSet<ClipId> = ids.iter().copied().collect();
    let mut groups: Vec<(TrackId, Samples)> = Vec::new();
    for t in &s.tracks {
        for c in t.clips().iter().filter(|c| ids.contains(&c.id) && !locked(c)) {
            let d = lands(dest, t.id);
            match groups.iter_mut().find(|g| g.0 == d) {
                Some(g) => g.1 = g.1.min(c.start),
                None => groups.push((d, c.start)),
            }
        }
    }
    groups
}

/// Move clips in Shuffle mode: each leaves its track (later clips close the gap, see
/// [`lift_plan`]), and the clips bound for each track land together, the earliest on the
/// [`shuffle_slot`] nearest `delta` away, pushing later clips right. With Automation Follows Edit
/// their automation travels with them. Refused, with the reason, when a locked clip or a
/// crossfade leaves no room.
pub fn shuffle_clips(s: &mut Session, ids: &[ClipId], delta: Samples, dest: &[(TrackId, TrackId)]) -> Result<usize, &'static str> {
    let groups: Vec<(TrackId, Samples, Samples)> =
        shuffle_groups(s, ids, dest).into_iter().map(|(d, first)| (d, first, shuffle_slot(s, d, first.saturating_add(delta), ids))).collect();
    let moving: HashSet<ClipId> = ids.iter().copied().collect();
    let sources: Vec<TrackId> = s.tracks.iter().filter(|t| t.clips().iter().any(|c| moving.contains(&c.id) && !locked(c))).map(|t| t.id).collect();
    let mut lifted = Vec::new();
    for t in sources {
        lifted.extend(lift_all(s, t, &moving).into_iter().map(|(c, points)| (lands(dest, t), c, points)));
    }
    for (d, first, at) in groups {
        let block: Vec<&(TrackId, Clip, Vec<(AutoParam, AutomationPoint)>)> = lifted.iter().filter(|l| l.0 == d).collect();
        let len = block.iter().map(|l| l.1.end()).max().unwrap_or(first).saturating_sub(first);
        make_room(s, d, at, len)?;
        let mut placed = Vec::new();
        for (_, c, points) in block {
            let mut c = c.clone();
            c.start = at.saturating_add(c.start.saturating_sub(first));
            put_automation(s, d, c.start, points);
            placed.push(c);
        }
        if let Some(pl) = s.track_mut(d).and_then(|t| t.playlist_mut()) {
            pl.clips.extend(placed);
            pl.sort();
        }
    }
    Ok(lifted.len())
}

/// How far a Grabber drag by `raw` moves `clip` in Grid mode: Absolute puts its sync point (its
/// start when it has none inside the clip) on the nearest grid line; Relative moves it by whole
/// grid steps, keeping its offset from the grid.
pub fn grid_delta(s: &Session, clip: &Clip, raw: Samples, relative: bool) -> Samples {
    let (g, sr, map, rate) = (&s.edit.grid, s.sample_rate, &s.tempo, s.frame_rate);
    let sync = if (0..clip.length).contains(&clip.sync_point) { clip.sync_point } else { 0 };
    let anchor = clip.start.saturating_add(sync);
    let from = if relative { g.line_at_or_before(anchor, sr, map, rate) } else { anchor };
    g.snap(from.saturating_add(raw), sr, map, rate).saturating_sub(from)
}

/// Trim a clip's edges to `start` and/or `end` like the Trim tool, never past its file. In
/// Shuffle mode the start stays put (the material slides under it) and later clips follow the
/// change, up to a locked clip; otherwise a trim stops at the neighbouring clips.
pub fn trim_clip(s: &mut Session, id: ClipId, start: Option<Samples>, end: Option<Samples>, shuffle: bool) -> Option<Clip> {
    let (track, old) = s.find_clip(id).filter(|(_, c)| !c.edit_locked).map(|(t, c)| (t, c.clone()))?;
    let (mut lo, mut hi) = (Samples::MIN, Samples::MAX);
    if shuffle {
        let room = shuffle_room(s, track, old.end()).max(0);
        lo = old.start.saturating_sub(room);
        hi = old.end().saturating_add(room);
    } else {
        let others = s.track(track).map(|t| t.clips()).unwrap_or_default().iter().filter(|c| c.id != id);
        if let Some(prev) = others.clone().filter(|c| c.start < old.start).map(Clip::end).max() {
            lo = lo.max(prev.min(old.start));
        }
        if let Some(next) = others.filter(|c| c.start > old.start).map(|c| c.start).min() {
            hi = hi.min(next.max(old.end()));
        }
        lo = lo.max(0);
    }
    let file = |c: &Clip| {
        let start = c.start.saturating_sub(c.source_offset());
        material_len(s, c).map(|n| (start.min(c.start), start.saturating_add(n).max(c.end())))
    };
    let t = TimeCtx::of(s);
    let mut c = old.clone();
    if let Some(at) = start {
        c.trim_start_to(at.max(file(&c).map_or(lo, |(f, _)| lo.max(f))));
        rebase_midi_start(&mut c, old.start, t);
        if shuffle {
            c.start = old.start;
        }
    }
    if let Some(at) = end {
        c.trim_end_to(at.min(file(&c).map_or(hi, |(_, f)| hi.min(f))));
    }
    let pl = s.track_mut(track).and_then(|t| t.playlist_mut())?;
    *pl.clip_mut(id)? = c.clone();
    pl.sort();
    if shuffle {
        let moved = ripple(s, track, old.start.saturating_add(1), c.end().saturating_sub(old.end()));
        follow_automation(s, track, old.end(), moved);
    }
    Some(c)
}

/// Length of the file or movie under a clip; None for MIDI, which has no end.
fn material_len(s: &Session, c: &Clip) -> Option<Samples> {
    match c.content {
        ClipContent::Audio { source, .. } => s.source(source).map(|src| i64::try_from(src.frames).unwrap_or(i64::MAX)),
        ClipContent::Video { source, .. } => s.video(source).map(|v| s.sample_rate.samples(v.duration.clamp(0.0, 1e7))),
        ClipContent::Midi { .. } => None,
    }
}

/// Create fades from a selection (the incumbent's Edit › Fades › Create semantics):
/// a range across a clip boundary makes a crossfade; at a clip start a fade-in; at its end a fade-out.
pub fn create_fades(s: &mut Session, track: TrackId, range: Range, shape: FadeShape) -> usize {
    if range.is_empty() {
        return 0;
    }
    let handles: Vec<(ClipId, Samples)> = {
        let Some(tr) = s.track(track) else { return 0 };
        tr.clips().iter().map(|c| (c.id, material_len(s, c).map_or(0, |n| n.saturating_sub(c.source_offset()).saturating_sub(c.length)))).collect()
    };
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    pl.sort();
    let mut made = 0;
    let mut xfaded: Vec<ClipId> = Vec::new();
    let n = pl.clips.len();
    // Crossfades between adjacent clips whose boundary is inside the range.
    for i in 0..n.saturating_sub(1) {
        let (Some(a), Some(b)) = (pl.clips.get(i).cloned(), pl.clips.get(i + 1).cloned()) else { continue };
        let boundary = a.end();
        if b.start == boundary && range.start < boundary && range.end > boundary {
            let before = boundary - range.start.max(a.start);
            let after = range.end.min(b.end()) - boundary;
            let a_handle = handles.iter().find(|h| h.0 == a.id).map_or(0, |h| h.1);
            let b_handle = b.source_offset();
            // Extend each clip into the other's territory using its handles.
            let ext_a = after.min(a_handle).max(0);
            let ext_b = before.min(b_handle).max(0);
            if let Some(ca) = pl.clips.get_mut(i) {
                ca.length += ext_a;
                ca.fade_out = Fade { len: (ext_a + before).min(ca.length), shape, curvature: 0.0 };
            }
            if let Some(cb) = pl.clips.get_mut(i + 1) {
                let old = cb.start;
                cb.trim_start_to(old - ext_b);
                cb.fade_in = Fade { len: (ext_b + after).min(cb.length), shape, curvature: 0.0 };
            }
            made += 1;
            xfaded.push(a.id);
            xfaded.push(b.id);
        }
    }
    for c in pl.clips.iter_mut().filter(|c| !xfaded.contains(&c.id)) {
        if range.start <= c.start && range.end > c.start && range.end < c.end() {
            c.fade_in = Fade { len: range.end - c.start, shape, curvature: 0.0 };
            made += 1;
        } else if range.end >= c.end() && range.start > c.start && range.start < c.end() {
            c.fade_out = Fade { len: c.end() - range.start, shape, curvature: 0.0 };
            made += 1;
        }
    }
    made
}

/// Remove fades on clips touching `range`.
pub fn delete_fades(s: &mut Session, track: TrackId, range: Range) -> usize {
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    let mut n = 0;
    let mut touched: HashSet<ClipId> = HashSet::new();
    for c in &mut pl.clips {
        if c.range().overlaps(&range) || (range.is_empty() && c.range().contains(range.start)) {
            touched.insert(c.id);
            if c.fade_in.len > 0 || c.fade_out.len > 0 {
                n += 1;
            }
            c.fade_in = Fade::default();
            c.fade_out = Fade::default();
        }
    }
    // Overlapping (crossfaded) neighbours in the range are trimmed back to the midpoint; other overlaps stay as they are.
    pl.sort();
    for i in 0..pl.clips.len().saturating_sub(1) {
        let (Some(a_end), Some(b_start)) = (pl.clips.get(i).map(Clip::end), pl.clips.get(i + 1).map(|c| c.start)) else { continue };
        let in_range = pl.clips.get(i).is_some_and(|a| touched.contains(&a.id)) || pl.clips.get(i + 1).is_some_and(|b| touched.contains(&b.id));
        if in_range && a_end > b_start {
            let mid = b_start + (a_end - b_start) / 2;
            if let Some(a) = pl.clips.get_mut(i) {
                a.trim_end_to(mid);
            }
            if let Some(b) = pl.clips.get_mut(i + 1) {
                b.trim_start_to(mid);
            }
        }
    }
    // Trimming can move a start past a later clip's start when one clip overlapped several.
    pl.sort();
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, Source, SourceId, TrackKind};

    fn session_with_clip() -> (Session, TrackId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.sources.push(Source {
            id: SourceId(500),
            name: "src".into(),
            path: "src.wav".into(),
            channels: 1,
            frames: 10_000,
            sample_rate: 48_000,
            format: soundcraft_audio_io::FileFormat::Wav,
            time_reference: 0,
            unsaved: false,
        });
        let id = s.new_clip_id();
        if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
            pl.clips.push(Clip::audio(id, "a", SourceId(500), 1000, 1000, 4000));
        }
        (s, t)
    }

    fn clips(s: &Session, t: TrackId) -> Vec<(Samples, Samples, Samples)> {
        s.track(t).unwrap().clips().iter().map(|c| (c.start, c.length, c.source_offset())).collect()
    }

    #[test]
    fn copy_range_resets_fades_at_cut_edges() {
        let (mut s, t) = session_with_clip();
        if let Some(c) = s.track_mut(t).and_then(|t| t.playlist_mut()).and_then(|p| p.clips.first_mut()) {
            c.fade_in = Fade { len: 100, shape: FadeShape::Linear, curvature: 0.0 };
            c.fade_out = Fade { len: 100, shape: FadeShape::Linear, curvature: 0.0 };
        }
        let fades = |r: Range| -> Vec<(Samples, Samples)> { copy_range(&s, t, r).iter().map(|c| (c.fade_in.len, c.fade_out.len)).collect() };
        assert_eq!(fades(Range::new(2000, 3000)), vec![(0, 0)]);
        assert_eq!(fades(Range::new(0, 3000)), vec![(100, 0)]);
        assert_eq!(fades(Range::new(2000, 6000)), vec![(0, 100)]);
        assert_eq!(fades(Range::new(0, 6000)), vec![(100, 100)]);
    }

    #[test]
    fn separate_splits_and_heal_rejoins() {
        let (mut s, t) = session_with_clip();
        assert_eq!(separate_at(&mut s, t, 3000), 1);
        assert_eq!(clips(&s, t), vec![(1000, 2000, 1000), (3000, 2000, 3000)]);
        assert_eq!(separate_at(&mut s, t, 1000), 0);
        assert_eq!(heal(&mut s, t, Range::new(0, 10_000)), 1);
        assert_eq!(clips(&s, t), vec![(1000, 4000, 1000)]);
    }

    #[test]
    fn heal_rejoins_cut_separation() {
        let (mut s, t) = session_with_clip();
        clear_range(&mut s, t, Range::new(2000, 3000), false);
        assert_eq!(heal(&mut s, t, Range::new(1500, 3500)), 1);
        assert_eq!(clips(&s, t), vec![(1000, 4000, 1000)]);
        // A selection that does not span the whole gap leaves it alone.
        let (mut s, t) = session_with_clip();
        clear_range(&mut s, t, Range::new(2000, 3000), false);
        assert_eq!(heal(&mut s, t, Range::new(2500, 3500)), 0);
        // Shuffled pieces no longer line up with their source and stay apart.
        let (mut s, t) = session_with_clip();
        clear_range(&mut s, t, Range::new(2000, 3000), true);
        assert_eq!(heal(&mut s, t, Range::new(0, 10_000)), 0);
        // Moved apart: the timeline gap no longer matches the source gap.
        let (mut s, t) = session_with_clip();
        clear_range(&mut s, t, Range::new(2000, 3000), false);
        if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
            pl.clips[1].start = 3500;
        }
        assert_eq!(heal(&mut s, t, Range::new(0, 10_000)), 0);
        // Overlapping neighbours whose offsets line up are not a separation.
        let (mut s, t) = session_with_clip();
        assert_eq!(separate_at(&mut s, t, 3000), 1);
        if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
            pl.clips[1].start = 2500;
        }
        assert_eq!(heal(&mut s, t, Range::new(0, 10_000)), 0);
        assert_eq!(clips(&s, t).len(), 2);
    }

    #[test]
    fn heal_skips_clips_with_different_playback() {
        for tweak in [(|c: &mut Clip| c.muted = true) as fn(&mut Clip), |c: &mut Clip| c.stretch = 1.5] {
            let (mut s, t) = session_with_clip();
            separate_at(&mut s, t, 3000);
            if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
                tweak(&mut pl.clips[1]);
            }
            assert_eq!(heal(&mut s, t, Range::new(0, 10_000)), 0);
            assert_eq!(s.track(t).unwrap().clips().len(), 2);
        }
    }

    #[test]
    fn heal_keeps_each_side_gain_envelope() {
        let cases: [(Vec<(Samples, f32)>, Vec<(Samples, f32)>); 4] = [
            (vec![(500, -3.0), (2500, -12.0)], vec![]),
            (vec![], vec![(0, -6.0), (1000, 0.0)]),
            (vec![(0, 0.0), (1999, -20.0)], vec![(-100, 4.0), (300, -9.0)]),
            (vec![(500, -3.0), (2500, -12.0)], vec![(0, -9.75), (500, -12.0)]),
        ];
        for (left_env, right_env) in cases {
            let (mut s, t) = session_with_clip();
            separate_at(&mut s, t, 3000);
            let pl = s.track_mut(t).and_then(|t| t.playlist_mut()).unwrap();
            pl.clips[0].gain_env = left_env;
            pl.clips[1].gain_env = right_env;
            let (l, r) = (pl.clips[0].clone(), pl.clips[1].clone());
            assert_eq!(heal(&mut s, t, Range::new(0, 10_000)), 1);
            let healed = &s.track(t).unwrap().clips()[0];
            for rel in 0..4000 {
                let want = if rel < 2000 { l.gain_at(rel) } else { r.gain_at(rel - 2000) };
                assert!((healed.gain_at(rel) - want).abs() < 1e-4, "sample {rel}: {} != {want}", healed.gain_at(rel));
            }
        }
    }

    #[test]
    fn heal_keeps_gain_envelopes_across_a_gap() {
        let (mut s, t) = session_with_clip();
        clear_range(&mut s, t, Range::new(2000, 3000), false);
        let pl = s.track_mut(t).and_then(|t| t.playlist_mut()).unwrap();
        pl.clips[0].gain_env = vec![(0, 0.0), (999, -6.0)];
        pl.clips[1].gain_env = vec![(0, -12.0), (1999, -3.0)];
        let (l, r) = (pl.clips[0].clone(), pl.clips[1].clone());
        assert_eq!(heal(&mut s, t, Range::new(1500, 3500)), 1);
        let healed = &s.track(t).unwrap().clips()[0];
        assert_eq!(healed.length, 4000);
        for rel in (0..1000).chain(2000..4000) {
            let want = if rel < 1000 { l.gain_at(rel) } else { r.gain_at(rel - 2000) };
            assert!((healed.gain_at(rel) - want).abs() < 1e-4, "sample {rel}: {} != {want}", healed.gain_at(rel));
        }
    }

    #[test]
    fn split_then_heal_rejoins_stretched_clip() {
        let (mut s, t) = session_with_clip();
        if let Some(c) = s.track_mut(t).and_then(|t| t.playlist_mut()).and_then(|p| p.clips.first_mut()) {
            c.stretch = 2.0;
        }
        assert_eq!(separate_at(&mut s, t, 3000), 1);
        assert_eq!(clips(&s, t), vec![(1000, 2000, 1000), (3000, 2000, 2000)]);
        assert_eq!(heal(&mut s, t, Range::new(0, 10_000)), 1);
        assert_eq!(clips(&s, t), vec![(1000, 4000, 1000)]);
    }

    #[test]
    fn clear_slip_and_shuffle() {
        let (mut s, t) = session_with_clip();
        let removed = clear_range(&mut s, t, Range::new(2000, 3000), false);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].start, 0);
        assert_eq!(clips(&s, t), vec![(1000, 1000, 1000), (3000, 2000, 3000)]);
        let (mut s, t) = session_with_clip();
        clear_range(&mut s, t, Range::new(2000, 3000), true);
        assert_eq!(clips(&s, t), vec![(1000, 1000, 1000), (2000, 2000, 3000)]);
    }

    #[test]
    fn copy_paste_overwrites() {
        let (mut s, t) = session_with_clip();
        let copied = copy_range(&s, t, Range::new(1500, 2500));
        assert_eq!(copied.len(), 1);
        assert_eq!((copied[0].start, copied[0].length, copied[0].source_offset()), (0, 1000, 1500));
        paste(&mut s, t, 4000, &copied, 1000, false).unwrap();
        assert_eq!(clips(&s, t), vec![(1000, 3000, 1000), (4000, 1000, 1500)]);
    }

    #[test]
    fn paste_shuffle_ripples() {
        let (mut s, t) = session_with_clip();
        let copied = copy_range(&s, t, Range::new(1000, 2000));
        paste(&mut s, t, 1000, &copied, 1000, true).unwrap();
        assert_eq!(clips(&s, t), vec![(1000, 1000, 1000), (2000, 4000, 1000)]);
    }

    #[test]
    fn insert_silence_and_trim() {
        let (mut s, t) = session_with_clip();
        insert_silence(&mut s, t, Range::new(2000, 2500));
        assert_eq!(clips(&s, t), vec![(1000, 1000, 1000), (2500, 3000, 2000)]);
        let (mut s, t) = session_with_clip();
        trim_to(&mut s, t, Range::new(1500, 2000));
        assert_eq!(clips(&s, t), vec![(1500, 500, 1500)]);
    }

    #[test]
    fn fades_and_crossfades() {
        let (mut s, t) = session_with_clip();
        assert_eq!(create_fades(&mut s, t, Range::new(500, 1500), FadeShape::Linear), 1);
        assert_eq!(s.track(t).unwrap().clips()[0].fade_in.len, 500);
        let (mut s, t) = session_with_clip();
        separate_at(&mut s, t, 3000);
        assert_eq!(create_fades(&mut s, t, Range::new(2900, 3100), FadeShape::EqualPower), 1);
        let c = s.track(t).unwrap().clips();
        assert_eq!(c[0].end(), 3100);
        assert_eq!(c[1].start, 2900);
        assert_eq!(c[0].fade_out.len, 200);
        assert_eq!(c[1].fade_in.len, 200);
        delete_fades(&mut s, t, Range::new(2900, 3100));
        let c = s.track(t).unwrap().clips();
        assert_eq!((c[0].end(), c[1].start), (3000, 3000));
    }

    fn three_clips() -> (Session, TrackId, [ClipId; 3]) {
        let (mut s, t) = session_with_clip();
        let ids = [s.new_clip_id(), s.new_clip_id(), s.new_clip_id()];
        if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
            pl.clips = vec![
                Clip::audio(ids[0], "a", SourceId(500), 0, 0, 1000),
                Clip::audio(ids[1], "b", SourceId(500), 1000, 1000, 2000),
                Clip::audio(ids[2], "c", SourceId(500), 3000, 5000, 1000),
            ];
        }
        (s, t, ids)
    }

    #[test]
    fn shuffle_moves_reorder_clips_and_keep_the_silence() {
        let (mut s, t, [_, b, _]) = three_clips();
        assert_eq!(shuffle_clips(&mut s, &[b], -900, &[]), Ok(1));
        assert_eq!(clips(&s, t), vec![(0, 2000, 1000), (2000, 1000, 0), (5000, 1000, 3000)]);
        let (mut s, t, [a, ..]) = three_clips();
        assert_eq!(shuffle_slot(&s, t, 900, &[a]), 0);
        assert_eq!(shuffle_slot(&s, t, 1100, &[a]), 2000);
        shuffle_clips(&mut s, &[a], 2500, &[]).unwrap();
        assert_eq!(clips(&s, t), vec![(0, 2000, 1000), (2000, 1000, 0), (5000, 1000, 3000)]);
    }

    #[test]
    fn shuffling_onto_an_empty_track_starts_it_at_zero() {
        let (mut s, t, [_, _, c]) = three_clips();
        let other = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        shuffle_clips(&mut s, &[c], 40_000, &[(t, other)]).unwrap();
        assert_eq!(clips(&s, other), vec![(0, 1000, 3000)]);
        assert_eq!(clips(&s, t), vec![(0, 1000, 0), (1000, 2000, 1000)]);
    }

    #[test]
    fn shuffle_trims_move_later_clips_and_slip_trims_stop_at_neighbours() {
        let (mut s, t, [a, b, c]) = three_clips();
        trim_clip(&mut s, b, None, Some(2500), true);
        trim_clip(&mut s, b, Some(1500), None, true);
        assert_eq!(clips(&s, t), vec![(0, 1000, 0), (1000, 1000, 1500), (4000, 1000, 3000)]);
        let (mut s, t, _) = three_clips();
        trim_clip(&mut s, a, None, Some(2500), false);
        trim_clip(&mut s, c, Some(2000), None, false);
        assert_eq!(clips(&s, t), vec![(0, 1000, 0), (1000, 2000, 1000), (3000, 3000, 1000)]);
        trim_clip(&mut s, c, None, Some(99_999), false);
        assert_eq!(clips(&s, t)[2], (3000, 9000, 1000));
    }

    #[test]
    fn shuffle_leaves_locked_clips_and_what_follows_them_in_place() {
        let (mut s, t, [a, b, c]) = three_clips();
        s.find_clip_mut(c).unwrap().time_locked = true;
        clear_range(&mut s, t, Range::new(1000, 3000), true);
        assert_eq!(clips(&s, t), vec![(0, 1000, 0), (5000, 1000, 3000)]);
        let (mut s, t, [a2, ..]) = three_clips();
        s.find_clip_mut(c).unwrap().edit_locked = true;
        assert_eq!(shuffle_room(&s, t, 0), 2000);
        assert_eq!(make_room(&mut s, t, 0, 2500), Err(NO_ROOM));
        assert_eq!(make_room(&mut s, t, 0, 2000), Ok(()));
        assert_eq!(clips(&s, t), vec![(2000, 1000, 0), (3000, 2000, 1000), (5000, 1000, 3000)]);
        assert_eq!(shuffle_clips(&mut s, &[a2], 2500, &[]), Ok(1));
        assert_eq!(clips(&s, t)[2], (5000, 1000, 3000));
        let (mut s, t, _) = three_clips();
        s.find_clip_mut(c).unwrap().time_locked = true;
        trim_clip(&mut s, b, None, Some(9000), true);
        assert_eq!(clips(&s, t), vec![(0, 1000, 0), (1000, 4000, 1000), (5000, 1000, 3000)]);
        assert_eq!(shuffle_slot(&s, t, 4000, &[a]), 4000);
    }

    #[test]
    fn multi_track_moves_keep_the_tracks_apart() {
        let (mut s, t1, [a, ..]) = three_clips();
        let t2 = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let t3 = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let m = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
        let d = s.new_clip_id();
        s.track_mut(t2).and_then(|t| t.playlist_mut()).unwrap().clips.push(Clip::audio(d, "d", SourceId(500), 0, 0, 500));
        let dest = destinations(&s, &[a, d], t1, t2);
        assert_eq!(dest, Some(vec![(t1, t2), (t2, t3)]));
        assert_eq!(destinations(&s, &[a, d], t1, m), None);
        assert_eq!(destinations(&s, &[a], t1, m), None);
        assert_eq!(move_clips(&mut s, &[a, d], 0, &dest.unwrap()), 2);
        assert_eq!(clips(&s, t2), vec![(0, 1000, 0)]);
        assert_eq!(clips(&s, t3), vec![(0, 500, 0)]);
    }

    #[test]
    fn grid_moves_snap_the_clip_start_or_keep_its_offset() {
        let (s, t) = session_with_clip();
        let step = s.edit.grid.step_samples(0, s.sample_rate, &s.tempo, s.frame_rate);
        let c = s.track(t).unwrap().clips()[0].clone();
        assert_eq!(c.start + grid_delta(&s, &c, step + 200, false), step);
        assert_eq!(c.start + grid_delta(&s, &c, step + 200, true), step + 1000);
        assert_eq!(grid_delta(&s, &c, step / 3, true), 0);
    }

    #[test]
    fn moves_overwrite_and_respect_locks() {
        let (mut s, t) = session_with_clip();
        let id = s.track(t).unwrap().clips()[0].id;
        assert_eq!(move_clips(&mut s, &[id], 500, &[]), 1);
        assert_eq!(clips(&s, t), vec![(1500, 4000, 1000)]);
        s.find_clip_mut(id).unwrap().time_locked = true;
        assert_eq!(move_clips(&mut s, &[id], 500, &[]), 0);
    }

    #[test]
    fn delete_fades_keeps_playlist_sorted_when_a_clip_overlaps_several() {
        let (mut s, t) = session_with_clip();
        let pl = s.track_mut(t).and_then(|t| t.playlist_mut()).expect("track has a playlist");
        pl.clips.clear();
        pl.clips.push(Clip::audio(ClipId(901), "a", SourceId(500), 0, 0, 1000));
        pl.clips.push(Clip::audio(ClipId(902), "b", SourceId(500), 100, 100, 100));
        pl.clips.push(Clip::audio(ClipId(903), "c", SourceId(500), 150, 150, 750));
        delete_fades(&mut s, t, Range::new(0, 10_000));
        let got = clips(&s, t);
        assert!(got.windows(2).all(|w| w[0].0 <= w[1].0), "playlist not sorted: {got:?}");
        assert!(got.iter().all(|c| c.1 > 0), "empty clip: {got:?}");
    }

    #[test]
    fn delete_fades_leaves_unrelated_overlaps_alone() {
        let (mut s, t) = session_with_clip();
        let ids: Vec<ClipId> = (0..4).map(|_| s.new_clip_id()).collect();
        if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
            pl.clips.clear();
            for (id, start, len) in [(ids[0], 0, 1100), (ids[1], 1000, 1000), (ids[2], 5000, 1100), (ids[3], 6000, 1000)] {
                let mut c = Clip::audio(id, "c", SourceId(500), start, start, len);
                c.fade_in = Fade { len: 100, shape: FadeShape::Linear, curvature: 0.0 };
                c.fade_out = Fade { len: 100, shape: FadeShape::Linear, curvature: 0.0 };
                pl.clips.push(c);
            }
        }
        assert_eq!(delete_fades(&mut s, t, Range::new(900, 1200)), 2);
        assert_eq!(clips(&s, t), vec![(0, 1050, 0), (1050, 950, 1050), (5000, 1100, 5000), (6000, 1000, 6000)]);
        let c = s.track(t).unwrap().clips();
        assert_eq!((c[1].fade_in.len, c[2].fade_out.len, c[3].fade_in.len), (0, 100, 100));
    }
}
