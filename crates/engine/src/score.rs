//! Notation: MIDI tracks laid out as measures of notes and rests, written as MusicXML
//! (File › Export › Sibelius, File › Send To › Sibelius) or engraved to SVG (File › Print Score).
//!
//! Notes are quantized to sixteenths. Each part is a sequence of chords split at note onsets,
//! note ends and bar lines. Per-pitch ties preserve the duration of each note, including
//! overlapping notes. A chord keeps at most [`MAX_PITCHES_PER_CHORD`] pitches.

pub use craft_score::{Item, Measure, Part};
use soundcraft_model::{ClipContent, Session, TrackId, TrackKind};
use soundcraft_time::TICKS_PER_QUARTER;

/// Ticks per score unit (a sixteenth).
const UNIT: i64 = TICKS_PER_QUARTER / 4;
/// Upper bound on engraved measures, so a hostile session cannot make a huge document.
const MAX_MEASURES: usize = 2000;
/// Upper bound on output size, so a hostile session cannot make a huge document.
#[cfg(test)]
const MAX_OUTPUT_BYTES: usize = 256 * 1024 * 1024; // 256 MiB
/// Upper bound on items per measure, so a hostile session cannot make a huge document.
const MAX_ITEMS_PER_MEASURE: usize = 128;
/// Upper bound on pitches per chord, so a hostile session cannot make a huge document.
const MAX_PITCHES_PER_CHORD: usize = 16;
/// Upper bound on notes collected per part, before `fill` builds onsets and spans, so a hostile
/// session cannot make `layout` accumulate an unbounded note vector.
const MAX_NOTES_PER_PART: usize = 100_000;
/// Note values a single notehead can show, in sixteenths (longest first).
const VALUES: [u32; 8] = [16, 12, 8, 6, 4, 3, 2, 1];

/// Score Setup (File › Score Setup…). Stored in the session's edit flags/values.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreSetup {
    pub title: String,
    pub composer: String,
    pub bars_per_system: u32,
    pub show_track_names: bool,
}

const TITLE_KEY: &str = "score.title=";
const COMPOSER_KEY: &str = "score.composer=";

impl ScoreSetup {
    pub fn from_session(s: &Session) -> Self {
        let text = |key: &str| s.edit.flags.iter().find_map(|f| f.strip_prefix(key)).map(str::to_string);
        let bars = s.edit.value("score.bars_per_system", 4.0);
        Self {
            title: text(TITLE_KEY).unwrap_or_else(|| s.name.clone()),
            composer: text(COMPOSER_KEY).unwrap_or_default(),
            bars_per_system: if bars.is_finite() { bars.clamp(1.0, 16.0) as u32 } else { 4 },
            show_track_names: !s.edit.flag("score.hide_names"),
        }
    }

    pub fn store(&self, s: &mut Session) {
        s.edit.flags.retain(|f| !f.starts_with(TITLE_KEY) && !f.starts_with(COMPOSER_KEY));
        let clean = |t: &str| t.chars().filter(|c| !c.is_control()).take(200).collect::<String>();
        s.edit.flags.insert(format!("{TITLE_KEY}{}", clean(&self.title)));
        s.edit.flags.insert(format!("{COMPOSER_KEY}{}", clean(&self.composer)));
        s.edit.values.insert("score.bars_per_system".into(), f64::from(self.bars_per_system.clamp(1, 16)));
        s.edit.set_flag("score.hide_names", !self.show_track_names);
    }
}

/// Lay out the MIDI (and instrument) tracks `tracks` (all of them when empty).
pub fn layout(s: &Session, tracks: &[TrackId]) -> Vec<Part> {
    let sr = s.sample_rate;
    let mut raw: Vec<(String, Vec<(i64, i64, u8)>)> = Vec::new();
    for t in &s.tracks {
        if !matches!(t.kind, TrackKind::Midi | TrackKind::Instrument) || (!tracks.is_empty() && !tracks.contains(&t.id)) {
            continue;
        }
        let mut notes = Vec::new();
        for c in t.clips() {
            let ClipContent::Midi { sequence } = &c.content else { continue };
            if c.muted {
                continue;
            }
            let base = s.tempo.samples_to_ticks(c.start, sr);
            let end = s.tempo.samples_to_ticks(c.start.saturating_add(c.length), sr);
            for n in &sequence.notes {
                if notes.len() >= MAX_NOTES_PER_PART {
                    break;
                }
                let st = base.saturating_add(n.start);
                if st < base || st >= end || n.length <= 0 {
                    continue;
                }
                let a = quantize(st);
                let b = quantize(st.saturating_add(n.length).min(end)).max(a + 1);
                notes.push((a, b, n.pitch));
            }
        }
        raw.push((t.name.clone(), notes));
    }
    let last = raw.iter().flat_map(|(_, n)| n.iter().map(|x| x.1)).max().unwrap_or(0);
    let bars = measure_grid(s, last);
    raw.into_iter()
        .map(|(name, notes)| {
            let avg = if notes.is_empty() { 60.0 } else { notes.iter().map(|n| f64::from(n.2)).sum::<f64>() / notes.len() as f64 };
            Part { name, bass: avg < 57.0, measures: fill(&bars, &notes) }
        })
        .collect()
}

fn quantize(tick: i64) -> i64 {
    (tick.max(0) + UNIT / 2) / UNIT
}

/// (start unit, length in units, numerator, denominator) per measure, covering `end_unit`.
fn measure_grid(s: &Session, end_unit: i64) -> Vec<(i64, i64, u32, u32)> {
    let mut v = Vec::new();
    let mut bar = 1;
    loop {
        let a = s.tempo.bar_start_tick(bar);
        let b = s.tempo.bar_start_tick(bar + 1);
        let m = s.tempo.meter_at_tick(a);
        if b <= a {
            break;
        }
        let (ua, ub) = (a / UNIT, b / UNIT);
        bar += 1;
        if ub <= ua {
            // A bar shorter than a sixteenth that does not reach the next unit: the following
            // bar carries this span, so the grid stays contiguous.
            continue;
        }
        // Bars shorter than a sixteenth are written as one measure of the units they cover, so
        // the time signature matches the measure's content.
        let (num, den) = if m.ticks_per_bar() < UNIT { (u32::try_from(ub - ua).unwrap_or(1), 16) } else { (m.numerator, m.denominator) };
        v.push((ua, ub - ua, num, den));
        if ub >= end_unit.max(1) || v.len() >= MAX_MEASURES {
            break;
        }
    }
    v
}

/// Split `units` into note values that a single notehead can show.
fn split_units(mut units: i64) -> Vec<u32> {
    let mut out = Vec::new();
    while units > 0 {
        let v = VALUES.iter().copied().find(|v| i64::from(*v) <= units).unwrap_or(1);
        out.push(v);
        units -= i64::from(v);
    }
    out
}

fn fill(bars: &[(i64, i64, u32, u32)], notes: &[(i64, i64, u8)]) -> Vec<Measure> {
    #[derive(Default)]
    struct Events {
        starts: Vec<(usize, u8)>,
        ends: Vec<(usize, u8)>,
    }
    let limit = bars.last().map_or(0, |(start, length, _, _)| start.saturating_add(*length));
    let mut events: std::collections::BTreeMap<i64, Events> = std::collections::BTreeMap::new();
    for (id, &(a, b, pitch)) in notes.iter().enumerate() {
        if a >= b || a >= limit || b <= 0 || pitch > 127 {
            continue;
        }
        events.entry(a.max(0)).or_default().starts.push((id, pitch));
        events.entry(b.min(limit)).or_default().ends.push((id, pitch));
    }
    let events: Vec<_> = events.into_iter().collect();
    // Only admitted notes may end activity; rejected starts must not silence later notes.
    let mut admitted = std::collections::BTreeSet::new();
    let mut active = [0usize; 128];
    let mut spans = Vec::new();
    for pair in events.windows(2) {
        let [(from, ev), (to, next)] = pair else { continue };
        for (id, pitch) in &ev.ends {
            if !admitted.remove(id) {
                continue;
            }
            if let Some(count) = active.get_mut(usize::from(*pitch)) {
                *count = count.saturating_sub(1);
            }
        }
        let tied_in: Vec<u8> = active
            .iter()
            .enumerate()
            .filter(|(p, n)| **n > 0 && !ev.starts.iter().any(|(_, pitch)| *pitch == *p as u8))
            .map(|(p, _)| p as u8)
            .collect();
        for (id, pitch) in &ev.starts {
            let idx = usize::from(*pitch);
            if active[idx] == 0 && active.iter().filter(|n| **n > 0).count() >= MAX_PITCHES_PER_CHORD {
                continue;
            }
            admitted.insert(*id);
            active[idx] = active[idx].saturating_add(1);
        }
        let pitches: Vec<u8> = active.iter().enumerate().filter(|(_, n)| **n > 0).map(|(p, _)| p as u8).collect();
        let mut ending = [0usize; 128];
        for (id, pitch) in &next.ends {
            if !admitted.contains(id) {
                continue;
            }
            if let Some(count) = ending.get_mut(usize::from(*pitch)) {
                *count = count.saturating_add(1);
            }
        }
        let tied_out: Vec<u8> = pitches
            .iter()
            .copied()
            .filter(|p| {
                active.get(usize::from(*p)).zip(ending.get(usize::from(*p))).is_some_and(|(a, b)| a > b)
                    && !next.starts.iter().any(|(_, pitch)| pitch == p)
            })
            .collect();
        if !pitches.is_empty() {
            spans.push((*from, *to, pitches, tied_in, tied_out));
        }
    }
    let mut measures = Vec::new();
    for (ms, ml, num, den) in bars {
        let me = ms + ml;
        let mut items = Vec::new();
        let mut cursor = *ms;
        let emit = |from: i64, to: i64, pitches: &[u8], tied_in: &[u8], tied_out: &[u8], items: &mut Vec<Item>| {
            let parts = split_units(to - from);
            let n = parts.len();
            for (k, u) in parts.into_iter().enumerate() {
                items.push(Item {
                    pitches: pitches.to_vec(),
                    units: u,
                    tie_stop: pitches.iter().copied().filter(|p| k > 0 || tied_in.contains(p)).collect(),
                    tie_start: pitches.iter().copied().filter(|p| k + 1 < n || tied_out.contains(p)).collect(),
                });
            }
        };
        for (a, b, ps, tied_in, tied_out) in spans.iter().filter(|(a, b, ..)| *b > *ms && *a < me) {
            let from = (*a).max(*ms).max(cursor);
            let to = (*b).min(me);
            if to <= from {
                continue;
            }
            if from > cursor {
                emit(cursor, from, &[], &[], &[], &mut items);
            }
            emit(from, to, ps, if from > *a { ps } else { tied_in }, if to < *b { ps } else { tied_out }, &mut items);
            cursor = to;
            if items.len() > MAX_ITEMS_PER_MEASURE {
                break;
            }
        }
        if cursor < me && items.len() <= MAX_ITEMS_PER_MEASURE {
            emit(cursor, me, &[], &[], &[], &mut items);
        }
        if items.len() > MAX_ITEMS_PER_MEASURE {
            let bar_len = me.saturating_sub(*ms);
            let mut keep = MAX_ITEMS_PER_MEASURE;
            loop {
                let kept: i64 = items.iter().take(keep).map(|i| i64::from(i.units)).sum();
                let rests = split_units(bar_len.saturating_sub(kept));
                if keep.saturating_add(rests.len()) <= MAX_ITEMS_PER_MEASURE || keep == 0 {
                    items.truncate(keep);
                    items.extend(rests.into_iter().map(|units| Item { pitches: Vec::new(), units, tie_start: Vec::new(), tie_stop: Vec::new() }));
                    break;
                }
                keep -= 1;
            }
            items.truncate(MAX_ITEMS_PER_MEASURE);
        }
        measures.push(Measure { numerator: *num, denominator: *den, items });
    }
    measures
}

/// (step, alter, octave) spelled with sharps.
fn shared_setup(setup: &ScoreSetup) -> craft_score::ScoreSetup {
    craft_score::ScoreSetup {
        software: "SoundCraft".into(),
        title: setup.title.clone(),
        composer: setup.composer.clone(),
        bars_per_system: setup.bars_per_system,
        show_track_names: setup.show_track_names,
    }
}

/// MusicXML 4.0 (partwise), written by the shared score serializer.
pub fn musicxml(parts: &[Part], setup: &ScoreSetup) -> Result<String, String> {
    craft_score::musicxml(parts, &shared_setup(setup))
}

/// A printable SVG page, written by the shared score serializer.
pub fn svg(parts: &[Part], setup: &ScoreSetup) -> Result<String, String> {
    craft_score::svg(parts, &shared_setup(setup))
}

#[cfg(test)]
fn musicxml_budgeted(parts: &[Part], setup: &ScoreSetup, budget: usize) -> Result<String, String> {
    craft_score::musicxml_budgeted(parts, &shared_setup(setup), budget)
}

#[cfg(test)]
fn svg_budgeted(parts: &[Part], setup: &ScoreSetup, budget: usize) -> Result<String, String> {
    craft_score::svg_budgeted(parts, &shared_setup(setup), budget)
}

#[cfg(test)]
mod tests {
    use super::*;

    proptest::proptest! {
        #[test]
        fn score_preserves_pitch_activity(notes in proptest::collection::vec((0i64..32, 1i64..32, 48u8..64), 0..20)) {
            let notes: Vec<_> = notes.into_iter().map(|(a, length, pitch)| (a, a + length, pitch)).collect();
            let measures = fill(&[(0, 16, 4, 4), (16, 16, 4, 4), (32, 16, 4, 4), (48, 16, 4, 4)], &notes);
            let mut at = 0i64;
            for m in measures {
                proptest::prop_assert_eq!(m.items.iter().map(|i| i.units).sum::<u32>(), 16);
                for item in m.items {
                    for tick in at..at + i64::from(item.units) {
                        let expected: Vec<u8> = (48..64).filter(|pitch| notes.iter().any(|(a, b, p)| p == pitch && *a <= tick && tick < *b)).collect();
                        proptest::prop_assert_eq!(&item.pitches, &expected);
                    }
                    let end = at + i64::from(item.units);
                    let tied_in: Vec<_> = item.pitches.iter().copied().filter(|p| {
                        notes.iter().any(|(a, b, pitch)| pitch == p && *a < at && at < *b)
                            && !notes.iter().any(|(a, _, pitch)| pitch == p && *a == at)
                    }).collect();
                    let tied_out: Vec<_> = item.pitches.iter().copied().filter(|p| {
                        end < 64 && notes.iter().any(|(a, b, pitch)| pitch == p && *a < end && end < *b)
                            && !notes.iter().any(|(a, _, pitch)| pitch == p && *a == end)
                    }).collect();
                    proptest::prop_assert_eq!(item.tie_stop, tied_in);
                    proptest::prop_assert_eq!(item.tie_start, tied_out);
                    at = end;
                }
            }
            proptest::prop_assert_eq!(at, 64);
        }
    }

    #[test]
    fn rejected_note_end_does_not_silence_later_admitted_pitch() {
        let mut notes: Vec<_> = (48..64).map(|pitch| (0, 30, pitch)).collect();
        notes.extend([(10, 50, 64), (30, 80, 64)]);
        let measures = fill(&[(0, 80, 20, 4)], &notes);
        let mut at = 0;
        for item in &measures[0].items {
            let end = at + i64::from(item.units);
            for tick in at..end {
                assert_eq!(item.pitches.contains(&64), (30..80).contains(&tick));
            }
            if item.pitches.contains(&64) {
                assert_eq!(item.tie_stop.contains(&64), at > 30);
                assert_eq!(item.tie_start.contains(&64), end < 80);
            }
            at = end;
        }
        assert_eq!(at, 80);
    }

    #[test]
    fn unequal_chord_durations_are_preserved_in_layout_and_exports() {
        let measures = fill(&[(0, 16, 4, 4)], &[(0, 4, 60), (0, 16, 64)]);
        let items = &measures[0].items;
        assert_eq!(items.iter().map(|i| i.units).sum::<u32>(), 16);
        assert_eq!(items.iter().filter(|i| i.pitches.contains(&60)).map(|i| i.units).sum::<u32>(), 4);
        assert_eq!(items.iter().filter(|i| i.pitches.contains(&64)).map(|i| i.units).sum::<u32>(), 16);
        assert_eq!(items[0].tie_start, vec![64]);
        assert_eq!(items[1].tie_stop, vec![64]);
        let parts = vec![Part { name: "Keys".into(), bass: false, measures }];
        let setup = ScoreSetup::from_session(&Session::default());
        let xml = musicxml(&parts, &setup).unwrap();
        let notes: Vec<_> = xml.split("<note>").skip(1).map(|n| n.split("</note>").next().unwrap()).collect();
        assert!(notes[0].contains("<step>C</step>") && notes[0].contains("<duration>4</duration>"));
        assert!(!notes[0].contains("<tie "));
        assert!(notes[1].contains("<tie type=\"start\"/>") && notes[1].contains("<chord/>"));
        assert!(notes[2].contains("<duration>12</duration>") && notes[2].contains("<tie type=\"stop\"/>"));
        assert_eq!(svg(&parts, &setup).unwrap().matches("stroke-width=\"0.9\"").count(), 1);
    }

    #[test]
    fn overlapping_notes_continue_and_repeated_notes_are_not_tied() {
        let m = fill(&[(0, 16, 4, 4), (16, 16, 4, 4)], &[(0, 20, 60), (4, 8, 64), (20, 24, 60)]);
        let items: Vec<_> = m.iter().flat_map(|m| &m.items).collect();
        assert_eq!(items.iter().filter(|i| i.pitches.contains(&60)).map(|i| i.units).sum::<u32>(), 24);
        assert_eq!(items.iter().filter(|i| i.pitches.contains(&64)).map(|i| i.units).sum::<u32>(), 4);
        assert_eq!(m[1].items[0].tie_stop, vec![60]);
        assert!(m[1].items[0].tie_start.is_empty());
        assert!(m[1].items[1].tie_stop.is_empty());
    }

    #[test]
    fn split_units_uses_representable_values() {
        assert_eq!(split_units(16), vec![16]);
        assert_eq!(split_units(5), vec![4, 1]);
        assert_eq!(split_units(7), vec![6, 1]);
        assert!(split_units(0).is_empty());
    }

    #[test]
    fn measures_fill_exactly_and_tie_across_bars() {
        // 4/4: one bar = 16 units. A note from unit 12 to 20 crosses the bar line.
        let bars = vec![(0, 16, 4, 4), (16, 16, 4, 4)];
        let m = fill(&bars, &[(0, 4, 60), (0, 4, 64), (12, 20, 67)]);
        assert_eq!(m.len(), 2);
        for meas in &m {
            assert_eq!(meas.items.iter().map(|i| i.units).sum::<u32>(), 16);
        }
        assert_eq!(m[0].items[0].pitches, vec![60, 64]);
        assert!(m[0].items.last().is_some_and(|i| !i.tie_start.is_empty()));
        assert_eq!(m[1].items[0].tie_stop, vec![67]);
    }

    #[test]
    fn demo_exports_well_formed_documents() {
        let e = crate::demo::demo_engine();
        let parts = layout(e.session(), &[]);
        assert!(!parts.is_empty(), "demo has MIDI tracks");
        let setup = ScoreSetup::from_session(e.session());
        let x = musicxml(&parts, &setup).expect("demo musicxml fits");
        assert!(x.contains("<score-partwise") && x.contains("<pitch>") && x.trim_end().ends_with("</score-partwise>"));
        assert_eq!(x.matches("<measure ").count(), x.matches("</measure>").count());
        let s = svg(&parts, &setup).expect("demo svg fits");
        assert!(s.starts_with("<svg") && s.contains("<ellipse"));
    }

    /// Note onsets (pitch count of every chord that is not a tie continuation).
    fn note_starts(parts: &[Part]) -> usize {
        parts.iter().flat_map(|p| &p.measures).flat_map(|m| &m.items).filter(|i| i.tie_stop.is_empty()).map(|i| i.pitches.len()).sum()
    }

    #[test]
    fn bars_shorter_than_a_sixteenth_keep_every_note() {
        let base = crate::demo::demo_engine();
        let baseline = note_starts(&layout(base.session(), &[]));
        assert!(baseline > 0, "demo has notes");
        for (n, d) in [(1, 32), (1, 64), (2, 64), (3, 64)] {
            for mid in [false, true] {
                let mut e = crate::demo::demo_engine();
                let tick = if mid { e.session().tempo.bar_start_tick(5) } else { 0 };
                e.session_mut().tempo.set_meter(tick, n, d).expect("valid meter");
                let parts = layout(e.session(), &[]);
                assert!(parts.iter().all(|p| !p.measures.is_empty()), "{n}/{d} mid={mid}: empty part");
                assert_eq!(note_starts(&parts), baseline, "{n}/{d} mid={mid}: notes lost");
                for p in &parts {
                    for m in &p.measures {
                        let units: u32 = m.items.iter().map(|i| i.units).sum();
                        // The written meter matches the measure's content (divisions are sixteenths).
                        assert_eq!(units * m.denominator, 16 * m.numerator, "{n}/{d} mid={mid}: meter/content mismatch");
                    }
                }
                let setup = ScoreSetup::from_session(e.session());
                let x = musicxml(&parts, &setup).expect("musicxml fits");
                assert!(x.contains("<pitch>"), "{n}/{d} mid={mid}: musicxml has no notes");
                let s = svg(&parts, &setup).expect("svg fits");
                assert!(s.contains("<ellipse"), "{n}/{d} mid={mid}: svg has no notes");
            }
        }
    }

    #[test]
    fn bars_of_a_sixteenth_a_32nd_and_a_64th_in_a_row_keep_a_contiguous_grid() {
        let base = crate::demo::demo_engine();
        let baseline = note_starts(&layout(base.session(), &[]));
        let mut e = crate::demo::demo_engine();
        // Bars 5 to 8 are 1/16, 1/32, 1/64 and 3/64 long, then 4/4 resumes.
        for (bar, (n, d)) in (5..).zip([(1, 16), (1, 32), (1, 64), (3, 64), (4, 4)]) {
            let tick = e.session().tempo.bar_start_tick(bar);
            e.session_mut().tempo.set_meter(tick, n, d).expect("valid meter");
        }
        let grid = measure_grid(e.session(), i64::MAX);
        assert_eq!(grid.first().map(|m| m.0), Some(0));
        for w in grid.windows(2) {
            assert_eq!(w[0].0 + w[0].1, w[1].0, "measures leave no gap or overlap: {w:?}");
        }
        assert_eq!(note_starts(&layout(e.session(), &[])), baseline, "notes lost");
    }

    #[test]
    fn output_is_bounded_by_byte_budget() {
        // A part with many notes must not produce an unbounded document.
        let bars: Vec<_> = (0..100).map(|i| (i * 16, 16, 4, 4)).collect();
        let mut notes = Vec::new();
        for i in 0..2000 {
            notes.push((i * 2, i * 2 + 1, 60 + (i % 24) as u8));
        }
        let measures = fill(&bars, &notes);
        let part = Part { name: "dense".into(), bass: false, measures };
        let setup = ScoreSetup { title: "t".into(), composer: String::new(), bars_per_system: 4, show_track_names: false };
        let s = svg(&[part], &setup);
        assert!(s.is_ok(), "dense but bounded score fits");
        let s = s.unwrap();
        assert!(s.len() <= MAX_OUTPUT_BYTES);
    }

    #[test]
    fn oversize_score_is_refused_not_aborted() {
        // Many dense parts drive the output past the byte budget: the call must return an error,
        // not grow a multi-GB string or abort. Use the budgeted entry points with a tiny budget so
        // the test exercises the over-budget path without allocating 256 MiB of score, and covers
        // both the per-measure checks and the header/staff-only growth.
        let bars: Vec<_> = (0..100).map(|i| (i * 16, 16, 4, 4)).collect();
        let mut notes = Vec::new();
        for i in 0..2000 {
            let onset = i * 2;
            notes.push((onset, onset + 1, 60 + (i % 24) as u8));
        }
        let measures = fill(&bars, &notes);
        let part = Part { name: "dense".into(), bass: false, measures };
        let setup = ScoreSetup { title: "t".into(), composer: String::new(), bars_per_system: 1, show_track_names: true };
        // Enough copies of the dense part to exceed a tiny budget.
        let parts: Vec<Part> = (0..64).map(|i| Part { name: format!("part{i}"), ..part.clone() }).collect();
        let tiny = 1024;
        let err = svg_budgeted(&parts, &setup, tiny).unwrap_err();
        assert!(err.contains("score too large"), "{err}");

        // MusicXML needs a budget that clears the header/part-list but trips in the measures, so
        // the per-measure check is what refuses the document (this test used to fail on the
        // part-list check alone).
        let boundless = usize::MAX;
        // The header plus part-list exactly as `parse` writes it, but with no measures: that is
        // the size a budget must clear before the per-measure check is reached.
        let header_parts: Vec<Part> = parts.iter().map(|p| Part { name: p.name.clone(), bass: p.bass, measures: Vec::new() }).collect();
        let header = musicxml_budgeted(&header_parts, &setup, boundless).map_or(0, |x| x.len());
        let full = musicxml_budgeted(&parts, &setup, boundless).map_or(0, |x| x.len());
        assert!(header > 0, "the bare header fits any budget");
        assert!(full > header, "the measures add output ({full} vs {header})");
        // A budget above the bare header but below the completed document must be refused by the
        // per-measure check.
        let between = header.saturating_add((full.saturating_sub(header)) / 2);
        assert!(between > header && between < full, "budget sits between header and document");
        // The header alone fits that budget, so the refusal below comes from the measures.
        assert!(musicxml_budgeted(&header_parts, &setup, between).is_ok(), "header fits the mid budget");
        let err = musicxml_budgeted(&parts, &setup, between).unwrap_err();
        assert!(err.contains("score too large"), "{err}");
        // And a header that does not fit is refused before any measure is built.
        let err = musicxml_budgeted(&parts, &setup, tiny).unwrap_err();
        assert!(err.contains("score too large"), "{err}");
    }

    #[test]
    fn item_cap_keeps_the_measure_duration() {
        // A 64/1 bar (256 sixteenths) with one note per sixteenth needs 256 items: over the cap.
        // What the cap discards must come back as rests, so the bar still fills completely and the
        // SVG layout denominator stays right.
        let bars = vec![(0i64, 256i64, 64u32, 4u32)];
        let notes: Vec<(i64, i64, u8)> = (0..256).map(|i| (i, i + 1, 60)).collect();
        let measures = fill(&bars, &notes);
        assert_eq!(measures.len(), 1);
        let m = &measures[0];
        assert!(m.items.len() <= MAX_ITEMS_PER_MEASURE, "capped at {MAX_ITEMS_PER_MEASURE}");
        let total: u32 = m.items.iter().map(|i| i.units).sum();
        assert_eq!(total, 256, "the bar still lasts 256 sixteenths");
        assert!(m.items.iter().any(|i| i.pitches.is_empty()), "the discarded suffix became rests");
    }

    #[test]
    fn bars_per_system_is_held_to_the_supported_range() {
        let setup = ScoreSetup { title: "t".into(), composer: String::new(), bars_per_system: u32::MAX, show_track_names: false };
        let part = Part { name: "p".into(), bass: false, measures: vec![Measure { numerator: 4, denominator: 4, items: Vec::new() }] };
        let out = svg_budgeted(std::slice::from_ref(&part), &setup, 1 << 20).unwrap();
        assert_eq!(out.matches("stroke-width=\"1\"").count(), 16, "sixteen barlines, not billions");
    }

    #[test]
    fn oversized_title_is_refused_by_the_header_check() {
        // An oversized part name/title must be caught by the header check even with no measures:
        // the escaped copy alone exceeds the budget. (A 54 MiB title blows past 64 KiB of escaped
        // text without touching the per-measure loop.)
        let setup = ScoreSetup { title: "&".repeat(64 * 1024), composer: "c".repeat(64 * 1024), bars_per_system: 4, show_track_names: false };
        let err = musicxml_budgeted(&[], &setup, 1024).unwrap_err();
        assert!(err.contains("score too large"), "{err}");
        let err = svg_budgeted(&[], &setup, 1024).unwrap_err();
        assert!(err.contains("score too large"), "{err}");
    }

    #[test]
    fn exports_drop_characters_xml_forbids() {
        let e = crate::demo::demo_engine();
        let mut parts = layout(e.session(), &[]);
        parts[0].name = "Lead\u{1}1\u{FFFE}".to_string();
        let mut setup = ScoreSetup::from_session(e.session());
        setup.title = "T\u{2}itle".to_string();
        setup.composer = "C\u{3}omp".to_string();
        for out in [musicxml(&parts, &setup).expect("musicxml fits"), svg(&parts, &setup).expect("svg fits")] {
            let bad: Vec<char> =
                out.chars().filter(|c| (*c < ' ' && !matches!(c, '\n' | '\t' | '\r')) || matches!(c, '\u{FFFE}' | '\u{FFFF}')).collect();
            assert!(bad.is_empty(), "forbidden characters: {bad:?}");
        }
    }
}
