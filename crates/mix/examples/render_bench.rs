//! Offline mix throughput on a synthetic many-track session:
//! `cargo run --release -p soundcraft-mix --example render_bench -- [tracks] [seconds]`

use soundcraft_audio_io::AudioBuffer;
use soundcraft_model::{ChannelFormat, Clip, Insert, Session, SourceAudio, SourceId, TrackKind};
use soundcraft_time::Range;
use std::sync::Arc;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<usize> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let tracks = args.first().copied().unwrap_or(64);
    let secs = args.get(1).copied().unwrap_or(30);
    let mut s = Session::default();
    let frames = 48_000 * secs;
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![(0..frames).map(|i| ((i as f32) * 0.01).sin() * 0.1).collect()] };
    s.pool.insert(SourceId(1_000_000), Arc::new(SourceAudio::new(buf)));
    for i in 0..tracks {
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let id = s.new_clip_id();
        let tr = s.track_mut(t).expect("track");
        if let Some(p) = tr.playlist_mut() {
            p.clips.push(Clip::audio(id, "c", SourceId(1_000_000), 0, 0, frames as i64));
        }
        // Every track gets an EQ and a compressor, every 4th a reverb.
        if std::env::var_os("NO_FX").is_none() {
            tr.mixer.inserts[0] = Some(Insert::new("eq_7band"));
        }
        if std::env::var_os("NO_FX").is_none() {
            tr.mixer.inserts[1] = Some(Insert::new("compressor"));
        }
        if i % 4 == 0 && std::env::var_os("NO_FX").is_none() {
            tr.mixer.inserts[2] = Some(Insert::new("plate_reverb"));
        }
    }
    let t0 = Instant::now();
    let out = soundcraft_mix::render_range(&s, Range::new(0, frames as i64), 512)?;
    let el = t0.elapsed().as_secs_f64();
    let peak = out.iter().flat_map(|c| c.iter()).fold(0.0f32, |m, v| m.max(v.abs()));
    println!("{tracks} tracks × {secs} s with EQ+comp (+reverb on 1/4): rendered in {el:.2} s → {:.1}× realtime (peak {peak:.2})", secs as f64 / el);
    Ok(())
}
