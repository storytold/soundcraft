//! MIDI Time Code encoding on the shared presentation-clock worker.
use crate::Player;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use crate::sync_clock::{CATCH_UP_SPACING, Presentation, SyncEncoder};
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use craft_midi_sync::sync::{Rate, full_frame, quarter_frames};
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use soundcraft_model::Session;
use soundcraft_time::FrameRate;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use soundcraft_time::Timecode;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use std::time::Instant;

pub const UNSUPPORTED_RATE: &str = "MIDI Time Code needs a session at 23.976, 24, 25, 29.97 non-drop, 29.97 drop-frame or 30 fps.";

pub fn supported(rate: FrameRate) -> bool {
    matches!(rate, FrameRate::Fps23_976 | FrameRate::Fps24 | FrameRate::Fps25 | FrameRate::Fps29_97 | FrameRate::Fps29_97Drop | FrameRate::Fps30)
}

// craft-midi encodes integer-rate labels. For drop frame, craft-time supplies the skipped labels
// and the wire rate bits distinguish them from 30 non-drop. A MIDI day wraps at midnight.
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn label(frame: i64, rate: FrameRate) -> (f64, Rate) {
    let day = if rate.is_drop() { 2_589_408 } else { rate.nominal() * 86_400 };
    let time = Timecode::from_frames(frame.rem_euclid(day), rate);
    let seconds = (time.hours * 3600 + time.minutes * 60 + time.seconds) as f64 + time.frames as f64 / rate.nominal() as f64;
    (seconds, Rate::of(rate.nominal() as u8))
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn locate(frame: i64, rate: FrameRate) -> [u8; 10] {
    let (seconds, wire) = label(frame, rate);
    let mut message = full_frame(seconds, wire);
    if rate.is_drop() {
        message[5] = (message[5] & 0x1f) | 0x40;
    }
    message
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub(crate) fn quarters(frame: i64, rate: FrameRate) -> [[u8; 2]; 8] {
    let (seconds, wire) = label(frame, rate);
    let mut messages = quarter_frames(seconds, wire);
    if rate.is_drop() {
        messages[7][1] = (messages[7][1] & !6) | 4;
    }
    messages
}

/// The generator follows the same per-port DAC timeline and calibration as Beat Clock.
impl Player {
    pub fn set_mtc_generator(&self, port: Option<(String, i64)>, compensate: bool) {
        self.try_set_mtc_generator(port, compensate);
    }

    /// A rejected update can be retried during the next transport refresh.
    pub fn try_set_mtc_generator(&self, port: Option<(String, i64)>, compensate: bool) -> bool {
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        {
            self.try_set_sync_encoder(port, compensate, Box::<Encoder>::default())
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            let _ = compensate;
            port.is_none()
        }
    }
}

/// Quarter frames in one group (two frames). A piece at most this many quarter frames late
/// is still sent, catching up on the grid at least `CATCH_UP_SPACING` apart. Readers check
/// piece order, so a dropped piece costs the whole group while a late one only shifts it.
/// Further behind, the generator sends a Full Message and rejoins at the next group.
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
const RESYNC_QUARTERS: f64 = 8.;

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
#[derive(Default)]
pub(crate) struct Encoder {
    pass: Option<(u64, bool)>,
    lost_while_rolling: bool,
    next: i64,
    group: [[u8; 2]; 8],
    stale: u64,
    last_sent: Option<Instant>,
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
impl SyncEncoder for Encoder {
    fn update(&mut self, block: Presentation, now: Instant, session: &Session, send: &mut dyn FnMut(Instant, &[u8])) -> Option<Instant> {
        let rate = session.frame_rate;
        if !supported(rate) {
            self.reset();
            return None;
        }
        let sr = session.sample_rate;
        let start = session.timecode_start as f64;
        let at = block.sample_heard_at(now) + start;
        let quarter = rate.samples_per_frame(sr) / 4.;
        let relaunch = self.pass.replace((block.pass, block.playing)) != Some((block.pass, block.playing));
        // More than one group behind, or the timeline moved back past the group the
        // generator joined: locate to the live frame like a recovery.
        let behind = at / quarter - self.next as f64;
        let resync = !relaunch && block.playing && (behind > RESYNC_QUARTERS || behind < -(RESYNC_QUARTERS + 1.));
        let recovering = std::mem::take(&mut self.lost_while_rolling) || resync;
        if relaunch || recovering {
            // A worker normally wakes just after the requested first sample.
            // A late attachment or remap well into the pass locates to the live frame.
            let origin = block.origin as f64 + start;
            // When a worker first sees the roll several pieces late, locate to the
            // live time and join the next complete quarter-frame group.
            let recent_origin = !recovering && block.playing && (0. ..=quarter).contains(&(at - origin));
            let first = if recent_origin { origin } else { at };
            let frame = rate.frames_at(first.floor() as i64, sr);
            let stamp = if recent_origin { block.instant_for(block.origin as f64) } else { now };
            send(stamp, &locate(frame, rate));
            // An even frame can begin between samples. Its first representable
            // sample still belongs to this quarter-frame group's start.
            let group = frame.div_euclid(2);
            let first_sample = rate.samples_at_frame(group * 2, sr);
            let next = (group + i64::from(first > first_sample as f64)).saturating_mul(8);
            if recovering && !relaunch {
                self.stale = self.stale.saturating_add(next.saturating_sub(self.next).max(0) as u64);
            }
            self.next = next;
        }
        if !block.playing {
            return None;
        }
        let target = |piece: i64| piece as f64 * quarter - start;
        let sample = target(self.next);
        let deadline = block.instant_for(sample);
        if sample >= block.limit {
            return None;
        }
        if deadline > now {
            return Some(deadline);
        }
        // A late piece catches up on the grid, never closer than the catch-up spacing.
        if let Some(last) = self.last_sent
            && now < last + CATCH_UP_SPACING
        {
            return Some(last + CATCH_UP_SPACING);
        }
        let index = self.next.rem_euclid(8) as usize;
        if index == 0 {
            self.group = quarters(self.next.div_euclid(4), rate);
        }
        send(deadline, &self.group[index]);
        self.last_sent = Some(now);
        self.next = self.next.saturating_add(1);
        let sample = target(self.next);
        (sample < block.limit).then(|| block.instant_for(sample).max(now + CATCH_UP_SPACING))
    }
    fn timing_lost(&mut self) {
        self.lost_while_rolling |= self.pass.is_some_and(|(_, playing)| playing);
    }
    fn reset(&mut self) {
        self.lost_while_rolling = false;
        self.pass = None;
    }
    fn remap(&mut self, old: &Session, new: &Session) {
        if (old.frame_rate, old.sample_rate, old.timecode_start) != (new.frame_rate, new.sample_rate, new.timecode_start) {
            self.reset();
        }
    }
    fn take_stale_dropped(&mut self) -> u64 {
        std::mem::take(&mut self.stale)
    }
}

#[cfg(all(test, any(target_os = "windows", target_os = "macos", target_os = "linux")))]
mod tests;
