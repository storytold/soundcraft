//! A JSFX effect as a [`soundcraft_dsp::Plugin`].

use crate::ast::{Expr, Func};
use crate::header::{JsfxSource, SliderDef, parse_source};
use crate::parse::{self, MAX_CH, Names, compile};
use crate::vm::{self, Machine};
use crate::{ID_PREFIX, JsfxError};
use soundcraft_dsp::{Category, ParamInfo, Plugin, PluginInfo, Taper, Unit};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Reads the text of an imported file by its `import` name; `None` when it cannot be found.
pub type ImportLoader<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Imports may import further files, to this depth.
const MAX_IMPORT_DEPTH: usize = 8;

/// Default memory size in `f64` slots (REAPER's default is 8M; the OS commits pages lazily).
const DEFAULT_MEM: usize = 1 << 22;
const MAX_MEM: usize = 1 << 24;
/// Loop-iteration budgets per section run.
const FUEL_INIT: u64 = 50_000_000;
const FUEL_SLIDER: u64 = 5_000_000;
const FUEL_BLOCK: u64 = 5_000_000;
/// Per sample, pooled over the block: a one-off spike (a script clearing a delay line once) fits,
/// and a block never does more than this times its length.
const FUEL_SAMPLE: u64 = 200_000;
/// Largest source file we load.
const MAX_FILE: u64 = 2 * 1024 * 1024;
/// JSFX cannot report its tail; assume a few seconds so delays and reverbs ring out.
const TAIL_SECONDS: f32 = 3.0;

struct Sections {
    init: Option<Expr>,
    slider: Option<Expr>,
    block: Option<Expr>,
    sample: Option<Expr>,
}

pub struct JsfxPlugin {
    info: &'static PluginInfo,
    code: Sections,
    funcs: Vec<Func>,
    m: Machine,
    /// Slider variable slot per parameter, parallel to `info.params`.
    slots: Vec<usize>,
    values: Vec<f32>,
    dirty: bool,
    faulted: bool,
    sr: f32,
    max_block: usize,
    channels: usize,
    latency: usize,
}

fn leak_str(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

fn param_info(def: &SliderDef) -> ParamInfo {
    let choices: &'static [&'static str] =
        if def.choices.is_empty() { &[] } else { Box::leak(def.choices.iter().map(|c| leak_str(c)).collect::<Vec<_>>().into_boxed_slice()) };
    let id = format!("slider{}", def.index);
    let name = if def.label.is_empty() { id.clone() } else { def.label.clone() };
    ParamInfo {
        id: leak_str(&id),
        name: leak_str(&name),
        min: def.min as f32,
        max: def.max as f32,
        default: def.default as f32,
        unit: if choices.is_empty() { Unit::None } else { Unit::Choice },
        taper: if def.log && def.min > 0.0 { Taper::Log } else { Taper::Linear },
        choices,
    }
}

/// One leaked `PluginInfo` per distinct (id, source); instantiating the same effect again reuses it.
fn info_for(id: &str, text: &str, src: &JsfxSource) -> &'static PluginInfo {
    static CACHE: OnceLock<Mutex<HashMap<u64, &'static PluginInfo>>> = OnceLock::new();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    text.hash(&mut h);
    let key = h.finish();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = guard.get(&key) {
        return i;
    }
    let desc = src.desc.trim();
    let name = if desc.is_empty() { id.strip_prefix(ID_PREFIX).unwrap_or(id) } else { desc };
    let short: String = name.chars().filter(|c| c.is_alphanumeric()).take(8).collect();
    let params: Vec<ParamInfo> = src.sliders.iter().map(param_info).collect();
    let info: &'static PluginInfo = Box::leak(Box::new(PluginInfo {
        id: leak_str(id),
        name: leak_str(name),
        short_name: leak_str(if short.is_empty() { "JSFX" } else { &short }),
        category: Category::Other,
        params: Box::leak(params.into_boxed_slice()),
        is_instrument: false,
        audiosuite: true,
    }));
    guard.insert(key, info);
    info
}

/// Most folder entries looked at when searching the Effects folder for a library by name.
const MAX_SEARCHED: usize = 50_000;
/// Deepest folder searched below the Effects folder.
const MAX_SEARCH_DEPTH: usize = 8;

/// The file called `file` (ignoring case) under `root` that shares the most folders with `near`.
fn find_by_name(root: &Path, file: &std::ffi::OsStr, near: &Path) -> Option<PathBuf> {
    let want = file.to_string_lossy().to_lowercase();
    let mut best: Option<(usize, PathBuf)> = None;
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            seen += 1;
            if seen > MAX_SEARCHED {
                return best.map(|(_, p)| p);
            }
            let Ok(kind) = e.file_type() else { continue };
            let p = e.path();
            if kind.is_dir() {
                if depth < MAX_SEARCH_DEPTH {
                    stack.push((p, depth + 1));
                }
            } else if kind.is_file() && e.file_name().to_string_lossy().to_lowercase() == want {
                let shared = p.parent().map_or(0, |d| d.components().zip(near.components()).take_while(|(a, b)| a == b).count());
                if best.as_ref().is_none_or(|(n, _)| shared > *n) {
                    best = Some((shared, p));
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Reads an effect file (size-capped): its plugin id (`jsfx:` + the path relative to `root`) and text.
fn read_effect(root: &Path, path: &Path) -> Result<(String, String), JsfxError> {
    let io = |e: std::io::Error| JsfxError::Io(format!("{}: {e}", path.display()));
    if std::fs::metadata(path).map_err(io)?.len() > MAX_FILE {
        return Err(JsfxError::Io(format!("{}: file too large", path.display())));
    }
    let bytes = std::fs::read(path).map_err(io)?;
    let rel = path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/");
    Ok((format!("{ID_PREFIX}{rel}"), String::from_utf8_lossy(&bytes).into_owned()))
}

/// Compiles each imported file's code into `names` (so its functions become callable) and collects
/// the code to run at the start of `@init`. An imported file with no `@section` lines is all code.
fn compile_imports(
    names: &mut Names,
    imports: &[String],
    loader: Option<ImportLoader>,
    depth: usize,
    parent_dir: &str,
    seen: &mut HashSet<String>,
    out: &mut Vec<Expr>,
) -> Result<(), JsfxError> {
    for name in imports {
        let Some(loader) = loader else { return Err(JsfxError::Io(format!("import `{name}`: no file loader"))) };
        if depth >= MAX_IMPORT_DEPTH {
            return Err(JsfxError::Unsupported("imports nested too deeply".into()));
        }
        // An import is looked up next to the file that imports it, then in each parent folder up to
        // the Effects folder (the loader refuses names that climb out of it).
        let mut candidates = Vec::new();
        let mut dir = parent_dir;
        while !dir.is_empty() {
            candidates.push(format!("{dir}/{name}"));
            dir = dir.rsplit_once('/').map_or("", |(d, _)| d);
        }
        candidates.push(name.clone());
        let Some((path, text)) = candidates.into_iter().find_map(|c| loader(&c).map(|t| (c, t))) else {
            return Err(JsfxError::Io(format!("import `{name}`: file not found")));
        };
        if !seen.insert(path.to_ascii_lowercase()) {
            continue;
        }
        let inc = parse_source(&text)?;
        let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
        compile_imports(names, &inc.imports, Some(loader), depth + 1, dir, seen, out)?;
        let code = if text.lines().any(|l| l.starts_with('@')) { compile(&inc.init.code, inc.init.line, names)? } else { compile(&text, 1, names)? };
        out.push(code);
    }
    Ok(())
}

impl JsfxPlugin {
    /// Compiles `text` as a JSFX file that imports nothing. `id` is the full SoundCraft id
    /// (`jsfx:<relative path>`).
    pub fn from_source(id: &str, text: &str) -> Result<Self, JsfxError> {
        Self::from_source_with(id, text, None)
    }

    /// Like [`JsfxPlugin::from_source`], resolving `import` lines through `loader`.
    pub fn from_source_with(id: &str, text: &str, loader: Option<ImportLoader>) -> Result<Self, JsfxError> {
        Self::build(id, text, loader, true)?.ok_or_else(|| JsfxError::Io("no instance was built".into()))
    }

    /// Compiles `text`; builds the instance only when `instantiate` (a check needs no memory and
    /// leaks no `PluginInfo`).
    fn build(id: &str, text: &str, loader: Option<ImportLoader>, instantiate: bool) -> Result<Option<Self>, JsfxError> {
        let mut src = parse_source(text)?;
        src.sliders.sort_by_key(|s| s.index);
        let mut names = Names::new();
        for s in &src.sliders {
            if let Some(v) = &s.var {
                names.alias(v, parse::SLIDER1 + s.index - 1);
            }
        }
        let mut init_parts = Vec::new();
        compile_imports(&mut names, &src.imports, loader, 0, "", &mut HashSet::new(), &mut init_parts)?;
        let mut sec = |s: &crate::header::Section| -> Result<Option<Expr>, JsfxError> {
            if s.code.trim().is_empty() { Ok(None) } else { compile(&s.code, s.line, &mut names).map(Some) }
        };
        let main_init = sec(&src.init)?;
        init_parts.extend(main_init);
        let init = match init_parts.len() {
            0 => None,
            1 => init_parts.pop(),
            _ => Some(Expr::Seq(init_parts)),
        };
        let code = Sections { init, slider: sec(&src.slider)?, block: sec(&src.block)?, sample: sec(&src.sample)? };
        if !instantiate {
            return Ok(None);
        }
        let mem = src.maxmem.map_or(DEFAULT_MEM, |m| m.clamp(1 << 10, MAX_MEM));
        let info = info_for(id, text, &src);
        let m = Machine::new(names.len(), mem).with_strings(std::mem::take(&mut names.strings));
        Ok(Some(Self {
            info,
            code,
            funcs: std::mem::take(&mut names.funcs),
            m,
            slots: src.sliders.iter().map(|s| parse::SLIDER1 + s.index - 1).collect(),
            values: info.params.iter().map(|p| p.default).collect(),
            dirty: true,
            faulted: false,
            sr: 44_100.0,
            max_block: 512,
            channels: 2,
            latency: 0,
        }))
    }

    /// Loads a JSFX file; `root` is the Effects folder the plugin id is made relative to.
    pub fn from_file(root: &Path, path: &Path) -> Result<Self, JsfxError> {
        Self::load_file(root, path, true)?.ok_or_else(|| JsfxError::Io("no instance was built".into()))
    }

    /// Whether the file compiles with this host, without building an instance: `Err` says what
    /// is missing (an unsupported function, an import that cannot be found, ...).
    pub fn check_file(root: &Path, path: &Path) -> Result<(), JsfxError> {
        Self::load_file(root, path, false).map(drop)
    }

    /// The description of a JSFX file from its header alone (name and sliders): nothing is compiled
    /// or run. The same `PluginInfo` an instance of the file reports.
    pub fn info_from_file(root: &Path, path: &Path) -> Result<&'static PluginInfo, JsfxError> {
        let (id, text) = read_effect(root, path)?;
        let mut src = parse_source(&text)?;
        src.sliders.sort_by_key(|s| s.index);
        Ok(info_for(&id, &text, &src))
    }

    fn load_file(root: &Path, path: &Path, instantiate: bool) -> Result<Option<Self>, JsfxError> {
        let (id, text) = read_effect(root, path)?;
        // Imports are looked up next to the effect, then in the Effects folder. Names that climb out of
        // those folders (absolute paths, `..`) are refused: effects are untrusted input.
        // The effect's own folder, each parent folder up to the Effects folder, then the Effects folder.
        let root = root.to_path_buf();
        let bases: Vec<std::path::PathBuf> =
            path.parent().into_iter().flat_map(Path::ancestors).take_while(|a| a.starts_with(&root)).map(Path::to_path_buf).collect();
        let read = |p: &Path| -> Option<String> {
            if std::fs::metadata(p).ok()?.len() > MAX_FILE {
                return None;
            }
            std::fs::read(p).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
        };
        let near = path.parent().map(Path::to_path_buf).unwrap_or_else(|| root.clone());
        let loader = move |name: &str| -> Option<String> {
            if !Path::new(name).components().all(|c| matches!(c, Component::Normal(_))) {
                return None;
            }
            bases.iter().chain(std::iter::once(&root)).find_map(|base| read(&base.join(name))).or_else(|| {
                // As REAPER does, a library not at its path is looked for by file name anywhere in
                // the Effects folder (the copy nearest the effect wins).
                let file = Path::new(name).file_name()?;
                read(&find_by_name(&root, file, &near)?)
            })
        };
        Self::build(&id, &text, Some(&loader), instantiate)
    }

    /// `true` once a section ran out of its budget; the effect then passes audio through untouched.
    pub fn is_faulted(&self) -> bool {
        self.faulted
    }

    fn param_index(&self, id: &str) -> Option<usize> {
        self.info.params.iter().position(|p| p.id == id)
    }

    fn push_sliders(&mut self) {
        for (slot, v) in self.slots.iter().zip(&self.values) {
            self.m.set_var(*slot, f64::from(*v));
        }
    }

    fn run(&mut self, which: fn(&Sections) -> &Option<Expr>, fuel: u64) {
        if !self.try_run(which, fuel) {
            log::warn!("jsfx {}: section ran out of its budget; bypassing", self.info.id);
            self.faulted = true;
        }
    }

    /// Runs a section; `false` when it ran out of its budget.
    fn try_run(&mut self, which: fn(&Sections) -> &Option<Expr>, fuel: u64) -> bool {
        which(&self.code).as_ref().is_none_or(|e| self.m.run(e, &self.funcs, fuel))
    }

    fn read_latency(&mut self) {
        let d = self.m.var(parse::PDC_DELAY);
        self.latency = if d.is_finite() && d > 0.0 { (d as usize).min(self.sr as usize * 4) } else { 0 };
    }

    /// Starts the effect from scratch (`prepare`, on the control thread): clears variables and
    /// memory, publishes host variables, runs `@init` then `@slider`.
    fn start(&mut self) {
        self.m.reset();
        self.faulted = false;
        self.m.set_var(parse::SRATE, f64::from(self.sr));
        self.m.set_var(parse::NUM_CH, self.channels as f64);
        self.m.set_var(parse::SAMPLESBLOCK, self.max_block as f64);
        self.m.set_var(parse::TEMPO, 120.0);
        self.m.set_var(parse::TS_NUM, 4.0);
        self.m.set_var(parse::TS_DENOM, 4.0);
        self.push_sliders();
        self.run(|c| &c.init, FUEL_INIT);
        self.run(|c| &c.slider, FUEL_SLIDER);
        self.dirty = false;
        self.read_latency();
    }
}

impl Plugin for JsfxPlugin {
    fn info(&self) -> &'static PluginInfo {
        self.info
    }

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.sr = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 44_100.0 };
        self.max_block = max_block.max(1);
        self.channels = channels.clamp(1, MAX_CH);
        self.start();
    }

    /// Play/Stop, on the audio thread: like REAPER at the start of playback, `@init` and `@slider`
    /// run again (unless the effect set `ext_noinit`) over the variables and memory as they are.
    /// Nothing is allocated or cleared, and `@init` gets a block's budget, not a load's: when it
    /// runs out, what the load's `@init` built stays and the effect keeps playing.
    fn reset(&mut self) {
        self.m.clear_midi();
        if self.faulted || vm::truthy(self.m.var(parse::EXT_NOINIT)) {
            return;
        }
        self.push_sliders();
        if !self.try_run(|c| &c.init, FUEL_BLOCK) {
            log::warn!("jsfx {}: @init ran out of its budget at the start of playback", self.info.id);
        }
        self.run(|c| &c.slider, FUEL_SLIDER);
        self.dirty = false;
        self.read_latency();
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(i) = self.param_index(id) else { return false };
        let Some(info) = self.info.params.get(i) else { return false };
        let v = info.clamp(value);
        if let Some(slot) = self.values.get_mut(i) {
            *slot = v;
        }
        if let Some(slot) = self.slots.get(i) {
            self.m.mark_slider_changed(slot - parse::SLIDER1 + 1);
        }
        self.dirty = true;
        true
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.m.push_midi(offset, 0x90, note & 0x7f, velocity & 0x7f);
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.m.push_midi(offset, 0x80, note & 0x7f, 0);
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.param_index(id).and_then(|i| self.values.get(i)).copied()
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        if self.faulted {
            return;
        }
        if self.dirty {
            self.push_sliders();
            self.run(|c| &c.slider, FUEL_SLIDER);
            self.dirty = false;
            self.read_latency();
        }
        self.run(|c| &c.block, FUEL_BLOCK);
        if self.code.sample.is_none() || self.faulted {
            self.m.clear_midi();
            return;
        }
        let ch = io.len().min(MAX_CH);
        let n = frames.min(io.iter().take(ch).map(Vec::len).min().unwrap_or(0));
        let mut pool = FUEL_SAMPLE.saturating_mul(n as u64);
        for i in 0..n {
            for (c, buf) in io.iter().take(ch).enumerate() {
                self.m.set_var(parse::SPL0 + c, f64::from(buf.get(i).copied().unwrap_or(0.0)));
            }
            if let Some(e) = &self.code.sample
                && !self.m.run_pooled(e, &self.funcs, &mut pool)
            {
                log::warn!("jsfx {}: @sample ran out of its budget; bypassing", self.info.id);
                self.faulted = true;
                self.m.clear_midi();
                return;
            }
            for (c, buf) in io.iter_mut().take(ch).enumerate() {
                let v = self.m.var(parse::SPL0 + c);
                if let Some(s) = buf.get_mut(i) {
                    *s = if v.is_finite() { v as f32 } else { 0.0 };
                }
            }
        }
        self.m.clear_midi();
    }

    fn latency(&self) -> usize {
        self.latency
    }

    fn tail_samples(&self) -> usize {
        if self.code.sample.is_some() { (self.sr * TAIL_SECONDS) as usize } else { 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(src: &str) -> JsfxPlugin {
        let mut p = JsfxPlugin::from_source("jsfx:test", src).unwrap();
        p.prepare(48_000.0, 64, 2);
        p
    }

    fn run(p: &mut JsfxPlugin, l: f32, r: f32, frames: usize) -> Vec<Vec<f32>> {
        let mut io = vec![vec![l; frames], vec![r; frames]];
        p.process(&mut io, frames);
        io
    }

    const GAIN: &str = "desc:Gain\nslider1:0<-60,12,0.1>Gain dB\n@init\ng=1;\n@slider\ng=10^(slider1/20);\n@sample\nspl0*=g;\nspl1*=g;\n";

    #[test]
    fn gain_follows_its_slider() {
        let mut p = load(GAIN);
        assert_eq!(p.info().name, "Gain");
        assert_eq!(p.info().params.len(), 1);
        let io = run(&mut p, 1.0, -1.0, 4);
        assert!((io[0][0] - 1.0).abs() < 1e-6 && (io[1][3] + 1.0).abs() < 1e-6);
        assert!(p.set_param("slider1", -6.0206));
        let io = run(&mut p, 1.0, 1.0, 4);
        assert!((io[0][0] - 0.5).abs() < 1e-3, "{}", io[0][0]);
        assert!(!p.set_param("nope", 1.0));
        assert_eq!(p.param("slider1"), Some(-6.0206));
    }

    #[test]
    fn params_are_clamped_and_nan_safe() {
        let mut p = load(GAIN);
        p.set_param("slider1", 1e9);
        assert_eq!(p.param("slider1"), Some(12.0));
        p.set_param("slider1", f32::NAN);
        assert_eq!(p.param("slider1"), Some(0.0));
    }

    #[test]
    fn state_persists_across_blocks_and_memory_works() {
        // One-sample delay through a memory cell.
        let mut p = load("desc:D\n@init\nbuf=0;\n@sample\nout=buf[0];\nbuf[0]=spl0;\nspl0=out;\n");
        let mut io = vec![vec![1.0, 2.0, 3.0], vec![0.0; 3]];
        p.process(&mut io, 3);
        assert_eq!(io[0], [0.0, 1.0, 2.0]);
        let mut io = vec![vec![9.0], vec![0.0]];
        p.process(&mut io, 1);
        assert_eq!(io[0], [3.0]);
        // Play/Stop keeps memory (REAPER does too); a new `prepare` starts from scratch.
        p.reset();
        let mut io = vec![vec![5.0], vec![0.0]];
        p.process(&mut io, 1);
        assert_eq!(io[0], [9.0]);
        p.prepare(48_000.0, 64, 2);
        let mut io = vec![vec![5.0], vec![0.0]];
        p.process(&mut io, 1);
        assert_eq!(io[0], [0.0]);
    }

    #[test]
    fn play_stop_reruns_init_cheaply_on_the_audio_thread() {
        // `@init` runs again at Play/Stop, over the variables as they are.
        let mut p = load("desc:R\n@init\nruns += 1;\n@sample\nspl0 = runs;\n");
        assert_eq!(run(&mut p, 0.0, 0.0, 1)[0][0], 1.0);
        p.reset();
        assert_eq!(run(&mut p, 0.0, 0.0, 1)[0][0], 2.0);
        // `ext_noinit` turns that off.
        let mut p = load("desc:N\n@init\next_noinit = 1; runs += 1;\n@sample\nspl0 = runs;\n");
        p.reset();
        assert_eq!(run(&mut p, 0.0, 0.0, 1)[0][0], 1.0);
        // An `@init` that fits a load's budget but not a block's: the reset stops early, the table
        // the load built stays, and the effect keeps playing.
        let mut p = load("desc:H\n@init\ni = 0; loop(10000000, i += 1;); mem[5] = 0.25;\n@sample\nspl0 = mem[5];\n");
        p.reset();
        assert!(!p.is_faulted());
        assert_eq!(run(&mut p, 0.0, 0.0, 1)[0][0], 0.25);
    }

    #[test]
    fn host_variables_and_latency() {
        let mut p = load("desc:V\n@init\npdc_delay=64;\n@sample\nspl0=srate;\nspl1=num_ch;\n");
        let io = run(&mut p, 0.0, 0.0, 1);
        assert_eq!((io[0][0], io[1][0]), (48_000.0, 2.0));
        assert_eq!(p.latency(), 64);
    }

    #[test]
    fn runaway_scripts_fault_and_pass_through() {
        let mut p = load("desc:Hang\n@sample\nwhile(1) ( x += 1; );\n");
        let io = run(&mut p, 0.25, 0.25, 8);
        assert!(p.is_faulted());
        assert!(io[0].iter().all(|v| v.is_finite()));
        let io = run(&mut p, 0.5, 0.5, 2);
        assert_eq!(io[0], [0.5, 0.5], "bypassed after the fault");
    }

    #[test]
    fn non_finite_output_becomes_silence() {
        let mut p = load("desc:Nan\n@sample\nspl0=log(-1);\nspl1=1/0;\n");
        let io = run(&mut p, 1.0, 1.0, 2);
        assert_eq!(io[0], [0.0, 0.0]);
        assert_eq!(io[1], [0.0, 0.0]);
    }

    #[test]
    fn mono_and_short_buffers_are_handled() {
        let mut p = load(GAIN);
        let mut io = vec![vec![1.0; 4]];
        p.process(&mut io, 100);
        let mut none: Vec<Vec<f32>> = Vec::new();
        p.process(&mut none, 10);
        p.prepare(0.0, 0, 0);
        assert_eq!(p.info().params[0].id, "slider1");
    }

    #[test]
    fn choice_sliders_and_aliases() {
        let mut p = load("desc:M\nslider1:mode=1<0,2,1{A,B,C}>Mode\n@sample\nspl0=mode;\n");
        assert_eq!(p.info().params[0].choices, ["A", "B", "C"]);
        assert_eq!(run(&mut p, 0.0, 0.0, 1)[0][0], 1.0);
        p.set_param("slider1", 2.2);
        assert_eq!(run(&mut p, 0.0, 0.0, 1)[0][0], 2.0);
    }

    #[test]
    fn unsupported_features_fail_to_load_with_a_reason() {
        let Err(e) = JsfxPlugin::from_source("jsfx:x", "desc:x\n@init\nx = file_open(y);\n") else { panic!("should not load") };
        assert!(matches!(e, JsfxError::Unsupported(_)), "{e}");
        assert!(JsfxPlugin::from_source("jsfx:x", "desc:x\n@sample\nspl0 = ;\n").is_err());
    }

    #[test]
    fn user_functions_run_in_sample_code() {
        let mut p = load("desc:F\nslider1:2<0,10>G\n@init\nfunction amp(x, g) local(y) ( y = x * g; y; );\n@sample\nspl0 = amp(spl0, slider1);\n");
        assert_eq!(run(&mut p, 0.25, 0.0, 1)[0][0], 0.5);
        p.set_param("slider1", 4.0);
        assert_eq!(run(&mut p, 0.25, 0.0, 1)[0][0], 1.0);
    }

    #[test]
    fn imports_bring_functions_and_run_their_init() {
        let lib = "function twice(x) ( x * 2; );\nbase = 10;\n";
        let loader = |n: &str| (n == "lib.jsfx-inc").then(|| lib.to_string());
        let src = "desc:I\nimport lib.jsfx-inc\n@sample\nspl0 = twice(spl0) + base;\n";
        let mut p = JsfxPlugin::from_source_with("jsfx:i", src, Some(&loader)).unwrap();
        p.prepare(48_000.0, 16, 2);
        assert_eq!(run(&mut p, 1.0, 0.0, 1)[0][0], 12.0);
        // Without a loader, or when the file is missing, the effect refuses to load.
        assert!(JsfxPlugin::from_source("jsfx:i", src).is_err());
        let none = |_: &str| None;
        assert!(matches!(JsfxPlugin::from_source_with("jsfx:i", src, Some(&none)), Err(JsfxError::Io(_))));
    }

    #[test]
    fn note_events_reach_midirecv() {
        let mut p = load("desc:M\n@block\nwhile (midirecv(t, m1, m2, m3)) ( m1 == 144 ? last = m2; );\n@sample\nspl0 = last;\n");
        p.note_on(0, 64, 100);
        assert_eq!(run(&mut p, 0.0, 0.0, 1)[0][0], 64.0);
        // Events are per block: the next block sees none, but the effect keeps its state.
        assert_eq!(run(&mut p, 0.0, 0.0, 1)[0][0], 64.0);
    }

    #[test]
    fn sliderchange_fires_once_after_a_parameter_edit() {
        let mut p = load("desc:S\nslider1:0<0,10>A\n@sample\nspl0 = sliderchange(slider1);\n");
        // The initial values count as a change, then it stays quiet until the slider moves again.
        assert_eq!(run(&mut p, 0.0, 0.0, 1)[0][0], 1.0);
        assert_eq!(run(&mut p, 0.0, 0.0, 1)[0][0], 0.0);
        p.set_param("slider1", 3.0);
        assert_eq!(run(&mut p, 0.0, 0.0, 1)[0][0], 1.0);
    }

    #[test]
    fn same_source_shares_one_plugin_info() {
        let a = load(GAIN);
        let b = load(GAIN);
        assert!(std::ptr::eq(a.info(), b.info()));
    }

    #[test]
    fn sections_without_sample_code_pass_audio_through() {
        let mut p = load("desc:Meter\n@block\nx+=1;\n");
        assert_eq!(run(&mut p, 0.3, 0.3, 2)[0], [0.3, 0.3]);
        assert_eq!(p.tail_samples(), 0);
    }
}
