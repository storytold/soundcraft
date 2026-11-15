//! The EEL2 expression tree.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    IDiv,
    Mod,
    Pow,
    Shl,
    Shr,
    And,
    Or,
    Xor,
    Eq,
    Ne,
    EqExact,
    NeExact,
    Lt,
    Le,
    Gt,
    Ge,
    /// Short-circuit operators; only used by the parser to build `LogicAnd`/`LogicOr`.
    LAnd,
    LOr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Builtin {
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Sqr,
    Sqrt,
    Pow,
    Exp,
    Log,
    Log10,
    Abs,
    Min,
    Max,
    Sign,
    Floor,
    Ceil,
    Int,
    Rand,
    Invsqrt,
    Memset,
    Memcpy,
    MemTop,
    FreeMBuf,
    /// `sliderchange(mask)`: handled specially (the argument may be a slider variable).
    SliderChange,
    /// Host notifications this host has no use for; they evaluate their arguments and give 0.
    Ignored,
    /// `midirecv(offset, msg1, msg2, msg3)`: the arguments are output places.
    MidiRecv,
    Fft,
    Ifft,
    FftReal,
    IfftReal,
    /// `match` / `matchi` (ignoring case).
    Match,
    MatchI,
    /// `get_host_numchan()`: the channels the host gives the effect.
    HostNumChan,
    FftPermute,
    FftIpermute,
    // Strings are numeric handles into the machine's string table.
    StrCpy,
    StrCat,
    StrLen,
    StrCmp,
    StrICmp,
    StrNCmp,
    StrNICmp,
    StrNCpy,
    StrNCat,
    StrGetChar,
    StrSetChar,
    StrCpySubstr,
    StrCpyFrom,
    /// `sprintf(dest, format, ...)`: variable arguments.
    Sprintf,
}

/// A compiled user-defined function instance (one per function and namespace).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Func {
    /// Variable slots the arguments are copied into.
    pub params: Vec<usize>,
    /// Parameters declared `name*`: their final value is copied back into the caller's variable.
    pub refs: Vec<bool>,
    pub body: Expr,
    /// How deep evaluating the body recurses (see `parse::height`).
    pub height: u32,
    /// What each call draws from the budget: the body's size (see `parse::size`), so calls that
    /// fan out through nested functions can't run unbounded work without a loop.
    pub cost: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Expr {
    Num(f64),
    /// A string (`#name`, `#`, or a literal): evaluates to its handle; `#s = x` copies into it.
    StrRef(usize),
    Var(usize),
    /// `spl(i)` / `slider(i)`: variable `base + (i - first)`, with `i` clamped to `first..=last`.
    VarIdx {
        base: usize,
        first: i64,
        last: i64,
        idx: Box<Expr>,
    },
    /// `mem[index]` (the index already includes the base variable).
    Mem(Box<Expr>),
    GMem(Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    LogicAnd(Box<Expr>, Box<Expr>),
    LogicOr(Box<Expr>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
    /// Target is `Var`, `VarIdx`, `Mem` or `GMem`; the operator is `None` for plain `=`.
    Assign(Box<Expr>, Option<BinOp>, Box<Expr>),
    Seq(Vec<Expr>),
    Call(Builtin, Vec<Expr>),
    UserCall(usize, Vec<Expr>),
    Loop(Box<Expr>, Box<Expr>),
    While(Box<Expr>),
    WhileBody(Box<Expr>, Box<Expr>),
}

impl Expr {
    /// Can this expression be assigned to?
    pub fn is_place(&self) -> bool {
        matches!(self, Expr::Var(_) | Expr::VarIdx { .. } | Expr::Mem(_) | Expr::GMem(_))
    }
}
