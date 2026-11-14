//! EEL2 parser: precedence climbing over the token stream, producing [`Expr`] with variable names
//! already resolved to slots.
//!
//! User-defined functions follow EEL2's rules. Parameters and `local(...)` names are private to
//! the function; any other variable is shared with the rest of the script, unless the function
//! lists its shared names with `global(...)`, which makes the unlisted ones private.
//! `instance(...)` names live in a namespace chosen by the caller (`a.method()` uses the
//! namespace `a`, so the variable is `a.name`; `this.name` names it explicitly; an unqualified call
//! from inside a method keeps the caller's namespace). Each function is compiled once per
//! namespace it is called with, so storage is static and no allocation happens at run time.
//! Functions must be defined before they are called, which also rules out recursion.

use crate::JsfxError;
use crate::ast::{BinOp, Builtin, Expr, Func, UnOp};
use crate::lex::{Lexed, Tok, lex};
use crate::vm::MAX_ARGS;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Reserved variable slots shared by the interpreter and the plugin wrapper.
pub(crate) const SPL0: usize = 0;
pub(crate) const MAX_CH: usize = 64;
pub(crate) const SLIDER1: usize = 64;
pub(crate) const MAX_SLIDERS: usize = 64;
pub(crate) const SRATE: usize = 128;
pub(crate) const NUM_CH: usize = 129;
pub(crate) const SAMPLESBLOCK: usize = 130;
pub(crate) const TEMPO: usize = 131;
pub(crate) const PLAY_STATE: usize = 132;
pub(crate) const PLAY_POSITION: usize = 133;
pub(crate) const BEAT_POSITION: usize = 134;
pub(crate) const TS_NUM: usize = 135;
pub(crate) const TS_DENOM: usize = 136;
pub(crate) const PDC_DELAY: usize = 137;
pub(crate) const PDC_BOT_CH: usize = 138;
pub(crate) const PDC_TOP_CH: usize = 139;
pub(crate) const PDC_MIDI: usize = 140;
pub(crate) const TRIGGER: usize = 141;
pub(crate) const EXT_NOINIT: usize = 142;
pub(crate) const EXT_NODENORM: usize = 143;
const FIRST_USER: usize = 144;

/// Parser recursion: nested parentheses, assignments, unary operators.
const MAX_DEPTH: u32 = 100;
const MAX_CHAIN: u32 = 500;
/// How deep evaluation may recurse (tree height, counting into called functions). The parser's
/// limits do not bound it: `a+b+c+...` builds a left-leaning chain, chains nest inside
/// parentheses, and function bodies stack up through calls. The deepest of 328 effects in a real
/// REAPER library reaches 48.
pub(crate) const MAX_HEIGHT: u32 = 256;
/// Function bodies are compiled once per namespace they are called with, so a few small
/// functions calling each other through several namespaces can multiply into a huge program.
/// Caps the tokens compiled that way per effect (a synth whose voices each hold filter objects,
/// `voice1.filter.tick()`, needs more than half a million).
const MAX_EXPANDED_TOKENS: usize = 1 << 21;

/// A parsed `function` definition, kept as tokens until it is instantiated by a call.
pub(crate) struct FnDef {
    params: Vec<String>,
    /// Which parameters were written `name*` (passed by reference).
    refs: Vec<bool>,
    instance: HashSet<String>,
    locals: HashSet<String>,
    globals: HashSet<String>,
    /// A `global(...)` list was given: only its names (and dotted ones) are shared.
    globals_only: bool,
    body: Vec<Lexed>,
}

/// Variable name → slot table and function registry, shared by every section of one effect.
pub(crate) struct Names {
    map: HashMap<String, usize>,
    next: usize,
    /// Each function name with its overloads (one per parameter count).
    defs: HashMap<String, Vec<Rc<FnDef>>>,
    instances: HashMap<String, usize>,
    in_progress: HashSet<String>,
    /// Tokens compiled into function instances so far (see `MAX_EXPANDED_TOKENS`).
    expanded: usize,
    /// Compiled function instances, indexed by `Expr::UserCall`.
    pub funcs: Vec<Func>,
    /// Initial contents of every string (named `#x`, literals, anonymous `#`); a string's handle is
    /// its index.
    pub strings: Vec<Vec<u8>>,
    string_names: HashMap<String, usize>,
    literals: HashMap<String, usize>,
}

impl Names {
    pub fn new() -> Self {
        let mut map = HashMap::new();
        for n in 0..MAX_CH {
            map.insert(format!("spl{n}"), SPL0 + n);
        }
        for n in 0..MAX_SLIDERS {
            map.insert(format!("slider{}", n + 1), SLIDER1 + n);
        }
        let fixed = [
            ("srate", SRATE),
            ("num_ch", NUM_CH),
            ("samplesblock", SAMPLESBLOCK),
            ("tempo", TEMPO),
            ("play_state", PLAY_STATE),
            ("play_position", PLAY_POSITION),
            ("beat_position", BEAT_POSITION),
            ("ts_num", TS_NUM),
            ("ts_denom", TS_DENOM),
            ("pdc_delay", PDC_DELAY),
            ("pdc_bot_ch", PDC_BOT_CH),
            ("pdc_top_ch", PDC_TOP_CH),
            ("pdc_midi", PDC_MIDI),
            ("trigger", TRIGGER),
            ("ext_noinit", EXT_NOINIT),
            ("ext_nodenorm", EXT_NODENORM),
        ];
        for (n, s) in fixed {
            map.insert(n.to_string(), s);
        }
        Self {
            map,
            next: FIRST_USER,
            defs: HashMap::new(),
            instances: HashMap::new(),
            in_progress: HashSet::new(),
            expanded: 0,
            funcs: Vec::new(),
            strings: Vec::new(),
            string_names: HashMap::new(),
            literals: HashMap::new(),
        }
    }

    fn new_string(&mut self, init: &[u8]) -> usize {
        self.strings.push(init.to_vec());
        self.strings.len() - 1
    }

    /// The handle of the named string `#name` (created empty on first use).
    pub fn string(&mut self, name: &str) -> usize {
        if let Some(&h) = self.string_names.get(name) {
            return h;
        }
        let h = self.new_string(b"");
        self.string_names.insert(name.to_string(), h);
        h
    }

    /// The handle of a string literal (equal literals share one).
    pub fn literal(&mut self, text: &str) -> usize {
        if let Some(&h) = self.literals.get(text) {
            return h;
        }
        let h = self.new_string(text.as_bytes());
        self.literals.insert(text.to_string(), h);
        h
    }

    /// A new anonymous string (each bare `#` is its own).
    pub fn fresh_string(&mut self) -> usize {
        self.new_string(b"")
    }

    pub fn slot(&mut self, name: &str) -> usize {
        if let Some(&s) = self.map.get(name) {
            return s;
        }
        let s = self.next;
        self.next += 1;
        self.map.insert(name.to_string(), s);
        s
    }

    /// Makes `name` another name for an existing slot (`slider1:amp=...` aliases).
    pub fn alias(&mut self, name: &str, slot: usize) {
        self.map.insert(name.to_string(), slot);
    }

    pub fn len(&self) -> usize {
        self.next
    }
}

fn builtin(name: &str) -> Option<(Builtin, usize, usize)> {
    use Builtin::*;
    Some(match name {
        "sin" => (Sin, 1, 1),
        "cos" => (Cos, 1, 1),
        "tan" => (Tan, 1, 1),
        "asin" => (Asin, 1, 1),
        "acos" => (Acos, 1, 1),
        "atan" => (Atan, 1, 1),
        "atan2" => (Atan2, 2, 2),
        "sqr" => (Sqr, 1, 1),
        "sqrt" => (Sqrt, 1, 1),
        "pow" => (Pow, 2, 2),
        "exp" => (Exp, 1, 1),
        "log" => (Log, 1, 1),
        "log10" => (Log10, 1, 1),
        "abs" => (Abs, 1, 1),
        "min" => (Min, 2, 2),
        "max" => (Max, 2, 2),
        "sign" => (Sign, 1, 1),
        "floor" => (Floor, 1, 1),
        "ceil" => (Ceil, 1, 1),
        "int" => (Int, 1, 1),
        "rand" => (Rand, 0, 1),
        "invsqrt" => (Invsqrt, 1, 1),
        "memset" => (Memset, 3, 3),
        "memcpy" => (Memcpy, 3, 3),
        "__memtop" => (MemTop, 0, 0),
        "freembuf" => (FreeMBuf, 1, 1),
        "sliderchange" => (SliderChange, 1, 1),
        "slider_automate" => (Ignored, 0, 2),
        // Hiding sliders only affects REAPER's slider panel.
        "slider_show" => (Ignored, 1, 2),
        "get_host_numchan" => (HostNumChan, 0, 0),
        "slider_next_chg" => (Ignored, 2, 2),
        "time" | "time_precise" => (Ignored, 0, 1),
        // Where the host puts the effect (track, chain position): not known here.
        "get_host_placement" => (Ignored, 0, 2),
        // This host has no MIDI output: sends are accepted and dropped.
        "midisend" => (Ignored, 3, 4),
        "midisend_buf" | "midirecv_buf" => (Ignored, 3, 3),
        "midirecv" => (MidiRecv, 3, 4),
        "fft" => (Fft, 2, 2),
        "ifft" => (Ifft, 2, 2),
        "fft_real" => (FftReal, 2, 2),
        "ifft_real" => (IfftReal, 2, 2),
        "fft_permute" => (FftPermute, 2, 2),
        "fft_ipermute" => (FftIpermute, 2, 2),
        "strcpy" => (StrCpy, 2, 2),
        "strcat" => (StrCat, 2, 2),
        "strlen" => (StrLen, 1, 1),
        "strcmp" => (StrCmp, 2, 2),
        "stricmp" => (StrICmp, 2, 2),
        "strncmp" => (StrNCmp, 3, 3),
        "strnicmp" => (StrNICmp, 3, 3),
        "strncpy" => (StrNCpy, 3, 3),
        "strncat" => (StrNCat, 3, 3),
        "str_getchar" => (StrGetChar, 2, 3),
        "str_setchar" => (StrSetChar, 3, 4),
        "strcpy_substr" => (StrCpySubstr, 3, 4),
        "strcpy_from" => (StrCpyFrom, 3, 3),
        "sprintf" => (Sprintf, 2, MAX_ARGS),
        "match" => (Match, 2, MAX_ARGS),
        "matchi" => (MatchI, 2, MAX_ARGS),
        // Printing has nowhere to go here.
        "printf" => (Ignored, 1, MAX_ARGS),
        _ => return None,
    })
}

/// Binary operator levels, lowest precedence first.
const LEVELS: &[&[(&str, BinOp)]] = &[
    &[("||", BinOp::LOr)],
    &[("&&", BinOp::LAnd)],
    &[
        ("===", BinOp::EqExact),
        ("!==", BinOp::NeExact),
        ("==", BinOp::Eq),
        ("!=", BinOp::Ne),
        ("<=", BinOp::Le),
        (">=", BinOp::Ge),
        ("<", BinOp::Lt),
        (">", BinOp::Gt),
    ],
    &[("&", BinOp::And), ("|", BinOp::Or), ("~", BinOp::Xor)],
    &[("<<", BinOp::Shl), (">>", BinOp::Shr)],
    &[("+", BinOp::Add), ("-", BinOp::Sub)],
    &[("*", BinOp::Mul), ("/", BinOp::Div), ("\\", BinOp::IDiv)],
    &[("%", BinOp::Mod)],
    &[("^", BinOp::Pow)],
];

type R<T> = Result<T, JsfxError>;

/// Compiles one section's source. `first_line` is the file line of its first line (for errors).
pub(crate) fn compile(src: &str, first_line: u32, names: &mut Names) -> R<Expr> {
    let toks = lex(src, first_line)?;
    let mut p = Parser { toks, pos: 0, names, depth: 0, scope: None, just_defined: false };
    let e = p.parse_seq(None)?;
    if !matches!(p.peek(), Tok::Eof) {
        return Err(p.err("unexpected token"));
    }
    check_height(&e, &names.funcs)?;
    Ok(e)
}

/// How deep evaluating `e` recurses: its tree height, where a call adds the height of the
/// function's body. Walks with its own stack, so a hostile tree cannot overflow this either.
pub(crate) fn height(e: &Expr, funcs: &[Func]) -> u32 {
    let mut max = 0;
    let mut stack = vec![(e, 1u32)];
    while let Some((e, d)) = stack.pop() {
        max = max.max(d);
        let d1 = d.saturating_add(1);
        match e {
            Expr::Num(_) | Expr::StrRef(_) | Expr::Var(_) => {}
            Expr::VarIdx { idx: x, .. } | Expr::Mem(x) | Expr::GMem(x) | Expr::Unary(_, x) | Expr::While(x) => stack.push((x, d1)),
            Expr::Bin(_, a, b) | Expr::LogicAnd(a, b) | Expr::LogicOr(a, b) | Expr::Assign(a, _, b) | Expr::Loop(a, b) | Expr::WhileBody(a, b) => {
                stack.push((a, d1));
                stack.push((b, d1));
            }
            Expr::Cond(c, t, o) => {
                stack.push((c, d1));
                stack.push((t, d1));
                if let Some(o) = o {
                    stack.push((o, d1));
                }
            }
            Expr::Seq(v) | Expr::Call(_, v) => stack.extend(v.iter().map(|x| (x, d1))),
            Expr::UserCall(id, args) => {
                stack.extend(args.iter().map(|x| (x, d1)));
                max = max.max(d.saturating_add(funcs.get(*id).map_or(0, |f| f.height)));
            }
        }
    }
    max
}

/// How many nodes `e` has, at least 1: what a call of a function with this body costs from the
/// budget. Calls inside count their arguments only; the called body pays when it runs.
pub(crate) fn size(e: &Expr) -> u64 {
    let mut n = 0u64;
    let mut stack = vec![e];
    while let Some(e) = stack.pop() {
        n += 1;
        match e {
            Expr::Num(_) | Expr::StrRef(_) | Expr::Var(_) => {}
            Expr::VarIdx { idx: x, .. } | Expr::Mem(x) | Expr::GMem(x) | Expr::Unary(_, x) | Expr::While(x) => stack.push(x),
            Expr::Bin(_, a, b) | Expr::LogicAnd(a, b) | Expr::LogicOr(a, b) | Expr::Assign(a, _, b) | Expr::Loop(a, b) | Expr::WhileBody(a, b) => {
                stack.push(a);
                stack.push(b);
            }
            Expr::Cond(c, t, o) => {
                stack.push(c);
                stack.push(t);
                stack.extend(o.as_deref());
            }
            Expr::Seq(v) | Expr::Call(_, v) | Expr::UserCall(_, v) => stack.extend(v),
        }
    }
    n
}

fn check_height(e: &Expr, funcs: &[Func]) -> R<u32> {
    let h = height(e, funcs);
    if h > MAX_HEIGHT { Err(JsfxError::Unsupported(format!("code nested {h} levels deep (at most {MAX_HEIGHT})"))) } else { Ok(h) }
}

/// The function instance whose body is being parsed.
struct Scope {
    def: Rc<FnDef>,
    ns: String,
    key: String,
}

struct Parser<'a> {
    toks: Vec<Lexed>,
    pos: usize,
    names: &'a mut Names,
    depth: u32,
    scope: Option<Scope>,
    /// The statement just parsed was a `function` definition (which needs no trailing `;`).
    just_defined: bool,
}

static EOF: Tok = Tok::Eof;

fn qualify(ns: &str, name: &str) -> String {
    if ns.is_empty() { name.to_string() } else { format!("{ns}.{name}") }
}

impl Parser<'_> {
    fn peek(&self) -> &Tok {
        self.toks.get(self.pos).map_or(&EOF, |l| &l.tok)
    }

    fn line(&self) -> u32 {
        self.toks.get(self.pos).or_else(|| self.toks.last()).map_or(0, |l| l.line)
    }

    fn err(&self, msg: &str) -> JsfxError {
        JsfxError::Syntax(format!("line {}: {msg}", self.line()))
    }

    fn is_p(&self, p: &str) -> bool {
        matches!(self.peek(), Tok::P(q) if *q == p)
    }

    fn eat_p(&mut self, p: &str) -> bool {
        let hit = self.is_p(p);
        if hit {
            self.pos += 1;
        }
        hit
    }

    fn expect_p(&mut self, p: &str) -> R<()> {
        if self.eat_p(p) { Ok(()) } else { Err(self.err(&format!("expected '{p}'"))) }
    }

    fn enter(&mut self) -> R<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH { Err(self.err("expression nested too deeply")) } else { Ok(()) }
    }

    fn at_end(&self, close: Option<&str>) -> bool {
        matches!(self.peek(), Tok::Eof) || close.is_some_and(|c| self.is_p(c))
    }

    /// Resolves a variable name to its slot in the current scope.
    fn var_slot(&mut self, name: &str) -> usize {
        let full = match &self.scope {
            None => name.to_string(),
            Some(sc) => {
                if let Some(rest) = name.strip_prefix("this.") {
                    qualify(&sc.ns, rest)
                } else if sc.def.params.iter().any(|p| p == name) {
                    // A parameter hides an instance variable of the same name: `this.x = x;`.
                    format!("{}|{name}", sc.key)
                } else if sc.def.instance.contains(name) {
                    qualify(&sc.ns, name)
                } else if sc.def.locals.contains(name) {
                    format!("{}|{name}", sc.key)
                } else if let Some(q) = self.in_instance(name) {
                    q
                } else if !sc.def.globals_only || sc.def.globals.contains(name) || name.contains('.') {
                    name.to_string()
                } else {
                    format!("{}|{name}", sc.key)
                }
            }
        };
        self.names.slot(&full)
    }

    fn parse_seq(&mut self, close: Option<&str>) -> R<Expr> {
        let mut v = Vec::new();
        loop {
            while self.eat_p(";") {}
            if self.at_end(close) {
                break;
            }
            self.just_defined = false;
            v.push(self.parse_assign()?);
            if !(self.is_p(";") || self.at_end(close) || self.just_defined) {
                return Err(self.err("expected ';'"));
            }
        }
        Ok(if v.len() <= 1 { v.pop().unwrap_or(Expr::Num(0.0)) } else { Expr::Seq(v) })
    }

    fn assign_op(&mut self) -> Option<Option<BinOp>> {
        let Tok::P(p) = self.peek() else { return None };
        let op = match *p {
            "=" => None,
            "+=" => Some(BinOp::Add),
            "-=" => Some(BinOp::Sub),
            "*=" => Some(BinOp::Mul),
            "/=" => Some(BinOp::Div),
            "\\=" => Some(BinOp::IDiv),
            "%=" => Some(BinOp::Mod),
            "^=" => Some(BinOp::Pow),
            "|=" => Some(BinOp::Or),
            "&=" => Some(BinOp::And),
            "~=" => Some(BinOp::Xor),
            _ => return None,
        };
        self.pos += 1;
        Some(op)
    }

    fn parse_assign(&mut self) -> R<Expr> {
        self.enter()?;
        let lhs = self.parse_ternary()?;
        let out = if let Some(op) = self.assign_op() {
            if matches!(lhs, Expr::StrRef(_)) {
                // `#s = x;` copies into the string, `#s += x;` appends.
                let f = match op {
                    None => Builtin::StrCpy,
                    Some(BinOp::Add) => Builtin::StrCat,
                    Some(_) => return Err(self.err("only = and += work on strings")),
                };
                let rhs = self.parse_assign()?;
                self.depth -= 1;
                return Ok(Expr::Call(f, vec![lhs, rhs]));
            }
            if !lhs.is_place() {
                return Err(self.err("cannot assign to this expression"));
            }
            let rhs = self.parse_assign()?;
            Expr::Assign(Box::new(lhs), op, Box::new(rhs))
        } else {
            lhs
        };
        self.depth -= 1;
        Ok(out)
    }

    fn parse_ternary(&mut self) -> R<Expr> {
        let cond = self.parse_bin(0)?;
        if !self.eat_p("?") {
            return Ok(cond);
        }
        let then = self.parse_assign()?;
        let other = if self.eat_p(":") { Some(Box::new(self.parse_assign()?)) } else { None };
        Ok(Expr::Cond(Box::new(cond), Box::new(then), other))
    }

    fn parse_bin(&mut self, level: usize) -> R<Expr> {
        let Some(ops) = LEVELS.get(level) else { return self.parse_unary() };
        let mut lhs = self.parse_bin(level + 1)?;
        let mut n = 0;
        while let Some(&(_, op)) = ops.iter().find(|(s, _)| self.is_p(s)) {
            self.pos += 1;
            n += 1;
            if n > MAX_CHAIN {
                return Err(self.err("expression too long"));
            }
            let mut rhs = self.parse_bin(level + 1)?;
            // EEL2 lets an assignment end a binary expression: `a && b = c` is `a && (b = c)`.
            if rhs.is_place()
                && let Some(aop) = self.assign_op()
            {
                rhs = Expr::Assign(Box::new(rhs), aop, Box::new(self.parse_assign()?));
            }
            lhs = match op {
                BinOp::LAnd => Expr::LogicAnd(Box::new(lhs), Box::new(rhs)),
                BinOp::LOr => Expr::LogicOr(Box::new(lhs), Box::new(rhs)),
                _ => Expr::Bin(op, Box::new(lhs), Box::new(rhs)),
            };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> R<Expr> {
        self.enter()?;
        let out = if self.eat_p("-") {
            match self.parse_unary()? {
                Expr::Num(n) => Expr::Num(-n),
                e => Expr::Unary(UnOp::Neg, Box::new(e)),
            }
        } else if self.eat_p("+") {
            self.parse_unary()?
        } else if self.eat_p("!") {
            Expr::Unary(UnOp::Not, Box::new(self.parse_unary()?))
        } else {
            self.parse_primary()?
        };
        self.depth -= 1;
        Ok(out)
    }

    fn parse_primary(&mut self) -> R<Expr> {
        let tok = self.peek().clone();
        self.pos += 1;
        match tok {
            Tok::Num(n) => {
                if self.eat_p("[") {
                    // `0[i]` is `mem[0 + i]`.
                    let idx = if self.is_p("]") { Expr::Num(0.0) } else { self.parse_assign()? };
                    self.expect_p("]")?;
                    return Ok(Expr::Mem(Box::new(Expr::Bin(BinOp::Add, Box::new(Expr::Num(n)), Box::new(idx)))));
                }
                Ok(Expr::Num(n))
            }
            Tok::Str(s) => {
                // Adjacent literals are one string, as in C.
                let mut s = s;
                while let Tok::Str(more) = self.peek().clone() {
                    self.pos += 1;
                    s.push_str(&more);
                }
                Ok(Expr::StrRef(self.names.literal(&s)))
            }
            Tok::P("(") => {
                let e = self.parse_seq(Some(")"))?;
                self.expect_p(")")?;
                if self.eat_p("[") {
                    // `(base)[offset]` is `mem[base + offset]`.
                    let idx = if self.is_p("]") { Expr::Num(0.0) } else { self.parse_assign()? };
                    self.expect_p("]")?;
                    return Ok(Expr::Mem(Box::new(Expr::Bin(BinOp::Add, Box::new(e), Box::new(idx)))));
                }
                Ok(e)
            }
            Tok::P("[") => {
                let idx = self.parse_assign()?;
                self.expect_p("]")?;
                Ok(Expr::Mem(Box::new(idx)))
            }
            Tok::Ident(name) => self.ident(&name),
            _ => {
                self.pos -= 1;
                Err(self.err("unexpected token in expression"))
            }
        }
    }

    fn ident(&mut self, name: &str) -> R<Expr> {
        if name.starts_with('#') {
            // A string is a number: the handle of an entry in the machine's string table.
            let h = if name == "#" { self.names.fresh_string() } else { self.names.string(name) };
            return Ok(Expr::StrRef(h));
        }
        if name == "function" && matches!(self.peek(), Tok::Ident(_)) {
            return self.parse_function_def();
        }
        if self.is_p("(") {
            self.pos += 1;
            return self.call(name);
        }
        if self.eat_p("[") {
            let idx = if self.is_p("]") { Expr::Num(0.0) } else { self.parse_assign()? };
            self.expect_p("]")?;
            if name == "gmem" {
                return Ok(Expr::GMem(Box::new(idx)));
            }
            let base = Expr::Var(self.var_slot(name));
            return Ok(Expr::Mem(Box::new(Expr::Bin(BinOp::Add, Box::new(base), Box::new(idx)))));
        }
        Ok(Expr::Var(self.var_slot(name)))
    }

    /// `function name(params) [local(..)] [instance(..)] [global(..)] ( body )`
    fn parse_function_def(&mut self) -> R<Expr> {
        if self.scope.is_some() {
            return Err(JsfxError::Unsupported("function definitions inside functions".into()));
        }
        let Tok::Ident(name) = self.peek().clone() else { return Err(self.err("expected a function name")) };
        self.pos += 1;
        self.expect_p("(")?;
        let mut params = Vec::new();
        let mut refs = Vec::new();
        while let Tok::Ident(p) = self.peek().clone() {
            params.push(p);
            self.pos += 1;
            refs.push(self.eat_p("*"));
            self.eat_p(",");
        }
        self.expect_p(")")?;
        let mut instance = HashSet::new();
        let mut locals = HashSet::new();
        let mut globals = HashSet::new();
        let mut globals_only = false;
        while let Tok::Ident(kw) = self.peek().clone() {
            if !matches!(kw.as_str(), "local" | "instance" | "global" | "globals") {
                break;
            }
            self.pos += 1;
            self.expect_p("(")?;
            let mut list = Vec::new();
            while let Tok::Ident(n) = self.peek().clone() {
                list.push(n);
                self.pos += 1;
                self.eat_p(",");
            }
            self.expect_p(")")?;
            match kw.as_str() {
                "instance" => instance.extend(list),
                "global" | "globals" => {
                    globals_only = true;
                    globals.extend(list);
                }
                _ => locals.extend(list),
            }
        }
        self.expect_p("(")?;
        let start = self.pos;
        let mut depth = 1u32;
        while let Some(l) = self.toks.get(self.pos) {
            match &l.tok {
                Tok::P("(") => depth += 1,
                Tok::P(")") => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                Tok::Eof => break,
                _ => {}
            }
            self.pos += 1;
        }
        if depth != 0 {
            return Err(self.err("unclosed function body"));
        }
        let line = self.line();
        let mut body = self.toks.get(start..self.pos).unwrap_or(&[]).to_vec();
        body.push(Lexed { tok: Tok::Eof, line });
        self.pos += 1; // the closing ')'
        let def = Rc::new(FnDef { params, refs, instance, locals, globals, globals_only, body });
        let overloads = self.names.defs.entry(name).or_default();
        match overloads.iter_mut().find(|d| d.params.len() == def.params.len()) {
            Some(slot) => *slot = def,
            None => overloads.push(def),
        }
        self.just_defined = true;
        Ok(Expr::Num(0.0))
    }

    /// Inside a function, a dotted name whose first part is one of its `instance(...)` names lives
    /// in the caller's namespace: with `instance(filter)`, `filter.a1` in `band1.f()` is
    /// `band1.filter.a1`.
    fn in_instance(&self, name: &str) -> Option<String> {
        let sc = self.scope.as_ref()?;
        let first = name.split('.').next()?;
        sc.def.instance.contains(first).then(|| qualify(&sc.ns, name))
    }

    /// The user function a call refers to and the namespace it runs in.
    fn find_fn(&self, name: &str) -> Option<(String, String)> {
        let cur = self.scope.as_ref().map(|s| s.ns.as_str()).unwrap_or("");
        if self.names.defs.contains_key(name) {
            return Some((name.to_string(), cur.to_string()));
        }
        // `a.b.method` may be the namespace `a.b` and function `method`, or namespace `a` and a
        // function that is itself named `b.method`: take the longest function name that exists.
        name.match_indices('.').find_map(|(i, _)| {
            let (prefix, f) = (name.get(..i)?, name.get(i + 1..)?);
            if !self.names.defs.contains_key(f) {
                return None;
            }
            let ns = match prefix.strip_prefix("this.") {
                Some(rest) => qualify(cur, rest),
                None if prefix == "this" => cur.to_string(),
                None => self.in_instance(prefix).unwrap_or_else(|| prefix.to_string()),
            };
            Some((f.to_string(), ns))
        })
    }

    fn parse_args(&mut self) -> R<Vec<Expr>> {
        let mut args = Vec::new();
        if !self.is_p(")") {
            loop {
                let mut arg = self.parse_assign()?;
                // An argument may be a sequence: `f(a; b, c)` passes `b`; a stray `;` before the
                // `)` or `,` is allowed.
                while self.eat_p(";") {
                    if self.is_p(")") || self.is_p(",") {
                        break;
                    }
                    arg = Expr::Seq(vec![arg, self.parse_assign()?]);
                }
                args.push(arg);
                if !self.eat_p(",") {
                    break;
                }
            }
        }
        self.expect_p(")")?;
        Ok(args)
    }

    fn user_call(&mut self, fname: &str, ns: String, args: Vec<Expr>) -> R<Expr> {
        let Some(overloads) = self.names.defs.get(fname) else {
            return Err(JsfxError::Unsupported(format!("function `{fname}`")));
        };
        // Overloads differ by parameter count: take the exact match, else the one with the fewest
        // parameters that still fits (missing trailing arguments become 0), else the one with the
        // most, which ignores the extra arguments as REAPER does.
        let Some(def) = overloads
            .iter()
            .find(|d| d.params.len() == args.len())
            .or_else(|| overloads.iter().filter(|d| d.params.len() > args.len()).min_by_key(|d| d.params.len()))
            .or_else(|| overloads.iter().max_by_key(|d| d.params.len()))
            .cloned()
        else {
            return Err(JsfxError::Unsupported(format!("function `{fname}`")));
        };
        let mut args = args;
        args.truncate(def.params.len());
        if def.params.len() > MAX_ARGS {
            return Err(JsfxError::Unsupported(format!("`{fname}` has more than {MAX_ARGS} parameters")));
        }
        let key = format!("{fname}/{}@{ns}", def.params.len());
        if let Some(&id) = self.names.instances.get(&key) {
            return Ok(Expr::UserCall(id, args));
        }
        if !self.names.in_progress.insert(key.clone()) {
            return Err(JsfxError::Unsupported(format!("recursive function `{fname}`")));
        }
        self.names.expanded = self.names.expanded.saturating_add(def.body.len());
        if self.names.expanded > MAX_EXPANDED_TOKENS {
            return Err(JsfxError::Unsupported("functions expand to too much code across namespaces".into()));
        }
        let params: Vec<usize> = def.params.iter().map(|p| self.names.slot(&format!("{key}|{p}"))).collect();
        let body = {
            let mut child = Parser {
                toks: def.body.clone(),
                pos: 0,
                names: &mut *self.names,
                depth: self.depth,
                scope: Some(Scope { def: Rc::clone(&def), ns, key: key.clone() }),
                just_defined: false,
            };
            let body = child.parse_seq(None)?;
            if !matches!(child.peek(), Tok::Eof) {
                return Err(child.err("unexpected token in function body"));
            }
            body
        };
        self.names.in_progress.remove(&key);
        let height = check_height(&body, &self.names.funcs)?;
        let id = self.names.funcs.len();
        let cost = size(&body);
        self.names.funcs.push(Func { params, refs: def.refs.clone(), body, height, cost });
        self.names.instances.insert(key, id);
        Ok(Expr::UserCall(id, args))
    }

    fn call(&mut self, name: &str) -> R<Expr> {
        match name {
            "loop" => {
                let n = self.parse_assign()?;
                self.expect_p(",")?;
                let body = self.parse_seq(Some(")"))?;
                self.expect_p(")")?;
                return Ok(Expr::Loop(Box::new(n), Box::new(body)));
            }
            "while" => {
                let cond = self.parse_seq(Some(")"))?;
                self.expect_p(")")?;
                return if self.eat_p("(") {
                    let body = self.parse_seq(Some(")"))?;
                    self.expect_p(")")?;
                    Ok(Expr::WhileBody(Box::new(cond), Box::new(body)))
                } else {
                    Ok(Expr::While(Box::new(cond)))
                };
            }
            "if" => return self.parse_if(),
            "spl" | "slider" if self.find_fn(name).is_none() => {
                let idx = self.parse_assign()?;
                self.expect_p(")")?;
                let (base, first, last) = if name == "spl" { (SPL0, 0, MAX_CH as i64 - 1) } else { (SLIDER1, 1, MAX_SLIDERS as i64) };
                return Ok(Expr::VarIdx { base, first, last, idx: Box::new(idx) });
            }
            _ => {}
        }
        if let Some((fname, ns)) = self.find_fn(name) {
            // A function named like a builtin that calls its own name inside calls the builtin
            // (scripts wrap `sqrt` or drawing functions this way).
            let wraps_builtin = builtin(&fname).is_some() && self.scope.as_ref().is_some_and(|sc| sc.key.starts_with(&format!("{fname}/")));
            if !wraps_builtin {
                let args = self.parse_args()?;
                return self.user_call(&fname, ns, args);
            }
        }
        let Some((b, min, max)) = builtin(name) else {
            return Err(JsfxError::Unsupported(format!("function `{name}`")));
        };
        let args = self.parse_args()?;
        if args.len() < min || args.len() > max {
            return Err(self.err(&format!("`{name}` takes {min} argument(s)")));
        }
        Ok(Expr::Call(b, args))
    }

    fn parse_if(&mut self) -> R<Expr> {
        let cond = self.parse_assign()?;
        if self.eat_p(",") {
            let then = self.parse_assign()?;
            let other = if self.eat_p(",") { Some(Box::new(self.parse_assign()?)) } else { None };
            self.expect_p(")")?;
            return Ok(Expr::Cond(Box::new(cond), Box::new(then), other));
        }
        self.expect_p(")")?;
        self.expect_p("(")?;
        let then = self.parse_seq(Some(")"))?;
        self.expect_p(")")?;
        let other = if matches!(self.peek(), Tok::Ident(s) if s == "else") {
            self.pos += 1;
            self.expect_p("(")?;
            let e = self.parse_seq(Some(")"))?;
            self.expect_p(")")?;
            Some(Box::new(e))
        } else {
            None
        };
        Ok(Expr::Cond(Box::new(cond), Box::new(then), other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> R<Expr> {
        compile(s, 1, &mut Names::new())
    }

    #[test]
    fn precedence() {
        // 1 + 2 * 3
        let e = parse("1+2*3").unwrap_or(Expr::Num(f64::NAN));
        assert!(matches!(e, Expr::Bin(BinOp::Add, _, r) if matches!(*r, Expr::Bin(BinOp::Mul, _, _))));
        // unary minus binds tighter than ^
        let e = parse("-2^2").unwrap_or(Expr::Num(f64::NAN));
        assert!(matches!(e, Expr::Bin(BinOp::Pow, l, _) if matches!(*l, Expr::Num(n) if n == -2.0)));
    }

    #[test]
    fn unsupported_things_are_named() {
        assert_eq!(parse("x = file_open(y);"), Err(JsfxError::Unsupported("function `file_open`".into())));
        assert_eq!(parse("x = mdct(y, 2);"), Err(JsfxError::Unsupported("function `mdct`".into())));
    }

    #[test]
    fn syntax_errors_carry_a_line() {
        let Err(JsfxError::Syntax(m)) = parse("a = 1;\nb = ;") else { panic!("expected a syntax error") };
        assert!(m.starts_with("line 2"), "{m}");
    }

    #[test]
    fn deep_nesting_is_rejected_not_overflowed() {
        let s = format!("{}1{}", "(".repeat(5000), ")".repeat(5000));
        assert!(parse(&s).is_err());
    }

    #[test]
    fn long_chains_and_call_chains_count_toward_the_height() {
        // Within the parser's chain limit, but too tall to evaluate safely.
        assert!(matches!(parse(&format!("x = {}a;", "a+".repeat(300))), Err(JsfxError::Unsupported(_))));
        assert!(parse(&format!("x = {}a;", "a+".repeat(200))).is_ok());
        // Chains nested at the bottom of chains add up.
        let nest = |levels: usize| (0..levels).fold("a".to_string(), |s, _| format!("({s}{})", "+a".repeat(100)));
        assert!(parse(&format!("x = {};", nest(2))).is_ok());
        assert!(matches!(parse(&format!("x = {};", nest(4))), Err(JsfxError::Unsupported(_))));
        // A function's body stacks on its caller's height.
        let chain = "+a".repeat(100);
        let src = format!("function f0(x) ( x{chain}; );\nfunction f1(x) ( f0(x){chain}; );\nfunction f2(x) ( f1(x){chain}; );\n");
        assert!(parse(&format!("{src}y = f1(1);")).is_ok());
        assert!(matches!(parse(&format!("{src}y = f2(1);")), Err(JsfxError::Unsupported(_))));
    }

    #[test]
    fn instantiation_across_namespaces_is_capped() {
        // Each level calls the one below through two namespaces: 2^40 instances if nothing stopped it.
        let mut src = String::from("function g0() instance(n) ( n += 1; );\n");
        for k in 1..=40 {
            src += &format!("function g{k}() ( this.a.g{p}(); this.b.g{p}(); );\n", p = k - 1);
        }
        let Err(JsfxError::Unsupported(m)) = parse(&(src + "g40();")) else { panic!("should be refused") };
        assert!(m.contains("too much code"), "{m}");
    }

    #[test]
    fn loops_and_conditionals_parse() {
        assert!(parse("loop(4, a += 1; b += 2;); while(i < 3) ( i += 1; ); x ? y = 1 : z = 2;").is_ok());
        assert!(parse("if (x) ( y = 1; ) else ( y = 2; );").is_ok());
    }

    #[test]
    fn function_definitions_need_no_trailing_semicolon() {
        assert!(parse("function f(a) ( a; )\nfunction g() local(x) instance(y) global(z) ( x = 1; )\nq = f(1);").is_ok());
        // Space-separated declaration lists are accepted.
        assert!(parse("function h() local(a b c) ( a = 1; ); h();").is_ok());
        // Definitions may follow their callers' text only if they come first.
        assert!(parse("h(); function h() ( 1; );").is_err());
    }
}
