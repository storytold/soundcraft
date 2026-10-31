//! End to end against the Audio Units Apple ships with macOS: scan → instantiate → params →
//! process → state, plus an instrument playing a note. Skips (passes) when a unit is missing.

use soundcraft_dsp::{Category, Plugin};

const LOWPASS: &str = "au:aufx:lpas:appl";
const DELAY: &str = "au:aufx:dely:appl";
const DLS: &str = "au:aumu:dls :appl";

fn available(id: &str) -> bool {
    let ok = soundcraft_au_host::scan().iter().any(|d| d.id == id);
    if !ok {
        eprintln!("{id} not installed; skipping");
    }
    ok
}

fn sine(freq: f32, sr: f32, n: usize, phase0: usize) -> Vec<f32> {
    (0..n).map(|i| (std::f32::consts::TAU * freq * (i + phase0) as f32 / sr).sin() * 0.5).collect()
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// Runs `freq` through the plugin in 512-frame blocks for ~0.5 s; RMS of the last half.
fn run_tone(p: &mut dyn Plugin, freq: f32) -> f32 {
    let sr = 48_000.0;
    let block = 512;
    let blocks = 48;
    let mut tail = Vec::new();
    for b in 0..blocks {
        let s = sine(freq, sr, block, b * block);
        let mut io = vec![s.clone(), s];
        p.process(&mut io, block);
        for c in &io {
            assert!(c.iter().all(|v| v.is_finite()));
        }
        if b >= blocks / 2 {
            tail.extend_from_slice(&io[0]);
        }
    }
    rms(&tail)
}

#[test]
fn scan_lists_apple_units() {
    let all = soundcraft_au_host::scan();
    for d in &all {
        assert!(d.id.starts_with(soundcraft_au_host::ID_PREFIX), "{d:?}");
        assert_eq!(soundcraft_au_host::parse_id(&d.id).map(|(t, s, m)| soundcraft_au_host::format_id(t, s, m)), Some(d.id.clone()));
        assert_eq!(d.is_instrument, d.component_type == "aumu");
    }
    if let Some(d) = all.iter().find(|d| d.id == LOWPASS) {
        assert_eq!(d.name, "AULowpass");
        assert_eq!(d.vendor, "Apple");
        assert_eq!(d.category, Category::Eq);
        assert!(!d.is_instrument);
    }
    if let Some(d) = all.iter().find(|d| d.id == DLS) {
        assert!(d.is_instrument);
        assert_eq!(d.category, Category::Instrument);
    }
    assert_eq!(soundcraft_au_host::rescan().len(), all.len());
}

#[test]
fn lowpass_attenuates_highs() {
    if !available(LOWPASS) {
        return;
    }
    let info = soundcraft_au_host::plugin_info(LOWPASS).unwrap();
    assert_eq!(info.id, LOWPASS);
    assert!(!info.params.is_empty(), "AULowpass has a cutoff");
    let mut p = soundcraft_au_host::create(LOWPASS).unwrap();
    p.prepare(48_000.0, 512, 2);
    let cutoff = info.params.iter().find(|q| q.unit == soundcraft_dsp::Unit::Hz).unwrap();
    assert!(p.set_param(cutoff.id, 500.0));
    assert!((p.param(cutoff.id).unwrap() - 500.0).abs() < 1e-3);
    assert!(!p.set_param("999999", 1.0));
    assert!(p.set_param(cutoff.id, f32::NAN), "NaN becomes the default, never reaches the unit");
    assert!(p.set_param(cutoff.id, 500.0));
    let low = run_tone(p.as_mut(), 100.0);
    p.reset();
    let high = run_tone(p.as_mut(), 12_000.0);
    eprintln!("lowpass rms: 100 Hz {low}, 12 kHz {high}");
    assert!(low > 0.2, "low tone passes: {low}");
    assert!(high < low * 0.1, "high tone attenuated: {high} vs {low}");

    // State round trip restores the cutoff.
    let state = p.save_state().expect("ClassInfo");
    assert!(p.set_param(cutoff.id, 5000.0));
    assert!(p.load_state(&state));
    assert!((p.param(cutoff.id).unwrap() - 500.0).abs() < 1.0, "{:?}", p.param(cutoff.id));
    // Hostile state is rejected without harm.
    assert!(!p.load_state(b""));
    assert!(!p.load_state(b"bplist00 garbage \x00\x01\x02\xff"));
    assert!(!p.load_state(&[0xffu8; 1000]));
    assert!(run_tone(p.as_mut(), 100.0).is_finite());
}

#[test]
fn state_reader_reads_while_another_thread_renders() {
    if !available(LOWPASS) {
        return;
    }
    let info = soundcraft_au_host::plugin_info(LOWPASS).unwrap();
    let cutoff = info.params.iter().find(|q| q.unit == soundcraft_dsp::Unit::Hz).unwrap();
    let mut p = soundcraft_au_host::create(LOWPASS).unwrap();
    p.prepare(48_000.0, 512, 2);
    assert!(p.set_param(cutoff.id, 500.0));
    let mut reader = p.state_reader().expect("every unit has a ClassInfo");
    let audio = std::thread::spawn(move || {
        for _ in 0..4 {
            run_tone(p.as_mut(), 100.0);
        }
        p
    });
    while !audio.is_finished() {
        assert!(reader.save_state().is_some());
    }
    let p = audio.join().unwrap();
    let state = reader.save_state().expect("ClassInfo");
    drop(p);
    let mut q = soundcraft_au_host::create(LOWPASS).unwrap();
    assert_eq!(reader.save_state(), None);
    assert!(q.load_state(&state));
    assert!((q.param(cutoff.id).unwrap() - 500.0).abs() < 1.0, "{:?}", q.param(cutoff.id));
}

#[test]
fn delay_runs_mono_stereo_and_odd_blocks() {
    if !available(DELAY) {
        return;
    }
    let mut p = soundcraft_au_host::instantiate(DELAY).unwrap();
    for (ch, block) in [(2usize, 1024usize), (1, 300), (2, 5000)] {
        p.prepare(44_100.0, block, ch);
        let mut io: Vec<Vec<f32>> = (0..ch).map(|_| sine(440.0, 44_100.0, block, 0)).collect();
        // Larger than prepared and empty blocks are clamped, not crashes.
        p.process(&mut io, block * 4);
        p.process(&mut io, 0);
        p.process(&mut [], block);
        for _ in 0..20 {
            p.process(&mut io, block);
        }
        assert!(io.iter().flatten().all(|v| v.is_finite()));
        assert!(rms(&io[0]) > 0.0);
    }
    // Hostile prepare values.
    p.prepare(f32::NAN, 0, 0);
    let mut io = vec![vec![0.25f32; 64]];
    p.process(&mut io, 64);
    assert!(io[0].iter().all(|v| v.is_finite()));
}

#[test]
fn dls_synth_plays_a_note() {
    if !available(DLS) {
        return;
    }
    let mut p = match soundcraft_au_host::instantiate(DLS) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("DLSMusicDevice unavailable: {e}");
            return;
        }
    };
    assert!(p.info().is_instrument);
    p.prepare(48_000.0, 512, 2);
    let mut silent = vec![vec![1.0f32; 512]; 2];
    p.process(&mut silent, 512);
    assert!(silent.iter().flatten().all(|v| v.is_finite() && v.abs() < 1e-3), "instruments replace the buffer");
    p.note_on(10, 60, 110);
    p.note_on(0, 200, 110); // ignored
    let mut energy = 0.0f32;
    for _ in 0..40 {
        let mut io = vec![vec![0.0f32; 512]; 2];
        p.process(&mut io, 512);
        assert!(io.iter().flatten().all(|v| v.is_finite()));
        energy = energy.max(rms(&io[0]));
    }
    eprintln!("DLS rms {energy}");
    assert!(energy > 1e-3, "note-on produced sound: {energy}");
    p.all_notes_off();
    // Mono output downmixes.
    p.prepare(48_000.0, 256, 1);
    p.note_on(0, 64, 100);
    let mut mono = vec![vec![0.0f32; 256]];
    let mut e = 0.0f32;
    for _ in 0..40 {
        p.process(&mut mono, 256);
        e = e.max(rms(&mono[0]));
    }
    assert!(e > 1e-3, "{e}");
    p.note_off(0, 64);
    p.reset();
    // Controllers reach the synth: MIDI volume 0 silences a note. (In this test because creating
    // two DLS instances at once on two threads crashes inside the unit.)
    p.prepare(48_000.0, 512, 2);
    let loudest = |p: &mut Box<dyn Plugin>, volume: u8| {
        p.midi(0, [0xB0, 7, volume]);
        p.note_on(0, 60, 110);
        let mut energy = 0.0f32;
        for _ in 0..20 {
            let mut io = vec![vec![0.0f32; 512]; 2];
            p.process(&mut io, 512);
            energy = energy.max(rms(&io[0]));
        }
        p.note_off(0, 60);
        p.reset();
        energy
    };
    let full = loudest(&mut p, 127);
    let muted = loudest(&mut p, 0);
    assert!(full > 1e-3 && muted < full * 0.01, "volume 127: {full}, volume 0: {muted}");
}
