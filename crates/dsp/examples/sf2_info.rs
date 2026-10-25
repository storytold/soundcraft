//! Print what a SoundFont holds: `cargo run -p soundcraft-dsp --example sf2_info -- bank.sf2`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

fn main() {
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).expect("read");
        let t = std::time::Instant::now();
        match soundcraft_dsp::soundfont::SoundFont::parse(&bytes) {
            Ok(sf) => {
                // Play middle C on the first preset for a second, to hear (well, measure) it.
                let sf = std::sync::Arc::new(sf);
                let mut player = soundcraft_dsp::create("soundfont").expect("player");
                player.prepare(48_000.0, 512, 2);
                player.set_soundfont(Some(std::sync::Arc::clone(&sf)));
                player.note_on(0, 60, 100);
                let mut buf = vec![vec![0.0f32; 512]; 2];
                let mut peak = 0.0f32;
                for _ in 0..94 {
                    player.process(&mut buf, 512);
                    peak = buf.iter().flatten().fold(peak, |m, x| m.max(x.abs()));
                }
                println!("middle C on preset 0: peak {peak:.3}");
                let sf = &*sf;
                println!("{path}: \"{}\" · {} sample points · {} presets · {:?}", sf.name, sf.samples.len(), sf.presets.len(), t.elapsed());
                for p in sf.presets.iter().take(200) {
                    let loops = p.regions.iter().filter(|r| r.loop_mode != soundcraft_dsp::soundfont::LoopMode::None).count();
                    let r0 = p.regions.first();
                    println!(
                        "  {:>3}:{:<3} {:<22} {} regions ({loops} looping) first: keys {:?} root {} rate {} env {:?}",
                        p.bank,
                        p.program,
                        p.name,
                        p.regions.len(),
                        r0.map(|r| r.keys),
                        r0.map_or(0, |r| r.root_key),
                        r0.map_or(0, |r| r.sample_rate),
                        r0.map(|r| r.env)
                    );
                }
            }
            Err(e) => println!("{path}: {e}"),
        }
    }
}
