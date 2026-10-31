//! SoundCraft's Audio Unit (AUv2, macOS) plugin host.
//!
//! Lists the effect (`aufx`), music effect (`aumf`) and instrument (`aumu`) Audio Units
//! registered with the system's component manager and exposes each one as a
//! [`soundcraft_dsp::Plugin`], so the mix engine treats them exactly like the built-in plugins.
//! Plugin ids are `au:<type>:<subtype>:<manufacturer>` with the three four-char codes written out
//! (`au:aufx:dely:appl`); a code with bytes outside printable ASCII is written `#` + 8 upper-case
//! hex digits instead. Parameter ids are global-scope `AudioUnitParameterID`s as decimal strings;
//! values are the Audio Unit's own (plain) values, ranges and units, so no mapping is needed.
//! Boolean parameters become toggles, indexed ones starting at 0 become choices.
//!
//! Like `soundcraft-vst3-host`, this is an isolated `unsafe` crate:
//! `unsafe` is denied crate-wide and allowed only in the private `ffi` module (hand-written
//! AudioToolbox / CoreFoundation declarations). The public API is safe, returns
//! `Result`/`Option` and never panics. Limits of in-process hosting: an Audio Unit that itself
//! crashes takes the process down with it. Components that require asynchronous instantiation
//! (out-of-process AUv3 extensions) are not listed.
//!
//! `PluginInfo` must be `&'static`: the registry leaks exactly one `PluginInfo` (with its param
//! table) per plugin *type* the first time that type is instantiated, and caches it.
//!
//! Plugin state: [`Plugin::save_state`] returns the unit's `ClassInfo` property list
//! (`kAudioUnitProperty_ClassInfo`) as a binary plist; [`Plugin::load_state`] parses it back and
//! sets it, then re-reads the parameter values.
//!
//! Host callbacks report the session's tempo, meter, position and play state.
//!
//! Not hosted yet: Audio Unit editor views (Cocoa `AUCocoaUIBase`), sidechain/extra buses, and
//! parameters outside the global scope.
//!
//! Off macOS (and on `wasm32`) the crate compiles to stubs: scans are empty and nothing can be
//! created.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod ffi;
#[cfg(target_os = "macos")]
mod plugin;
#[cfg(target_os = "macos")]
mod registry;

#[cfg(target_os = "macos")]
pub use plugin::AuPlugin;

use soundcraft_dsp::{Category, Plugin, PluginInfo};

/// Prefix of every hosted-Audio-Unit plugin id.
pub const ID_PREFIX: &str = "au:";

/// `aufx`: an audio effect.
pub const TYPE_EFFECT: u32 = u32::from_be_bytes(*b"aufx");
/// `aumf`: a music effect (an effect that also takes MIDI).
pub const TYPE_MUSIC_EFFECT: u32 = u32::from_be_bytes(*b"aumf");
/// `aumu`: a music device (instrument).
pub const TYPE_INSTRUMENT: u32 = u32::from_be_bytes(*b"aumu");
/// The component types SoundCraft hosts.
pub const SUPPORTED_TYPES: [u32; 3] = [TYPE_EFFECT, TYPE_MUSIC_EFFECT, TYPE_INSTRUMENT];

/// True when a component type is one SoundCraft hosts (effects, music effects, instruments).
pub fn is_supported_type(ty: u32) -> bool {
    SUPPORTED_TYPES.contains(&ty)
}

/// Writes a four-char code: its 4 characters when all are printable ASCII (`"dls "`), else `#`
/// followed by 8 upper-case hex digits.
pub fn fourcc_string(code: u32) -> String {
    let b = code.to_be_bytes();
    if b.iter().all(|c| (0x20..=0x7e).contains(c)) { b.iter().map(|&c| char::from(c)).collect() } else { format!("#{code:08X}") }
}

/// Parses one four-char code written by [`fourcc_string`].
pub fn parse_fourcc(s: &str) -> Option<u32> {
    match parse_field(s)? {
        (code, "") => Some(code),
        _ => None,
    }
}

/// Parses one code at the start of `s`, returning it and the rest.
fn parse_field(s: &str) -> Option<(u32, &str)> {
    let bytes = s.as_bytes();
    // `#XXXXXXXX`, only when followed by the end or a field separator (a printable code may
    // itself start with `#`).
    if bytes.first() == Some(&b'#')
        && let Some(hex) = s.get(1..9)
        && hex.bytes().all(|c| c.is_ascii_hexdigit())
        && matches!(bytes.get(9), None | Some(b':'))
        && let Ok(code) = u32::from_str_radix(hex, 16)
    {
        return Some((code, s.get(9..)?));
    }
    let four = bytes.get(..4)?;
    if !four.iter().all(|c| (0x20..=0x7e).contains(c)) {
        return None;
    }
    let code = u32::from_be_bytes([*four.first()?, *four.get(1)?, *four.get(2)?, *four.get(3)?]);
    Some((code, s.get(4..)?))
}

/// The SoundCraft id of a component: `au:<type>:<subtype>:<manufacturer>`.
pub fn format_id(ty: u32, subtype: u32, manufacturer: u32) -> String {
    format!("{ID_PREFIX}{}:{}:{}", fourcc_string(ty), fourcc_string(subtype), fourcc_string(manufacturer))
}

/// The `(type, subtype, manufacturer)` codes of an `au:…` id (`None` for other ids or a
/// component type SoundCraft does not host).
pub fn parse_id(id: &str) -> Option<(u32, u32, u32)> {
    let rest = id.strip_prefix(ID_PREFIX)?;
    let (ty, rest) = parse_field(rest)?;
    let (sub, rest) = parse_field(rest.strip_prefix(':')?)?;
    let (manu, rest) = parse_field(rest.strip_prefix(':')?)?;
    (rest.is_empty() && is_supported_type(ty)).then_some((ty, sub, manu))
}

/// The insert-menu category of a component, from its type, subtype and name.
pub fn category_for(ty: u32, subtype: u32, name: &str) -> Category {
    if ty == TYPE_INSTRUMENT {
        return Category::Instrument;
    }
    let by_sub = match &subtype.to_be_bytes() {
        b"dely" | b"sdly" | b"mdly" => Some(Category::Delay),
        b"lpas" | b"hpas" | b"bpas" | b"hshf" | b"lshf" | b"pmeq" | b"nbeq" | b"geq " => Some(Category::Eq),
        b"dcmp" | b"mcmp" | b"lmtr" | b"xpnd" | b"ngte" => Some(Category::Dynamics),
        b"rvb2" | b"mrev" => Some(Category::Reverb),
        b"dist" => Some(Category::Harmonic),
        b"tmpt" | b"pitc" | b"nutp" => Some(Category::PitchShift),
        _ => None,
    };
    if let Some(c) = by_sub {
        return c;
    }
    let n = name.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&["reverb", "verb", "room", "hall", "plate"]) {
        Category::Reverb
    } else if has(&["delay", "echo"]) {
        Category::Delay
    } else if has(&["eq", "filter", "lowpass", "highpass", "bandpass", "shelf"]) {
        Category::Eq
    } else if has(&["compress", "limit", "gate", "expand", "dynamic", "de-ess"]) {
        Category::Dynamics
    } else if has(&["chorus", "flang", "phase", "tremolo", "vibrato", "rotary", "ring mod"]) {
        Category::Modulation
    } else if has(&["distort", "satur", "overdrive", "fuzz", "bitcrush"]) {
        Category::Harmonic
    } else if has(&["pitch", "tune", "shift"]) {
        Category::PitchShift
    } else {
        Category::Other
    }
}

/// Splits a component name `"Vendor: Plugin"` into `(vendor, name)`.
pub fn split_component_name(full: &str) -> (String, String) {
    match full.split_once(':') {
        Some((v, n)) if !n.trim().is_empty() => (v.trim().to_string(), n.trim().to_string()),
        _ => (String::new(), full.trim().to_string()),
    }
}

/// One Audio Unit found by a scan.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct AuDescriptor {
    /// SoundCraft id: `au:<type>:<subtype>:<manufacturer>`.
    pub id: String,
    pub name: String,
    pub vendor: String,
    /// `major.minor.bugfix` from the component version.
    pub version: String,
    /// The four-char codes as written in the id.
    pub component_type: String,
    pub subtype: String,
    pub manufacturer: String,
    pub category: Category,
    pub is_instrument: bool,
}

/// Why an Audio Unit operation failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AuError {
    #[error("no Audio Unit `{0}`")]
    NotFound(String),
    #[error("cannot create Audio Unit `{0}`: {1}")]
    Instantiate(String, String),
    #[error("Audio Unit processing: {0}")]
    Process(String),
    #[error("Audio Units are not supported on this platform")]
    Unsupported,
}

#[cfg(target_os = "macos")]
mod api {
    use super::{AuDescriptor, AuError, Plugin, PluginInfo, registry};

    pub fn scan() -> Vec<AuDescriptor> {
        registry::scan()
    }
    pub fn rescan() -> Vec<AuDescriptor> {
        registry::rescan()
    }
    pub fn instantiate(id: &str) -> Result<Box<dyn Plugin>, AuError> {
        registry::instantiate(id)
    }
    pub fn plugin_info(id: &str) -> Option<&'static PluginInfo> {
        registry::plugin_info(id)
    }
}

#[cfg(not(target_os = "macos"))]
mod api {
    use super::{AuDescriptor, AuError, Plugin, PluginInfo};

    pub fn scan() -> Vec<AuDescriptor> {
        Vec::new()
    }
    pub fn rescan() -> Vec<AuDescriptor> {
        Vec::new()
    }
    pub fn instantiate(_id: &str) -> Result<Box<dyn Plugin>, AuError> {
        Err(AuError::Unsupported)
    }
    pub fn plugin_info(_id: &str) -> Option<&'static PluginInfo> {
        None
    }
}

/// Every hostable Audio Unit registered on the system (effects, music effects, instruments).
/// Listed on first call (reads the component registry, creates no instances), then cached.
pub fn scan() -> Vec<AuDescriptor> {
    api::scan()
}

/// Forgets the cached scan (and remembered failures) and lists again.
pub fn rescan() -> Vec<AuDescriptor> {
    api::rescan()
}

/// Creates a plugin by `au:<type>:<subtype>:<manufacturer>`, prepared for 48 kHz stereo with
/// 1024-frame blocks (like `soundcraft_dsp::create`), with the reason when it fails.
pub fn instantiate(id: &str) -> Result<Box<dyn Plugin>, AuError> {
    api::instantiate(id)
}

/// Like [`instantiate`], but returns the concrete [`AuPlugin`].
#[cfg(target_os = "macos")]
pub fn instantiate_plugin(id: &str) -> Result<AuPlugin, AuError> {
    registry::instantiate_plugin(id)
}

/// Creates a plugin by `au:…` id; `None` if unknown or it fails to load.
pub fn create(id: &str) -> Option<Box<dyn Plugin>> {
    parse_id(id)?;
    match api::instantiate(id) {
        Ok(p) => Some(p),
        Err(e) => {
            log::warn!("{e}");
            None
        }
    }
}

/// The description of an `au:…` plugin. The first call for a type instantiates it once to read
/// its parameters; the result is cached (and leaked, see the crate docs).
pub fn plugin_info(id: &str) -> Option<&'static PluginInfo> {
    api::plugin_info(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fourccs_round_trip() {
        assert_eq!(parse_fourcc("aufx"), Some(TYPE_EFFECT));
        assert_eq!(fourcc_string(TYPE_INSTRUMENT), "aumu");
        assert_eq!(fourcc_string(u32::from_be_bytes(*b"dls ")), "dls ");
        assert_eq!(fourcc_string(0x0102_0304), "#01020304");
        assert_eq!(parse_fourcc("#01020304"), Some(0x0102_0304));
        assert_eq!(parse_fourcc("#0102030"), None);
        assert_eq!(parse_fourcc("#abc"), Some(u32::from_be_bytes(*b"#abc")));
        assert_eq!(parse_fourcc("abc"), None);
        assert_eq!(parse_fourcc("abcde"), None);
        assert_eq!(parse_fourcc("ab\u{e9}"), None);
        assert_eq!(parse_fourcc(""), None);
        for code in [0u32, 0x7f7f_7f7f, 0x2020_2020, u32::MAX, u32::from_be_bytes(*b"a:b:")] {
            assert_eq!(parse_fourcc(&fourcc_string(code)), Some(code), "{code:08X}");
        }
    }

    #[test]
    fn ids_parse_and_format() {
        let dly = (TYPE_EFFECT, u32::from_be_bytes(*b"dely"), u32::from_be_bytes(*b"appl"));
        assert_eq!(format_id(dly.0, dly.1, dly.2), "au:aufx:dely:appl");
        assert_eq!(parse_id("au:aufx:dely:appl"), Some(dly));
        let dls = (TYPE_INSTRUMENT, u32::from_be_bytes(*b"dls "), u32::from_be_bytes(*b"appl"));
        assert_eq!(parse_id("au:aumu:dls :appl"), Some(dls));
        // Colons inside a code are fine: fields have fixed widths.
        let odd = (TYPE_MUSIC_EFFECT, u32::from_be_bytes(*b"a:b:"), 0x0000_00FF);
        let s = format_id(odd.0, odd.1, odd.2);
        assert_eq!(s, "au:aumf:a:b::#000000FF");
        assert_eq!(parse_id(&s), Some(odd));
        for bad in [
            "au:",
            "au:aufx",
            "au:aufx:dely",
            "au:aufx:dely:",
            "au:aufx:dely:appl:",
            "au:aufx:dely:appl ",
            "au:aufxdelyappl",
            "au:aufx;dely;appl",
            "AU:aufx:dely:appl",
            "au:aufc:dely:appl",
            "au:auou:def :appl",
            "au:aufx:dé:appl",
            "vst3:0123",
            "eq_7band",
            "",
        ] {
            assert_eq!(parse_id(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn categories_and_names() {
        let c = |ty: &[u8; 4], sub: &[u8; 4], name: &str| category_for(u32::from_be_bytes(*ty), u32::from_be_bytes(*sub), name);
        assert_eq!(c(b"aumu", b"dls ", "DLSMusicDevice"), Category::Instrument);
        assert_eq!(c(b"aufx", b"dely", "AUDelay"), Category::Delay);
        assert_eq!(c(b"aufx", b"lpas", "AULowpass"), Category::Eq);
        assert_eq!(c(b"aufx", b"dcmp", "AUDynamicsProcessor"), Category::Dynamics);
        assert_eq!(c(b"aufx", b"mrev", "AUMatrixReverb"), Category::Reverb);
        assert_eq!(c(b"aufx", b"xxxx", "Big Plate Verb"), Category::Reverb);
        assert_eq!(c(b"aufx", b"xxxx", "Tape Echo"), Category::Delay);
        assert_eq!(c(b"aufx", b"xxxx", "Thing"), Category::Other);
        assert_eq!(split_component_name("Apple: AUDelay"), ("Apple".into(), "AUDelay".into()));
        assert_eq!(split_component_name("NoVendor"), (String::new(), "NoVendor".into()));
        assert_eq!(split_component_name("Odd:"), (String::new(), "Odd:".into()));
    }

    #[test]
    fn foreign_ids_are_rejected_without_scanning() {
        assert!(create("eq_7band").is_none());
        assert!(create("vst3:0123456789ABCDEF0123456789ABCDEF").is_none());
        assert!(plugin_info("reverb").is_none());
        assert!(plugin_info("au:aufx").is_none());
    }

    #[test]
    fn unknown_components_are_errors_not_crashes() {
        let missing = "au:aufx:zzzz:zzzz";
        assert!(create(missing).is_none());
        assert!(plugin_info(missing).is_none());
        assert!(instantiate(missing).is_err());
    }
}
