//! Notation: MIDI tracks laid out as measures of notes and rests, written as MusicXML
//! (File › Export › Sibelius, File › Send To › Sibelius) or engraved to SVG (File › Print Score).
//!
//! Notes are quantized to sixteenths. Each part is a single voice: notes that start together form
//! a chord, overlapping notes are cut at the next onset, and anything crossing a bar line is split
//! and tied.

use soundcraft_model::{ClipContent, Session, TrackId, TrackKind};
use soundcraft_time::TICKS_PER_QUARTER;
use std::fmt::Write as _;

/// Ticks per score unit (a sixteenth).
const UNIT: i64 = TICKS_PER_QUARTER / 4;
/// Upper bound on engraved measures, so a hostile session cannot make a huge document.
const MAX_MEASURES: usize = 2000;
/// Upper bound on output size, so a hostile session cannot make a huge document.
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

/// The output has grown past the byte budget: refuse to grow it further.
fn over_budget(buf: &str, budget: usize) -> Result<(), String> {
    if buf.len() > budget {
        return Err(format!("score too large: over {} bytes", budget));
    }
    Ok(())
}

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

/// One measure: its meter and its items (notes, chords or rests), in sixteenths.
#[derive(Debug, Clone, PartialEq)]
pub struct Measure {
    pub numerator: u32,
    pub denominator: u32,
    pub items: Vec<Item>,
}

/// A notehead group (chord) or a rest (no pitches).
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub pitches: Vec<u8>,
    /// Duration in sixteenths; always one of [`VALUES`].
    pub units: u32,
    pub tie_start: bool,
    pub tie_stop: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub name: String,
    /// True when the part reads best in bass clef.
    pub bass: bool,
    pub measures: Vec<Measure>,
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
        let (ua, ub) = (a / UNIT, b / UNIT);
        if ub <= ua {
            break;
        }
        v.push((ua, ub - ua, m.numerator, m.denominator));
        bar += 1;
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
    // Onsets → chords, each held until its longest *retained* note ends or the next onset. A pitch
    // the cap rejects must not extend the onset end.
    let mut onsets: std::collections::BTreeMap<i64, (i64, Vec<u8>)> = std::collections::BTreeMap::new();
    for (a, b, p) in notes {
        let (ends, pitches) = onsets.entry(*a).or_insert((*b, Vec::new()));
        let new_pitch = !pitches.contains(p);
        if new_pitch && pitches.len() >= MAX_PITCHES_PER_CHORD {
            // Chord full and this pitch is absent: it is dropped and must not extend the onset end.
            continue;
        }
        // A retained pitch (already in the chord, or one the cap accepts) may extend the onset
        // end, even when it reappears with a longer duration.
        *ends = (*ends).max(*b);
        if new_pitch {
            pitches.push(*p);
        }
    }
    let starts: Vec<i64> = onsets.keys().copied().collect();
    let mut spans: Vec<(i64, i64, Vec<u8>)> = Vec::new();
    for (i, (a, (b, mut ps))) in onsets.into_iter().enumerate() {
        let end = starts.get(i + 1).map_or(b, |n| b.min(*n));
        ps.sort_unstable();
        spans.push((a, end, ps));
    }
    let mut measures = Vec::new();
    for (ms, ml, num, den) in bars {
        let me = ms + ml;
        let mut items = Vec::new();
        let mut cursor = *ms;
        let emit = |from: i64, to: i64, pitches: &[u8], tied_in: bool, tied_out: bool, items: &mut Vec<Item>| {
            let parts = split_units(to - from);
            let n = parts.len();
            for (k, u) in parts.into_iter().enumerate() {
                let rest = pitches.is_empty();
                items.push(Item {
                    pitches: pitches.to_vec(),
                    units: u,
                    tie_stop: !rest && (tied_in || k > 0),
                    tie_start: !rest && (tied_out || k + 1 < n),
                });
            }
        };
        for (a, b, ps) in spans.iter().filter(|(a, b, _)| *b > *ms && *a < me) {
            let from = (*a).max(*ms).max(cursor);
            let to = (*b).min(me);
            if to <= from {
                continue;
            }
            if from > cursor {
                emit(cursor, from, &[], false, false, &mut items);
            }
            emit(from, to, ps, *a < *ms, *b > me, &mut items);
            cursor = to;
            // Cap items per measure to avoid pathological memory use.
            if items.len() > MAX_ITEMS_PER_MEASURE {
                break;
            }
        }
        if cursor < me {
            emit(cursor, me, &[], false, false, &mut items);
        }
        // Cap the item count without shortening the measure: whatever suffix the cap discards is
        // replaced with representable rests, so the bar's duration (and the SVG layout denominator)
        // stays right. Shrinking the item list further makes room for those rests.
        if items.len() > MAX_ITEMS_PER_MEASURE {
            let bar_len = me.saturating_sub(*ms);
            let mut keep = MAX_ITEMS_PER_MEASURE;
            loop {
                let kept: i64 = items.iter().take(keep).map(|i| i64::from(i.units)).sum();
                let rests = split_units(bar_len.saturating_sub(kept));
                if keep.saturating_add(rests.len()) <= MAX_ITEMS_PER_MEASURE || keep == 0 {
                    items.truncate(keep);
                    items.extend(rests.into_iter().map(|units| Item { pitches: Vec::new(), units, tie_start: false, tie_stop: false }));
                    break;
                }
                keep -= 1;
            }
            // Safety net for a hostile meter so wide that even whole-bar rests exceed the cap.
            items.truncate(MAX_ITEMS_PER_MEASURE);
        }
        measures.push(Measure { numerator: *num, denominator: *den, items });
    }
    measures
}

/// (step, alter, octave) spelled with sharps.
fn spell(pitch: u8) -> (char, i32, i32) {
    const STEPS: [(char, i32); 12] =
        [('C', 0), ('C', 1), ('D', 0), ('D', 1), ('E', 0), ('F', 0), ('F', 1), ('G', 0), ('G', 1), ('A', 0), ('A', 1), ('B', 0)];
    let (step, alter) = STEPS[usize::from(pitch % 12)];
    (step, alter, i32::from(pitch / 12) - 1)
}

fn xml_escape(s: &str) -> String {
    s.chars()
        .filter(|c| matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}'))
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn note_type(units: u32) -> (&'static str, bool) {
    match units {
        16 => ("whole", false),
        12 => ("half", true),
        8 => ("half", false),
        6 => ("quarter", true),
        4 => ("quarter", false),
        3 => ("eighth", true),
        2 => ("eighth", false),
        _ => ("16th", false),
    }
}

/// MusicXML 4.0 (partwise) for `parts`.
pub fn musicxml(parts: &[Part], setup: &ScoreSetup) -> Result<String, String> {
    musicxml_budgeted(parts, setup, MAX_OUTPUT_BYTES)
}

/// [`musicxml`] with an explicit byte budget (kept separate so tests can pass a tiny budget
/// instead of building a 256 MiB score).
fn musicxml_budgeted(parts: &[Part], setup: &ScoreSetup, budget: usize) -> Result<String, String> {
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n");
    x.push_str("<!DOCTYPE score-partwise PUBLIC \"-//Recordare//DTD MusicXML 4.0 Partwise//EN\" \"http://www.musicxml.org/dtds/partwise.dtd\">\n");
    x.push_str("<score-partwise version=\"4.0\">\n");
    // The header (and an oversized title/composer, escaped) can itself exceed the budget even with
    // no measures, so check before the per-measure loop too.
    over_budget(&x, budget)?;
    let _ = writeln!(x, "  <work><work-title>{}</work-title></work>", xml_escape(&setup.title));
    x.push_str("  <identification>\n");
    if !setup.composer.is_empty() {
        let _ = writeln!(x, "    <creator type=\"composer\">{}</creator>", xml_escape(&setup.composer));
    }
    x.push_str("    <encoding><software>SoundCraft</software></encoding>\n  </identification>\n  <part-list>\n");
    for (i, p) in parts.iter().enumerate() {
        let _ = writeln!(x, "    <score-part id=\"P{}\"><part-name>{}</part-name></score-part>", i + 1, xml_escape(&p.name));
    }
    x.push_str("  </part-list>\n");
    over_budget(&x, budget)?;
    for (i, p) in parts.iter().enumerate() {
        let _ = writeln!(x, "  <part id=\"P{}\">", i + 1);
        let mut meter = (0, 0);
        for (mi, m) in p.measures.iter().enumerate() {
            let _ = writeln!(x, "    <measure number=\"{}\">", mi + 1);
            if mi == 0 || meter != (m.numerator, m.denominator) {
                x.push_str("      <attributes>");
                if mi == 0 {
                    x.push_str("<divisions>4</divisions><key><fifths>0</fifths></key>");
                }
                let _ = write!(x, "<time><beats>{}</beats><beat-type>{}</beat-type></time>", m.numerator, m.denominator);
                if mi == 0 {
                    x.push_str(if p.bass { "<clef><sign>F</sign><line>4</line></clef>" } else { "<clef><sign>G</sign><line>2</line></clef>" });
                }
                x.push_str("</attributes>\n");
                meter = (m.numerator, m.denominator);
            }
            for it in &m.items {
                let (ty, dot) = note_type(it.units);
                if it.pitches.is_empty() {
                    let _ = writeln!(
                        x,
                        "      <note><rest/><duration>{}</duration><voice>1</voice><type>{ty}</type>{}</note>",
                        it.units,
                        if dot { "<dot/>" } else { "" }
                    );
                    continue;
                }
                for (k, pitch) in it.pitches.iter().enumerate() {
                    let (step, alter, octave) = spell(*pitch);
                    x.push_str("      <note>");
                    if k > 0 {
                        x.push_str("<chord/>");
                    }
                    let _ = write!(x, "<pitch><step>{step}</step>");
                    if alter != 0 {
                        let _ = write!(x, "<alter>{alter}</alter>");
                    }
                    let _ = write!(x, "<octave>{octave}</octave></pitch><duration>{}</duration>", it.units);
                    if it.tie_stop {
                        x.push_str("<tie type=\"stop\"/>");
                    }
                    if it.tie_start {
                        x.push_str("<tie type=\"start\"/>");
                    }
                    let _ = write!(x, "<voice>1</voice><type>{ty}</type>");
                    if dot {
                        x.push_str("<dot/>");
                    }
                    if alter != 0 {
                        x.push_str("<accidental>sharp</accidental>");
                    }
                    if it.tie_stop || it.tie_start {
                        x.push_str("<notations>");
                        if it.tie_stop {
                            x.push_str("<tied type=\"stop\"/>");
                        }
                        if it.tie_start {
                            x.push_str("<tied type=\"start\"/>");
                        }
                        x.push_str("</notations>");
                    }
                    x.push_str("</note>\n");
                }
            }
            x.push_str("    </measure>\n");
            over_budget(&x, budget)?;
        }
        x.push_str("  </part>\n");
    }
    x.push_str("</score-partwise>\n");
    over_budget(&x, budget)?;
    Ok(x)
}

/// Staff position of `pitch` in half-spaces above the bottom line (treble E4 / bass G2).
fn staff_step(pitch: u8, bass: bool) -> i32 {
    let (step, _, octave) = spell(pitch);
    let idx = "CDEFGAB".find(step).map_or(0, |i| i as i32);
    let diatonic = octave * 7 + idx;
    diatonic - if bass { 2 * 7 + 4 } else { 4 * 7 + 2 }
}

/// An engraved score as a printable SVG page (A4 portrait width, as tall as needed).
pub fn svg(parts: &[Part], setup: &ScoreSetup) -> Result<String, String> {
    svg_budgeted(parts, setup, MAX_OUTPUT_BYTES)
}

/// [`svg`] with an explicit byte budget (kept separate so tests can pass a tiny budget instead of
/// building a 256 MiB score).
fn svg_budgeted(parts: &[Part], setup: &ScoreSetup, budget: usize) -> Result<String, String> {
    const W: f64 = 794.0;
    const MARGIN: f64 = 48.0;
    const SP: f64 = 7.0; // staff space
    const STAFF_GAP: f64 = 70.0;
    const SYSTEM_GAP: f64 = 40.0;
    let n_measures = parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
    let per = setup.bars_per_system.max(1) as usize;
    let systems = n_measures.div_ceil(per).max(1);
    let label_w = if setup.show_track_names { 70.0 } else { 0.0 };
    let staff_x0 = MARGIN + label_w + 34.0;
    let staff_x1 = W - MARGIN;
    let sys_h = parts.len().max(1) as f64 * STAFF_GAP + SYSTEM_GAP;
    let top = 110.0;
    let h = top + systems as f64 * sys_h + MARGIN;
    let mut o = String::new();
    let _ =
        writeln!(o, "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{W}\" height=\"{h:.0}\" viewBox=\"0 0 {W} {h:.0}\" font-family=\"serif\">");
    let _ = writeln!(o, "<rect width=\"100%\" height=\"100%\" fill=\"white\"/>");
    let _ = writeln!(o, "<text x=\"{}\" y=\"56\" font-size=\"24\" text-anchor=\"middle\">{}</text>", W / 2.0, xml_escape(&setup.title));
    if !setup.composer.is_empty() {
        let _ = writeln!(o, "<text x=\"{}\" y=\"84\" font-size=\"13\" text-anchor=\"end\">{}</text>", W - MARGIN, xml_escape(&setup.composer));
        over_budget(&o, budget)?;
    }
    over_budget(&o, budget)?;
    let bar_w = (staff_x1 - staff_x0) / per as f64;
    for sys in 0..systems {
        let y_sys = top + sys as f64 * sys_h;
        for (pi, p) in parts.iter().enumerate() {
            let y0 = y_sys + pi as f64 * STAFF_GAP; // top line
            let bottom = y0 + 4.0 * SP;
            if setup.show_track_names && sys == 0 {
                let _ = writeln!(o, "<text x=\"{MARGIN}\" y=\"{:.1}\" font-size=\"11\">{}</text>", y0 + 2.0 * SP + 4.0, xml_escape(&p.name));
            }
            for l in 0..5 {
                let y = y0 + f64::from(l) * SP;
                let _ = writeln!(
                    o,
                    "<line x1=\"{:.1}\" y1=\"{y:.1}\" x2=\"{staff_x1:.1}\" y2=\"{y:.1}\" stroke=\"black\" stroke-width=\"0.8\"/>",
                    staff_x0 - 30.0
                );
            }
            let clef = if p.bass { "𝄢" } else { "𝄞" };
            let _ = writeln!(
                o,
                "<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"{}\">{clef}</text>",
                staff_x0 - 29.0,
                bottom + if p.bass { -SP } else { SP * 0.6 },
                if p.bass { 26 } else { 36 }
            );
            for k in 0..per {
                let mi = sys * per + k;
                let x0 = staff_x0 + k as f64 * bar_w;
                let _ = writeln!(
                    o,
                    "<line x1=\"{:.1}\" y1=\"{y0:.1}\" x2=\"{:.1}\" y2=\"{bottom:.1}\" stroke=\"black\" stroke-width=\"1\"/>",
                    x0 + bar_w,
                    x0 + bar_w
                );
                let Some(m) = p.measures.get(mi) else { continue };
                if pi == 0 {
                    let _ = writeln!(o, "<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"9\">{}</text>", x0 + 2.0, y0 - 8.0, mi + 1);
                }
                let total: u32 = m.items.iter().map(|i| i.units).sum::<u32>().max(1);
                let mut acc = 0u32;
                for it in &m.items {
                    let x = x0 + 10.0 + (bar_w - 16.0) * f64::from(acc) / f64::from(total);
                    acc += it.units;
                    let (_, dot) = note_type(it.units);
                    if it.pitches.is_empty() {
                        rest(&mut o, x, y0, SP, it.units);
                        if dot {
                            let _ = writeln!(o, "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"1.3\"/>", x + 7.0, y0 + 1.5 * SP);
                        }
                        continue;
                    }
                    let steps: Vec<i32> = it.pitches.iter().map(|p2| staff_step(*p2, p.bass)).collect();
                    let filled = it.units < 8;
                    for (pitch, st) in it.pitches.iter().zip(&steps) {
                        let y = bottom - f64::from(*st) * SP / 2.0;
                        // Ledger lines.
                        let mut l = if *st < 0 { -2 } else { 10 };
                        while (*st < 0 && l >= *st) || (*st > 8 && l <= *st) {
                            let ly = bottom - f64::from(l) * SP / 2.0;
                            let _ = writeln!(
                                o,
                                "<line x1=\"{:.1}\" y1=\"{ly:.1}\" x2=\"{:.1}\" y2=\"{ly:.1}\" stroke=\"black\" stroke-width=\"0.8\"/>",
                                x - 7.0,
                                x + 7.0
                            );
                            l += if *st < 0 { -2 } else { 2 };
                        }
                        let fill = if filled { "black" } else { "white" };
                        let _ = writeln!(
                            o,
                            "<ellipse cx=\"{x:.1}\" cy=\"{y:.1}\" rx=\"4.6\" ry=\"3.3\" transform=\"rotate(-20 {x:.1} {y:.1})\" fill=\"{fill}\" stroke=\"black\" stroke-width=\"1.1\"/>"
                        );
                        if spell(*pitch).1 != 0 {
                            let _ = writeln!(o, "<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"12\">♯</text>", x - 14.0, y + 4.0);
                        }
                        if dot {
                            let _ = writeln!(o, "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"1.3\"/>", x + 8.0, y - 1.5);
                        }
                    }
                    if it.units < 16 {
                        let (lo, hi) = (steps.iter().copied().min().unwrap_or(4), steps.iter().copied().max().unwrap_or(4));
                        let up = (lo + hi) < 8;
                        let (sx, y_from, y_to) = if up {
                            (x + 4.3, bottom - f64::from(lo) * SP / 2.0, bottom - f64::from(hi) * SP / 2.0 - 3.5 * SP)
                        } else {
                            (x - 4.3, bottom - f64::from(hi) * SP / 2.0, bottom - f64::from(lo) * SP / 2.0 + 3.5 * SP)
                        };
                        let _ = writeln!(
                            o,
                            "<line x1=\"{sx:.1}\" y1=\"{y_from:.1}\" x2=\"{sx:.1}\" y2=\"{y_to:.1}\" stroke=\"black\" stroke-width=\"1\"/>"
                        );
                        let flags = match it.units {
                            1 => 2,
                            2 | 3 => 1,
                            _ => 0,
                        };
                        for f in 0..flags {
                            let fy = y_to + f64::from(f) * if up { 5.0 } else { -5.0 };
                            let dy = if up { 9.0 } else { -9.0 };
                            let _ = writeln!(
                                o,
                                "<path d=\"M{sx:.1} {fy:.1} q 7 {:.1} 5 {dy:.1}\" fill=\"none\" stroke=\"black\" stroke-width=\"1.4\"/>",
                                dy * 0.5
                            );
                        }
                    }
                    if it.tie_start {
                        let y = bottom - f64::from(steps[0]) * SP / 2.0 + 6.0;
                        let _ = writeln!(o, "<path d=\"M{:.1} {y:.1} q 8 5 16 0\" fill=\"none\" stroke=\"black\" stroke-width=\"0.9\"/>", x + 3.0);
                    }
                }
                // Check each measure, so a run of rest-only or missing measures that only draws
                // bar lines cannot grow the output past the budget untouched.
                over_budget(&o, budget)?;
            }
            // The staff markup (lines, clef, track names) itself grows with the part count even
            // without any items, so check once per staff too.
            over_budget(&o, budget)?;
        }
        // System bracket.
        let y_last = y_sys + (parts.len().max(1) - 1) as f64 * STAFF_GAP + 4.0 * SP;
        let _ = writeln!(
            o,
            "<line x1=\"{:.1}\" y1=\"{y_sys:.1}\" x2=\"{:.1}\" y2=\"{y_last:.1}\" stroke=\"black\" stroke-width=\"1.5\"/>",
            staff_x0 - 30.0,
            staff_x0 - 30.0
        );
        over_budget(&o, budget)?;
    }
    o.push_str("</svg>\n");
    over_budget(&o, budget)?;
    Ok(o)
}

fn rest(o: &mut String, x: f64, y0: f64, sp: f64, units: u32) {
    let mid = y0 + 2.0 * sp;
    match units {
        16 => {
            let _ = writeln!(o, "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"9\" height=\"{:.1}\"/>", x - 4.5, y0 + sp, sp / 2.0);
        }
        8 | 12 => {
            let _ = writeln!(o, "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"9\" height=\"{:.1}\"/>", x - 4.5, mid - sp / 2.0, sp / 2.0);
        }
        4 | 6 => {
            let _ = writeln!(
                o,
                "<path d=\"M{:.1} {:.1} l 4 5 l -4 4 l 4 5 q -6 -2 -2 5\" fill=\"none\" stroke=\"black\" stroke-width=\"1.8\"/>",
                x - 2.0,
                mid - 9.0
            );
        }
        _ => {
            let _ = writeln!(o, "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"2\"/>", x - 2.0, mid - 3.0);
            let _ = writeln!(
                o,
                "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"black\" stroke-width=\"1.2\"/>",
                x + 2.5,
                mid - 4.0,
                x - 1.0,
                mid + 7.0
            );
            if units == 1 {
                let _ = writeln!(o, "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"2\"/>", x - 3.5, mid + 2.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_units_uses_representable_values() {
        assert_eq!(split_units(16), vec![16]);
        assert_eq!(split_units(5), vec![4, 1]);
        assert_eq!(split_units(7), vec![6, 1]);
        assert!(split_units(0).is_empty());
    }

    #[test]
    fn spelling_and_staff_steps() {
        assert_eq!(spell(60), ('C', 0, 4));
        assert_eq!(spell(61), ('C', 1, 4));
        assert_eq!(staff_step(64, false), 0); // E4: bottom treble line
        assert_eq!(staff_step(77, false), 8); // F5: top treble line
        assert_eq!(staff_step(43, true), 0); // G2: bottom bass line
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
        assert!(m[0].items.last().is_some_and(|i| i.tie_start));
        assert!(m[1].items[0].tie_stop);
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
