//! Host-side transport and transient beat grid for an external 24 PPQN source.
use crate::{SoundApp, TransportRequest, Value, json};
use soundcraft_playback::clock_follow::{Action, Snapshot, State};
use soundcraft_time::{SampleRate, TICKS_PER_QUARTER, TempoMap};

const TICKS_PER_PULSE: i64 = TICKS_PER_QUARTER / 24;

#[derive(Default)]
pub(crate) struct Host {
    pub(crate) enabled: bool,
    pub(crate) show: bool,
    pub(super) snapshot: Snapshot,
    transport_error_us: Option<i64>,
    actions: Vec<Action>,
    grid_dirty: bool,
    // Musical pulse and timestamp of the last grid segment, in the input clock.
    anchor: Option<(i64, i64)>,
}

impl SoundApp {
    pub(crate) fn clock_follow_state(&self) -> Value {
        let s = self.clock_follow.snapshot;
        let state = match s.state {
            State::Waiting => "Waiting",
            State::Locked => "Locked",
            State::Coasting => "Coasting",
            State::Stopped => "Stopped",
        };
        let source = self.ui.clock_follow_input.as_ref();
        json!({"input":source,"enabled":self.clock_follow.enabled, "loss_policy":if self.ui.clock_follow_stop_on_loss {"stop"} else {"coast"},
            "input_offset_samples":source.and_then(|p|self.ui.clock_follow_input_offsets.get(p)).copied().unwrap_or(0),
            "state":state,"estimated_bpm":s.bpm,"jitter_us":s.jitter_us,"arrival_residual_us":s.phase_error_us,"phase_error_us":self.clock_follow.transport_error_us,"dropouts":s.dropouts})
    }

    pub(crate) fn clock_follow_command(&mut self, p: &Value) -> Result<Value, String> {
        let args = p.as_object().ok_or("setup.midi_clock_follow expects an object")?;
        if args.keys().any(|k| !matches!(k.as_str(), "input" | "enabled" | "loss_policy" | "input_offset_samples")) {
            return Err("Unknown external MIDI clock setting".into());
        }
        if args.is_empty() {
            return Ok(self.clock_follow_state());
        }
        let source = match args.get("input") {
            None => self.ui.clock_follow_input.clone(),
            Some(Value::Null) => None,
            Some(Value::String(s)) if !s.trim().is_empty() && s.len() <= 1024 => Some(s.clone()),
            _ => return Err("input must be a MIDI port name of up to 1024 bytes or null".into()),
        };
        let enabled = match args.get("enabled") {
            Some(b) => b.as_bool().ok_or("enabled must be a boolean")?,
            None => self.clock_follow.enabled && source.is_some(),
        };
        let stop = match args.get("loss_policy") {
            None => self.ui.clock_follow_stop_on_loss,
            Some(Value::String(s)) if s == "coast" => false,
            Some(Value::String(s)) if s == "stop" => true,
            _ => return Err("loss_policy must be coast or stop".into()),
        };
        let limit = soundcraft_playback::max_sync_offset_samples(self.engine.session().sample_rate);
        let offset = args
            .get("input_offset_samples")
            .map(|v| v.as_i64().filter(|n| n.unsigned_abs() <= limit as u64).ok_or("MIDI input calibration must be within the sync timing horizon"))
            .transpose()?;
        if enabled && source.is_none() || offset.is_some() && source.is_none() {
            return Err("Select a MIDI clock input".into());
        }
        if enabled && self.mtc_online {
            return Err("Take MTC chase offline before following MIDI clock".into());
        }
        if enabled && source.as_ref().is_some_and(|name| self.clock_master_uses(name)) {
            return Err("The MIDI clock input is also enabled as a clock output".into());
        }
        if let (Some(offset), Some(name)) = (offset, &source) {
            if self.ui.clock_follow_input_offsets.len() >= 256 && !self.ui.clock_follow_input_offsets.contains_key(name) {
                return Err("Too many calibrated MIDI inputs".into());
            }
            self.ui.clock_follow_input_offsets.insert(name.clone(), offset);
        }
        if self.clock_follow.enabled != enabled || self.ui.clock_follow_input != source {
            // Finish a pending recording on the grid that captured it before releasing the route.
            if self.clock_follow.enabled && self.is_playing() {
                self.stop_play();
            }
            if let Some(m) = &self.midi_inputs {
                m.feed().set_clock_follow(None, true, 0);
            }
            self.engine.playback_tempo = None;
            self.clock_follow = Host { enabled, show: self.clock_follow.show, ..Host::default() };
            self.refresh_follow_playback();
        }
        self.ui.clock_follow_input = source;
        self.ui.clock_follow_stop_on_loss = stop;
        self.service_clock_follow();
        Ok(self.clock_follow_state())
    }

    fn clock_master_uses(&self, name: &str) -> bool {
        self.ui.midi_sync.enabled && self.ui.midi_sync.ports.get(name).is_some_and(|p| p.enabled)
    }

    fn reset_follow_grid(&mut self, bpm: Option<f64>) {
        let score = &self.engine.session().tempo;
        let mut grid = TempoMap::default();
        let _ = grid.set_tempo(0, bpm.unwrap_or_else(|| score.tempo_at_tick(0)));
        for meter in score.meters() {
            let _ = grid.set_meter(meter.tick, meter.numerator, meter.denominator);
        }
        self.engine.playback_tempo = Some(grid);
    }

    fn refresh_follow_playback(&self) {
        if let Some(player) = &self.player {
            player.update_session(self.engine.playback_session());
        }
    }

    pub(crate) fn service_clock_follow(&mut self) {
        if !self.clock_follow.enabled {
            return;
        }
        if self.engine.transport_requests.iter().any(|r| matches!(r, TransportRequest::ResetSession)) {
            if let Some(inputs) = &self.midi_inputs {
                inputs.feed().set_clock_follow(None, true, 0);
            }
            self.clock_follow = Host::default();
            self.engine.playback_tempo = None;
            return;
        }
        let Some(name) = &self.ui.clock_follow_input else { return };
        let Some(inputs) = &self.midi_inputs else { return };
        let feed = inputs.feed();
        // Keep the identity across disconnects. The timeout decides whether a live show coasts.
        let id = feed.ports().assign(name);
        if feed.ports().name(id) != Some(name.as_str()) {
            return;
        }
        // A first Continue without an SPP resumes at the insertion where follow was enabled.
        let sr = self.engine.session().sample_rate;
        let initial_pulse = self.engine.transport_tempo().samples_to_ticks(self.position(), sr) / TICKS_PER_PULSE;
        feed.set_clock_follow(Some(id), !self.ui.clock_follow_stop_on_loss, initial_pulse);
        let limit = soundcraft_playback::max_sync_offset_samples(sr);
        let offset = self.ui.clock_follow_input_offsets.get(name).copied().unwrap_or_default().clamp(-limit, limit);
        let mut events = std::mem::take(&mut self.clock_follow.actions);
        let s = feed.clock_follow_status(&mut events);
        self.clock_follow.snapshot = s;
        if self.engine.playback_tempo.is_none() {
            self.reset_follow_grid(s.bpm);
        }
        for event in events.drain(..) {
            match event {
                Action::Locate(pulse) => {
                    self.clock_follow.anchor = None;
                    if self.is_playing() {
                        self.stop_play();
                    }
                    if pulse == 0 {
                        self.reset_follow_grid(s.bpm);
                    }
                    let at = self.engine.transport_tempo().tick_to_samples(pulse.saturating_mul(TICKS_PER_PULSE), sr);
                    // The normal locate command updates the stopped insertion and both transport cursors.
                    let _ = self.run("transport.locate", json!({"at":at}));
                }
                Action::Play { pulse, at } => {
                    self.clock_follow.anchor = Some((pulse, at));
                    let tick = pulse.saturating_mul(TICKS_PER_PULSE);
                    if let (Some(bpm), Some(grid)) = (s.bpm, &mut self.engine.playback_tempo) {
                        let _ = grid.set_tempo(tick, bpm);
                    }
                    let origin = self.engine.transport_tempo().tick_to_samples(tick, sr);
                    let age = sr.samples(s.now_us.saturating_sub(at).max(0) as f64 / 1e6);
                    let lead = self.player.as_ref().map_or(0, |p| sr.samples(p.midi_sync_status().reported_lead_us as f64 / 1e6));
                    self.engine
                        .transport_requests
                        .push(TransportRequest::PlayFrom(origin.saturating_add(age).saturating_add(lead).saturating_add(offset).max(0)));
                }
                Action::Stop => {
                    self.clock_follow.anchor = None;
                    self.engine.transport_requests.push(TransportRequest::Stop);
                }
            }
        }
        self.clock_follow.actions = events;
        if let Some(grid) = self.engine.playback_tempo.as_mut() {
            self.clock_follow.grid_dirty |= update_grid(grid, &mut self.clock_follow.anchor, s, sr);
            if self.clock_follow.grid_dirty && self.player.as_ref().is_none_or(|p| p.update_follow_grid(grid)) {
                self.clock_follow.grid_dirty = false;
            }
        }
        self.clock_follow.transport_error_us = (s.state == State::Locked && self.is_playing()).then(|| {
            let heard = self.player.as_ref().map_or(self.engine.transport.position, |player| player.presented_position());
            let tick = s.pulse.saturating_mul(TICKS_PER_PULSE);
            let elapsed = sr.samples(s.now_us.saturating_sub(s.stamp_us).max(0) as f64 / 1e6);
            let master = self.engine.transport_tempo().tick_to_samples(tick, sr).saturating_add(elapsed).saturating_add(offset);
            (sr.seconds(heard.saturating_sub(master)) * 1e6).round() as i64
        });
    }
    pub(crate) fn clock_follow_window(&mut self, ctx: &egui::Context) {
        if !self.clock_follow.show {
            return;
        }
        let mut open = true;
        egui::Window::new(crate::localization::text("MIDI clock synchronization")).open(&mut open).show(ctx, |ui| self.clock_follow_settings(ui));
        self.clock_follow.show = open;
    }
    fn clock_follow_settings(&mut self, ui: &mut egui::Ui) {
        let mut name = self.ui.clock_follow_input.clone();
        craft_ui::ComboBox::from_label(crate::localization::text("MIDI clock input"))
            .selected_text(name.as_deref().unwrap_or(crate::localization::text("None")))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut name, None, crate::localization::text("None"));
                if let Some(m) = &self.midi_inputs {
                    for p in m.ports().iter().filter(|p| p.enabled) {
                        ui.selectable_value(&mut name, Some(p.name.clone()), &p.name);
                    }
                }
            });
        let mut enabled = self.clock_follow.enabled;
        let mut stop = self.ui.clock_follow_stop_on_loss;
        crate::widgets::checkbox(ui, &mut enabled, crate::localization::text("Follow MIDI clock"));
        crate::widgets::checkbox(ui, &mut stop, crate::localization::text("Stop if MIDI clock is lost"));
        if name != self.ui.clock_follow_input || enabled != self.clock_follow.enabled || stop != self.ui.clock_follow_stop_on_loss {
            let enabled = enabled && name.is_some();
            if let Err(error) = self.clock_follow_command(&json!({"input":name,"enabled":enabled,"loss_policy":if stop {"stop"}else{"coast"}})) {
                self.ui.status = crate::localization::error_text(&error);
            }
        }
        if let Some(name) = &self.ui.clock_follow_input {
            let mut offset = self.ui.clock_follow_input_offsets.get(name).copied().unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label(crate::localization::text("MIDI input offset (samples)"));
                let limit = soundcraft_playback::max_sync_offset_samples(self.engine.session().sample_rate);
                if ui.add(egui::DragValue::new(&mut offset).range(-limit..=limit)).changed() {
                    let _ = self.clock_follow_command(&json!({"input_offset_samples":offset}));
                }
            });
        }
    }
}

// Time between observed clock positions sets the segment behind the pulse. The fit sets the
// slope ahead of it. Keeping both anchors preserves elapsed bars across a live tempo change.
fn update_grid(grid: &mut TempoMap, anchor: &mut Option<(i64, i64)>, s: Snapshot, sr: SampleRate) -> bool {
    let Some(bpm) = s.bpm else { return false };
    let Some((last, stamp)) = *anchor else {
        if s.state == State::Waiting && (grid.tempo_at_tick(0) - bpm).abs() >= 0.05 {
            let _ = grid.set_tempo(0, bpm);
            return true;
        }
        return false;
    };
    let pulses = s.pulse.saturating_sub(last);
    if s.state != State::Locked || pulses < 6 {
        return false;
    }
    let tick = s.pulse.saturating_mul(TICKS_PER_PULSE);
    let first = last.saturating_mul(TICKS_PER_PULSE);
    let observed = s.stamp_us.saturating_sub(stamp);
    let predicted = grid.tick_to_samples(tick, sr).saturating_sub(grid.tick_to_samples(first, sr));
    let error = predicted.saturating_sub(sr.samples(observed as f64 / 1e6)).unsigned_abs();
    if (grid.tempo_at_tick(tick) - bpm).abs() < 0.15 && error < sr.samples(0.003) as u64 {
        return false;
    }
    if observed <= 0 {
        return false;
    }
    let measured = pulses as f64 * 2_500_000. / observed as f64;
    if grid.set_tempo(first, measured).is_err() {
        return false;
    }
    let _ = grid.set_tempo(tick, bpm);
    *anchor = Some((s.pulse, s.stamp_us));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_playback::midi::MidiInputs;

    #[test]
    fn follows_only_the_selected_port_retains_project_tempo_and_allows_record() {
        let mut app = SoundApp::new(Default::default(), None, crate::Services::default());
        let saved = app.engine.session().tempo.clone();
        app.run("transport.locate", json!({"at":48000})).unwrap();
        app.handle_transport(0.);
        app.midi_inputs = Some(MidiInputs::with_port_names(vec![], vec!["MPC".into(), "keys".into()]));
        app.run("setup.midi_clock_follow", json!({"input":"MPC","enabled":true,"input_offset_samples":120})).unwrap();
        let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
        let src = feed.ports().assign("MPC");
        let other = feed.ports().assign("keys");
        feed.receive(other, &[0xfa]);
        feed.receive(other, &[0xf8]);
        app.service_clock_follow();
        assert!(app.engine.transport_requests.is_empty());
        feed.receive(src, &[0xfb]);
        feed.receive(src, &[0xf8]);
        app.service_clock_follow();
        assert!(matches!(app.engine.transport_requests.as_slice(), [TransportRequest::PlayFrom(p)] if *p>=48_120));
        app.handle_transport(0.);
        feed.receive(src, &[0xfc]);
        app.service_clock_follow();
        app.handle_transport(0.);
        feed.receive(src, &[0xf2, 11, 0]);
        app.service_clock_follow();
        app.handle_transport(0.);
        let at = app.engine.transport_tempo().tick_to_samples(11 * 6 * TICKS_PER_PULSE, app.engine.session().sample_rate);
        assert_eq!(app.position(), at);
        feed.receive(src, &[0xfb]);
        feed.receive(src, &[0xf8]);
        app.service_clock_follow();
        assert!(matches!(app.engine.transport_requests.as_slice(),[TransportRequest::PlayFrom(p)] if *p>=at+120));
        app.handle_transport(0.);
        assert!(app.is_playing());
        app.engine.transport.recording = true;
        app.service_clock_follow();
        assert!(app.engine.transport.recording && app.clock_follow.enabled);
        app.engine.transport.recording = false;
        feed.receive(src, &[0xfc]);
        app.service_clock_follow();
        app.handle_transport(0.);
        assert!(!app.is_playing());
        assert_eq!(app.run("setup.midi_clock_follow", json!({})).unwrap()["state"], "Stopped");
        app.run("setup.midi_clock_follow", json!({"enabled":false})).unwrap();
        assert!(app.engine.playback_tempo.is_none());
        assert_eq!(app.engine.session().tempo, saved);
    }

    #[test]
    fn master_and_reader_conflicts_are_rejected_in_both_directions() {
        let mut app = SoundApp::new(Default::default(), None, crate::Services::default());
        app.run("setup.midi_beat_clock", json!({"enabled":true,"ports":{"MPC":{"enabled":true}}})).unwrap();
        assert!(app.run("setup.midi_clock_follow", json!({"input":"MPC","enabled":true})).is_err());
        app.run("setup.midi_clock_follow", json!({"input":"DIN","enabled":true})).unwrap();
        assert!(app.run("setup.mtc", json!({"input":"N/A","online":true})).is_err());
        assert!(app.run("setup.midi_beat_clock", json!({"enabled":true,"ports":{"DIN":{"enabled":true}}})).is_err());
        assert!(app.run("setup.midi_sync_status", json!({})).unwrap()["clock_follow"]["enabled"].as_bool().unwrap());
        app.run("setup.midi_clock_follow", json!({"enabled":false})).unwrap();
        app.run("setup.mtc", json!({"input":"MPC","online":true})).unwrap();
        assert!(app.run("setup.midi_clock_follow", json!({"input":"DIN","enabled":true})).is_err());
    }

    #[test]
    fn tempo_step_updates_future_beat_spacing_and_preserves_elapsed_bars() {
        let mut grid = TempoMap::default();
        let rate = SampleRate::new(48000).unwrap();
        let mut anchor = Some((96, 2_000_000));
        let s = Snapshot { pulse: 120, stamp_us: 2_666_667, bpm: Some(90.), state: State::Locked, ..Default::default() };
        assert!(update_grid(&mut grid, &mut anchor, s, rate));
        assert_eq!(grid.tick_to_samples(96 * TICKS_PER_PULSE, rate), 96_000);
        assert!(grid.tick_to_samples(120 * TICKS_PER_PULSE, rate).abs_diff(128_000) <= 1);
        assert!(grid.tick_to_samples(144 * TICKS_PER_PULSE, rate).abs_diff(160_000) <= 1);
    }
}
