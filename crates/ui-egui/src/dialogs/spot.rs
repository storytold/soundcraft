//! The Spot dialog: exact positions for clips in Spot mode.

use crate::theme::mono;
use egui::{Id, RichText, TextEdit, Ui, vec2};
use serde_json::{Value, json};
use soundcraft_engine::cmd::MAX_POSITION;
use soundcraft_engine::edit::first_movable;
use soundcraft_model::{ClipId, Session, SourceId, TrackId};
use soundcraft_time::{Samples, TimeFormat, format_length};

#[derive(Debug, Clone)]
pub enum Target {
    /// Clips moving together; the fields follow the earliest.
    Clips { clips: Vec<ClipId>, track: Option<TrackId> },
    /// The edge of a clip the Trim tool took.
    Trim { clip: ClipId, start: bool },
    /// An audio file dropped from the Clip List.
    Source { source: SourceId, track: TrackId },
}

#[derive(Debug, Clone)]
pub struct Spot {
    pub target: Target,
    pub start: Samples,
    pub len: Samples,
    sync: Samples,
    stamp: Option<Samples>,
    format: TimeFormat,
    text: [String; 3],
    fresh: bool,
}

const FIELDS: [&str; 3] = ["Start", "Sync Point", "End"];

impl Spot {
    fn new(s: &Session, target: Target, start: Samples, len: Samples, sync: Samples, stamp: Option<Samples>) -> Spot {
        Spot { target, start, len, sync, stamp, format: s.edit.main_counter, text: Default::default(), fresh: true }
    }

    /// Clicked or dragged clips, moved `delta` so far.
    pub fn clips(s: &Session, clips: Vec<ClipId>, track: Option<TrackId>, delta: Samples) -> Option<Spot> {
        let c = first_movable(s, &clips)?;
        let stamp = c.source().and_then(|id| s.source(id)).map(|src| recorded_at(s, src.time_reference).saturating_add(c.source_offset()));
        Some(Spot::new(s, Target::Clips { clips, track }, c.start.saturating_add(delta), c.length, c.sync_point, stamp))
    }

    /// A clip's start or end edge, dragged to `to`.
    pub fn trim(s: &Session, clip: ClipId, start: bool, to: Samples) -> Option<Spot> {
        let (_, c) = s.find_clip(clip)?;
        let (a, b) = if start { (to.min(c.end().saturating_sub(1)), c.end()) } else { (c.start, to.max(c.start.saturating_add(1))) };
        Some(Spot::new(s, Target::Trim { clip, start }, a, b.saturating_sub(a), 0, None))
    }

    /// An audio file from the Clip List, dropped at `at`.
    pub fn source(s: &Session, source: SourceId, track: TrackId, at: Samples) -> Option<Spot> {
        let src = s.source(source)?;
        let len = i64::try_from(src.frames).unwrap_or(0);
        Some(Spot::new(s, Target::Source { source, track }, at, len, 0, Some(recorded_at(s, src.time_reference))))
    }

    fn value(&self, field: usize) -> Samples {
        match field {
            0 => self.start,
            1 => self.start.saturating_add(self.sync),
            _ => self.start.saturating_add(self.len),
        }
    }

    fn set(&mut self, field: usize, v: Samples) {
        match (&self.target, field) {
            (Target::Trim { start: true, .. }, _) => {
                self.len = self.start.saturating_add(self.len).saturating_sub(v);
                self.start = v;
            }
            (Target::Trim { .. }, _) => self.len = v.saturating_sub(self.start),
            (_, 0) => self.start = v,
            (_, 1) => self.start = v.saturating_sub(self.sync),
            _ => self.start = v.saturating_sub(self.len),
        }
    }

    fn editable(&self, field: usize) -> bool {
        match self.target {
            Target::Trim { start, .. } => field == if start { 0 } else { 2 },
            _ => true,
        }
    }

    pub(super) fn timecode_error(&self, s: &Session) -> Option<String> {
        if self.format != TimeFormat::Timecode {
            return None;
        }
        let position = (0..3).filter(|&field| self.editable(field)).map(|field| self.value(field)).find(|&value| value < 0)?;
        Some(crate::localized_format!(
            "{value} is before the session start ({start})",
            value = s.format_position(position, self.format),
            start = s.format_position(0, self.format)
        ))
    }

    /// The command OK runs; None when nothing would change.
    pub fn command(&self, s: &Session) -> Option<(String, Value)> {
        Some(match &self.target {
            Target::Clips { clips, track } => {
                let first = first_movable(s, clips)?.start;
                let by = self.start.saturating_sub(first);
                if by == 0 && track.is_none() {
                    return None;
                }
                let mut p = json!({"clips": clips.iter().map(|c| c.0).collect::<Vec<_>>(), "by": by});
                if let Some(t) = track {
                    p["track"] = json!(t.0);
                }
                ("edit.move_clips".into(), p)
            }
            Target::Trim { clip, start: true } => ("edit.trim_clip".into(), json!({"clip": clip.0, "start": self.start})),
            Target::Trim { clip, .. } => ("edit.trim_clip".into(), json!({"clip": clip.0, "end": self.value(2)})),
            Target::Source { source, track } => ("clip.place_source".into(), json!({"source": source.0, "track": track.0, "at": self.start})),
        })
    }
}

/// Timeline position of a BWF time stamp (samples since midnight).
fn recorded_at(s: &Session, time_reference: u64) -> Samples {
    i64::try_from(time_reference).unwrap_or(0).saturating_sub(s.timecode_start)
}

/// One editable Spot field: accept the entered position when it parses and lies in `-MAX_POSITION..=MAX_POSITION`,
/// or show the formatted value when read-only.
fn spot_field_row(ui: &mut Ui, s: &Session, d: &mut Spot, i: usize, label: &str) {
    let show = |at: Samples, f: TimeFormat| s.format_position(at, f);
    let lab = ui.label(label);
    if d.editable(i) {
        let id = Id::unique(("spot_field", i));
        if !ui.memory(|m| m.has_focus(id)) {
            d.text[i] = show(d.value(i), d.format);
        }
        let r = ui.add(TextEdit::singleline(&mut d.text[i]).id(id).font(mono(13.0)).desired_width(150.0)).labelled_by(lab.id);
        if r.changed()
            && let Ok(v) = s.parse_position(&d.text[i], d.format)
            && (-MAX_POSITION..=MAX_POSITION).contains(&v)
        {
            d.set(i, v);
        }
        if std::mem::take(&mut d.fresh) {
            r.request_focus();
            select_all(ui, id, &d.text[i]);
        }
    } else {
        ui.label(RichText::new(show(d.value(i), d.format)).font(mono(13.0)));
    }
    ui.end_row();
}

fn original_time_stamp_row(ui: &mut Ui, s: &Session, d: &mut Spot, at: Samples) {
    ui.label(crate::localization::text("Original Time Stamp"));
    ui.horizontal(|ui| {
        ui.label(RichText::new(s.format_position(at, TimeFormat::Timecode)).font(mono(13.0)));
        if ui.button(crate::localization::text("Use")).on_hover_text(crate::localization::text("Spot to where it was recorded")).clicked() {
            d.start = at;
        }
    });
    ui.end_row();
}

/// The dialog's fields; the caller adds OK and Cancel.
pub fn fields(ui: &mut Ui, s: &Session, d: &mut Spot) {
    egui::Grid::new("spot").num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
        let scale = ui.label(crate::localization::text("Time Scale"));
        let time_scale = egui::ComboBox::from_id_salt("spot_scale").selected_text(crate::localization::text(d.format.label())).show_ui(ui, |ui| {
            for f in TimeFormat::ALL {
                ui.selectable_value(&mut d.format, f, crate::localization::text(f.label()));
            }
        });
        time_scale.response.labelled_by(scale.id);
        ui.end_row();
        for (i, label) in FIELDS.into_iter().enumerate() {
            if i == 1 && matches!(d.target, Target::Trim { .. }) {
                continue;
            }
            spot_field_row(ui, s, d, i, label);
        }
        ui.label(crate::localization::text("Duration"));
        ui.label(RichText::new(format_length(d.start, d.len, d.format, s.sample_rate, &s.tempo, s.frame_rate)).font(mono(13.0)));
        ui.end_row();
        if let Some(at) = d.stamp {
            original_time_stamp_row(ui, s, d, at);
        }
    });
}

fn select_all(ui: &Ui, id: Id, text: &str) {
    let mut state = TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
    state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::default(), egui::text::CCursor::new(text.chars().count()))));
    state.store(ui.ctx(), id);
}

#[cfg(test)]
mod timecode_tests {
    use super::*;
    #[test]
    fn spot_rejects_a_timecode_before_session_start() {
        let mut e = soundcraft_engine::demo::demo_engine();
        let s = e.session_mut();
        s.timecode_start = s.sample_rate.samples(3600.);
        s.edit.main_counter = TimeFormat::Timecode;
        let clip = s.tracks.iter().filter_map(|t| t.playlist()).flat_map(|p| &p.clips).next().unwrap().id;
        let old = s.find_clip(clip).unwrap().1.start;
        let mut trim = Spot::trim(s, clip, false, old + 1).unwrap();
        trim.set(2, s.parse_position("00:59:59:00", TimeFormat::Timecode).unwrap());
        assert!(trim.timecode_error(s).unwrap().contains("01:00:00:00"));
        let mut d = Spot::clips(s, vec![clip], None, 0).unwrap();
        d.set(0, s.parse_position("00:59:59:00", TimeFormat::Timecode).unwrap());
        assert!(d.timecode_error(s).unwrap().contains("01:00:00:00"));
        assert_eq!(s.find_clip(clip).unwrap().1.start, old);
        d.set(0, s.parse_position("01:00:01:00", TimeFormat::Timecode).unwrap());
        assert!(d.timecode_error(s).is_none());
        let (cmd, p) = d.command(s).unwrap();
        e.execute(&cmd, &p).unwrap();
        assert_eq!(e.session().find_clip(clip).unwrap().1.start, e.session().sample_rate.samples(1.));
    }
}
