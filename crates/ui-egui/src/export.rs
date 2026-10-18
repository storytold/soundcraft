//! File exports overlay existing live-state captures on a detached document.
//! This isolates document writes; it does not establish capture freshness or identity.
use crate::SoundApp;
use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::TrackId;

pub(crate) fn is_detached(id: &str) -> bool {
    matches!(id, "file.bounce_mix" | "file.bounce_stems" | "session.save_copy")
}

impl SoundApp {
    pub(super) fn live_plugin_states(&self) -> soundcraft_playback::PluginStates {
        #[cfg(test)]
        if let Some(states) = &self.captured_states_for_test {
            return states.clone();
        }
        self.player.as_ref().map(|player| player.capture_states()).unwrap_or_default()
    }

    pub(crate) fn export_engine(&self) -> Result<Engine, String> {
        detached_engine(&self.engine, self.live_plugin_states())
    }
}

fn detached_engine(original: &Engine, states: Vec<(TrackId, usize, Vec<u8>)>) -> Result<Engine, String> {
    let mut copy = Engine::new(original.session().clone());
    copy.path = original.path.clone();
    for (track, slot, bytes) in states {
        let slot = if slot == soundcraft_mix::INSTRUMENT_SLOT { json!("instrument") } else { json!(slot) };
        copy.execute("mix.insert_state", &json!({"track": track.0, "slot": slot, "state": soundcraft_model::b64::encode(&bytes)}))
            .map_err(|error| error.to_string())?;
    }
    Ok(copy)
}

#[cfg(test)]
mod tests;
