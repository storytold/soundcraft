//! Guitar tablature: a MIDI track's notes as string and fret numbers, laid out in measures on the
//! sixteenth grid that [`crate::score`] notates with.
//!
//! MIDI channel N (1-based) plays string N, string 1 being the highest-pitched string, and the fret
//! is the note minus that string's open pitch. Notes whose channel names no string, or whose fret
//! would be negative or above [`MAX_FRET`], get the lowest playable fret on a string no other
//! note of the same onset uses.

use crate::score::{self, GridNote, UNIT};
use serde_json::{Value, json};
use soundcraft_model::{ClipContent, ClipId, Session, Track, TrackId};
use soundcraft_time::Samples;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The highest fret a note is placed on.
pub const MAX_FRET: u8 = 24;
/// Instruments with 4 to 8 strings (basses, guitars, extended-range guitars).
pub const MIN_STRINGS: usize = *soundcraft_model::GUITAR_STRINGS.start();
pub const MAX_STRINGS: usize = *soundcraft_model::GUITAR_STRINGS.end();
/// Cap on the search for a chord's fingering, so a hostile chord cannot stall the UI.
const SEARCH_BUDGET: usize = 20_000;

/// A named tuning; open-string MIDI pitches, string 1 (highest) first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub strings: &'static [u8],
}

/// E standard first: it is the default.
pub const PRESETS: &[Preset] = &[
    Preset { id: "e_standard", name: "E Standard", strings: &[64, 59, 55, 50, 45, 40] },
    Preset { id: "drop_d", name: "Drop D", strings: &[64, 59, 55, 50, 45, 38] },
    Preset { id: "d_standard", name: "D Standard", strings: &[62, 57, 53, 48, 43, 38] },
    Preset { id: "c_standard", name: "C Standard", strings: &[60, 55, 51, 46, 41, 36] },
    Preset { id: "b_standard", name: "B Standard", strings: &[59, 54, 50, 45, 40, 35] },
    Preset { id: "drop_a", name: "Drop A", strings: &[59, 54, 50, 45, 40, 33] },
    Preset { id: "seven_b_standard", name: "7-String B Standard", strings: &[64, 59, 55, 50, 45, 40, 35] },
    Preset { id: "seven_drop_a", name: "7-String Drop A", strings: &[64, 59, 55, 50, 45, 40, 33] },
    Preset { id: "bass_e_standard", name: "Bass E Standard", strings: &[43, 38, 33, 28] },
];

/// E standard, the tuning of tracks without one.
pub const DEFAULT_TUNING: &[u8] = &[64, 59, 55, 50, 45, 40];

/// Whether `strings` is a usable tuning: 4 to 8 MIDI pitches, highest first.
pub fn valid_tuning(strings: &[u8]) -> bool {
    soundcraft_model::valid_guitar_tuning(strings)
}

/// The track's tuning, falling back to E standard when it has none (or a corrupt one).
pub fn tuning_of(track: &Track) -> Vec<u8> {
    track.guitar_tuning.as_deref().filter(|t| valid_tuning(t)).unwrap_or(DEFAULT_TUNING).to_vec()
}

/// The preset `strings` matches, if any.
pub fn preset_of(strings: &[u8]) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.strings == strings)
}

const PITCH_CLASSES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// Pitch class name of `pitch`, as tab string labels show it.
pub fn pitch_class(pitch: u8) -> &'static str {
    PITCH_CLASSES.get(usize::from(pitch % 12)).copied().unwrap_or("?")
}

/// Scientific pitch name (middle C, 60, is "C4"; a guitar's low E is "E2").
pub fn pitch_name(pitch: u8) -> String {
    format!("{}{}", pitch_class(pitch), i32::from(pitch / 12) - 1)
}

/// Parse a scientific pitch name ("E2", "F#3", "Bb1") or a MIDI number.
fn parse_pitch(text: &str) -> Option<u8> {
    let text = text.trim();
    if let Ok(n) = text.parse::<u8>() {
        return (n <= 127).then_some(n);
    }
    let mut chars = text.chars();
    let step = chars.next()?.to_ascii_uppercase();
    let base: i32 = match step {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest: String = chars.collect();
    let (alter, octave) = match rest.chars().next() {
        Some('#') => (1, &rest[1..]),
        Some('b') => (-1, &rest[1..]),
        _ => (0, rest.as_str()),
    };
    let octave: i32 = octave.parse().ok()?;
    let pitch = (octave.checked_add(1)?).checked_mul(12)?.checked_add(base + alter)?;
    u8::try_from(pitch).ok().filter(|p| *p <= 127)
}

/// A tuning from a command parameter: a preset id or name, an array of MIDI numbers or pitch
/// names, or a string of pitch names ("E2 A2 D3 G3 B3 E4"). Strings may come in any order;
/// string 1 is always the highest.
pub fn parse_tuning(v: &Value) -> Result<Vec<u8>, String> {
    let mut strings: Vec<u8> = match v {
        Value::String(text) => {
            let key = text.trim().to_ascii_lowercase().replace([' ', '-'], "_");
            if let Some(p) = PRESETS.iter().find(|p| p.id == key || p.name.to_ascii_lowercase().replace([' ', '-'], "_") == key) {
                return Ok(p.strings.to_vec());
            }
            text.split([' ', ','])
                .filter(|w| !w.is_empty())
                .map(|w| parse_pitch(w).ok_or_else(|| format!("unknown pitch `{w}`")))
                .collect::<Result<_, _>>()?
        }
        Value::Array(items) => {
            if items.len() > MAX_STRINGS {
                return Err(format!("a tuning has at most {MAX_STRINGS} strings"));
            }
            items
                .iter()
                .map(|i| match i {
                    Value::Number(n) => {
                        n.as_u64().and_then(|n| u8::try_from(n).ok()).filter(|n| *n <= 127).ok_or_else(|| format!("pitch {n} is out of range"))
                    }
                    Value::String(s) => parse_pitch(s).ok_or_else(|| format!("unknown pitch `{s}`")),
                    _ => Err("tuning entries are MIDI numbers or pitch names".to_string()),
                })
                .collect::<Result<_, _>>()?
        }
        _ => return Err("`tuning` must be a preset id, a list of pitches or a string of pitch names".into()),
    };
    strings.sort_unstable_by(|a, b| b.cmp(a));
    if !valid_tuning(&strings) {
        return Err(format!("a tuning has {MIN_STRINGS} to {MAX_STRINGS} strings"));
    }
    Ok(strings)
}

/// JSON description of a tuning.
pub fn tuning_json(strings: &[u8]) -> Value {
    json!({
        "preset": preset_of(strings).map(|p| p.id),
        "name": preset_of(strings).map_or("Custom", |p| p.name),
        "strings": strings,
        "names": strings.iter().map(|p| pitch_name(*p)).collect::<Vec<_>>(),
    })
}

/// A note placed on the fretboard. `string` and `fret` are `None` when it cannot be played in
/// this tuning (out of range, or more notes than strings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabNote {
    pub pitch: u8,
    pub channel: u8,
    /// 1-based; 1 is the highest string.
    pub string: Option<u8>,
    pub fret: Option<u8>,
}

/// The notes starting together on one sixteenth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabEvent {
    /// Sixteenths from the start of the measure.
    pub unit: i64,
    pub notes: Vec<TabNote>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabMeasure {
    /// 1-based bar number.
    pub number: usize,
    /// First sixteenth of the measure, from the session start.
    pub start: i64,
    /// Length in sixteenths.
    pub units: i64,
    pub numerator: u32,
    pub denominator: u32,
    pub events: Vec<TabEvent>,
}

/// A track as tablature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub track: TrackId,
    pub name: String,
    pub tuning: Vec<u8>,
    pub measures: Vec<TabMeasure>,
}

impl Tab {
    /// Notes that could not be placed on a string.
    pub fn unplayable(&self) -> usize {
        self.measures.iter().flat_map(|m| &m.events).flat_map(|e| &e.notes).filter(|n| n.string.is_none()).count()
    }
}

fn fret_on(pitch: u8, open: u8) -> Option<u8> {
    pitch.checked_sub(open).filter(|f| *f <= MAX_FRET)
}

/// Place the notes of one onset (`(pitch, channel)` pairs) on strings of `tuning`.
/// Notes come back highest pitch first. A pitch on two channels is a unison on two strings.
pub fn assign(chord: &[(u8, u8)], tuning: &[u8]) -> Vec<TabNote> {
    let mut notes: Vec<TabNote> = Vec::with_capacity(chord.len());
    for (pitch, channel) in chord {
        if !notes.iter().any(|n| n.pitch == *pitch && n.channel == *channel) {
            notes.push(TabNote { pitch: *pitch, channel: *channel, string: None, fret: None });
        }
    }
    notes.sort_by(|a, b| b.pitch.cmp(&a.pitch).then(a.channel.cmp(&b.channel)));
    let mut used = vec![false; tuning.len()];
    // Channel N is string N, when that gives a playable fret on a free string.
    for n in &mut notes {
        let s = usize::from(n.channel);
        if let (Some(open), Some(false)) = (tuning.get(s), used.get(s))
            && let Some(f) = fret_on(n.pitch, *open)
        {
            n.string = u8::try_from(s + 1).ok();
            n.fret = Some(f);
            if let Some(u) = used.get_mut(s) {
                *u = true;
            }
        }
    }
    // The rest: the lowest playable position on the free strings.
    let rest: Vec<usize> = (0..notes.len()).filter(|i| notes.get(*i).is_some_and(|n| n.string.is_none())).collect();
    if rest.is_empty() {
        return notes;
    }
    let pitches: Vec<u8> = rest.iter().filter_map(|i| notes.get(*i).map(|n| n.pitch)).collect();
    let best = fingering(&pitches, tuning, &used);
    for (k, choice) in best.into_iter().enumerate() {
        if let (Some(i), Some(s)) = (rest.get(k), choice)
            && let (Some(n), Some(open)) = (notes.get_mut(*i), tuning.get(s))
        {
            n.string = u8::try_from(s + 1).ok();
            n.fret = fret_on(n.pitch, *open);
        }
    }
    notes
}

/// Strings (0-based) for `pitches` on the strings not `used`: as many notes placed as possible,
/// then the lowest highest fret, then the lowest fret sum. A depth-first search that tries the
/// lowest fret first, so its first answer is the greedy one and a spent budget still answers.
fn fingering(pitches: &[u8], tuning: &[u8], used: &[bool]) -> Vec<Option<usize>> {
    struct Search<'a> {
        pitches: &'a [u8],
        tuning: &'a [u8],
        used: Vec<bool>,
        current: Vec<Option<usize>>,
        best: Vec<Option<usize>>,
        best_score: (usize, u32, u32),
        nodes: usize,
    }
    impl Search<'_> {
        fn score(&self) -> (usize, u32, u32) {
            let frets: Vec<u32> = self
                .current
                .iter()
                .zip(self.pitches)
                .filter_map(|(s, p)| s.and_then(|s| self.tuning.get(s)).and_then(|o| fret_on(*p, *o)).map(u32::from))
                .collect();
            // Fewer unplaced notes, then a lower position, then fewer frets in all.
            (self.pitches.len() - frets.len(), frets.iter().copied().max().unwrap_or(0), frets.iter().sum())
        }
        fn go(&mut self, k: usize) {
            self.nodes += 1;
            if self.nodes > SEARCH_BUDGET {
                return;
            }
            let Some(pitch) = self.pitches.get(k).copied() else {
                let score = self.score();
                if score < self.best_score {
                    self.best_score = score;
                    self.best.clone_from(&self.current);
                }
                return;
            };
            let mut options: Vec<(u8, usize)> = self
                .tuning
                .iter()
                .enumerate()
                .filter(|(s, _)| self.used.get(*s) == Some(&false))
                .filter_map(|(s, open)| fret_on(pitch, *open).map(|f| (f, s)))
                .collect();
            options.sort_unstable();
            for (_, s) in options {
                if let Some(u) = self.used.get_mut(s) {
                    *u = true;
                }
                self.current.push(Some(s));
                self.go(k + 1);
                self.current.pop();
                if let Some(u) = self.used.get_mut(s) {
                    *u = false;
                }
            }
            self.current.push(None);
            self.go(k + 1);
            self.current.pop();
        }
    }
    let mut search = Search {
        pitches,
        tuning,
        used: used.to_vec(),
        current: Vec::with_capacity(pitches.len()),
        best: vec![None; pitches.len()],
        best_score: (usize::MAX, u32::MAX, u32::MAX),
        nodes: 0,
    };
    search.go(0);
    search.best
}

/// The selected MIDI or instrument track: the first selected one, else the track of the first
/// selected MIDI clip.
pub fn selected_track(s: &Session) -> Option<TrackId> {
    let midi = |id: &TrackId| s.track(*id).is_some_and(|t| t.kind.is_midi());
    s.tracks
        .iter()
        .find(|t| t.kind.is_midi() && s.edit.selected_tracks.contains(&t.id))
        .map(|t| t.id)
        .or_else(|| {
            s.edit.selected_clips.iter().find_map(|c| s.find_clip(*c).filter(|(_, c)| matches!(c.content, ClipContent::Midi { .. })).map(|(t, _)| t))
        })
        .filter(midi)
}

/// The track the tab view shows: the [`selected_track`], else the first MIDI or instrument track.
pub fn target_track(s: &Session) -> Option<TrackId> {
    selected_track(s).or_else(|| s.tracks.iter().find(|t| t.kind.is_midi()).map(|t| t.id))
}

/// Lay out `track` as tab, from bar 1 through its last note.
pub fn layout(s: &Session, track: TrackId) -> Option<Tab> {
    let t = s.track(track)?;
    if !t.kind.is_midi() {
        return None;
    }
    let tuning = tuning_of(t);
    let notes = score::grid_notes(s, t);
    let last = notes.iter().map(|n| n.end).max().unwrap_or(0);
    let grid = score::measure_grid(s, last);
    Some(Tab { track, name: t.name.clone(), tuning: tuning.clone(), measures: measures(&grid, &notes, &tuning) })
}

fn measures(grid: &[(i64, i64, u32, u32)], notes: &[GridNote], tuning: &[u8]) -> Vec<TabMeasure> {
    let mut onsets: BTreeMap<i64, Vec<(u8, u8)>> = BTreeMap::new();
    for n in notes {
        onsets.entry(n.start).or_default().push((n.pitch, n.channel));
    }
    grid.iter()
        .enumerate()
        .map(|(i, (start, units, numerator, denominator))| {
            let end = start.saturating_add(*units);
            let events = onsets.range(*start..end).map(|(at, chord)| TabEvent { unit: at - start, notes: assign(chord, tuning) }).collect();
            TabMeasure { number: i + 1, start: *start, units: *units, numerator: *numerator, denominator: *denominator, events }
        })
        .collect()
}

/// A note placed on the fretboard at its exact place on the timeline (the Edit window's tab
/// lane). Notes are fingered together with the others starting on the same sixteenth, as in
/// [`layout`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedNote {
    pub clip: ClipId,
    /// Onset in samples (follows the tempo map).
    pub at: Samples,
    pub note: TabNote,
}

/// Every note of `track`'s clips (muted clips too), with its string and fret, sorted by onset.
pub fn timeline(s: &Session, track: &Track) -> Vec<TimedNote> {
    let tuning = tuning_of(track);
    let sr = s.sample_rate;
    // (sixteenth, clip, tick, pitch, channel)
    let mut raw: Vec<(i64, ClipId, i64, u8, u8)> = Vec::new();
    for c in track.clips() {
        let ClipContent::Midi { sequence } = &c.content else { continue };
        let base = s.tempo.samples_to_ticks(c.start, sr);
        let end = s.tempo.samples_to_ticks(c.start.saturating_add(c.length), sr);
        for n in &sequence.notes {
            let tick = base.saturating_add(n.start);
            if tick < base || tick >= end {
                continue;
            }
            raw.push((score::quantize(tick), c.id, tick, n.pitch, n.channel));
        }
    }
    raw.sort_by_key(|r| (r.0, r.2));
    let mut out = Vec::with_capacity(raw.len());
    for group in raw.chunk_by(|a, b| a.0 == b.0) {
        let chord: Vec<(u8, u8)> = group.iter().map(|r| (r.3, r.4)).collect();
        let placed = assign(&chord, &tuning);
        for (_, clip, tick, pitch, channel) in group {
            let found = placed.iter().find(|n| n.pitch == *pitch && n.channel == *channel).copied();
            let note = found.unwrap_or(TabNote { pitch: *pitch, channel: *channel, string: None, fret: None });
            out.push(TimedNote { clip: *clip, at: s.tempo.tick_to_samples(*tick, sr), note });
        }
    }
    out.sort_by_key(|n| n.at);
    out
}

/// Where `at` falls in the tab: the measure's index and the position in sixteenths (fractional)
/// from its start. `None` before bar 1 or after the last measure.
pub fn locate(s: &Session, tab: &Tab, at: Samples) -> Option<(usize, f64)> {
    let unit = s.tempo.samples_to_ticks_f(at.max(0), s.sample_rate) / UNIT as f64;
    if !unit.is_finite() {
        return None;
    }
    let i = tab.measures.partition_point(|m| (m.start as f64) <= unit).checked_sub(1)?;
    let m = tab.measures.get(i)?;
    let offset = unit - m.start as f64;
    (offset < m.units as f64).then_some((i, offset))
}

/// The tab as plain text: one block of string lines per `bars_per_line` measures, each sixteenth
/// three characters wide.
pub fn text(tab: &Tab, bars_per_line: usize) -> String {
    let per = bars_per_line.max(1);
    let labels: Vec<String> = tab.tuning.iter().map(|p| pitch_class(*p).to_string()).collect();
    let width = labels.iter().map(String::len).max().unwrap_or(1);
    let mut out = String::new();
    for chunk in tab.measures.chunks(per) {
        let _ = writeln!(out, "{:width$} {}", "", chunk.first().map_or(0, |m| m.number));
        for (s, label) in labels.iter().enumerate() {
            let string = u8::try_from(s + 1).ok();
            let _ = write!(out, "{label:>width$}|");
            for m in chunk {
                let mut cells = vec![String::from("---"); usize::try_from(m.units).unwrap_or(0).min(64)];
                for e in &m.events {
                    if let Some(n) = e.notes.iter().find(|n| n.string == string)
                        && let (Some(f), Some(cell)) = (n.fret, usize::try_from(e.unit).ok().and_then(|u| cells.get_mut(u)))
                    {
                        *cell = format!("{f:-<3}");
                    }
                }
                out.push_str(&cells.concat());
                out.push('|');
            }
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

/// JSON for the control protocol (`track.guitar_tab`).
pub fn to_json(tab: &Tab) -> Value {
    let measures: Vec<Value> = tab
        .measures
        .iter()
        .map(|m| {
            json!({
                "number": m.number,
                "start_unit": m.start,
                "units": m.units,
                "meter": [m.numerator, m.denominator],
                "events": m.events.iter().map(|e| json!({
                    "unit": e.unit,
                    "notes": e.notes.iter().map(|n| json!({"pitch": n.pitch, "channel": u16::from(n.channel) + 1, "string": n.string, "fret": n.fret})).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "track": tab.track.0,
        "name": tab.name,
        "tuning": tuning_json(&tab.tuning),
        "measures": measures,
        "unplayable": tab.unplayable(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(notes: &[TabNote]) -> Vec<(u8, Option<u8>, Option<u8>)> {
        notes.iter().map(|n| (n.pitch, n.string, n.fret)).collect()
    }

    #[test]
    fn channel_names_the_string() {
        let e = PRESETS[0].strings;
        // Channel 6 (index 5) is the low E string: A2 at fret 5 there, not the open A string.
        assert_eq!(placed(&assign(&[(45, 5)], e)), vec![(45, Some(6), Some(5))]);
        // Channel 1 is the high E string.
        assert_eq!(placed(&assign(&[(67, 0)], e)), vec![(67, Some(1), Some(3))]);
        // A power chord on channels 6, 5, 4.
        assert_eq!(placed(&assign(&[(40, 5), (47, 4), (52, 3)], e)), vec![(52, Some(4), Some(2)), (47, Some(5), Some(2)), (40, Some(6), Some(0))]);
    }

    #[test]
    fn falls_back_to_the_lowest_position() {
        let e = PRESETS[0].strings;
        // A2 on channel 1 would be fret -19: the open A string instead.
        assert_eq!(placed(&assign(&[(45, 0)], e)), vec![(45, Some(5), Some(0))]);
        // Channel 10 names no string.
        assert_eq!(placed(&assign(&[(64, 9)], e)), vec![(64, Some(1), Some(0))]);
        // B5 (83) on channel 6 would be fret 43: the 19th fret of string 1.
        assert_eq!(placed(&assign(&[(83, 5)], e)), vec![(83, Some(1), Some(19))]);
    }

    #[test]
    fn fallback_never_doubles_a_string() {
        let e = PRESETS[0].strings;
        // B3 and C4 together: both lowest on string 2, so C4 moves to string 3 (or B3 does).
        let notes = assign(&[(59, 0), (60, 0)], e);
        let strings: Vec<_> = notes.iter().map(|n| n.string).collect();
        assert!(strings.iter().all(Option::is_some));
        assert_ne!(strings[0], strings[1]);
        // A low-E power chord all on channel 1: E2 B2 E3 on the bottom three strings.
        assert_eq!(placed(&assign(&[(40, 0), (47, 0), (52, 0)], e)), vec![(52, Some(4), Some(2)), (47, Some(5), Some(2)), (40, Some(6), Some(0))]);
        // A channel-mapped note keeps its string; the fallback note works around it.
        let n = assign(&[(45, 5), (45 + 5, 5)], e);
        assert_eq!(n.iter().filter(|n| n.string == Some(6)).count(), 1);
    }

    #[test]
    fn unisons_on_two_channels_take_two_strings() {
        let e = PRESETS[0].strings;
        // E4 on string 1 open and on string 2 at fret 5 (channel N = string N).
        let n = assign(&[(64, 0), (64, 1)], e);
        assert_eq!(
            n.iter().map(|n| (n.pitch, n.channel, n.string, n.fret)).collect::<Vec<_>>(),
            vec![(64, 0, Some(1), Some(0)), (64, 1, Some(2), Some(5))]
        );
        // The same note twice on one channel is one note.
        assert_eq!(assign(&[(64, 0), (64, 0)], e).len(), 1);
    }

    #[test]
    fn unplayable_notes_are_marked() {
        let e = PRESETS[0].strings;
        // Below the low E, and above fret 24 on the high string.
        assert_eq!(placed(&assign(&[(30, 0), (100, 0)], e)), vec![(100, None, None), (30, None, None)]);
        // Seven notes on six strings: one is left over.
        let chord: Vec<(u8, u8)> = [40, 45, 50, 55, 59, 64, 69].iter().map(|p| (*p, 0)).collect();
        assert_eq!(assign(&chord, e).iter().filter(|n| n.string.is_none()).count(), 1);
    }

    #[test]
    fn tunings_shift_frets() {
        let b = &parse_tuning(&json!("b_standard")).unwrap_or_default();
        assert_eq!(b.as_slice(), &[59, 54, 50, 45, 40, 35]);
        // B1 is the open low string in B standard and fret 2 of the low string in Drop A.
        assert_eq!(placed(&assign(&[(35, 5)], b)), vec![(35, Some(6), Some(0))]);
        let drop_a = parse_tuning(&json!("Drop A")).unwrap_or_default();
        assert_eq!(placed(&assign(&[(35, 5)], &drop_a)), vec![(35, Some(6), Some(2))]);
        let drop_d = parse_tuning(&json!("drop-d")).unwrap_or_default();
        assert_eq!(placed(&assign(&[(38, 0)], &drop_d)), vec![(38, Some(6), Some(0))]);
        // E standard cannot reach D2.
        assert_eq!(placed(&assign(&[(38, 0)], DEFAULT_TUNING)), vec![(38, None, None)]);
    }

    #[test]
    fn parses_custom_tunings() {
        assert_eq!(parse_tuning(&json!("E2 A2 D3 G3 B3 E4")), Ok(DEFAULT_TUNING.to_vec()));
        assert_eq!(parse_tuning(&json!([40, 45, 50, 55, 59, 64])), Ok(DEFAULT_TUNING.to_vec()));
        assert_eq!(parse_tuning(&json!(["B1", "E2", "A2", "D3", "G3", "B3", "E4"])), Ok(PRESETS[6].strings.to_vec()));
        assert_eq!(parse_tuning(&json!("Bb1 Eb2 Ab2 Db3")), Ok(vec![49, 44, 39, 34]));
        assert!(parse_tuning(&json!([40, 45])).is_err());
        assert!(parse_tuning(&json!([400])).is_err());
        assert!(parse_tuning(&json!("H2 A2 D3 G3")).is_err());
        assert!(parse_tuning(&json!(vec![40; 100])).is_err());
        assert!(parse_tuning(&json!({})).is_err());
        assert_eq!(pitch_name(40), "E2");
        assert_eq!(parse_pitch("C-1"), Some(0));
        assert_eq!(parse_pitch("G9"), Some(127));
        assert_eq!(parse_pitch("G#9"), None);
    }

    #[test]
    fn hostile_chords_stay_bounded() {
        let chord: Vec<(u8, u8)> = (0..=127).map(|p| (p, p % 16)).collect();
        let notes = assign(&chord, PRESETS[7].strings);
        assert_eq!(notes.len(), 128);
        let mut strings: Vec<u8> = notes.iter().filter_map(|n| n.string).collect();
        let placed = strings.len();
        strings.sort_unstable();
        strings.dedup();
        assert_eq!(strings.len(), placed, "no string twice");
        assert!(placed <= 7);
    }

    #[test]
    fn layout_groups_onsets_into_measures() {
        let grid = vec![(0, 16, 4, 4), (16, 16, 4, 4)];
        let n = |start, pitch, channel| GridNote { start, end: start + 2, pitch, channel };
        let m = measures(&grid, &[n(0, 40, 5), n(0, 47, 4), n(4, 45, 0), n(18, 64, 0)], DEFAULT_TUNING);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].events.len(), 2);
        assert_eq!(m[0].events[0].unit, 0);
        assert_eq!(m[0].events[0].notes.len(), 2);
        assert_eq!(m[0].events[1].unit, 4);
        assert_eq!(m[1].number, 2);
        assert_eq!(m[1].events[0].unit, 2);
        let tab = Tab { track: TrackId(1), name: "Gtr".into(), tuning: DEFAULT_TUNING.to_vec(), measures: m };
        let text = text(&tab, 4);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[1], format!("E|{}|------0--{}|", "-".repeat(48), "-".repeat(39)));
        assert_eq!(lines[5], format!("A|2-----------0--{}|{}|", "-".repeat(33), "-".repeat(48)));
        assert!(lines[6].starts_with("E|0--"));
    }

    #[test]
    fn timeline_matches_the_tab_and_follows_tempo() {
        let mut e = crate::demo::demo_engine();
        let Some(track) = target_track(e.session()) else { return };
        // A tempo change must move the notes' timeline positions, not their fingering.
        let s = e.session();
        let t = s.track(track).map(|t| timeline(s, t)).unwrap_or_default();
        assert!(!t.is_empty());
        assert!(t.windows(2).all(|w| w[0].at <= w[1].at));
        let tab =
            layout(s, track).map(|t| t.measures.iter().flat_map(|m| &m.events).flat_map(|e| e.notes.clone()).collect::<Vec<_>>()).unwrap_or_default();
        for n in &t {
            assert!(tab.iter().any(|m| m.pitch == n.note.pitch && m.string == n.note.string && m.fret == n.note.fret), "{n:?}");
        }
        // Half the tempo from the start: notes spread out twice as far on the timeline (the clip,
        // fixed in samples, then holds fewer of them), with the same fingering.
        let bpm = s.tempo.tempo_at_tick(0);
        let gap = |t: &[TimedNote]| t.iter().map(|n| n.at).find(|a| *a > t[0].at).unwrap_or(0) - t[0].at;
        let before = gap(&t);
        assert!(e.execute("event.tempo", &json!({"bpm": bpm / 2.0, "at": 0})).is_ok());
        let s = e.session();
        let after = s.track(track).map(|t| timeline(s, t)).unwrap_or_default();
        assert!(!after.is_empty() && after.len() <= t.len());
        assert!(after.iter().zip(&t).all(|(a, b)| a.note == b.note));
        assert!((gap(&after) - 2 * before).abs() <= 2, "{} vs {}", gap(&after), before);
    }

    #[test]
    fn demo_midi_tracks_lay_out_and_locate() {
        let e = crate::demo::demo_engine();
        let s = e.session();
        let track = target_track(s).unwrap_or(TrackId(0));
        let tab = layout(s, track);
        assert!(tab.as_ref().is_some_and(|t| !t.measures.is_empty()));
        let tab = tab.unwrap_or(Tab { track, name: String::new(), tuning: Vec::new(), measures: Vec::new() });
        assert_eq!(locate(s, &tab, 0), Some((0, 0.0)));
        let bar2 = s.tempo.tick_to_samples(s.tempo.bar_start_tick(2), s.sample_rate) + 1;
        if tab.measures.len() > 1 {
            assert_eq!(locate(s, &tab, bar2).map(|(i, _)| i), Some(1));
        }
        assert_eq!(to_json(&tab)["measures"].as_array().map(Vec::len), Some(tab.measures.len()));
    }
}
