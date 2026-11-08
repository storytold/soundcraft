//! A track's hosted instrument as an automation target, on top of the shared session format.
//!
//! The format stores an [`AutoParam::Plugin`] slot as a plain `u8`, so the instrument needs no
//! schema change: it takes [`INSTRUMENT_AUTO_SLOT`], past the insert slots. Parsing and labels
//! that know about it live here, because `AutoParam::parse` and `AutoParam::label` only know
//! insert letters.

use craft_scraft_format::{AutoParam, Insert, Track};

/// The [`AutoParam::Plugin`] slot of a track's instrument (`plugin:instrument:<id>`), past the
/// insert slots and next to trim's `u8::MAX`.
pub const INSTRUMENT_AUTO_SLOT: u8 = u8::MAX - 1;

/// Parses ids like `volume`, `pan`, `send_a_level`, `plugin:a:threshold` and
/// `plugin:instrument:1575` ([`AutoParam::parse`] plus the instrument).
pub fn parse_auto_param(s: &str) -> Option<AutoParam> {
    let orig = s.trim();
    if let Some(prefix) = orig.get(..18)
        && prefix.eq_ignore_ascii_case("plugin:instrument:")
    {
        // The prefix is ASCII, so the parameter starts right after it, in its original case.
        let param = orig.get(18..)?;
        return Some(AutoParam::Plugin { slot: INSTRUMENT_AUTO_SLOT, param: param.to_string() });
    }
    AutoParam::parse(orig)
}

/// [`AutoParam::label`], with the instrument's lanes named as such.
pub fn auto_param_label(p: &AutoParam) -> String {
    match p {
        AutoParam::Plugin { slot: INSTRUMENT_AUTO_SLOT, param } => format!("instrument {param}"),
        p => p.label(),
    }
}

/// Track helpers for the instrument slot.
pub trait TrackPlugins {
    /// The plugin an [`AutoParam::Plugin`] slot addresses: an insert, or the instrument
    /// ([`INSTRUMENT_AUTO_SLOT`]).
    fn plugin_at(&self, slot: u8) -> Option<&Insert>;
}

impl TrackPlugins for Track {
    fn plugin_at(&self, slot: u8) -> Option<&Insert> {
        if slot == INSTRUMENT_AUTO_SLOT { self.instrument.as_ref() } else { self.mixer.inserts.get(usize::from(slot)).and_then(Option::as_ref) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instrument_param_ids() {
        let strum = parse_auto_param("plugin:Instrument:1575");
        assert_eq!(strum, Some(AutoParam::Plugin { slot: INSTRUMENT_AUTO_SLOT, param: "1575".into() }));
        assert_eq!(strum.as_ref().map(auto_param_label).as_deref(), Some("instrument 1575"));
        assert_eq!(parse_auto_param(" plugin:instrument:Cutoff "), Some(AutoParam::Plugin { slot: INSTRUMENT_AUTO_SLOT, param: "Cutoff".into() }));
        assert_eq!(parse_auto_param("plugin:instrument"), None);
        // Everything else is the shared format's.
        assert_eq!(parse_auto_param("plugin:a:threshold"), Some(AutoParam::Plugin { slot: 0, param: "threshold".into() }));
        assert_eq!(parse_auto_param("send_b_level"), Some(AutoParam::SendLevel(1)));
        assert_eq!(parse_auto_param("nope"), None);
        assert_eq!(auto_param_label(&AutoParam::Plugin { slot: 0, param: "x".into() }), "insert A x");
    }
}
