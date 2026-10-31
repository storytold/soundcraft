#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
//! SoundCraft's JSFX host.
//!
//! Loads REAPER-style JS effects (`.jsfx` text files, usually extension-less) and exposes each as a
//! [`soundcraft_dsp::Plugin`], so the mix engine treats them like the built-in processors. Plugin
//! ids are `jsfx:<path relative to the Effects folder>`; parameter ids are `slider1`..`slider64`.
//!
//! The EEL2 language is implemented from scratch (clean-room): a lexer, a precedence-climbing parser
//! producing a small expression tree with resolved variable slots, and a tree-walking interpreter.
//! Hostile scripts cannot hang or crash the host: every loop iteration, and builtins in proportion
//! to their work (FFTs, memory fills and copies), draw on a per-section fuel budget (a section that
//! exhausts it faults the effect, which then passes audio through), memory accesses are
//! bounds-checked, the tree height (counting into called functions, so the interpreter's stack)
//! and how much code functions expand to are capped at compile time, and the interpreter never
//! allocates while processing.
//!
//! Supported today: the `desc`/`sliderN`/`in_pin`/`out_pin`/`options`/`import` header, the `@init`,
//! `@slider`, `@block` and `@sample` sections, operators, `mem[]`/`gmem[]` (gmem is per instance),
//! `loop`/`while`/`if`, user-defined functions (parameters, `local`, `instance`, `global`, `this.`,
//! `ns.method()`), `spl()`/`slider()`, `sliderchange`, MIDI input (`midirecv`, fed by note events),
//! `fft`/`ifft`/`fft_real`/`ifft_real`/`fft_permute`/`fft_ipermute`, strings (`strcpy` and
//! friends, `sprintf`, `match`/`matchi`) and the common math builtins. Not yet: `@serialize`,
//! `@gfx`, MIDI output (sends are dropped), `mdct`/convolution, file functions. Scripts that need
//! them fail to load with [`JsfxError::Unsupported`] naming the missing feature.
//!
//! `PluginInfo` must be `&'static`: one `PluginInfo` is leaked per distinct effect source and cached,
//! so the leak is bounded by the number of different effects actually loaded.

mod ast;
mod fft;
mod header;
mod lex;
mod matching;
mod parse;
mod plugin;
mod registry;
pub mod scan;
mod strfmt;
mod vm;

pub use plugin::{ImportLoader, JsfxPlugin};
pub use registry::{add_search_dir, create, parse_id, plugin_info, rescan, scan};
pub use scan::{JsfxEntry, soundcraft_data_dir, soundcraft_effects_dir};

/// Prefix of every hosted-JSFX plugin id.
pub const ID_PREFIX: &str = "jsfx:";

/// Why a JSFX operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JsfxError {
    #[error("syntax error: {0}")]
    Syntax(String),
    /// The script uses a language feature this host does not implement yet.
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("i/o error: {0}")]
    Io(String),
}
