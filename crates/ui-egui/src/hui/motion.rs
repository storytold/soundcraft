use crate::surface_protocol::{STRIPS, mcu};
use soundcraft_model::TrackId;
use std::time::{Duration, Instant};

pub(super) const FADER_MAX: f32 = 16_383.0;
pub(super) const GESTURE_GAP: Duration = Duration::from_millis(300);
const PICKUP_TOLERANCE: u16 = 164;

#[derive(Default)]
pub(super) struct Motion {
    pub(super) strips: [Strip; mcu::FADERS],
    pub(super) pan_until: Option<Instant>,
}

#[derive(Default)]
pub(super) struct Strip {
    pub(super) track: Option<TrackId>,
    pub(super) sent: Option<u16>,
    previous: Option<u16>,
    picked_up: bool,
    held: Option<bool>,
    moved_at: Option<Instant>,
}

impl Motion {
    pub(super) fn map(&mut self, tracks: [Option<TrackId>; STRIPS], master: Option<TrackId>) {
        for (strip, track) in self.strips.iter_mut().zip(tracks.into_iter().chain([master])) {
            if strip.track != track {
                *strip = Strip { track, ..Strip::default() };
            }
        }
    }

    pub(super) fn active(&self, now: Instant) -> bool {
        self.pan_until.is_some_and(|until| now < until) || self.strips.iter().any(|strip| strip.active(now))
    }

    pub(super) fn deadline(&self, now: Instant) -> Option<Instant> {
        self.strips
            .iter()
            .filter(|strip| strip.held != Some(true))
            .filter_map(|strip| strip.moved_at.map(|at| at + GESTURE_GAP))
            .chain(self.pan_until)
            .filter(|at| *at > now)
            .min()
    }
}

impl Strip {
    pub(super) fn active(&self, now: Instant) -> bool {
        self.held == Some(true) || self.moved_at.is_some_and(|at| now < at + GESTURE_GAP)
    }

    pub(super) fn writing(&self, now: Instant, motorized: bool) -> bool {
        if !motorized && !self.picked_up {
            return false;
        }
        // A release ends Touch immediately. A movement decoded in the same frame can
        // still contribute its final value. Devices without touch use the idle timeout.
        match self.held {
            Some(false) => self.moved_at == Some(now),
            _ => self.active(now),
        }
    }

    pub(super) fn touch(&mut self, held: bool) {
        self.held = Some(held);
    }

    pub(super) fn should_send(&mut self, value: u16, now: Instant) -> bool {
        if self.active(now) || self.sent == Some(value) {
            return false;
        }
        self.sent = Some(value);
        true
    }

    pub(super) fn accept(&mut self, value: u16, target: u16, motorized: bool, now: Instant) -> bool {
        if motorized && self.held != Some(true) && self.sent == Some(value) {
            return false;
        }
        let previous = self.previous.replace(value);
        if !motorized && !self.picked_up {
            let crossed = previous.is_some_and(|old| (old.min(value)..=old.max(value)).contains(&target));
            if value.abs_diff(target) > PICKUP_TOLERANCE && !crossed {
                return false;
            }
            self.picked_up = true;
        }
        self.moved_at = Some(now);
        true
    }
}

pub(super) fn position(db: f32) -> u16 {
    (soundcraft_model::fader_db_to_pos(db) * FADER_MAX).round() as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remap_forgets_pickup_and_requests_new_motor_feedback() {
        let now = Instant::now();
        let mut motion = Motion::default();
        let mut tracks = [None; STRIPS];
        tracks[0] = Some(TrackId(1));
        motion.map(tracks, None);
        let strip = &mut motion.strips[0];
        assert!(!strip.accept(3_000, 12_000, false, now));
        assert!(!strip.accept(8_000, 12_000, false, now));
        assert!(strip.accept(13_000, 12_000, false, now));
        assert!(!strip.should_send(12_000, now));
        assert!(strip.should_send(12_000, now + GESTURE_GAP));
        assert!(!strip.accept(12_000, 12_000, true, now + GESTURE_GAP));

        tracks[0] = Some(TrackId(9));
        motion.map(tracks, None);
        let strip = &mut motion.strips[0];
        assert!(strip.should_send(12_000, now));
        assert!(!strip.accept(8_000, 12_000, false, now));
        assert!(strip.accept(11_900, 12_000, false, now));
    }
}
