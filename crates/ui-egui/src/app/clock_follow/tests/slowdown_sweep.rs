//! Replay a 20% slowdown across every F8 phase of a beat and four callback phases.
//! Set SOUNDCRAFT_SWEEP_DIR to export a row for every expected or extra PCM attack.

use super::sampler_song::sampler_song;
use super::*;
use soundcraft_mix::MixEngine;
use soundcraft_model::{ClipContent, Session};
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

const RATE: i64 = 48_000;
const CHUNK: usize = 512;
const FIRST: i64 = 360;
const LAST: i64 = 480;
const NOTES: usize = ((LAST - FIRST) / 12 + 1) as usize;
const HEADER: &str = "from_bpm,to_bpm,buffer,lead_ms,cut_pulse,callback_phase_samples,stem,event_pulse,ref_sample,actual_sample,err_samples,tail_samples,waveform_ok,slot_count,first_offbeat_pulse,first_beat_pulse";

#[derive(Clone, Copy, Debug)]
struct Case {
    bpm: i64,
    buffer: usize,
    lead_ms: i64,
    cut: i64,
    phase: usize,
}

impl Case {
    fn at(self, pulse: i64) -> i64 {
        let period = RATE * 60 / (self.bpm * 24);
        pulse.min(self.cut) * period + (pulse - self.cut).max(0) * (period * 5 / 4)
    }

    fn first_callback(self) -> i64 {
        self.at(FIRST - 24) + self.phase as i64
    }

    fn first_sample(self) -> i64 {
        self.first_callback() + self.lead_ms * 48
    }
}

fn micros(samples: i64) -> i64 {
    (samples * 1_000_000 + RATE / 2) / RATE
}

fn note_song(bpm: i64, first: i64, end: i64) -> Session {
    let mut song = sampler_song(TempoMap::new(bpm as f64, 4, 4).unwrap(), SampleRate::HZ_48000, NOTES, end);
    let clip = &mut song.tracks[0].playlist_mut().unwrap().clips[0];
    let sequence = match &mut clip.content {
        ClipContent::Midi { sequence } => Some(sequence),
        _ => None,
    }
    .unwrap();
    for (i, note) in sequence.notes.iter_mut().enumerate() {
        note.start += first * TICKS_PER_PULSE;
        note.velocity = 60 + i as u8 * 6;
    }
    song
}

// Merge brief zero crossings within a click without joining separate attacks.
fn bursts(pcm: &[f32]) -> Vec<std::ops::Range<usize>> {
    let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
    for (i, sample) in pcm.iter().enumerate() {
        if sample.abs() > 1e-10 {
            if let Some(last) = runs.last_mut()
                && i - last.end <= 12
            {
                last.end = i + 1;
            } else {
                runs.push(i..i + 1);
            }
        }
    }
    runs
}

fn templates() -> [Vec<Vec<f32>>; 2] {
    let song = note_song(120, 0, 150_000);
    let mut click = Session { tempo: song.tempo.clone(), sample_rate: song.sample_rate, ..Default::default() };
    click.edit.click = true;
    let mut mix = MixEngine::new(RATE as f32, CHUNK);
    let mut audio = vec![vec![0.; CHUNK]; 2];
    let mut pcm = Vec::new();
    for at in (0..125_000).step_by(CHUNK) {
        mix.render(&song, at, CHUNK, &mut audio);
        pcm.extend_from_slice(&audio[0]);
    }
    let hats: Vec<_> = bursts(&pcm).into_iter().map(|r| pcm[r].to_vec()).collect();
    assert_eq!(hats.len(), NOTES);
    assert!(hats.iter().enumerate().all(|(i, wave)| wave.len() == 96 && !hats[..i].contains(wave)));
    let mut clicks = Vec::new();
    for at in [0, RATE / 2] {
        let mut metro = MixEngine::new(RATE as f32, 2048);
        metro.metronome = true;
        let mut block = vec![vec![0.; 2048]; 2];
        metro.render(&click, at, 2048, &mut block);
        let runs = bursts(&block[0]);
        assert_eq!(runs.len(), 1);
        clicks.push(block[0][runs[0].clone()].to_vec());
    }
    [hats, clicks]
}

fn render(case: Case, prediction: bool) -> [Vec<f32>; 2] {
    let inputs = MidiInputs::with_port_names(vec![], vec!["wire".into()]);
    let feed = inputs.feed();
    let port = feed.ports().assign("wire");
    feed.set_clock_follow(Some(port), true, 0, false);
    feed.set_clock(0);
    feed.receive(port, &[0xfa]);
    let mut actions = Vec::new();
    let rate = SampleRate::HZ_48000;
    let mut song = note_song(case.bpm, FIRST, case.at(LAST) + RATE);
    let mut click = Session { tempo: song.tempo.clone(), sample_rate: rate, ..Default::default() };
    click.edit.click = true;
    let mut hats = MixEngine::new(RATE as f32, CHUNK);
    let mut metro = MixEngine::new(RATE as f32, CHUNK);
    metro.metronome = true;
    let mut audio = vec![vec![0.; CHUNK]; 2];
    let mut pcm = [Vec::new(), Vec::new()];
    let mut pulse = 0;
    let mut callback = case.first_callback();
    while callback + case.lead_ms * 48 < case.at(LAST) + 7680 {
        let pos = callback + case.lead_ms * 48;
        while case.at(pulse) <= callback {
            let arrival = case.at(pulse);
            feed.set_clock(micros(arrival));
            feed.receive(port, &[0xf8]);
            let snap = feed.clock_follow_status(&mut actions);
            actions.clear();
            // The device already owns all samples preceding the next presentation block.
            let guard = rate.samples((2.5 / snap.bpm.unwrap_or(120.)).max(case.buffer as f64 / RATE as f64));
            let immutable_until = if arrival >= case.first_callback() { pos + guard } else { 0 };
            if update_grid(&mut song.tempo, Some((0, 0)), 0, snap, rate, immutable_until) {
                click.tempo = song.tempo.clone();
            }
            pulse += 1;
        }
        feed.set_clock(micros(callback));
        for offset in (0..case.buffer).step_by(CHUNK) {
            let n = (case.buffer - offset).min(CHUNK);
            let at = pos + offset as i64;
            let hard = feed.clock_render_tick();
            let limit = if prediction { feed.clock_render_tick_for_output(micros(callback), micros(at + n as i64)) } else { hard };
            for (i, (mix, session)) in [(&mut hats, &song), (&mut metro, &click)].into_iter().enumerate() {
                if let Some(tick) = limit.filter(|_| limit > hard) {
                    mix.render_with_clock_prediction(session, at, n, &mut audio, tick);
                } else {
                    mix.render_with_clock_limit(session, at, n, &mut audio, limit);
                }
                pcm[i].extend_from_slice(&audio[0][..n]);
            }
        }
        callback += case.buffer as i64;
    }
    pcm
}

#[derive(Default, Debug)]
struct Counts {
    missing: usize,
    extra: usize,
    damaged: usize,
}

fn measure(case: Case, hats: bool, pcm: &[f32], references: &[Vec<f32>], counts: &mut Counts, report: &mut impl Write) -> io::Result<()> {
    let stride = if hats { 12 } else { 24 };
    let pulses: Vec<_> = (FIRST..=LAST).step_by(stride).collect();
    let runs = bursts(pcm);
    let mut slots = vec![Vec::new(); pulses.len()];
    for (ordinal, run) in runs.iter().enumerate() {
        // The first sine sample of the metronome is zero at the actual attack.
        let onset = case.first_sample() + run.start as i64 - i64::from(!hats);
        let wave = &pcm[run.clone()];
        // Prefer the encoded velocity even if an adjacent note disappeared. Fall back to
        // sequence order / nearest ideal attack only to localize damaged or click waveforms.
        let identity = if hats { references.iter().position(|r| r == wave) } else { None };
        let idx = identity
            .or_else(|| (runs.len() == pulses.len()).then_some(ordinal))
            .unwrap_or_else(|| pulses.iter().enumerate().min_by_key(|(_, p)| onset.abs_diff(case.at(**p))).unwrap().0);
        let template = if hats { idx } else { usize::from(pulses[idx] % 96 != 0) };
        slots[idx].push((onset, run.len(), wave == references[template]));
    }
    let stem = if hats { "hat" } else { "click" };
    let first_offbeat = case.cut / 24 * 24 + if case.cut % 24 < 12 { 12 } else { 36 };
    let first_beat = (case.cut / 24 + 1) * 24;
    for (pulse, slot) in pulses.into_iter().zip(slots) {
        let expected = case.at(pulse);
        let mut row = |event: Option<(i64, usize, bool)>| -> io::Result<()> {
            write!(
                report,
                "{},{},{},{},{},{},{stem},{pulse},{expected},",
                case.bpm,
                case.bpm * 4 / 5,
                case.buffer,
                case.lead_ms,
                case.cut,
                case.phase
            )?;
            if let Some((onset, length, ok)) = event {
                write!(report, "{onset},{},{length},{ok},", onset - expected)?;
            } else {
                write!(report, ",,,false,")?;
            }
            writeln!(report, "{},{first_offbeat},{first_beat}", slot.len())
        };
        if slot.is_empty() {
            counts.missing += 1;
            row(None)?;
        }
        counts.extra += slot.len().saturating_sub(1);
        for event in &slot {
            counts.damaged += usize::from(!event.2);
            row(Some(*event))?;
        }
    }
    Ok(())
}

fn report(dir: Option<&Path>, policy: &str) -> io::Result<Box<dyn Write>> {
    if let Some(dir) = dir {
        std::fs::create_dir_all(dir)?;
        let mut out = BufWriter::new(File::create(dir.join(format!("slow-{policy}.csv")))?);
        writeln!(out, "{HEADER}")?;
        Ok(Box::new(out))
    } else {
        Ok(Box::new(io::sink()))
    }
}

#[test]
#[ignore = "96 tempo-change and callback phases per device setting; run explicitly for clock-follow changes"]
fn slowdown_device_phase_sweep_preserves_pcm() -> io::Result<()> {
    let dir = std::env::var_os("SOUNDCRAFT_SWEEP_DIR").map(std::path::PathBuf::from);
    let mut reports = [report(dir.as_deref(), "hard")?, report(dir.as_deref(), "predict")?];
    let mut counts = [Counts::default(), Counts::default()];
    let refs = templates();
    for bpm in [200, 240, 300] {
        for buffer in [128, 512, 2048] {
            for lead_ms in [30, 50] {
                for cut in 384..408 {
                    for phase in [0, buffer / 4, buffer / 2, 3 * buffer / 4] {
                        let case = Case { bpm, buffer, lead_ms, cut, phase };
                        let hard = render(case, false);
                        let predicted = render(case, true);
                        for (policy, stems) in [hard, predicted].iter().enumerate() {
                            for (i, pcm) in stems.iter().enumerate() {
                                measure(case, i == 0, pcm, &refs[i], &mut counts[policy], &mut reports[policy])?;
                            }
                        }
                    }
                }
            }
        }
    }
    for report in &mut reports {
        report.flush()?;
    }
    for (policy, c) in ["hard", "predict"].into_iter().zip(&counts) {
        eprintln!("{policy}: {c:?}");
    }
    assert!(counts.iter().all(|c| c.missing == 0 && c.extra == 0 && c.damaged == 0), "{counts:?}");
    Ok(())
}
