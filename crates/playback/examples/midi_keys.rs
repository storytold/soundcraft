//! A keyboard without the hardware: makes a virtual MIDI output port and plays an arpeggio on it,
//! so MIDI input can be tried (and tested by agents) on a machine with no MIDI device.
//!
//! `cargo run -p soundcraft-playback --example midi_keys -- [seconds] [channel 1-16]`
//!
//! SoundCraft lists the port as "SoundCraft test keys" in Setup › Playback Engine a moment after
//! this starts. Linux and macOS only (Windows has no virtual MIDI ports).

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use midir::os::unix::VirtualOutput;
    use std::time::{Duration, Instant};
    let mut args = std::env::args().skip(1);
    let seconds: f64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(10.0);
    let channel: u8 = args.next().and_then(|a| a.parse::<u8>().ok()).map_or(0, |c| c.clamp(1, 16) - 1);
    let mut out = midir::MidiOutput::new("SoundCraft test keys")?.create_virtual("SoundCraft test keys").map_err(|e| e.to_string())?;
    println!("virtual MIDI port \"SoundCraft test keys\" is up; playing on channel {} for {seconds} s", channel + 1);
    // Time for a listener to notice the port and connect.
    std::thread::sleep(Duration::from_secs(4));
    let start = Instant::now();
    for note in [60u8, 64, 67, 72].into_iter().cycle() {
        if start.elapsed().as_secs_f64() >= seconds {
            break;
        }
        out.send(&[0x90 | channel, note, 100])?;
        std::thread::sleep(Duration::from_millis(200));
        out.send(&[0x80 | channel, note, 0])?;
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn main() {
    eprintln!("virtual MIDI ports need Linux or macOS");
}
