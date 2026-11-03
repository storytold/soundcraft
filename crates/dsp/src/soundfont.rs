//! SoundFont 2 (`.sf2`) banks: the samples and the presets that map them across the keyboard.
//!
//! [`SoundFont::parse`] reads the RIFF `sfbk` file (written from the SoundFont 2.04 specification:
//! the `smpl` sample data and the `pdta` "hydra" of presets, instruments, zones and generators)
//! and flattens each preset into [`Region`]s: one sample with the key and velocity ranges it
//! answers, its loop, tuning, attenuation, pan and volume envelope, every generator already
//! combined (instrument values, plus the preset's additive offsets, global zones applied). A
//! player then needs nothing but a preset's regions and the sample data.
//!
//! Not read: modulators (the default velocity-to-attenuation curve is left to the player), the
//! filter and modulation envelope/LFO generators, 24-bit `sm24` data, ROM samples. Input is
//! hostile: every index and size is checked, a damaged file is refused, never trusted.

/// Largest file section kept per record list (a real bank has a few thousand records; the
/// terminal record is not counted).
const MAX_RECORDS: usize = 1 << 16;

/// Largest sample data kept, in sample points (128M, 256 MiB). A real bank is far smaller.
const MAX_SAMPLES: usize = 1 << 27;

/// Most zones one bag list may hold. A drum kit with 128 keys and stereo pairs is far below.
const MAX_ZONES: usize = 4096;

/// Most regions one preset may hold (a layered preset, not a whole bank's cross product).
const MAX_REGIONS: usize = 1024;

/// Most regions kept across the whole bank (real banks stay in the tens of thousands).
const MAX_TOTAL_REGIONS: usize = 1 << 18;

/// Most preset-zone/instrument-zone pairs resolved across the whole bank. This is the budget a
/// damaged file would otherwise turn into millions of combinations.
const MAX_RESOLVES: usize = 1 << 20;

/// A parsed bank.
pub struct SoundFont {
    /// The bank's name (`INAM`), or empty.
    pub name: String,
    /// Every sample point of the bank, 16-bit mono, as stored; regions index into it.
    pub samples: Vec<i16>,
    /// Presets in bank/program order.
    pub presets: Vec<Preset>,
}

impl std::fmt::Debug for SoundFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SoundFont").field("name", &self.name).field("samples", &self.samples.len()).field("presets", &self.presets.len()).finish()
    }
}

/// One playable sound of the bank (a General MIDI bank has one per program).
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub name: String,
    pub bank: u16,
    pub program: u16,
    pub regions: Vec<Region>,
}

/// How a region's sample loops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopMode {
    /// Plays once to its end.
    None,
    /// Loops for as long as the voice sounds.
    Continuous,
    /// Loops while the key is held, then plays on to the end.
    UntilRelease,
}

/// A volume envelope, times in seconds and the sustain as a gain (0..=1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Envelope {
    pub delay: f32,
    pub attack: f32,
    pub hold: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

/// One sample mapped to a key and velocity range, everything resolved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    /// Inclusive key and velocity ranges.
    pub keys: (u8, u8),
    pub vels: (u8, u8),
    /// Sample points `start..end` of [`SoundFont::samples`], and the loop inside them.
    pub start: usize,
    pub end: usize,
    pub loop_start: usize,
    pub loop_end: usize,
    pub loop_mode: LoopMode,
    /// The rate the sample was recorded at.
    pub sample_rate: u32,
    /// The key that plays the sample at its own pitch.
    pub root_key: u8,
    /// Tuning in cents (coarse, fine and the sample's pitch correction).
    pub tune_cents: f32,
    /// Cents per key away from the root (100: normal keyboard; 0: same pitch on every key).
    pub key_cents: f32,
    /// Attenuation in dB (0 or less).
    pub gain_db: f32,
    /// -1 (left) ..= 1 (right).
    pub pan: f32,
    pub env: Envelope,
    /// Notes of the same non-zero class cut each other off (open and closed hi-hat).
    pub exclusive_class: u16,
}

// Generator numbers (SoundFont 2.04, section 8.1.2).
const START_OFS: usize = 0;
const END_OFS: usize = 1;
const LOOP_START_OFS: usize = 2;
const LOOP_END_OFS: usize = 3;
const START_COARSE: usize = 4;
const END_COARSE: usize = 12;
const PAN: usize = 17;
const DELAY_VOL: usize = 33;
const ATTACK_VOL: usize = 34;
const HOLD_VOL: usize = 35;
const DECAY_VOL: usize = 36;
const SUSTAIN_VOL: usize = 37;
const RELEASE_VOL: usize = 38;
const INSTRUMENT: usize = 41;
const KEY_RANGE: usize = 43;
const VEL_RANGE: usize = 44;
const LOOP_START_COARSE: usize = 45;
const ATTENUATION: usize = 48;
const LOOP_END_COARSE: usize = 50;
const COARSE_TUNE: usize = 51;
const FINE_TUNE: usize = 52;
const SAMPLE_ID: usize = 53;
const SAMPLE_MODES: usize = 54;
const SCALE_TUNING: usize = 56;
const EXCLUSIVE_CLASS: usize = 57;
const ROOT_KEY: usize = 58;
const GENERATORS: usize = 61;

/// Generators a preset zone adds to the instrument's value (the others are instrument-only or
/// ranges, which intersect).
const ADDITIVE: [usize; 12] =
    [PAN, DELAY_VOL, ATTACK_VOL, HOLD_VOL, DECAY_VOL, SUSTAIN_VOL, RELEASE_VOL, ATTENUATION, COARSE_TUNE, FINE_TUNE, SCALE_TUNING, 8];

/// One zone: its generator amounts, ranges, and the instrument or sample it points to.
#[derive(Debug, Clone, Copy)]
struct Zone {
    gens: [Option<i16>; GENERATORS],
    keys: Option<(u8, u8)>,
    vels: Option<(u8, u8)>,
    link: Option<u16>,
}

impl Zone {
    const EMPTY: Zone = Zone { gens: [None; GENERATORS], keys: None, vels: None, link: None };
}

/// A list of zones and, if the first one has no link, the global zone before them.
struct ZoneList {
    global: Option<Zone>,
    zones: Vec<Zone>,
}

struct SampleHeader {
    start: u32,
    end: u32,
    loop_start: u32,
    loop_end: u32,
    rate: u32,
    pitch: u8,
    correction: i8,
    kind: u16,
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes([*b.get(i)?, *b.get(i + 1)?, *b.get(i + 2)?, *b.get(i + 3)?]))
}

/// A fixed-size name field: up to its first NUL, printable ASCII kept.
fn name_at(b: &[u8], i: usize, len: usize) -> String {
    let raw = b.get(i..i + len).unwrap_or(&[]);
    let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
    raw.iter().take(end).map(|&c| if (0x20..0x7F).contains(&c) { c as char } else { '?' }).collect::<String>().trim().to_string()
}

/// The sub-chunks of a chunk body: `(id, body)`, a short last one truncated to what is there.
fn chunks(mut b: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    while b.len() >= 8 && out.len() < 64 {
        let id = [b[0], b[1], b[2], b[3]];
        let size = u32_at(b, 4).map_or(0, |s| s as usize);
        let body = &b[8..];
        let size = size.min(body.len());
        out.push((id, &body[..size]));
        // Chunks are padded to an even size.
        let next = size.saturating_add(size & 1);
        b = body.get(next..).unwrap_or(&[]);
    }
    out
}

/// The records of a `pdta` list, `size` bytes each, without the terminal record.
fn records<'a>(body: &'a [u8], size: usize, what: &str) -> Result<Vec<&'a [u8]>, String> {
    if !body.len().is_multiple_of(size) || body.len() < size {
        return Err(format!("damaged {what} records"));
    }
    let n = body.len() / size;
    if n > MAX_RECORDS {
        return Err(format!("too many {what} records"));
    }
    Ok(body.chunks_exact(size).take(n - 1).collect())
}

impl SoundFont {
    /// Parse a whole `.sf2` file.
    pub fn parse(bytes: &[u8]) -> Result<SoundFont, String> {
        if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"sfbk" {
            return Err("not a SoundFont 2 file".into());
        }
        let riff = u32_at(bytes, 4).map_or(0, |s| s as usize).min(bytes.len() - 8);
        let body = bytes.get(12..8 + riff).unwrap_or(&[]);
        let mut name = String::new();
        let mut smpl: &[u8] = &[];
        let mut pdta: Vec<([u8; 4], &[u8])> = Vec::new();
        for (id, list) in chunks(body) {
            if &id != b"LIST" || list.len() < 4 {
                continue;
            }
            let (kind, inner) = list.split_at(4);
            for (sub, data) in chunks(inner) {
                match (kind, &sub) {
                    (b"INFO", b"INAM") => name = name_at(data, 0, data.len().min(256)),
                    (b"sdta", b"smpl") => smpl = data,
                    (b"pdta", _) => pdta.push((sub, data)),
                    _ => {}
                }
            }
        }
        let part =
            |id: &[u8; 4]| pdta.iter().find(|(x, _)| x == id).map(|(_, d)| *d).ok_or_else(|| format!("no {} list", String::from_utf8_lossy(id)));
        let phdr = records(part(b"phdr")?, 38, "preset")?;
        let pbag = part(b"pbag")?;
        let pgen = part(b"pgen")?;
        let inst = records(part(b"inst")?, 22, "instrument")?;
        let ibag = part(b"ibag")?;
        let igen = part(b"igen")?;
        let shdr = records(part(b"shdr")?, 46, "sample")?;
        if smpl.is_empty() {
            return Err("no sample data".into());
        }
        if smpl.len() / 2 > MAX_SAMPLES {
            return Err(format!("too much sample data ({} points)", smpl.len() / 2));
        }
        let samples: Vec<i16> = smpl.as_chunks::<2>().0.iter().map(|p| i16::from_le_bytes([p[0], p[1]])).collect();
        let headers: Vec<SampleHeader> = shdr
            .iter()
            .map(|r| SampleHeader {
                start: u32_at(r, 20).unwrap_or(0),
                end: u32_at(r, 24).unwrap_or(0),
                loop_start: u32_at(r, 28).unwrap_or(0),
                loop_end: u32_at(r, 32).unwrap_or(0),
                rate: u32_at(r, 36).unwrap_or(0),
                pitch: r.get(40).copied().unwrap_or(60),
                correction: r.get(41).map_or(0, |&c| c as i8),
                kind: u16_at(r, 44).unwrap_or(1),
            })
            .collect();
        // Bag index of each instrument, plus the terminal record's for the last one's end.
        let inst_bags: Vec<usize> = inst_bag_starts(part(b"inst")?);
        let instruments: Vec<ZoneList> = (0..inst.len())
            .map(|i| zone_list(inst_bags.get(i).copied(), inst_bags.get(i + 1).copied(), ibag, igen, SAMPLE_ID))
            .collect::<Result<_, _>>()?;
        let preset_bags: Vec<usize> = preset_bag_starts(part(b"phdr")?);
        let mut presets = Vec::with_capacity(phdr.len());
        // Preset zones times instrument zones: a few damaged records would otherwise ask for
        // millions of combinations (and keep the regions) from a few kilobytes.
        let mut total_regions = 0usize;
        let mut resolves = 0usize;
        for (i, r) in phdr.iter().enumerate() {
            if total_regions >= MAX_TOTAL_REGIONS || resolves >= MAX_RESOLVES {
                break;
            }
            let list = zone_list(preset_bags.get(i).copied(), preset_bags.get(i + 1).copied(), pbag, pgen, INSTRUMENT)?;
            let mut regions = Vec::new();
            for pz in &list.zones {
                let Some(ins) = pz.link.and_then(|l| instruments.get(usize::from(l))) else { continue };
                for iz in &ins.zones {
                    if regions.len() >= MAX_REGIONS || resolves >= MAX_RESOLVES {
                        break;
                    }
                    resolves += 1;
                    if let Some(region) = resolve(list.global.as_ref(), pz, ins.global.as_ref(), iz, &headers, samples.len()) {
                        regions.push(region);
                        total_regions += 1;
                    }
                }
                if regions.len() >= MAX_REGIONS {
                    break;
                }
            }
            presets.push(Preset { name: name_at(r, 0, 20), program: u16_at(r, 20).unwrap_or(0), bank: u16_at(r, 22).unwrap_or(0), regions });
        }
        presets.retain(|p| !p.regions.is_empty());
        presets.sort_by_key(|p| (p.bank, p.program));
        if presets.is_empty() {
            return Err("no playable preset".into());
        }
        Ok(SoundFont { name, samples, presets })
    }

    /// The preset a Program Change selects: that program in bank 0 (then any bank), if any.
    pub fn preset_for_program(&self, program: u8) -> Option<usize> {
        let p = u16::from(program);
        self.presets.iter().position(|x| x.bank == 0 && x.program == p).or_else(|| self.presets.iter().position(|x| x.program == p))
    }
}

/// The bag index each preset starts at (every `phdr` record, terminal included).
fn preset_bag_starts(body: &[u8]) -> Vec<usize> {
    body.as_chunks::<38>().0.iter().take(MAX_RECORDS + 1).map(|r| usize::from(u16_at(r, 24).unwrap_or(0))).collect()
}

/// The bag index each instrument starts at (every `inst` record, terminal included).
fn inst_bag_starts(body: &[u8]) -> Vec<usize> {
    body.as_chunks::<22>().0.iter().take(MAX_RECORDS + 1).map(|r| usize::from(u16_at(r, 20).unwrap_or(0))).collect()
}

/// The zones of bags `from..to`, their generators read from `gens`; `link` is the generator
/// that ends a zone (instrument for presets, sample for instruments).
fn zone_list(from: Option<usize>, to: Option<usize>, bags: &[u8], gens: &[u8], link: usize) -> Result<ZoneList, String> {
    let (Some(from), Some(to)) = (from, to) else { return Err("damaged zone index".into()) };
    // Bag `to` (the next list's first, or the terminal record) must exist: it ends the last zone.
    if to < from || to - from > MAX_ZONES || to.saturating_add(1).saturating_mul(4) > bags.len() {
        return Err("damaged zone index".into());
    }
    let gen_at = |bag: usize| u16_at(bags, bag * 4).map(usize::from);
    let mut zones = Vec::with_capacity(to - from);
    let mut global = None;
    for (n, bag) in (from..to).enumerate() {
        let (Some(g0), Some(g1)) = (gen_at(bag), gen_at(bag + 1)) else { return Err("damaged zone index".into()) };
        if g1 < g0 || g1.saturating_mul(4) > gens.len() {
            return Err("damaged generator index".into());
        }
        let mut z = Zone::EMPTY;
        for g in g0..g1 {
            let (Some(oper), Some(lo), Some(hi)) = (u16_at(gens, g * 4), gens.get(g * 4 + 2), gens.get(g * 4 + 3)) else { break };
            let oper = usize::from(oper);
            let amount = i16::from_le_bytes([*lo, *hi]);
            match oper {
                KEY_RANGE => z.keys = Some(((*lo).min(127), (*hi).min(127))),
                VEL_RANGE => z.vels = Some(((*lo).min(127), (*hi).min(127))),
                x if x == link => {
                    z.link = Some(u16::from_le_bytes([*lo, *hi]));
                    // Generators after the link are ignored.
                    break;
                }
                x if x < GENERATORS => z.gens[x] = Some(amount),
                _ => {}
            }
        }
        if z.link.is_some() {
            zones.push(z);
        } else if n == 0 {
            global = Some(z);
        }
    }
    Ok(ZoneList { global, zones })
}

fn intersect(a: Option<(u8, u8)>, b: Option<(u8, u8)>) -> Option<(u8, u8)> {
    let (a, b) = (a.unwrap_or((0, 127)), b.unwrap_or((0, 127)));
    let (lo, hi) = (a.0.max(b.0), a.1.min(b.1));
    (lo <= hi).then_some((lo, hi))
}

/// Timecents to seconds (the SoundFont minimum, -12000, is about a millisecond).
fn seconds(tc: i32) -> f32 {
    2f32.powf(tc.clamp(-12_000, 8_000) as f32 / 1200.0)
}

/// One instrument zone under one preset zone, as a playable region (`None` when it cannot play).
fn resolve(pglobal: Option<&Zone>, pz: &Zone, iglobal: Option<&Zone>, iz: &Zone, headers: &[SampleHeader], points: usize) -> Option<Region> {
    let inst = |g: usize, d: i32| iz.gens[g].or_else(|| iglobal.and_then(|z| z.gens[g])).map_or(d, i32::from);
    let add = |g: usize| pz.gens[g].or_else(|| pglobal.and_then(|z| z.gens[g])).map_or(0, i32::from);
    let total = |g: usize, d: i32| inst(g, d).saturating_add(if ADDITIVE.contains(&g) { add(g) } else { 0 });
    let keys = intersect(intersect(iz.keys.or(iglobal.and_then(|z| z.keys)), None), pz.keys.or(pglobal.and_then(|z| z.keys)))?;
    let vels = intersect(iz.vels.or(iglobal.and_then(|z| z.vels)), pz.vels.or(pglobal.and_then(|z| z.vels)))?;
    let h = headers.get(usize::from(iz.link?))?;
    if h.kind & 0x8000 != 0 || !(1_000..=400_000).contains(&h.rate) {
        return None;
    }
    let at = |base: u32, fine: usize, coarse: usize| -> usize {
        let v = i64::from(base) + i64::from(inst(fine, 0)) + i64::from(inst(coarse, 0)) * 32_768;
        v.clamp(0, points as i64) as usize
    };
    let start = at(h.start, START_OFS, START_COARSE);
    let end = at(h.end, END_OFS, END_COARSE);
    if end < start.saturating_add(2) {
        return None;
    }
    let (ls, le) = (at(h.loop_start, LOOP_START_OFS, LOOP_START_COARSE), at(h.loop_end, LOOP_END_OFS, LOOP_END_COARSE));
    let loop_ok = ls >= start && le <= end && le >= ls.saturating_add(2);
    let loop_mode = match inst(SAMPLE_MODES, 0) & 3 {
        1 if loop_ok => LoopMode::Continuous,
        3 if loop_ok => LoopMode::UntilRelease,
        _ => LoopMode::None,
    };
    let root = inst(ROOT_KEY, -1);
    let root_key = if (0..=127).contains(&root) {
        root as u8
    } else if h.pitch <= 127 {
        h.pitch
    } else {
        60
    };
    let cb = |g: usize| total(g, 0).clamp(0, 1440) as f32;
    Some(Region {
        keys,
        vels,
        start,
        end,
        loop_start: ls,
        loop_end: le,
        loop_mode,
        sample_rate: h.rate,
        root_key,
        tune_cents: (total(COARSE_TUNE, 0).clamp(-120, 120) * 100 + total(FINE_TUNE, 0).clamp(-99, 99) + i32::from(h.correction)) as f32,
        key_cents: total(SCALE_TUNING, 100).clamp(0, 1200) as f32,
        gain_db: -cb(ATTENUATION) / 10.0,
        pan: total(PAN, 0).clamp(-500, 500) as f32 / 500.0,
        env: Envelope {
            delay: seconds(total(DELAY_VOL, -12_000)),
            attack: seconds(total(ATTACK_VOL, -12_000)),
            hold: seconds(total(HOLD_VOL, -12_000)),
            decay: seconds(total(DECAY_VOL, -12_000)),
            sustain: 10f32.powf(-cb(SUSTAIN_VOL) / 200.0),
            release: seconds(total(RELEASE_VOL, -12_000)),
        },
        exclusive_class: inst(EXCLUSIVE_CLASS, 0).clamp(0, i32::from(u16::MAX)) as u16,
    })
}

/// A small bank built in code, for tests here and in other crates (no third-party audio).
#[doc(hidden)]
pub mod fixture {
    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        if body.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn list(kind: &[u8; 4], parts: &[Vec<u8>]) -> Vec<u8> {
        let mut body = kind.to_vec();
        for p in parts {
            body.extend_from_slice(p);
        }
        chunk(b"LIST", &body)
    }

    fn name20(n: &str) -> Vec<u8> {
        let mut v = n.as_bytes().to_vec();
        v.resize(20, 0);
        v
    }

    fn gen_rec(oper: u16, amount: [u8; 2]) -> Vec<u8> {
        let mut v = oper.to_le_bytes().to_vec();
        v.extend_from_slice(&amount);
        v
    }

    fn i(a: i16) -> [u8; 2] {
        a.to_le_bytes()
    }

    /// A small bank generated here (a sine wave, no third-party audio): one instrument with a
    /// global zone (attack) and two zones split at key 60, one looping; preset "Test" (bank 0,
    /// program 5) adds a semitone and 6 dB of attenuation in its global zone.
    pub fn test_bank() -> Vec<u8> {
        let wave: Vec<u8> = (0..2_000).flat_map(|n| (((n as f32 * 0.0626).sin() * 16_000.0) as i16).to_le_bytes()).collect();
        let mut phdr = Vec::new();
        for (name, prog, bag) in [("Test", 5u16, 0u16), ("EOP", 0, 2)] {
            phdr.extend(name20(name));
            phdr.extend(prog.to_le_bytes());
            phdr.extend(0u16.to_le_bytes());
            phdr.extend(bag.to_le_bytes());
            phdr.extend([0u8; 12]);
        }
        // Preset bags: global (gens 0..2), instrument zone (gens 2..3), terminal.
        let pbag: Vec<u8> = [0u16, 2, 3].iter().flat_map(|g| [g.to_le_bytes(), [0, 0]].concat()).collect();
        let mut pgen = gen_rec(51, i(1));
        pgen.extend(gen_rec(48, i(60)));
        pgen.extend(gen_rec(41, i(0)));
        pgen.extend(gen_rec(0, i(0)));
        let mut inst = Vec::new();
        for (name, bag) in [("Sine", 0u16), ("EOI", 3)] {
            inst.extend(name20(name));
            inst.extend(bag.to_le_bytes());
        }
        // Instrument bags: global (attack), low zone, high zone (looping), terminal.
        let ibag: Vec<u8> = [0u16, 1, 3, 6].iter().flat_map(|g| [g.to_le_bytes(), [0, 0]].concat()).collect();
        let mut igen = gen_rec(34, i(-1200));
        igen.extend(gen_rec(43, [0, 59]));
        igen.extend(gen_rec(53, i(0)));
        igen.extend(gen_rec(43, [60, 127]));
        igen.extend(gen_rec(54, i(1)));
        igen.extend(gen_rec(53, i(0)));
        igen.extend(gen_rec(0, i(0)));
        let mut shdr = Vec::new();
        for (name, start, end, ls, le, rate) in [("sine", 0u32, 1_990u32, 100u32, 1_100u32, 44_100u32), ("EOS", 0, 0, 0, 0, 0)] {
            shdr.extend(name20(name));
            for v in [start, end, ls, le, rate] {
                shdr.extend(v.to_le_bytes());
            }
            shdr.extend([69u8, 0]);
            shdr.extend(0u16.to_le_bytes());
            shdr.extend(1u16.to_le_bytes());
        }
        let imod = vec![0u8; 10];
        let body = [
            b"sfbk".to_vec(),
            list(b"INFO", &[chunk(b"INAM", b"Generated test bank\0")]),
            list(b"sdta", &[chunk(b"smpl", &wave)]),
            list(
                b"pdta",
                &[
                    chunk(b"phdr", &phdr),
                    chunk(b"pbag", &pbag),
                    chunk(b"pmod", &imod),
                    chunk(b"pgen", &pgen),
                    chunk(b"inst", &inst),
                    chunk(b"ibag", &ibag),
                    chunk(b"imod", &imod),
                    chunk(b"igen", &igen),
                    chunk(b"shdr", &shdr),
                ],
            ),
        ]
        .concat();
        chunk(b"RIFF", &body)
    }

    /// A bank whose one preset resolves `zones` regions from one instrument (all the same sample),
    /// for the parser's budget tests.
    pub fn crowded_bank(zones: usize) -> Vec<u8> {
        let zones = zones.clamp(1, usize::from(u16::MAX) - 2);
        let wave: Vec<u8> = (0..200).flat_map(|n| (((n as f32 * 0.3).sin() * 12_000.0) as i16).to_le_bytes()).collect();
        let mut phdr = Vec::new();
        for (name, prog, bag) in [("Crowded", 0u16, 0u16), ("EOP", 0, 1)] {
            phdr.extend(name20(name));
            phdr.extend(prog.to_le_bytes());
            phdr.extend(0u16.to_le_bytes());
            phdr.extend(bag.to_le_bytes());
            phdr.extend([0u8; 12]);
        }
        // One preset zone linking instrument 0, then the terminal bag.
        let pbag: Vec<u8> = [0u16, 1].iter().flat_map(|g| [g.to_le_bytes(), [0, 0]].concat()).collect();
        let pgen = gen_rec(41, i(0));
        let mut inst = Vec::new();
        for (name, bag) in [("Many", 0u16), ("EOI", zones as u16)] {
            inst.extend(name20(name));
            inst.extend(bag.to_le_bytes());
        }
        // One bag per instrument zone, each with one sample link (so none is a global zone).
        let mut ibag = Vec::new();
        let mut igen = Vec::new();
        for k in 0..=zones {
            ibag.extend((k as u16).to_le_bytes());
            ibag.extend([0u8, 0]);
            igen.extend(gen_rec(53, i(0)));
        }
        let mut shdr = Vec::new();
        for (name, start, end, rate, pitch) in [("sine", 0u32, 100u32, 44_100u32, 69u8), ("EOS", 0, 0, 0, 0)] {
            shdr.extend(name20(name));
            for v in [start, end, 0, 0, rate] {
                shdr.extend(v.to_le_bytes());
            }
            shdr.extend([pitch, 0]);
            shdr.extend(0u16.to_le_bytes());
            shdr.extend(1u16.to_le_bytes());
        }
        let imod = vec![0u8; 10];
        let body = [
            b"sfbk".to_vec(),
            list(b"sdta", &[chunk(b"smpl", &wave)]),
            list(
                b"pdta",
                &[
                    chunk(b"phdr", &phdr),
                    chunk(b"pbag", &pbag),
                    chunk(b"pmod", &imod),
                    chunk(b"pgen", &pgen),
                    chunk(b"inst", &inst),
                    chunk(b"ibag", &ibag),
                    chunk(b"imod", &imod),
                    chunk(b"igen", &igen),
                    chunk(b"shdr", &shdr),
                ],
            ),
        ]
        .concat();
        chunk(b"RIFF", &body)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    use super::*;

    use super::fixture::test_bank;

    #[test]
    fn a_bank_flattens_into_regions_with_every_generator_combined() {
        let sf = SoundFont::parse(&test_bank()).unwrap();
        assert_eq!(sf.name, "Generated test bank");
        assert_eq!(sf.samples.len(), 2_000);
        assert_eq!(sf.presets.len(), 1);
        let p = &sf.presets[0];
        assert_eq!((p.name.as_str(), p.bank, p.program), ("Test", 0, 5));
        assert_eq!(p.regions.len(), 2);
        let (lo, hi) = (&p.regions[0], &p.regions[1]);
        assert_eq!((lo.keys, hi.keys), ((0, 59), (60, 127)));
        assert_eq!((lo.loop_mode, hi.loop_mode), (LoopMode::None, LoopMode::Continuous));
        assert_eq!((hi.start, hi.end, hi.loop_start, hi.loop_end), (0, 1_990, 100, 1_100));
        assert_eq!((hi.sample_rate, hi.root_key), (44_100, 69));
        // The preset's semitone and attenuation are added to the instrument's values.
        assert_eq!(hi.tune_cents, 100.0);
        assert!((hi.gain_db + 6.0).abs() < 1e-6);
        // The instrument's global zone gives every zone its attack (-1200 timecents: 0.5 s).
        assert!((lo.env.attack - 0.5).abs() < 1e-4 && (hi.env.attack - 0.5).abs() < 1e-4);
        assert_eq!(hi.env.sustain, 1.0);
        assert_eq!(sf.preset_for_program(5), Some(0));
        assert_eq!(sf.preset_for_program(6), None);
    }

    #[test]
    fn a_zone_list_beyond_the_budget_is_refused() {
        let at_limit = MAX_ZONES as u16;
        let bags: Vec<u8> = (0..=at_limit + 1).flat_map(|i| [i.to_le_bytes(), [0, 0]].concat()).collect();
        let mut gens = Vec::new();
        for _ in 0..=at_limit + 1 {
            gens.extend(53u16.to_le_bytes());
            gens.extend([0u8, 0]);
        }
        let zones = zone_list(Some(0), Some(usize::from(at_limit)), &bags, &gens, SAMPLE_ID).unwrap();
        assert_eq!(zones.zones.len(), usize::from(at_limit));
        assert!(zone_list(Some(0), Some(usize::from(at_limit) + 1), &bags, &gens, SAMPLE_ID).is_err());
    }

    #[test]
    fn one_preset_cannot_hold_the_whole_cross_product() {
        let sf = SoundFont::parse(&super::fixture::crowded_bank(2_000)).unwrap();
        assert_eq!(sf.presets.len(), 1);
        assert_eq!(sf.presets[0].regions.len(), MAX_REGIONS);
    }

    #[test]
    fn damaged_banks_are_refused_without_panicking() {
        let good = test_bank();
        assert!(SoundFont::parse(b"RIFF\0\0\0\0WAVE").is_err());
        assert!(SoundFont::parse(&[]).is_err());
        for cut in (0..good.len()).step_by(7) {
            let _ = SoundFont::parse(&good[..cut]);
        }
        // Every byte flipped in turn: refused or parsed, never a panic or a region out of bounds.
        for k in 12..good.len() {
            let mut bad = good.clone();
            bad[k] ^= 0xA5;
            if let Ok(sf) = SoundFont::parse(&bad) {
                for r in sf.presets.iter().flat_map(|p| &p.regions) {
                    assert!(r.start < r.end && r.end <= sf.samples.len());
                    assert!(r.loop_mode == LoopMode::None || (r.loop_start >= r.start && r.loop_end <= r.end));
                }
            }
        }
    }
}
