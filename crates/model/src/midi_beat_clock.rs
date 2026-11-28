use crate::Session;

/// Legacy MIDI Beat Clock output settings stored in a session's flags. The desktop host
/// imports them once into the per-computer MIDI sync preferences.
pub struct MidiBeatClock<'a> {
    pub enabled: bool,
    pub destination: Option<&'a str>,
    /// Signed offset in samples. Positive values delay the outgoing clock.
    pub offset: i64,
}

impl<'a> MidiBeatClock<'a> {
    pub fn from_session(session: &'a Session) -> Self {
        let edit = &session.edit;
        let destination = edit.flags.iter().find_map(|flag| flag.strip_prefix("midi.beat_clock.destination=")).filter(|name| !name.is_empty());
        Self {
            enabled: edit.flag("midi.beat_clock"),
            destination,
            offset: edit.value("midi.beat_clock.offset", 0.0).clamp(-10_000_000.0, 10_000_000.0) as i64,
        }
    }
}
