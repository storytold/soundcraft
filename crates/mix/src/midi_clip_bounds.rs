//! Preserve the musical start and exclusive end of MIDI clips across tempo-map changes.
use soundcraft_model::{ClipContent, ClipId, Session};
use soundcraft_time::{SampleRate, Samples, TempoMap};

#[derive(Clone, Copy)]
struct Anchor {
    tick: i64,
    fraction: f64,
}

impl Anchor {
    fn new(sample: Samples, tempo: &TempoMap, sr: SampleRate) -> Self {
        let nearest = tempo.samples_to_ticks(sample, sr);
        let at = tempo.tick_to_samples(nearest, sr);
        if sample == at {
            return Self { tick: nearest, fraction: 0. };
        }
        // Preserve sub-tick trims, including the distinction between a note exactly
        // at the excluded end and one a sample before it.
        let tick = if sample < at { nearest.saturating_sub(1) } else { nearest };
        let left = tempo.tick_to_samples(tick, sr);
        let right = tempo.tick_to_samples(tick.saturating_add(1), sr);
        let fraction = sample.saturating_sub(left) as f64 / right.saturating_sub(left).max(1) as f64;
        Self { tick, fraction }
    }

    fn sample(self, tempo: &TempoMap, sr: SampleRate) -> Samples {
        let left = tempo.tick_to_samples(self.tick, sr);
        if self.fraction == 0. {
            return left;
        }
        let right = tempo.tick_to_samples(self.tick.saturating_add(1), sr);
        left.saturating_add((right.saturating_sub(left) as f64 * self.fraction).round() as Samples)
    }
}

struct ClipBounds {
    track: usize,
    playlist: usize,
    clip: usize,
    start: Anchor,
    end: Anchor,
}

/// Cached on the host for the lifetime of one immutable score revision. A tempo-only
/// update reuses the same playlist layout and always maps the original boundaries.
#[derive(Default)]
pub struct MidiClipBeatBounds(Vec<ClipBounds>);

impl MidiClipBeatBounds {
    pub fn from_score(score: &Session, include: impl Fn(ClipId) -> bool) -> Self {
        let mut bounds = Vec::new();
        for (track, tr) in score.tracks.iter().enumerate().filter(|(_, tr)| tr.ticks_timebase) {
            for (playlist, pl) in tr.playlists.iter().enumerate() {
                for (clip, c) in pl.clips.iter().enumerate() {
                    // Keep locked clip boundaries at their protected sample positions.
                    if !c.edit_locked && !c.time_locked && matches!(c.content, ClipContent::Midi { .. }) && include(c.id) {
                        bounds.push(ClipBounds {
                            track,
                            playlist,
                            clip,
                            start: Anchor::new(c.start, &score.tempo, score.sample_rate),
                            end: Anchor::new(c.end(), &score.tempo, score.sample_rate),
                        });
                    }
                }
            }
        }
        Self(bounds)
    }

    /// Apply to the same playlist layout used by `from_score`, before changing its structure.
    pub fn apply(&self, session: &mut Session, tempo: &TempoMap) {
        for b in &self.0 {
            let clip = &mut session.tracks[b.track].playlists[b.playlist].clips[b.clip];
            clip.start = b.start.sample(tempo, session.sample_rate);
            clip.length = b.end.sample(tempo, session.sample_rate).saturating_sub(clip.start).max(0);
        }
    }
}
