use super::strip::strip_channels;
use super::{Plugin, Session, TrackId, main_channels};

/// Slot index used for a track's instrument plugin in [`InstanceSpec`] and [`PreparedInstance`]
/// (insert slots use their index).
pub const INSTRUMENT_SLOT: usize = usize::MAX;

/// Hosted third-party plugin ids (CLAP or VST3): created off the audio thread in realtime use.
pub fn is_third_party(id: &str) -> bool {
    id.starts_with(soundcraft_clap_host::ID_PREFIX)
        || id.starts_with(soundcraft_vst3_host::ID_PREFIX)
        || id.starts_with(soundcraft_au_host::ID_PREFIX)
}

/// A third-party plugin instance a session needs: which slot, which plugin, how many channels,
/// and the state to restore.
#[derive(Debug, Clone, PartialEq)]
pub struct InstanceSpec {
    pub track: TrackId,
    /// Insert slot index, or [`INSTRUMENT_SLOT`].
    pub slot: usize,
    pub id: String,
    /// Channel count the instance is prepared for.
    pub ch: usize,
    /// Decoded [`Insert::state`](soundcraft_model::Insert::state).
    pub state: Option<Vec<u8>>,
}

/// A plugin instance created and prepared off the audio thread, handed to [`MixEngine::adopt`].
pub struct PreparedInstance {
    pub track: TrackId,
    pub slot: usize,
    pub id: String,
    pub ch: usize,
    pub plugin: Box<dyn Plugin>,
}

/// Every third-party plugin slot (inserts and instruments) of a session, with the channel count
/// [`MixEngine::sync`] prepares it for. Inactive and master tracks are included (they have strips).
pub fn instance_specs(s: &Session) -> Vec<InstanceSpec> {
    let main_ch = main_channels(s);
    let mut out = Vec::new();
    for t in &s.tracks {
        let ch = strip_channels(t, main_ch);
        for (i, ins) in t.mixer.inserts.iter().enumerate() {
            if let Some(ins) = ins
                && is_third_party(&ins.plugin)
            {
                out.push(InstanceSpec { track: t.id, slot: i, id: ins.plugin.clone(), ch, state: ins.state_bytes() });
            }
        }
        if let Some(ins) = &t.instrument
            && is_third_party(&ins.plugin)
        {
            out.push(InstanceSpec { track: t.id, slot: INSTRUMENT_SLOT, id: ins.plugin.clone(), ch: ch.max(2), state: ins.state_bytes() });
        }
    }
    out
}

/// A plugin made by [`create_instance`].
pub struct NewInstance {
    pub plugin: Box<dyn Plugin>,
    /// The plugin refused the stored state and starts from its defaults.
    pub refused_state: bool,
}

/// Creates a plugin (built-in or hosted), restores `state` (if any) and prepares it. Expensive
/// for third-party plugins: in realtime use, call it on the control thread.
pub fn create_instance(id: &str, state: Option<&[u8]>, sample_rate: f32, max_block: usize, ch: usize) -> Option<NewInstance> {
    let mut plugin = create_plugin(id)?;
    let refused_state = state.is_some_and(|st| !plugin.load_state(st));
    if refused_state {
        log::warn!("{id}: the plugin did not accept its stored state");
    }
    plugin.prepare(sample_rate, max_block, ch);
    Some(NewInstance { plugin, refused_state })
}

/// How often a strip's plugins were taken out of the signal path for putting out NaN or infinity.
/// Each stays out until the engine is next reset (playback starts or stops).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PluginFailures {
    /// It only grows.
    pub count: u32,
    /// The slot of the latest: an insert index or [`INSTRUMENT_SLOT`].
    pub last: usize,
}

impl PluginFailures {
    pub(crate) fn record(&mut self, slot: usize) {
        self.count = self.count.wrapping_add(1);
        self.last = slot;
    }
}

/// A built-in plugin, else a hosted CLAP plugin (`clap:<id>`).
fn create_plugin(id: &str) -> Option<Box<dyn Plugin>> {
    // Tests never load real plug-ins; adopt a fake instead. Scanning the machine's plug-in folders
    // loads every installed plug-in in-process, and some never unload, so a test that reached the
    // real hosts would hang on a developer machine (CI has none installed).
    #[cfg(test)]
    if is_third_party(id) {
        return None;
    }
    soundcraft_dsp::create(id)
        .or_else(|| soundcraft_clap_host::create(id))
        .or_else(|| soundcraft_vst3_host::create(id))
        .or_else(|| soundcraft_au_host::create(id))
}
