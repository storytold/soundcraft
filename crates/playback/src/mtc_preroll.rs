//! Decide once at initial MTC lock whether a dry guide contains only digital
//! silence between the stopped locate and the expected presentation sample.
use soundcraft_model::{ClipContent, Route, Session, TrackKind};

pub(crate) fn silent_pcm_prefix(s: &Session, from: i64, to: i64) -> bool {
    // This runs on the audio thread at lock. Complex sessions retain the
    // protected downbeat without an unbounded search through their playlists.
    if to <= from || s.edit.click || s.tracks.len() > 16 || s.edit.values.len() > 64 || s.edit.values.keys().any(|key| key.starts_with("clip_fx.")) {
        return false;
    }
    let mut clips_left = 64;
    let mut samples_left = 32768;
    let mut checked_audio = false;
    for track in &s.tracks {
        let mixer = &track.mixer;
        if !matches!(track.kind, TrackKind::Audio | TrackKind::Master)
            || track.instrument.is_some()
            || track.elastic.is_some()
            || !track.automation.is_empty()
            || mixer.record_arm
            || mixer.input_monitor
            || mixer.input != Route::None
            || mixer.inserts.len() > 16
            || mixer.sends.len() > 16
            || mixer.inserts.iter().any(Option::is_some)
            || mixer.sends.iter().any(Option::is_some)
        {
            return false;
        }
        let Some(playlist) = track.playlist() else { return false };
        if playlist.clips.len() > clips_left {
            return false;
        }
        clips_left -= playlist.clips.len();
        for clip in &playlist.clips {
            let begin = from.max(clip.start);
            let end = to.min(clip.end());
            if begin >= end {
                continue;
            }
            let ClipContent::Audio { source, offset } = clip.content else { return false };
            let Some(audio) = s.pool.get(source) else { return false };
            if clip.stretch != 1.0 || audio.buffer.channels.is_empty() || audio.buffer.channels.len() > 8 {
                return false;
            }
            let a = offset.saturating_add(begin.saturating_sub(clip.start)).max(0) as usize;
            let b = offset.saturating_add(end.saturating_sub(clip.start)).max(0) as usize;
            for channel in &audio.buffer.channels {
                let span = &channel[a.min(channel.len())..b.min(channel.len())];
                if span.len() > samples_left || span.iter().any(|v| *v != 0.0) {
                    return false;
                }
                samples_left -= span.len();
            }
            checked_audio = true;
        }
    }
    checked_audio
}
