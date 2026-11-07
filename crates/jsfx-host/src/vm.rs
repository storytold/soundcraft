//! Tree-walking EEL2 interpreter. Never allocates while evaluating, never panics: memory accesses are
//! bounds-checked (out-of-range reads give 0, writes are dropped) and every loop iteration (and the
//! work of builtins such as `fft` and `memset`) draws on a fuel budget so a runaway script is cut
//! off instead of hanging the audio thread.

use crate::ast::{BinOp, Builtin, Expr, Func, UnOp};
use crate::fft;
use crate::matching::{MatchScratch, convert, matches};
use crate::parse::{MAX_SLIDERS, NUM_CH, SLIDER1};
use crate::strfmt::{STR_MAX, format_into};

/// EEL2 treats values closer to zero than this as false and equal.
const EPS: f64 = 1e-5;
/// EEL2 caps `loop` counts and `while` iterations at this many.
const MAX_ITER: usize = 1 << 20;
/// Most arguments a user function call can carry.
pub(crate) const MAX_ARGS: usize = 64;
/// Pending MIDI events kept per block; extra events are dropped.
const MIDI_CAP: usize = 1024;

pub(crate) fn truthy(x: f64) -> bool {
    x.abs() > EPS
}

fn to_i(x: f64) -> i64 {
    // Saturating; NaN becomes 0.
    x as i64
}

// What builtins that touch many memory cells cost, in loop iterations (the fuel unit). A loop
// iteration runs a few operations; filling or copying a cell is far cheaper; an FFT does about
// log2(n) butterflies per point.
fn cells_cost(n: usize) -> usize {
    n / 16
}

fn transform_cost(n: usize) -> usize {
    n.saturating_mul(n.max(1).ilog2() as usize) / 4
}

fn permute_cost(n: usize) -> usize {
    n / 4
}

fn b2f(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}

pub(crate) fn binop(op: BinOp, a: f64, b: f64) -> f64 {
    match op {
        BinOp::Add => a + b,
        BinOp::Sub => a - b,
        BinOp::Mul => a * b,
        BinOp::Div => {
            if b == 0.0 {
                0.0
            } else {
                a / b
            }
        }
        BinOp::IDiv => {
            let d = to_i(b);
            if d == 0 { 0.0 } else { to_i(a).wrapping_div(d) as f64 }
        }
        BinOp::Mod => {
            let d = to_i(b);
            if d == 0 { 0.0 } else { to_i(a).wrapping_rem(d) as f64 }
        }
        BinOp::Pow => a.powf(b),
        BinOp::Shl => {
            let s = to_i(b);
            if (0..64).contains(&s) { to_i(a).wrapping_shl(s as u32) as f64 } else { 0.0 }
        }
        BinOp::Shr => {
            let s = to_i(b);
            if (0..64).contains(&s) { to_i(a).wrapping_shr(s as u32) as f64 } else { 0.0 }
        }
        BinOp::And => (to_i(a) & to_i(b)) as f64,
        BinOp::Or => (to_i(a) | to_i(b)) as f64,
        BinOp::Xor => (to_i(a) ^ to_i(b)) as f64,
        BinOp::Eq => b2f((a - b).abs() < EPS),
        BinOp::Ne => b2f((a - b).abs() >= EPS),
        BinOp::EqExact => b2f(a == b),
        BinOp::NeExact => b2f(a != b),
        BinOp::Lt => b2f(a < b),
        BinOp::Le => b2f(a <= b),
        BinOp::Gt => b2f(a > b),
        BinOp::Ge => b2f(a >= b),
        // Handled with short-circuiting in `eval`; kept total for completeness.
        BinOp::LAnd => b2f(truthy(a) && truthy(b)),
        BinOp::LOr => b2f(truthy(a) || truthy(b)),
    }
}

/// Slot for a memory index, `None` when out of range.
fn index(v: f64, len: usize) -> Option<usize> {
    let i = v + 0.0001;
    if i >= 0.0 && i < len as f64 { Some(i as usize) } else { None }
}

/// Where an assignment lands.
#[derive(Clone, Copy)]
enum Loc {
    Var(usize),
    Mem(usize),
    GMem(usize),
    Nowhere,
}

#[derive(Clone, Copy, Default)]
struct MidiEvent {
    offset: u32,
    msg: [u8; 3],
}

pub(crate) struct Machine {
    pub vars: Vec<f64>,
    mem: Vec<f64>,
    gmem: Vec<f64>,
    rng: u64,
    fuel: u64,
    out_of_fuel: bool,
    /// Sliders changed since `sliderchange` last looked (bit n-1 for slider n).
    changed: u64,
    midi: Vec<MidiEvent>,
    midi_head: usize,
    /// The string table (handles are indices) and the contents `reset` restores.
    strings: Vec<Vec<u8>>,
    str_init: Vec<Vec<u8>>,
    /// Scratch buffers for string functions; sized once so steady-state use does not allocate.
    sbuf: Vec<u8>,
    body: Vec<u8>,
    matching: MatchScratch,
    /// The complex buffer `fft_real` / `ifft_real` work in (sized on first use).
    fft_scratch: Vec<f64>,
}

impl Machine {
    pub fn new(nvars: usize, mem_len: usize) -> Self {
        Self {
            vars: vec![0.0; nvars],
            mem: vec![0.0; mem_len],
            gmem: vec![0.0; 1 << 16],
            rng: 0x9E37_79B9_7F4A_7C15,
            fuel: 0,
            out_of_fuel: false,
            changed: u64::MAX,
            midi: Vec::with_capacity(MIDI_CAP),
            midi_head: 0,
            strings: Vec::new(),
            str_init: Vec::new(),
            sbuf: Vec::with_capacity(2 * STR_MAX),
            body: Vec::with_capacity(STR_MAX),
            matching: MatchScratch::new(STR_MAX),
            fft_scratch: Vec::new(),
        }
    }

    /// Installs the string table (`init[h]` is the starting content of handle `h`).
    pub fn with_strings(mut self, init: Vec<Vec<u8>>) -> Self {
        self.strings = init
            .iter()
            .map(|s| {
                let mut v = Vec::with_capacity(STR_MAX);
                v.extend_from_slice(s.get(..s.len().min(STR_MAX)).unwrap_or(&[]));
                v
            })
            .collect();
        self.str_init = init;
        self
    }

    fn string_index(&self, h: f64) -> Option<usize> {
        (h.is_finite() && h >= 0.0 && (h as usize) < self.strings.len()).then_some(h as usize)
    }

    /// Replaces string `dst` with the scratch buffer's contents (capped).
    fn store_scratch(&mut self, dst: f64) {
        if let Some(i) = self.string_index(dst)
            && let Some(s) = self.strings.get_mut(i)
        {
            s.clear();
            s.extend_from_slice(self.sbuf.get(..self.sbuf.len().min(STR_MAX)).unwrap_or(&[]));
        }
    }

    /// `sbuf = dst[..] + src[..n]` (`dst` empty when `append` is false).
    fn build(&mut self, dst: f64, src: f64, n: usize, offset: usize, append: bool) {
        self.sbuf.clear();
        if append && let Some(d) = self.string_index(dst).and_then(|i| self.strings.get(i)) {
            self.sbuf.extend_from_slice(d);
        }
        if let Some(s) = self.string_index(src).and_then(|i| self.strings.get(i)) {
            let from = offset.min(s.len());
            let to = from.saturating_add(n).min(s.len());
            self.sbuf.extend_from_slice(s.get(from..to).unwrap_or(&[]));
        }
    }

    fn string_bytes(&self, h: f64) -> &[u8] {
        self.string_index(h).and_then(|i| self.strings.get(i)).map_or(&[], Vec::as_slice)
    }

    /// Zeroes all variables and memory (memory is reallocated zeroed, which the OS provides lazily).
    pub fn reset(&mut self) {
        self.vars.fill(0.0);
        self.mem = vec![0.0; self.mem.len()];
        self.gmem.fill(0.0);
        self.changed = u64::MAX;
        self.clear_midi();
        for (s, init) in self.strings.iter_mut().zip(&self.str_init) {
            s.clear();
            s.extend_from_slice(init.get(..init.len().min(STR_MAX)).unwrap_or(&[]));
        }
    }

    pub fn var(&self, slot: usize) -> f64 {
        self.vars.get(slot).copied().unwrap_or(0.0)
    }

    pub fn set_var(&mut self, slot: usize, v: f64) {
        if let Some(x) = self.vars.get_mut(slot) {
            *x = v;
        }
    }

    /// Notes that slider `n` (1-based) changed, for `sliderchange`.
    pub fn mark_slider_changed(&mut self, n: usize) {
        if (1..=MAX_SLIDERS).contains(&n) {
            self.changed |= 1u64 << (n - 1);
        }
    }

    /// Queues a MIDI event for `midirecv`; dropped when the block's queue is full.
    pub fn push_midi(&mut self, offset: usize, status: u8, d1: u8, d2: u8) {
        if self.midi.len() < MIDI_CAP {
            self.midi.push(MidiEvent { offset: offset.min(u32::MAX as usize) as u32, msg: [status, d1, d2] });
        }
    }

    pub fn clear_midi(&mut self) {
        self.midi.clear();
        self.midi_head = 0;
    }

    /// Runs `e` with a budget of `fuel` loop iterations (builtins charge for their work too). `false` when the budget ran out.
    pub fn run(&mut self, e: &Expr, funcs: &[Func], fuel: u64) -> bool {
        let mut pool = fuel;
        self.run_pooled(e, funcs, &mut pool)
    }

    /// Like [`Machine::run`], drawing on `pool` and leaving in it what is unused.
    pub fn run_pooled(&mut self, e: &Expr, funcs: &[Func], pool: &mut u64) -> bool {
        self.fuel = *pool;
        self.out_of_fuel = false;
        self.eval(e, funcs);
        *pool = self.fuel;
        !self.out_of_fuel
    }

    fn tick(&mut self) -> bool {
        if self.fuel == 0 {
            self.out_of_fuel = true;
            false
        } else {
            self.fuel -= 1;
            true
        }
    }

    /// Draws `cost` from the budget for a builtin whose work grows with its arguments; `false`
    /// (and the run stops) when not enough is left, and the builtin then does nothing.
    fn charge(&mut self, cost: usize) -> bool {
        let cost = cost as u64;
        if self.fuel < cost {
            self.fuel = 0;
            self.out_of_fuel = true;
            false
        } else {
            self.fuel -= cost;
            true
        }
    }

    fn rand01(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64
    }

    fn locate(&mut self, target: &Expr, f: &[Func]) -> Loc {
        match target {
            Expr::Var(s) => Loc::Var(*s),
            Expr::VarIdx { base, first, last, idx } => {
                let i = to_i(self.eval(idx, f)).clamp(*first, *last);
                Loc::Var((*base as i64 + (i - first)) as usize)
            }
            Expr::Mem(i) => {
                let i = self.eval(i, f);
                index(i, self.mem.len()).map_or(Loc::Nowhere, Loc::Mem)
            }
            Expr::GMem(i) => {
                let i = self.eval(i, f);
                index(i, self.gmem.len()).map_or(Loc::Nowhere, Loc::GMem)
            }
            _ => Loc::Nowhere,
        }
    }

    fn load(&self, loc: Loc) -> f64 {
        match loc {
            Loc::Var(s) => self.var(s),
            Loc::Mem(k) => self.mem.get(k).copied().unwrap_or(0.0),
            Loc::GMem(k) => self.gmem.get(k).copied().unwrap_or(0.0),
            Loc::Nowhere => 0.0,
        }
    }

    /// Writes `v` to `place` when it is something that can be assigned.
    fn put(&mut self, place: Option<&Expr>, v: f64, f: &[Func]) {
        if let Some(p) = place.filter(|p| p.is_place()) {
            let loc = self.locate(p, f);
            self.save(loc, v);
        }
    }

    fn save(&mut self, loc: Loc, v: f64) {
        match loc {
            Loc::Var(s) => self.set_var(s, v),
            Loc::Mem(k) => {
                if let Some(x) = self.mem.get_mut(k) {
                    *x = v;
                }
            }
            Loc::GMem(k) => {
                if let Some(x) = self.gmem.get_mut(k) {
                    *x = v;
                }
            }
            Loc::Nowhere => {}
        }
    }

    fn eval(&mut self, e: &Expr, f: &[Func]) -> f64 {
        match e {
            Expr::Num(n) => *n,
            Expr::StrRef(h) => *h as f64,
            Expr::Var(s) => self.var(*s),
            Expr::VarIdx { .. } | Expr::Mem(_) | Expr::GMem(_) => {
                let loc = self.locate(e, f);
                self.load(loc)
            }
            Expr::Unary(UnOp::Neg, x) => -self.eval(x, f),
            Expr::Unary(UnOp::Not, x) => b2f(!truthy(self.eval(x, f))),
            Expr::Bin(op, a, b) => {
                let a = self.eval(a, f);
                let b = self.eval(b, f);
                binop(*op, a, b)
            }
            Expr::LogicAnd(a, b) => b2f(truthy(self.eval(a, f)) && truthy(self.eval(b, f))),
            Expr::LogicOr(a, b) => b2f(truthy(self.eval(a, f)) || truthy(self.eval(b, f))),
            Expr::Cond(c, t, o) => {
                if truthy(self.eval(c, f)) {
                    self.eval(t, f)
                } else if let Some(o) = o {
                    self.eval(o, f)
                } else {
                    0.0
                }
            }
            Expr::Assign(target, op, rhs) => {
                let r = self.eval(rhs, f);
                let loc = self.locate(target, f);
                let v = op.map_or(r, |o| binop(o, self.load(loc), r));
                self.save(loc, v);
                v
            }
            Expr::Seq(v) => {
                let mut last = 0.0;
                for x in v {
                    last = self.eval(x, f);
                }
                last
            }
            Expr::Call(b, args) => self.call(*b, args, f),
            Expr::UserCall(id, args) => self.user_call(*id, args, f),
            Expr::Loop(n, body) => {
                let n = self.eval(n, f);
                let n = if n >= 1.0 { (n as usize).min(MAX_ITER) } else { 0 };
                for _ in 0..n {
                    if !self.tick() {
                        break;
                    }
                    self.eval(body, f);
                }
                0.0
            }
            Expr::While(cond) => {
                for _ in 0..MAX_ITER {
                    if !self.tick() || !truthy(self.eval(cond, f)) {
                        break;
                    }
                }
                0.0
            }
            Expr::WhileBody(cond, body) => {
                for _ in 0..MAX_ITER {
                    if !self.tick() || !truthy(self.eval(cond, f)) {
                        break;
                    }
                    self.eval(body, f);
                }
                0.0
            }
        }
    }

    /// The complex buffer `fft`-style functions work on: `n` points starting at `start`.
    // `user_call` and `sprintf` keep their argument arrays out of `eval`'s frame: `eval` recurses
    // once per tree level, so its frame size times `parse::MAX_HEIGHT` is the stack it needs.
    #[inline(never)]
    fn user_call(&mut self, id: usize, args: &[Expr], f: &[Func]) -> f64 {
        let mut tmp = [0.0f64; MAX_ARGS];
        for (slot, a) in tmp.iter_mut().zip(args) {
            *slot = self.eval(a, f);
        }
        let Some(func) = f.get(id) else { return 0.0 };
        if !self.charge(func.cost as usize) {
            return 0.0;
        }
        for (slot, v) in func.params.iter().zip(tmp) {
            self.set_var(*slot, v);
        }
        let ret = self.eval(&func.body, f);
        // By-reference parameters (`name*`) hand their final value back to the caller.
        for ((slot, by_ref), arg) in func.params.iter().zip(&func.refs).zip(args) {
            if *by_ref && arg.is_place() {
                let v = self.var(*slot);
                let loc = self.locate(arg, f);
                self.save(loc, v);
            }
        }
        ret
    }

    #[inline(never)]
    fn sprintf(&mut self, args: &[Expr], f: &[Func]) -> f64 {
        let mut vals = [0.0f64; MAX_ARGS];
        let (mut dst, mut fmt) = (0.0, 0.0);
        for (i, a) in args.iter().enumerate() {
            let v = self.eval(a, f);
            match i {
                0 => dst = v,
                1 => fmt = v,
                _ => {
                    if let Some(slot) = vals.get_mut(i - 2) {
                        *slot = v;
                    }
                }
            }
        }
        let n = args.len().saturating_sub(2).min(MAX_ARGS);
        let fmt_bytes = self.string_index(fmt).and_then(|i| self.strings.get(i)).map_or(&[][..], Vec::as_slice);
        format_into(&mut self.sbuf, &mut self.body, fmt_bytes, vals.get(..n).unwrap_or(&[]), &self.strings);
        self.store_scratch(dst);
        dst
    }

    /// `cost` is the work per point, in loop iterations (see [`Machine::charge`]).
    fn fft_buffer(&mut self, start: f64, n: f64, cost: fn(usize) -> usize) -> Option<&mut [f64]> {
        let n = fft::points(n)?;
        if !self.charge(cost(n)) {
            return None;
        }
        let s = index(start, self.mem.len())?;
        self.mem.get_mut(s..s.checked_add(2 * n)?)
    }

    /// `fft_real` / `ifft_real` over `size` real values at `start`. The spectrum is `size / 2`
    /// complex values in the permuted order `fft` leaves (so `fft_permute(buf, size / 2)` puts it
    /// in natural order), the first holding DC and Nyquist. Neither direction is scaled.
    fn fft_real(&mut self, start: f64, size: f64, inverse: bool) {
        let Some(n) = fft::points(size).filter(|n| *n >= 4) else { return };
        if !self.charge(transform_cost(n)) {
            return;
        }
        let Some(s) = index(start, self.mem.len()) else { return };
        let Some(cells) = s.checked_add(n).and_then(|e| self.mem.get_mut(s..e)) else { return };
        let buf = &mut self.fft_scratch;
        buf.clear();
        if inverse {
            buf.extend_from_slice(cells);
            fft::permute(buf);
            // Rebuild the full spectrum from its first half (it is conjugate-symmetric).
            buf.resize(2 * n, 0.0);
            for k in 1..n / 2 {
                let (re, im) = (buf[2 * k], buf[2 * k + 1]);
                buf[2 * (n - k)] = re;
                buf[2 * (n - k) + 1] = -im;
            }
            buf[n] = buf[1];
            buf[n + 1] = 0.0;
            buf[1] = 0.0;
            fft::permute(buf);
            fft::ifft(buf);
            for i in 0..n {
                buf[i] = buf[2 * i];
            }
        } else {
            buf.extend(cells.iter().flat_map(|c| [*c, 0.0]));
            fft::fft(buf);
            fft::permute(buf);
            // Bins 0..n/2 in natural order; Nyquist's real part goes where DC's (zero) imaginary was.
            buf[1] = buf[n];
        }
        buf.truncate(n);
        if !inverse {
            fft::permute(buf);
        }
        cells.copy_from_slice(buf);
    }

    /// `match(needle, haystack, ...)`: 1 when the haystack fits; what the `%` conversions covered
    /// goes into the extra arguments (numbers into variables, `%s` into strings).
    fn match_call(&mut self, args: &[Expr], f: &[Func], fold: bool) -> f64 {
        let needle = args.first().map_or(-1.0, |e| self.eval(e, f));
        let hay = args.get(1).map_or(-1.0, |e| self.eval(e, f));
        let (ni, hi) = (self.string_index(needle), self.string_index(hay));
        let m = &mut self.matching;
        m.needle.clear();
        m.hay.clear();
        if let Some(s) = ni.and_then(|i| self.strings.get(i)) {
            m.needle.extend_from_slice(s);
        }
        if let Some(s) = hi.and_then(|i| self.strings.get(i)) {
            m.hay.extend_from_slice(s);
        }
        let cost = m.needle.len() + m.hay.len();
        if !self.charge(cost) {
            return 0.0;
        }
        // Backtracking can do far more than the strings' length: the steps taken are paid too.
        let (hit, steps) = matches(&mut self.matching, fold);
        if !self.charge(steps) || !hit {
            return 0.0;
        }
        for k in 0..self.matching.caps.len() {
            let Some(c) = self.matching.caps.get(k).copied() else { break };
            let Some(text) = self.matching.hay.get(c.start..c.start + c.len) else { continue };
            if c.kind == b's' {
                let dst = args.get(2 + k).map_or(-1.0, |e| self.eval(e, f));
                self.sbuf.clear();
                if let Some(t) = self.matching.hay.get(c.start..c.start + c.len) {
                    self.sbuf.extend_from_slice(t);
                }
                self.store_scratch(dst);
            } else if let Some(v) = convert(c.kind, text) {
                self.put(args.get(2 + k), v, f);
            }
        }
        1.0
    }

    fn call(&mut self, b: Builtin, args: &[Expr], f: &[Func]) -> f64 {
        match b {
            Builtin::SliderChange => {
                let mask = match args.first() {
                    Some(Expr::Var(s)) if (SLIDER1..SLIDER1 + MAX_SLIDERS).contains(s) => 1u64 << (s - SLIDER1),
                    Some(a) => to_i(self.eval(a, f)) as u64,
                    None => 0,
                };
                let hit = self.changed & mask != 0;
                self.changed &= !mask;
                return b2f(hit);
            }
            Builtin::MidiRecv => {
                let Some(ev) = self.midi.get(self.midi_head).copied() else { return 0.0 };
                self.midi_head += 1;
                let off = f64::from(ev.offset);
                let [status, d1, d2] = ev.msg.map(f64::from);
                // The 3-argument form packs both data bytes into one value: `msg23 = d1 + d2 * 256`.
                let vals = if args.len() == 3 { [off, status, d1 + d2 * 256.0, 0.0] } else { [off, status, d1, d2] };
                for (a, v) in args.iter().zip(vals) {
                    let loc = self.locate(a, f);
                    self.save(loc, v);
                }
                return 1.0;
            }
            Builtin::Sprintf => return self.sprintf(args, f),
            Builtin::Match | Builtin::MatchI => return self.match_call(args, f, b == Builtin::MatchI),
            _ => {}
        }
        let a0 = args.first().map_or(0.0, |e| self.eval(e, f));
        let a1 = args.get(1).map_or(0.0, |e| self.eval(e, f));
        let a2 = args.get(2).map_or(0.0, |e| self.eval(e, f));
        let a3 = args.get(3).map_or(0.0, |e| self.eval(e, f));
        let count = |v: f64| if v >= 1.0 { v as usize } else { 0 };
        match b {
            Builtin::StrCpy => {
                self.build(a0, a1, usize::MAX, 0, false);
                self.store_scratch(a0);
                a0
            }
            Builtin::StrCat => {
                self.build(a0, a1, usize::MAX, 0, true);
                self.store_scratch(a0);
                a0
            }
            Builtin::StrNCpy => {
                self.build(a0, a1, count(a2), 0, false);
                self.store_scratch(a0);
                a0
            }
            Builtin::StrNCat => {
                self.build(a0, a1, count(a2), 0, true);
                self.store_scratch(a0);
                a0
            }
            Builtin::StrCpyFrom | Builtin::StrCpySubstr => {
                let len = self.string_bytes(a1).len() as i64;
                let off = to_i(a2);
                let off = if off < 0 { (len + off).max(0) } else { off.min(len) } as usize;
                let n = if b == Builtin::StrCpySubstr && args.len() >= 4 { count(a3) } else { usize::MAX };
                self.build(a0, a1, n, off, false);
                self.store_scratch(a0);
                a0
            }
            Builtin::StrLen => self.string_bytes(a0).len() as f64,
            Builtin::StrCmp | Builtin::StrICmp | Builtin::StrNCmp | Builtin::StrNICmp => {
                let fold = matches!(b, Builtin::StrICmp | Builtin::StrNICmp);
                let limit = if matches!(b, Builtin::StrNCmp | Builtin::StrNICmp) { count(a2) } else { usize::MAX };
                let (x, y) = (self.string_bytes(a0), self.string_bytes(a1));
                // Compared byte by byte, folding case on the fly (no copies).
                let fold_c = |c: &u8| if fold { c.to_ascii_lowercase() } else { *c };
                let ord = x.iter().take(limit).map(fold_c).cmp(y.iter().take(limit).map(fold_c));
                f64::from(ord as i8)
            }
            Builtin::StrGetChar => {
                let s = self.string_bytes(a0);
                let off = to_i(a1);
                let i = if off < 0 { s.len() as i64 + off } else { off };
                usize::try_from(i).ok().and_then(|i| s.get(i)).map_or(0.0, |c| f64::from(*c))
            }
            Builtin::StrSetChar => {
                if let Some(s) = self.string_index(a0).and_then(|i| self.strings.get_mut(i)) {
                    let off = to_i(a1);
                    let i = if off < 0 { s.len() as i64 + off } else { off };
                    if let Ok(i) = usize::try_from(i) {
                        if i < s.len() {
                            if let Some(c) = s.get_mut(i) {
                                *c = a2 as i64 as u8;
                            }
                        } else if i == s.len() && s.len() < STR_MAX {
                            s.push(a2 as i64 as u8);
                        }
                    }
                }
                a2
            }
            Builtin::Sin => a0.sin(),
            Builtin::Cos => a0.cos(),
            Builtin::Tan => a0.tan(),
            Builtin::Asin => a0.asin(),
            Builtin::Acos => a0.acos(),
            Builtin::Atan => a0.atan(),
            Builtin::Atan2 => a0.atan2(a1),
            Builtin::Sqr => a0 * a0,
            Builtin::Sqrt => a0.abs().sqrt(),
            Builtin::Pow => a0.powf(a1),
            Builtin::Exp => a0.exp(),
            Builtin::Log => a0.ln(),
            Builtin::Log10 => a0.log10(),
            Builtin::Abs => a0.abs(),
            Builtin::Min => a0.min(a1),
            Builtin::Max => a0.max(a1),
            Builtin::Sign => {
                if a0 > 0.0 {
                    1.0
                } else if a0 < 0.0 {
                    -1.0
                } else {
                    0.0
                }
            }
            Builtin::Floor => a0.floor(),
            Builtin::Ceil => a0.ceil(),
            Builtin::Int => a0.trunc(),
            Builtin::Rand => {
                let range = if args.is_empty() { 1.0 } else { a0 };
                range * self.rand01()
            }
            Builtin::Invsqrt => {
                if a0 > 0.0 {
                    1.0 / a0.sqrt()
                } else {
                    0.0
                }
            }
            Builtin::Memset => {
                if let Some(d) = index(a0, self.mem.len()) {
                    let n = if a2 > 0.0 { (a2 as usize).min(self.mem.len() - d) } else { 0 };
                    if self.charge(cells_cost(n))
                        && let Some(s) = self.mem.get_mut(d..d + n)
                    {
                        s.fill(a1);
                    }
                }
                a0
            }
            Builtin::Memcpy => {
                if let (Some(d), Some(s)) = (index(a0, self.mem.len()), index(a1, self.mem.len())) {
                    let n = if a2 > 0.0 { (a2 as usize).min(self.mem.len() - d).min(self.mem.len() - s) } else { 0 };
                    if n > 0 && self.charge(cells_cost(n)) {
                        self.mem.copy_within(s..s + n, d);
                    }
                }
                a0
            }
            Builtin::MemTop => self.mem.len() as f64,
            Builtin::HostNumChan => self.var(NUM_CH),
            Builtin::FftReal => {
                self.fft_real(a0, a1, false);
                0.0
            }
            Builtin::IfftReal => {
                self.fft_real(a0, a1, true);
                0.0
            }
            Builtin::FreeMBuf | Builtin::Ignored => 0.0,
            Builtin::Fft => {
                if let Some(buf) = self.fft_buffer(a0, a1, transform_cost) {
                    fft::fft(buf);
                }
                0.0
            }
            Builtin::Ifft => {
                if let Some(buf) = self.fft_buffer(a0, a1, transform_cost) {
                    fft::ifft(buf);
                }
                0.0
            }
            Builtin::FftPermute | Builtin::FftIpermute => {
                if let Some(buf) = self.fft_buffer(a0, a1, permute_cost) {
                    fft::permute(buf);
                }
                0.0
            }
            // Handled before the arguments are evaluated.
            Builtin::SliderChange | Builtin::MidiRecv | Builtin::Sprintf | Builtin::Match | Builtin::MatchI => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{Names, compile};

    fn run(src: &str) -> (Machine, Names) {
        let mut names = Names::new();
        let e = compile(src, 1, &mut names).unwrap();
        let mut m = Machine::new(names.len(), 1024).with_strings(names.strings.clone());
        m.run(&e, &names.funcs, 1_000_000);
        (m, names)
    }

    /// Runs `src` and returns the contents of the named string `#name`.
    fn text(src: &str, name: &str) -> String {
        let (m, mut names) = run(src);
        let h = names.string(name);
        String::from_utf8_lossy(m.string_bytes(h as f64)).into_owned()
    }

    #[test]
    fn strings_copy_concatenate_and_measure() {
        assert_eq!(text(r##"strcpy(#a, "hello"); strcat(#a, " world");"##, "#a"), "hello world");
        assert_eq!(val(r##"strcpy(#a, "hello"); x = strlen(#a);"##, "x"), 5.0);
        assert_eq!(text(r##"strcpy(#a, "abcdef"); strncpy(#b, #a, 3); strncat(#b, "XYZ", 1);"##, "#b"), "abcX");
        assert_eq!(text(r##"strcpy(#a, "abcdef"); strcpy_substr(#b, #a, 2, 3);"##, "#b"), "cde");
        assert_eq!(text(r##"strcpy(#a, "abcdef"); strcpy_substr(#b, #a, -2);"##, "#b"), "ef");
        assert_eq!(text(r##"strcpy(#a, "abcdef"); strcpy_from(#b, #a, 4);"##, "#b"), "ef");
        // Copying a string onto itself, and an unknown handle, are harmless.
        assert_eq!(text(r##"strcpy(#a, "xy"); strcat(#a, #a);"##, "#a"), "xyxy");
        assert_eq!(val("x = strlen(999); strcpy(999, 0);", "x"), 0.0);
    }

    #[test]
    fn assigning_to_a_string_copies_or_appends() {
        assert_eq!(text(r##"#a = "foo"; #a += "bar";"##, "#a"), "foobar");
        assert_eq!(text(r##"#a = "x"; #b = #a; #a = "y";"##, "#b"), "x", "a copy, not an alias");
        assert_eq!(val(r##"#a = "hello"; n = strlen(#a = "hi");"##, "n"), 2.0);
        let mut names = Names::new();
        assert!(compile(r##"#a *= 2;"##, 1, &mut names).is_err());
    }

    #[test]
    fn string_chars_and_comparison() {
        assert_eq!(val(r##"strcpy(#a, "ABC"); x = str_getchar(#a, 1); y = str_getchar(#a, -1);"##, "x"), 66.0);
        assert_eq!(val(r##"strcpy(#a, "ABC"); y = str_getchar(#a, -1);"##, "y"), 67.0);
        assert_eq!(text(r##"strcpy(#a, "ABC"); str_setchar(#a, 1, 'x'); str_setchar(#a, 3, 'D'); str_setchar(#a, 99, 'Z');"##, "#a"), "AxCD");
        assert_eq!(val(r##"x = strcmp("a", "b");"##, "x"), -1.0);
        assert_eq!(val(r##"x = strcmp("b", "a");"##, "x"), 1.0);
        assert_eq!(val(r##"x = strcmp("abc", "abc");"##, "x"), 0.0);
        assert_eq!(val(r##"x = stricmp("ABC", "abc");"##, "x"), 0.0);
        assert_eq!(val(r##"x = strncmp("abcX", "abcY", 3);"##, "x"), 0.0);
        assert_eq!(val(r##"x = strnicmp("ABx", "aby", 2);"##, "x"), 0.0);
    }

    #[test]
    fn sprintf_builds_strings_from_numbers_and_strings() {
        assert_eq!(text(r##"sprintf(#out, "%d dB at %.1f Hz (%s)", 3, 440.25, "peak");"##, "#out"), "3 dB at 440.2 Hz (peak)");
        assert_eq!(text(r##"strcpy(#fmt, "%03d"); sprintf(#out, #fmt, 7);"##, "#out"), "007");
        // Literals are shared, and an anonymous `#` is a fresh string each time.
        assert_eq!(val(r##"x = "same" == "same"; y = # == #;"##, "x"), 1.0);
        assert_eq!(val(r##"y = # == #;"##, "y"), 0.0);
    }

    #[test]
    fn string_state_is_restored_by_reset() {
        let mut names = Names::new();
        let e = compile(r##"strcat(#a, "x");"##, 1, &mut names).unwrap();
        let mut m = Machine::new(names.len(), 16).with_strings(names.strings.clone());
        m.run(&e, &names.funcs, 100);
        m.run(&e, &names.funcs, 100);
        let h = names.string("#a") as f64;
        assert_eq!(m.string_bytes(h), b"xx");
        m.reset();
        assert_eq!(m.string_bytes(h), b"");
    }

    fn val(src: &str, var: &str) -> f64 {
        let (m, mut names) = run(src);
        m.var(names.slot(var))
    }

    #[test]
    fn arithmetic_and_assignment() {
        assert_eq!(val("x = 1 + 2 * 3;", "x"), 7.0);
        assert_eq!(val("x = 2; x *= 5; x -= 1;", "x"), 9.0);
        assert_eq!(val("x = 7 % 4; y = 7 / 0;", "x"), 3.0);
        assert_eq!(val("x = 7 / 0;", "x"), 0.0);
        assert_eq!(val("x = 5 & 3 | 8;", "x"), 9.0);
        assert_eq!(val("x = 1 << 4;", "x"), 16.0);
    }

    #[test]
    fn comparisons_use_epsilon_and_logic_short_circuits() {
        assert_eq!(val("x = 1 == 1.000001;", "x"), 1.0);
        assert_eq!(val("x = 1 === 1.000001;", "x"), 0.0);
        assert_eq!(val("x = 0 && (y = 5);", "y"), 0.0);
        assert_eq!(val("x = 1 || (y = 5);", "y"), 0.0);
        assert_eq!(val("x = !0;", "x"), 1.0);
    }

    #[test]
    fn control_flow() {
        assert_eq!(val("i = 0; loop(10, i += 2;);", "i"), 20.0);
        assert_eq!(val("i = 0; while(i < 5) ( i += 1; );", "i"), 5.0);
        assert_eq!(val("i = 0; while(i += 1; i < 7);", "i"), 7.0);
        assert_eq!(val("x = 3 > 2 ? 10 : 20;", "x"), 10.0);
        assert_eq!(val("if (1) ( x = 4; ) else ( x = 9; );", "x"), 4.0);
    }

    #[test]
    fn memory_is_offset_by_the_base_variable() {
        assert_eq!(val("buf = 100; buf[3] = 7; x = mem[103];", "x"), 7.0);
        assert_eq!(val("buf = 10; buf[0] = 1; buf[1] = 2; memcpy(buf + 5, buf, 2); x = buf[6];", "x"), 2.0);
        assert_eq!(val("memset(0, 3, 4); x = mem[0] + mem[3] + mem[4];", "x"), 6.0);
        assert_eq!(val("gmem[2] = 5; x = gmem[2];", "x"), 5.0);
        assert_eq!(val("buf[2] += 4; buf[2] *= 3; x = buf[2];", "x"), 12.0);
    }

    #[test]
    fn out_of_range_memory_is_harmless() {
        assert_eq!(val("mem[-5] = 1; mem[99999999999] = 1; x = mem[-5] + mem[1e300];", "x"), 0.0);
        assert_eq!(val("x = mem[0/0];", "x"), 0.0);
        assert_eq!(val("memset(1000, 1, 1e12); memcpy(1000, 0, 1e12); x = 1;", "x"), 1.0);
    }

    #[test]
    fn runaway_loops_are_cut_off() {
        let mut names = Names::new();
        let e = compile("while(1) ( x += 1; );", 1, &mut names).unwrap();
        let mut m = Machine::new(names.len(), 16);
        assert!(!m.run(&e, &names.funcs, 1000));
        let e = compile("loop(1e9, loop(1e9, x += 1;););", 1, &mut names).unwrap();
        assert!(!m.run(&e, &names.funcs, 5000));
    }

    #[test]
    fn the_deepest_accepted_code_runs_on_a_small_stack() {
        // Per shape of code, the largest size the parser accepts. Chains grow the tree one frame
        // per level; builtin calls (eval → call → eval) and user calls cost more per level but
        // are bounded by the parser's nesting limit, which function bodies inherit from callers.
        let shapes: [fn(usize) -> String; 4] = [
            |n| format!("y = {}a;", "a+".repeat(n)),
            |n| format!("y = {}0.5{};", "sin(".repeat(n), ")".repeat(n)),
            |n| format!("y = {}a{}{};", "sin(".repeat(30), "+a".repeat(n), ")".repeat(30)),
            |n| {
                let mut s = String::from("function f0(x) ( sprintf(#s, \"%d\", x); );\n");
                for k in 1..=n {
                    s += &format!("function f{k}(x) ( sprintf(#s, \"%d\", f{}(x)); );\n", k - 1);
                }
                s + &format!("y = f{n}(1);")
            },
        ];
        let mut tallest = 0;
        for shape in shapes {
            let mut deepest = None;
            for n in 1..=1000 {
                let mut names = Names::new();
                match compile(&shape(n), 1, &mut names) {
                    Ok(e) => deepest = Some((e, names)),
                    Err(_) => break,
                }
            }
            let (e, mut names) = deepest.unwrap();
            let (funcs, nvars, strings) = (std::mem::take(&mut names.funcs), names.len(), std::mem::take(&mut names.strings));
            tallest = tallest.max(crate::parse::height(&e, &funcs));
            // Needs about 128 KiB in debug and release builds; audio threads have several times
            // this much.
            let ok = std::thread::Builder::new()
                .stack_size(256 * 1024)
                .spawn(move || Machine::new(nvars, 16).with_strings(strings).run(&e, &funcs, 1000))
                .unwrap()
                .join()
                .unwrap();
            assert!(ok);
        }
        assert!(tallest > crate::parse::MAX_HEIGHT - 4, "{tallest}");
    }

    #[test]
    fn expensive_builtins_draw_on_the_budget() {
        // A few loop iterations, each doing a big FFT or fill: cut off by the budget, not by the
        // iteration count.
        let mut names = Names::new();
        for src in [
            "loop(100, fft(0, 32768));",
            "loop(100, memset(0, 1, 65536));",
            "loop(100, memcpy(0, 65536, 65536));",
            "loop(100, fft_permute(0, 32768));",
        ] {
            let e = compile(src, 1, &mut names).unwrap();
            let mut m = Machine::new(names.len(), 1 << 17);
            assert!(!m.run(&e, &names.funcs, 100_000), "{src}");
        }
        // The same work fits a budget sized for it.
        let e = compile("loop(2, fft(0, 1024));", 1, &mut names).unwrap();
        assert!(Machine::new(names.len(), 4096).run(&e, &names.funcs, 10_000));
    }

    #[test]
    fn a_match_pays_for_its_backtracking() {
        // A short pattern that backtracks a lot over a short haystack (and never fits): cheap by
        // length, so a thousand of them used to fit a budget of 100 000.
        let fits = |src: &str| {
            let mut names = Names::new();
            let e = compile(src, 1, &mut names).unwrap();
            Machine::new(names.len(), 16).with_strings(names.strings.clone()).run(&e, &names.funcs, 100_000)
        };
        let needle = "%s".repeat(12) + "X";
        assert!(!fits(&format!("loop(1000, match(\"{needle}\", \"{}\"));", "a".repeat(40))));
        // An ordinary match still costs about its length.
        assert!(fits("loop(1000, match(\"%ddB\", \"12dB\", v));"));
    }

    #[test]
    fn calls_fanning_out_through_nested_functions_draw_on_the_budget() {
        // No loop anywhere: each level calls the one below ten times, so `f6()` runs a million
        // `f0` bodies. Calls pay for their bodies, so the budget still cuts this off.
        let mut src = String::from("function f0() ( x += 1; x += 1; x += 1; x += 1; x += 1; );\n");
        for k in 1..=6 {
            src += &format!("function f{k}() ( {} );\n", format!("f{}();", k - 1).repeat(10));
        }
        src += "f6();";
        let mut names = Names::new();
        let e = compile(&src, 1, &mut names).unwrap();
        assert!(!Machine::new(names.len(), 16).run(&e, &names.funcs, 100_000));
        // An ordinary call fits.
        let e = compile("function sq(a) ( a * a; ); y = sq(3);", 1, &mut names).unwrap();
        assert!(Machine::new(names.len(), 16).run(&e, &names.funcs, 100));
    }

    #[test]
    fn math_builtins() {
        assert!((val("x = sin($pi/2);", "x") - 1.0).abs() < 1e-12);
        assert_eq!(val("x = max(3, min(9, 5));", "x"), 5.0);
        assert_eq!(val("x = sign(-3) + int(2.9) + floor(-0.5);", "x"), 0.0);
        assert_eq!(val("x = sqrt(-4);", "x"), 2.0);
        assert!(val("x = rand(1);", "x") < 1.0);
    }

    #[test]
    fn user_functions_have_params_locals_and_a_return_value() {
        assert_eq!(val("function sq(a) ( a * a; ); x = sq(7);", "x"), 49.0);
        assert_eq!(val("function f(a, b) local(t) ( t = a - b; t * 2; ); x = f(10, 4);", "x"), 12.0);
        // Arguments are evaluated before any parameter is written, so nesting a call is safe.
        assert_eq!(val("function f(a, b) ( a * 10 + b; ); x = f(f(1, 2), f(3, 4));", "x"), 154.0);
        // Calling again reuses the instance; later definitions can call earlier ones.
        assert_eq!(val("function a(x) ( x + 1; ); function b(x) ( a(a(x)); ); y = b(5) + b(0);", "y"), 9.0);
    }

    #[test]
    fn star_parameters_write_back_to_the_callers_variables() {
        let src = "function split(l, r, mid*, side*) ( mid = (l + r) * 0.5; side = (l - r) * 0.5; ); split(3, 1, m, s);";
        assert_eq!(val(src, "m"), 2.0);
        assert_eq!(val(src, "s"), 1.0);
        // Works on memory cells too, and a plain parameter does not write back.
        assert_eq!(val("function f(a, b*) ( a = 9; b = 7; ); x = 1; f(x, buf[2]); y = buf[2];", "y"), 7.0);
        assert_eq!(val("function f(a, b*) ( a = 9; b = 7; ); x = 1; f(x, z);", "x"), 1.0);
        // A constant for a by-reference parameter is accepted and simply not written back.
        assert_eq!(val("function f(b*) ( b = 5; b + 1; ); x = f(2);", "x"), 6.0);
    }

    #[test]
    fn functions_can_be_overloaded_by_parameter_count() {
        let defs = "function f(a b c) ( 300 + a + b + c; ); function f(a b) ( 200 + a + b; ); ";
        assert_eq!(val(&format!("{defs} x = f(1, 2, 3);"), "x"), 306.0);
        assert_eq!(val(&format!("{defs} x = f(1, 2);"), "x"), 203.0);
        // One argument: the smallest overload that fits, the missing one reads 0.
        assert_eq!(val(&format!("{defs} x = f(1);"), "x"), 201.0);
        // Too many: the largest overload, the extra arguments ignored (as REAPER does).
        assert_eq!(val(&format!("{defs} x = f(1, 2, 3, 4);"), "x"), 306.0);
        assert!(val("x = rand();", "x") < 1.0);
    }

    #[test]
    fn real_fft_round_trips() {
        // A cosine in bin 2 of 16 points, forward then inverse: 16 times the input back.
        let src = "i = 0; loop(16, i[] = cos(2 * $pi * 2 * i / 16); i += 1); fft_real(0, 16); fft_permute(0, 8); re2 = 4[]; dc = 0[]; \
                   fft_ipermute(0, 8); ifft_real(0, 16); back = 3[] / 16; want = cos(2 * $pi * 2 * 3 / 16);";
        assert!((val(src, "re2") - 8.0).abs() < 1e-9);
        assert!(val(src, "dc").abs() < 1e-9);
        assert!((val(src, "back") - val(src, "want")).abs() < 1e-9);
    }

    #[test]
    fn match_captures_into_variables_and_strings() {
        assert_eq!(val("ok = match(\"%ddB\", \"12dB\", v); x = ok * 100 + v;", "x"), 112.0);
        assert_eq!(val("x = match(\"%ddB\", \"flat\", v);", "x"), 0.0);
        assert_eq!(val("x = matchi(\"*.WAV\", \"kick.wav\");", "x"), 1.0);
        assert_eq!(val("#name = \"\"; match(\"%s=%d\", \"gain=3\", #name, v); x = strlen(#name) * 10 + v;", "x"), 43.0);
    }

    #[test]
    fn assignments_end_binary_expressions() {
        assert_eq!(val("c = 3; 1 && c -= 1 > 0; x = c;", "x"), 2.0);
        assert_eq!(val("0 || g = 5; x = g;", "x"), 5.0);
    }

    #[test]
    fn wrapping_a_builtin_calls_the_builtin() {
        assert_eq!(val("function sqrt(v) ( 1 + sqrt(v) ); x = sqrt(16);", "x"), 5.0);
    }

    #[test]
    fn function_variables_are_global_unless_declared_otherwise() {
        assert_eq!(val("function f() ( g = 5; ); f(); x = g;", "x"), 5.0);
        assert_eq!(val("function f(g) ( g = 5; ); f(1); x = g;", "x"), 0.0);
        // A `global(...)` list makes the names it leaves out private.
        assert_eq!(val("function f() global(g) ( g = 5; h = 6; ); f(); x = g; y = h;", "x"), 5.0);
        assert_eq!(val("function f() global(g) ( g = 5; h = 6; ); f(); x = g; y = h;", "y"), 0.0);
        // Locals keep per-function storage, separate from a global of the same name.
        assert_eq!(val("t = 3; function f() local(t) ( t = 99; ); f(); x = t;", "x"), 3.0);
    }

    #[test]
    fn dotted_names_under_an_instance_name_follow_the_namespace() {
        let src = "function f_init() ( this.k = 2; ); \
                   function b_init() instance(filter) ( filter.f_init(); filter.m = 3; ); \
                   band1.b_init(); x = band1.filter.k * 10 + band1.filter.m; y = filter.m;";
        assert_eq!(val(src, "x"), 23.0);
        assert_eq!(val(src, "y"), 0.0);
    }

    #[test]
    fn parameters_hide_instance_variables_of_the_same_name() {
        let src = "function set(v) instance(v) ( this.v = v; ); a.set(4); x = a.v;";
        assert_eq!(val(src, "x"), 4.0);
    }

    #[test]
    fn instance_variables_live_in_a_namespace() {
        let src = "function inc() instance(n) ( n += 1; ); a.inc(); a.inc(); b.inc(); x = a.n; y = b.n;";
        assert_eq!(val(src, "x"), 2.0);
        assert_eq!(val(src, "y"), 1.0);
        let src = "function add(v) instance(sum) ( this.sum += v; ); p.add(3); p.add(4); x = p.sum;";
        assert_eq!(val(src, "x"), 7.0);
        let src = "function inner() instance(k) ( k += 10; ); function outer() instance(j) ( inner(); j += 1; ); q.outer(); x = q.k + q.j;";
        assert_eq!(val(src, "x"), 11.0);
    }

    #[test]
    fn recursion_and_unknown_calls_are_compile_errors() {
        let mut names = Names::new();
        assert!(compile("function f(x) ( f(x); ); f(1);", 1, &mut names).is_err());
    }

    #[test]
    fn char_constants_parenthesised_indexing_and_short_calls() {
        assert_eq!(val("x = 'b'; y = $'b';", "x"), 98.0);
        assert_eq!(val("x = 'ab';", "x"), (97 * 256 + 98) as f64);
        assert_eq!(val("mem[7] = 3; n = 4; x = ((n + 1) * 1)[2];", "x"), 3.0);
        // A call with fewer arguments than parameters zeroes the missing ones.
        assert_eq!(val("function f(a, b) ( a + b + 100; ); x = f(1);", "x"), 101.0);
        // A function named with a dot is reachable through a namespace.
        assert_eq!(val("function m.set(v) instance(s) ( s = v; ); p.m.set(5); x = p.s;", "x"), 5.0);
    }

    #[test]
    fn spl_and_slider_functions_are_places() {
        assert_eq!(val("spl(3) = 5; x = spl3;", "x"), 5.0);
        assert_eq!(val("slider2 = 8; x = slider(2);", "x"), 8.0);
        assert_eq!(val("spl(999) = 1; x = spl63;", "x"), 1.0);
    }

    #[test]
    fn sliderchange_reports_each_change_once() {
        let mut names = Names::new();
        let e = compile("a = sliderchange(slider2); b = sliderchange(slider2); c = sliderchange(1);", 1, &mut names).unwrap();
        let mut m = Machine::new(names.len(), 16);
        m.changed = 0;
        m.mark_slider_changed(2);
        m.run(&e, &names.funcs, 1000);
        assert_eq!((m.var(names.slot("a")), m.var(names.slot("b")), m.var(names.slot("c"))), (1.0, 0.0, 0.0));
    }

    #[test]
    fn midirecv_pops_queued_events_into_its_arguments() {
        let mut names = Names::new();
        let e = compile("while (midirecv(t, m1, m2, m3)) ( n += 1; sum += m2; last = m1; );", 1, &mut names).unwrap();
        let mut m = Machine::new(names.len(), 16);
        m.push_midi(4, 0x90, 60, 100);
        m.push_midi(9, 0x80, 61, 0);
        m.run(&e, &names.funcs, 1000);
        assert_eq!((m.var(names.slot("n")), m.var(names.slot("sum")), m.var(names.slot("last"))), (2.0, 121.0, 128.0));
        m.clear_midi();
        m.run(&e, &names.funcs, 1000);
        assert_eq!(m.var(names.slot("n")), 2.0);
    }

    #[test]
    fn fft_round_trips_through_memory() {
        let src = "i = 0; loop(16, buf[i*2] = i; i += 1;); fft(0, 16); fft_permute(0, 16); dc = buf[0]; ifft(0, 16);";
        let mut names = Names::new();
        // fft_permute after fft makes natural order; ifft then expects permuted order again.
        let e = compile(&src.replace("ifft(0, 16);", "fft_ipermute(0, 16); ifft(0, 16); back = buf[10];"), 1, &mut names).unwrap();
        let mut m = Machine::new(names.len(), 256);
        m.run(&e, &names.funcs, 100_000);
        assert_eq!(m.var(names.slot("dc")), 120.0);
        assert!((m.var(names.slot("back")) / 16.0 - 5.0).abs() < 1e-9);
    }
}
