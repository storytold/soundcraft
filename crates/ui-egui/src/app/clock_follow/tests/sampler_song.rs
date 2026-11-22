use soundcraft_midi::{Note, Sequence};
use soundcraft_model::{ChannelFormat, Clip, Insert, Session, SourceAudio, SourceId, TrackKind};
use soundcraft_time::{SampleRate, TempoMap};
use std::sync::Arc;

/// An eighth-note pattern playing a known 96-sample burst at its native pitch.
pub(super) fn sampler_song(grid: TempoMap, sr: SampleRate, count: usize, end: i64) -> Session {
    let mut song = Session { tempo: grid, sample_rate: sr, ..Default::default() };
    let source = SourceId(200);
    // A varying waveform reveals duplicated or stretched samples. Keep its envelope
    // above the gate so a single voice has one unambiguous attack and release.
    let burst = (0..96).map(|i| 0.5 + 0.3 * (i as f32 * 0.31).sin()).collect();
    song.pool.insert(source, Arc::new(SourceAudio::new(craft_audio::AudioBuffer { sample_rate: sr.hz(), channels: vec![burst] })));
    let id = song.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let clip = song.new_clip_id();
    let tr = song.track_mut(id).unwrap();
    let mut inst = Insert::new("sampler");
    inst.sample = Some(source);
    inst.params.insert("root_note".into(), 60.);
    inst.params.insert("attack".into(), 0.);
    tr.instrument = Some(inst);
    let mut seq = Sequence::default();
    for i in 0..count {
        seq.notes.push(Note { start: i as i64 * 480, length: 80, pitch: 60, channel: 0, velocity: 110, release_velocity: 64 });
    }
    tr.playlist_mut().unwrap().clips.push(Clip::midi(clip, "hats", 0, end.saturating_add(i64::from(sr.hz())), seq));
    song
}
