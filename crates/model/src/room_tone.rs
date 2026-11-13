//! The session's room tone: a stretch of recorded silence used to fill gaps instead of digital
//! silence.

use crate::SourceId;
use soundcraft_time::{MAX_POSITION, Samples};

/// A window onto an audio source chosen as the session's room tone. Filling a gap places clips
/// that play this window, so the fill stays ordinary, editable clips on the timeline.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RoomTone {
    pub source: SourceId,
    /// Where the window starts in the source (samples).
    pub offset: Samples,
    /// Window length (samples, > 0).
    pub length: Samples,
    /// Clip gain the fill clips get (dB), taken from the clip the tone was chosen from.
    #[serde(default)]
    pub gain_db: f32,
}

impl RoomTone {
    /// The window repaired for a source of `frames` frames, or `None` when nothing playable is left.
    pub fn sanitized(mut self, frames: u64) -> Option<RoomTone> {
        let frames = Samples::try_from(frames).unwrap_or(MAX_POSITION).min(MAX_POSITION);
        if self.offset < 0 || self.offset >= frames || self.length <= 0 {
            return None;
        }
        self.length = self.length.min(frames - self.offset);
        self.gain_db = if self.gain_db.is_finite() { self.gain_db.clamp(-144.0, 36.0) } else { 0.0 };
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(offset: Samples, length: Samples) -> RoomTone {
        RoomTone { source: SourceId(1), offset, length, gain_db: 0.0 }
    }

    #[test]
    fn a_window_past_the_source_is_cut_to_fit_or_dropped() {
        assert_eq!(tone(100, 50).sanitized(1000), Some(tone(100, 50)));
        assert_eq!(tone(900, 500).sanitized(1000), Some(tone(900, 100)));
        assert_eq!(tone(1000, 10).sanitized(1000), None);
        assert_eq!(tone(-1, 10).sanitized(1000), None);
        assert_eq!(tone(0, 0).sanitized(1000), None);
        let loud = RoomTone { gain_db: f32::NAN, ..tone(0, 10) };
        assert_eq!(loud.sanitized(1000).map(|t| t.gain_db), Some(0.0));
    }
}
