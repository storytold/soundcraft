use crate::surface_protocol::{STRIPS, mcu};
use soundcraft_model::TrackId;
use std::time::{Duration, Instant};

pub(super) const FADER_MAX: f32 = 16_383.0;
pub(super) const GESTURE_GAP: Duration = Duration::from_millis(300);
const TOUCH_TIMEOUT: Duration = Duration::from_secs(5);
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
    /// Final accepted Touch-mode move since the last automation sample.
    pub(super) unsampled: Option<u16>,
    /// Transport sample at physical touch-down in this pass.
    pub(super) touch_start: Option<i64>,
    previous: Option<u16>,
    picked_up: bool,
    contact: Contact,
    moved_at: Option<Instant>,
}

#[derive(Default)]
enum Contact {
    #[default]
    Unknown,
    HeldUntil(Instant),
    Released,
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
        self.strips.iter().filter_map(Strip::deadline).chain(self.pan_until).filter(|at| *at > now).min()
    }
}

impl Strip {
    fn held(&self, now: Instant) -> bool {
        matches!(self.contact, Contact::HeldUntil(until) if now < until)
    }

    fn deadline(&self) -> Option<Instant> {
        let movement = self.moved_at.map(|at| at + GESTURE_GAP);
        match self.contact {
            Contact::HeldUntil(until) => movement.max(Some(until)),
            _ => movement,
        }
    }

    pub(super) fn active(&self, now: Instant) -> bool {
        self.deadline().is_some_and(|until| now < until)
    }

    /// A different writer moved this track away from the physical fader. Keep
    /// its last physical position so a subsequent crossing can pick up again.
    pub(super) fn rearm(&mut self, target: u16) {
        if self.picked_up && self.previous.is_some_and(|value| value.abs_diff(target) > PICKUP_TOLERANCE) {
            self.picked_up = false;
            self.moved_at = None;
            self.unsampled = None;
        }
    }

    pub(super) fn writing(&self, now: Instant, motorized: bool) -> bool {
        if !motorized && !self.picked_up {
            return false;
        }
        // An explicit release flushes the final move, then ends Touch immediately.
        match self.contact {
            Contact::Released => false,
            _ => self.active(now),
        }
    }

    pub(super) fn touch(&mut self, held: bool, now: Instant) {
        self.contact = if held { Contact::HeldUntil(now + TOUCH_TIMEOUT) } else { Contact::Released };
    }

    pub(super) fn should_send(&mut self, value: u16, now: Instant) -> bool {
        if self.active(now) || self.sent == Some(value) {
            return false;
        }
        self.sent = Some(value);
        true
    }

    pub(super) fn accept(&mut self, value: u16, target: u16, motorized: bool, now: Instant) -> bool {
        if motorized && !self.held(now) && self.sent == Some(value) {
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
        if self.held(now) {
            self.touch(true, now);
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
    fn a_held_master_refreshes_its_timeout_only_on_its_own_activity() {
        let now = Instant::now();
        let mut motion = Motion::default();
        let master = &mut motion.strips[STRIPS];
        assert!(master.should_send(4_000, now));
        master.touch(true, now);
        assert!(master.accept(4_000, 4_000, true, now));
        let last_move = now + Duration::from_secs(4);
        assert!(master.accept(6_000, 4_000, true, last_move));
        assert_eq!(motion.deadline(last_move), Some(last_move + TOUCH_TIMEOUT));
        let just_before_timeout = last_move + TOUCH_TIMEOUT - Duration::from_millis(1);
        motion.strips[0].touch(true, just_before_timeout);
        assert!(!motion.strips[STRIPS].should_send(7_000, just_before_timeout));
        let timeout = last_move + TOUCH_TIMEOUT;
        assert!(!motion.strips[STRIPS].writing(timeout, true));
        assert!(motion.strips[STRIPS].should_send(7_000, timeout));
        assert!(!motion.strips[STRIPS].accept(7_000, 7_000, true, timeout));
        assert!(motion.strips[0].active(timeout));
    }

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
