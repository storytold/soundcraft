//! Keyboard editing for the toolbar's position and selection counters.

use soundcraft_model::Session;
use soundcraft_time::{MAX_POSITION, Samples, TimeFormat, Timecode, format_length, parse_position};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CounterTarget {
    Main,
    Start,
    End,
    Length,
}

#[derive(Debug, Clone)]
pub(crate) struct CounterEdit {
    pub target: CounterTarget,
    pub format: TimeFormat,
    pub unit: Option<usize>,
    pub value: Samples,
    typed: String,
    calculation: Option<(i64, Samples)>,
}

impl CounterEdit {
    /// Starts editing a counter value, either as a whole value or at one component.
    pub fn new(target: CounterTarget, format: TimeFormat, value: Samples, unit: Option<usize>) -> Self {
        CounterEdit { target, format, unit, value, typed: String::new(), calculation: None }
    }

    /// Formats the current provisional value for the toolbar.
    pub fn display(&self, session: &Session) -> String {
        display(session, self.target, self.value, self.format)
    }

    /// Appends a digit using right-to-left counter entry rules.
    pub fn digit(&mut self, session: &Session, digit: char) {
        if !digit.is_ascii_digit() {
            return;
        }
        let widths = widths(self.format, session.frame_rate.nominal());
        let cap = self.unit.map_or(18, |unit| widths.get(unit).copied().unwrap_or(18).max(1));
        if self.typed.len() >= cap {
            self.typed.clear();
        }
        self.typed.push(digit);
        let count = component_ranges(&self.display(session), self.format).len().max(1);
        let relative = self.calculation;
        let mut parts = if let Some(unit) = self.unit {
            if relative.is_some() {
                let mut parts = vec![0; count];
                if let Some(part) = parts.get_mut(unit) {
                    *part = digits_value(&self.typed);
                }
                parts
            } else {
                let mut parts = components(&self.display(session), self.format);
                parts.resize(count, 0);
                if self.typed.len() == 1 {
                    for part in parts.iter_mut().skip(unit) {
                        *part = 0;
                    }
                }
                if let Some(part) = parts.get_mut(unit) {
                    *part = digits_value(&self.typed);
                }
                parts
            }
        } else {
            right_aligned_components(&self.typed, &widths, count)
        };
        normalize_component_mins(&mut parts, self.format, if relative.is_some() { CounterTarget::Length } else { self.target });
        if relative.is_none() {
            normalize_drop_frame_label(&mut parts, self.format, session);
        }
        self.value = if let Some((direction, base)) = relative {
            let amount = compose(session, CounterTarget::Length, self.format, &parts);
            base.saturating_add(direction.saturating_mul(amount)).clamp(0, MAX_POSITION)
        } else {
            compose(session, self.target, self.format, &parts).clamp(0, MAX_POSITION)
        };
    }

    /// Starts an addition or subtraction relative to the current value.
    pub fn begin_calculation(&mut self, direction: i64) {
        self.typed.clear();
        self.calculation = Some((direction.signum(), self.value));
    }

    /// Increments or decrements the selected counter component.
    pub fn step(&mut self, session: &Session, direction: i64) {
        self.typed.clear();
        self.calculation = None;
        let shown = self.display(session);
        let mut parts = components(&shown, self.format);
        let unit = self.unit.unwrap_or_else(|| parts.len().saturating_sub(1));
        self.unit = Some(unit);
        step_components(&mut parts, unit, direction, self.format, self.target, self.value, session);
        normalize_component_mins(&mut parts, self.format, self.target);
        normalize_drop_frame_label(&mut parts, self.format, session);
        self.value = compose(session, self.target, self.format, &parts).clamp(0, MAX_POSITION);
    }

    /// Selects the adjacent counter component without committing the edit.
    pub fn move_unit(&mut self, direction: i32, session: &Session) {
        self.typed.clear();
        self.calculation = None;
        let count = component_ranges(&self.display(session), self.format).len().max(1);
        let current = self.unit.unwrap_or(count.saturating_sub(1));
        self.unit = Some(if direction < 0 { current.saturating_sub(1) } else { current.saturating_add(1).min(count.saturating_sub(1)) });
    }
}

/// Formats a counter value, treating selection lengths as durations.
pub(crate) fn display(session: &Session, target: CounterTarget, value: Samples, format: TimeFormat) -> String {
    if target == CounterTarget::Length {
        format_length(session.edit.selection.start, value, format, session.sample_rate, &session.tempo, session.frame_rate)
    } else {
        session.format_position(value, format)
    }
}

/// Returns the byte ranges occupied by numeric components in formatted counter text.
pub(crate) fn component_ranges(text: &str, format: TimeFormat) -> Vec<std::ops::Range<usize>> {
    if format == TimeFormat::Samples {
        let start = usize::from(text.starts_with('-'));
        return (start < text.len()).then_some(start..text.len()).into_iter().collect();
    }
    let mut ranges = Vec::new();
    let mut start = None;
    for (index, ch) in text.char_indices() {
        if ch.is_ascii_digit() {
            start.get_or_insert(index);
        } else if let Some(from) = start.take() {
            ranges.push(from..index);
        }
    }
    if let Some(from) = start {
        ranges.push(from..text.len());
    }
    ranges
}

/// Parses every numeric component from formatted counter text.
fn components(text: &str, format: TimeFormat) -> Vec<i64> {
    component_ranges(text, format).iter().map(|range| text.get(range.clone()).and_then(|part| part.parse().ok()).unwrap_or(0)).collect()
}

/// Returns the entry width of each component for a time format.
fn widths(format: TimeFormat, nominal_frames: i64) -> Vec<usize> {
    match format {
        TimeFormat::BarsBeats => vec![18, 2, 3],
        TimeFormat::MinSecs => vec![18, 2, 3],
        TimeFormat::Timecode => vec![18, 2, 2, if nominal_frames >= 100 { 3 } else { 2 }],
        TimeFormat::FeetFrames => vec![18, 2],
        TimeFormat::Samples => vec![18],
    }
}

/// Parses counter digits while saturating malformed or overflowing input.
fn digits_value(text: &str) -> i64 {
    text.parse().unwrap_or(i64::MAX)
}

/// Distributes unseparated digits across counter components from right to left.
fn right_aligned_components(digits: &str, widths: &[usize], count: usize) -> Vec<i64> {
    let mut parts = vec![0; count];
    let mut end = digits.len();
    for unit in (0..count).rev() {
        if end == 0 {
            break;
        }
        let width = widths.get(unit).copied().unwrap_or(18);
        let start = if unit == 0 { 0 } else { end.saturating_sub(width) };
        if let Some(value) = digits.get(start..end).and_then(|part| part.parse().ok()) {
            parts[unit] = value;
        }
        end = start;
    }
    parts
}

/// Steps one component and propagates any borrow or carry to larger units.
fn step_components(parts: &mut [i64], unit: usize, direction: i64, format: TimeFormat, target: CounterTarget, value: Samples, session: &Session) {
    let original = parts.to_vec();
    if format == TimeFormat::Timecode && session.frame_rate.is_drop() && unit == 3 && direction < 0 {
        let dropped = if session.frame_rate.nominal() >= 60 { 4 } else { 2 };
        if part(parts, 1) % 10 != 0 && part(parts, 2) == 0 && part(parts, 3) == dropped {
            if let Some(frame) = parts.get_mut(3) {
                *frame = session.frame_rate.nominal().saturating_sub(1);
            }
            if let Some(second) = parts.get_mut(2) {
                *second = 59;
            }
            if let Some(minute) = parts.get_mut(1) {
                *minute = minute.saturating_sub(1);
            }
            return;
        }
    }
    let Some(part) = parts.get_mut(unit) else { return };
    *part = part.saturating_add(direction);
    if unit == 0 {
        return;
    }
    // Lengths are relative to the selection start; position counters use their own location.
    let meter_sample = if target == CounterTarget::Length { session.edit.selection.start } else { value };
    let meter_tick = session.tempo.samples_to_ticks(meter_sample, session.sample_rate);
    let meter = session.tempo.meter_at_tick(meter_tick);
    let bounds = |index: usize| -> Option<(i64, i64)> {
        match (format, index) {
            (TimeFormat::BarsBeats, 1) => {
                let minimum = i64::from(target != CounterTarget::Length);
                Some((minimum, i64::from(meter.numerator.max(1))))
            }
            (TimeFormat::BarsBeats, 2) => Some((0, meter.ticks_per_beat().max(1))),
            (TimeFormat::MinSecs, 1) | (TimeFormat::Timecode, 1 | 2) => Some((0, 60)),
            (TimeFormat::MinSecs, 2) => Some((0, 1_000)),
            (TimeFormat::Timecode, 3) => Some((0, session.frame_rate.nominal().max(1))),
            (TimeFormat::FeetFrames, 1) => Some((0, 16)),
            _ => None,
        }
    };
    let mut index = unit;
    while index > 0 {
        let Some((minimum, radix)) = bounds(index) else { break };
        if parts[index] < minimum {
            parts[index] = minimum.saturating_add(radix).saturating_sub(1);
            parts[index - 1] = parts[index - 1].saturating_sub(1);
        } else if parts[index] >= minimum.saturating_add(radix) {
            parts[index] = parts[index].saturating_sub(radix);
            parts[index - 1] = parts[index - 1].saturating_add(1);
        } else {
            break;
        }
        index -= 1;
    }
    let first_minimum = i64::from(format == TimeFormat::BarsBeats && target != CounterTarget::Length);
    if parts.first().is_some_and(|part| *part < first_minimum) {
        parts.copy_from_slice(&original);
    }
}

/// Enforces the zero- or one-based minimums of formatted counter components.
fn normalize_component_mins(parts: &mut [i64], format: TimeFormat, target: CounterTarget) {
    for part in parts.iter_mut() {
        *part = (*part).max(0);
    }
    if format == TimeFormat::BarsBeats && target != CounterTarget::Length {
        if let Some(bar) = parts.get_mut(0) {
            *bar = (*bar).max(1);
        }
        if let Some(beat) = parts.get_mut(1) {
            *beat = (*beat).max(1);
        }
    }
}

/// Moves an entered drop-frame label to the first valid frame when necessary.
fn normalize_drop_frame_label(parts: &mut [i64], format: TimeFormat, session: &Session) {
    if format != TimeFormat::Timecode || !session.frame_rate.is_drop() {
        return;
    }
    let minute = part(parts, 1);
    let second = part(parts, 2);
    let dropped = if session.frame_rate.nominal() >= 60 { 4 } else { 2 };
    if minute % 10 != 0
        && second == 0
        && let Some(frame) = parts.get_mut(3)
    {
        *frame = (*frame).max(dropped);
    }
}

/// Converts edited counter components back to an absolute position or duration.
fn compose(session: &Session, target: CounterTarget, format: TimeFormat, parts: &[i64]) -> Samples {
    if target == CounterTarget::Length && format == TimeFormat::BarsBeats {
        let start_tick = session.tempo.samples_to_ticks(session.edit.selection.start, session.sample_rate);
        let meter = session.tempo.meter_at_tick(start_tick);
        let ticks = part(parts, 0)
            .saturating_mul(meter.ticks_per_bar())
            .saturating_add(part(parts, 1).saturating_mul(meter.ticks_per_beat()))
            .saturating_add(part(parts, 2));
        return session.tempo.tick_to_samples(start_tick.saturating_add(ticks), session.sample_rate).saturating_sub(session.edit.selection.start);
    }
    let text = match format {
        TimeFormat::BarsBeats => format!("{}|{}|{}", part(parts, 0), part(parts, 1), part(parts, 2)),
        TimeFormat::MinSecs => format!("{}:{:02}.{:03}", part(parts, 0), part(parts, 1), part(parts, 2)),
        TimeFormat::Timecode => format!("{:02}:{:02}:{:02}:{:02}", part(parts, 0), part(parts, 1), part(parts, 2), part(parts, 3)),
        TimeFormat::FeetFrames => format!("{}+{:02}", part(parts, 0), part(parts, 1)),
        TimeFormat::Samples => part(parts, 0).to_string(),
    };
    let value = if target == CounterTarget::Length {
        parse_position(&text, format, session.sample_rate, &session.tempo, session.frame_rate, 0, 0).unwrap_or(0)
    } else {
        session.parse_position(&text, format).unwrap_or(0)
    };
    if format == TimeFormat::Timecode
        && let Ok(label) = Timecode::parse(&text)
    {
        let offset = if target == CounterTarget::Length { 0 } else { session.timecode_start };
        let actual = session.frame_rate.frames_at(value.saturating_add(offset), session.sample_rate);
        value.saturating_add(label.to_frames(session.frame_rate).saturating_sub(actual).signum())
    } else {
        value
    }
}

/// Reads a component, returning zero when the format does not provide it.
fn part(parts: &[i64], index: usize) -> i64 {
    parts.get(index).copied().unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Whole-field entry assigns the newest digits to the smallest units.
    #[test]
    fn whole_counter_entry_fills_from_the_smallest_unit() {
        let session = Session::default();
        let mut edit = CounterEdit::new(CounterTarget::Main, TimeFormat::Timecode, 0, None);
        for digit in "123416".chars() {
            edit.digit(&session, digit);
        }
        assert_eq!(edit.display(&session), "00:12:34:16");
    }

    /// Typed unit edits clear smaller units while arrow edits preserve them.
    #[test]
    fn unit_typing_zeros_smaller_units_but_arrows_preserve_them() {
        let session = Session::default();
        let value = session.parse_position("01:02:03:04", TimeFormat::Timecode).unwrap();
        let mut edit = CounterEdit::new(CounterTarget::Main, TimeFormat::Timecode, value, Some(1));
        edit.digit(&session, '5');
        assert_eq!(edit.display(&session), "01:05:00:00");
        edit.step(&session, 1);
        assert_eq!(edit.display(&session), "01:06:00:00");
    }

    /// Borrowing from a zero component leaves all smaller components intact.
    #[test]
    fn decrementing_a_zero_unit_borrows_without_changing_smaller_units() {
        let session = Session::default();
        let value = session.parse_position("01:00:00:15", TimeFormat::Timecode).unwrap();
        let mut edit = CounterEdit::new(CounterTarget::Main, TimeFormat::Timecode, value, Some(1));
        edit.step(&session, -1);
        assert_eq!(edit.display(&session), "00:59:00:15");
    }

    /// Frame fields accept three digits at rates of 100 fps and above.
    #[test]
    fn high_frame_rate_timecode_accepts_three_frame_digits() {
        let session = Session { frame_rate: soundcraft_time::FrameRate::Fps120, ..Session::default() };
        let mut edit = CounterEdit::new(CounterTarget::Main, TimeFormat::Timecode, 0, Some(3));
        for digit in "119".chars() {
            edit.digit(&session, digit);
        }
        assert_eq!(edit.display(&session), "00:00:00:119");
    }

    /// Drop-frame stepping skips labels omitted at non-tenth minutes.
    #[test]
    fn drop_frame_entry_skips_labels_that_do_not_exist() {
        let session = Session { frame_rate: soundcraft_time::FrameRate::Fps29_97Drop, ..Session::default() };
        let mut edit = CounterEdit::new(CounterTarget::Main, TimeFormat::Timecode, 0, Some(1));
        edit.digit(&session, '1');
        assert_eq!(edit.display(&session), "00:01:00;02");
        edit.unit = Some(3);
        edit.step(&session, -1);
        assert_eq!(edit.display(&session), "00:00:59;29");
    }

    /// Horizontal navigation clamps at the first and last component.
    #[test]
    fn left_and_right_choose_adjacent_units() {
        let session = Session::default();
        let mut edit = CounterEdit::new(CounterTarget::Start, TimeFormat::MinSecs, 0, Some(1));
        edit.move_unit(-1, &session);
        assert_eq!(edit.unit, Some(0));
        edit.move_unit(1, &session);
        edit.move_unit(1, &session);
        edit.move_unit(1, &session);
        assert_eq!(edit.unit, Some(2));
    }

    /// Bars|Beats duration entry uses zero-based duration components.
    #[test]
    fn length_bar_entry_uses_zero_based_components() {
        let session = Session::default();
        let mut edit = CounterEdit::new(CounterTarget::Length, TimeFormat::BarsBeats, 0, None);
        for digit in "1000".chars() {
            edit.digit(&session, digit);
        }
        assert_eq!(edit.value, 24_000);
        assert_eq!(edit.display(&session), "0| 1| 000");
    }

    /// Relative additions and subtractions operate on the active timebase.
    #[test]
    fn calculator_entry_adds_and_subtracts_a_time_value() {
        let session = Session::default();
        let ten_seconds = session.parse_position("00:10.000", TimeFormat::MinSecs).unwrap();
        let mut edit = CounterEdit::new(CounterTarget::Start, TimeFormat::MinSecs, ten_seconds, None);

        edit.begin_calculation(1);
        for digit in "1500".chars() {
            edit.digit(&session, digit);
        }
        assert_eq!(edit.display(&session), "0:11.500");

        edit.begin_calculation(-1);
        for digit in "2500".chars() {
            edit.digit(&session, digit);
        }
        assert_eq!(edit.display(&session), "0:09.000");
    }

    /// Main-counter borrowing uses its own meter when the selection is elsewhere.
    #[test]
    fn main_counter_borrows_with_the_meter_at_its_own_position() {
        let mut session = Session::default();
        let bar_three = session.tempo.bar_start_tick(3);
        session.tempo.set_meter(bar_three, 3, 4).unwrap();
        session.edit.selection.start = session.tempo.samples_at_bar_beat(soundcraft_time::BarBeat { bar: 3, beat: 1, tick: 0 }, session.sample_rate);
        let value = session.tempo.samples_at_bar_beat(soundcraft_time::BarBeat { bar: 2, beat: 1, tick: 0 }, session.sample_rate);
        let mut edit = CounterEdit::new(CounterTarget::Main, TimeFormat::BarsBeats, value, Some(1));

        edit.step(&session, -1);

        assert_eq!(edit.display(&session), "1| 4| 000");
    }
}
