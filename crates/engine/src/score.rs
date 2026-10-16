//! Notation: MIDI tracks laid out as measures of notes and rests, written as MusicXML
//! (File › Export › Sibelius, File › Send To › Sibelius) or engraved to SVG (File › Print Score).
//!
//! Notes are quantized to sixteenths. Each part is a sequence of chords split at note onsets,
//! note ends and bar lines.
//! Per-pitch ties preserve the duration of each note, including overlapping notes.

use soundcraft_model::{ClipContent, Session, TrackId, TrackKind};
use soundcraft_time::TICKS_PER_QUARTER;
use std::fmt::Write as _;

/// Ticks per score unit (a sixteenth).
const UNIT: i64 = TICKS_PER_QUARTER / 4;
/// Upper bound on engraved measures, so a hostile session cannot make a huge document.
const MAX_MEASURES: usize = 2000;
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
    /// Pitches continuing into the next item.
    pub tie_start: Vec<u8>,
    /// Pitches continuing from the previous item.
    pub tie_stop: Vec<u8>,
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
    #[derive(Default)]
    struct Events {
        starts: Vec<u8>,
        ends: Vec<u8>,
    }
    let limit = bars.last().map_or(0, |(start, length, _, _)| start.saturating_add(*length));
    let mut events: std::collections::BTreeMap<i64, Events> = std::collections::BTreeMap::new();
    for &(a, b, pitch) in notes {
        if a >= b || a >= limit || b <= 0 || pitch > 127 {
            continue;
        }
        events.entry(a.max(0)).or_default().starts.push(pitch);
        events.entry(b.min(limit)).or_default().ends.push(pitch);
    }
    let events: Vec<_> = events.into_iter().collect();
    let mut active = [0usize; 128];
    let mut spans = Vec::new();
    for pair in events.windows(2) {
        let [(from, ev), (to, next)] = pair else { continue };
        for pitch in &ev.ends {
            if let Some(count) = active.get_mut(usize::from(*pitch)) {
                *count = count.saturating_sub(1);
            }
        }
        let tied_in: Vec<u8> = active.iter().enumerate().filter(|(p, n)| **n > 0 && !ev.starts.contains(&(*p as u8))).map(|(p, _)| p as u8).collect();
        for pitch in &ev.starts {
            if let Some(count) = active.get_mut(usize::from(*pitch)) {
                *count = count.saturating_add(1);
            }
        }
        let pitches: Vec<u8> = active.iter().enumerate().filter(|(_, n)| **n > 0).map(|(p, _)| p as u8).collect();
        let mut ending = [0usize; 128];
        for pitch in &next.ends {
            if let Some(count) = ending.get_mut(usize::from(*pitch)) {
                *count = count.saturating_add(1);
            }
        }
        let tied_out: Vec<u8> = pitches
            .iter()
            .copied()
            .filter(|p| active.get(usize::from(*p)).zip(ending.get(usize::from(*p))).is_some_and(|(a, b)| a > b) && !next.starts.contains(p))
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
        }
        if cursor < me {
            emit(cursor, me, &[], &[], &[], &mut items);
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
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
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
pub fn musicxml(parts: &[Part], setup: &ScoreSetup) -> String {
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n");
    x.push_str("<!DOCTYPE score-partwise PUBLIC \"-//Recordare//DTD MusicXML 4.0 Partwise//EN\" \"http://www.musicxml.org/dtds/partwise.dtd\">\n");
    x.push_str("<score-partwise version=\"4.0\">\n");
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
                    if it.tie_stop.contains(pitch) {
                        x.push_str("<tie type=\"stop\"/>");
                    }
                    if it.tie_start.contains(pitch) {
                        x.push_str("<tie type=\"start\"/>");
                    }
                    let _ = write!(x, "<voice>1</voice><type>{ty}</type>");
                    if dot {
                        x.push_str("<dot/>");
                    }
                    if alter != 0 {
                        x.push_str("<accidental>sharp</accidental>");
                    }
                    if it.tie_stop.contains(pitch) || it.tie_start.contains(pitch) {
                        x.push_str("<notations>");
                        if it.tie_stop.contains(pitch) {
                            x.push_str("<tied type=\"stop\"/>");
                        }
                        if it.tie_start.contains(pitch) {
                            x.push_str("<tied type=\"start\"/>");
                        }
                        x.push_str("</notations>");
                    }
                    x.push_str("</note>\n");
                }
            }
            x.push_str("    </measure>\n");
        }
        x.push_str("  </part>\n");
    }
    x.push_str("</score-partwise>\n");
    x
}

/// Staff position of `pitch` in half-spaces above the bottom line (treble E4 / bass G2).
fn staff_step(pitch: u8, bass: bool) -> i32 {
    let (step, _, octave) = spell(pitch);
    let idx = "CDEFGAB".find(step).map_or(0, |i| i as i32);
    let diatonic = octave * 7 + idx;
    diatonic - if bass { 2 * 7 + 4 } else { 4 * 7 + 2 }
}

/// An engraved score as a printable SVG page (A4 portrait width, as tall as needed).
pub fn svg(parts: &[Part], setup: &ScoreSetup) -> String {
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
    }
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
                    for pitch in &it.tie_start {
                        let above = it.pitches.first().is_some_and(|low| pitch > low);
                        let direction = if above { -1.0 } else { 1.0 };
                        let y = bottom - f64::from(staff_step(*pitch, p.bass)) * SP / 2.0 + direction * 6.0;
                        let _ = writeln!(
                            o,
                            "<path d=\"M{:.1} {y:.1} q 8 {} 16 0\" fill=\"none\" stroke=\"black\" stroke-width=\"0.9\"/>",
                            x + 3.0,
                            direction * 5.0
                        );
                    }
                }
            }
        }
        // System bracket.
        let y_last = y_sys + (parts.len().max(1) - 1) as f64 * STAFF_GAP + 4.0 * SP;
        let _ = writeln!(
            o,
            "<line x1=\"{:.1}\" y1=\"{y_sys:.1}\" x2=\"{:.1}\" y2=\"{y_last:.1}\" stroke=\"black\" stroke-width=\"1.5\"/>",
            staff_x0 - 30.0,
            staff_x0 - 30.0
        );
    }
    o.push_str("</svg>\n");
    o
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

    proptest::proptest! {
        #[test]
        fn score_preserves_pitch_activity(notes in proptest::collection::vec((0i64..32, 1i64..32, 48u8..72), 0..20)) {
            let notes: Vec<_> = notes.into_iter().map(|(a, length, pitch)| (a, a + length, pitch)).collect();
            let measures = fill(&[(0, 16, 4, 4), (16, 16, 4, 4), (32, 16, 4, 4), (48, 16, 4, 4)], &notes);
            let mut at = 0i64;
            for m in measures {
                proptest::prop_assert_eq!(m.items.iter().map(|i| i.units).sum::<u32>(), 16);
                for item in m.items {
                    for tick in at..at + i64::from(item.units) {
                        let expected: Vec<u8> = (48..72).filter(|pitch| notes.iter().any(|(a, b, p)| p == pitch && *a <= tick && tick < *b)).collect();
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
        assert!(m[0].items.last().is_some_and(|i| !i.tie_start.is_empty()));
        assert_eq!(m[1].items[0].tie_stop, vec![67]);
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
        let xml = musicxml(&parts, &setup);
        let notes: Vec<_> = xml.split("<note>").skip(1).map(|n| n.split("</note>").next().unwrap()).collect();
        assert!(notes[0].contains("<step>C</step>") && notes[0].contains("<duration>4</duration>"));
        assert!(!notes[0].contains("<tie "));
        assert!(notes[1].contains("<tie type=\"start\"/>") && notes[1].contains("<chord/>"));
        assert!(notes[2].contains("<duration>12</duration>") && notes[2].contains("<tie type=\"stop\"/>"));
        assert_eq!(svg(&parts, &setup).matches("stroke-width=\"0.9\"").count(), 1);
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
    fn demo_exports_well_formed_documents() {
        let e = crate::demo::demo_engine();
        let parts = layout(e.session(), &[]);
        assert!(!parts.is_empty(), "demo has MIDI tracks");
        let setup = ScoreSetup::from_session(e.session());
        let x = musicxml(&parts, &setup);
        assert!(x.contains("<score-partwise") && x.contains("<pitch>") && x.trim_end().ends_with("</score-partwise>"));
        assert_eq!(x.matches("<measure ").count(), x.matches("</measure>").count());
        let s = svg(&parts, &setup);
        assert!(s.starts_with("<svg") && s.contains("<ellipse"));
    }
}
