//! Host-side transport and transient beat grid for an external 24 PPQN source.
use crate::{SoundApp, TransportRequest, Value, json};
use soundcraft_playback::clock_follow::{Action, Snapshot, State};
use soundcraft_time::{SampleRate, TICKS_PER_QUARTER, TempoMap};

const TICKS_PER_PULSE: i64 = TICKS_PER_QUARTER / 24;

#[derive(Default)]
pub(crate) struct Host {
    pub(crate) enabled: bool,
    pub(super) snapshot: Snapshot,
    transport_error_us: Option<i64>,
    actions: Vec<Action>,
    grid_dirty: bool,
    /// While stopped, new tempo estimates begin here so earlier score time stays fixed.
    waiting_tick: i64,
    pending_cue: Option<i64>,
    /// External start waiting for the first real output timestamp, superseded by a new transport command.
    waiting_start: Option<(i64, i64)>,
    // Fixed pulse and device timestamp at Start/Continue; later edits preserve this origin.
    anchor: Option<(i64, i64)>,
}

impl crate::UiState {
    pub(crate) fn clock_follow_input_offset(&self) -> i64 {
        self.clock_follow_input.as_ref().and_then(|name| self.midi_sync.ports.get(name)).map_or(0, |p| p.in_offset_samples)
    }
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
            "input_offset_samples":self.ui.clock_follow_input_offset(),
            "state":state,"estimated_bpm":s.bpm,"jitter_us":s.jitter_us,"arrival_residual_us":s.phase_error_us,"phase_error_us":self.clock_follow.transport_error_us,"dropouts":s.dropouts})
    }

    pub(crate) fn clock_follow_command(&mut self, p: &Value) -> Result<Value, String> {
        self.migrate_midi_sync();
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
        let previous = source.as_ref().and_then(|name| self.ui.midi_sync.ports.get(name)).map_or(0, |p| p.in_offset_samples);
        let offset = args
            .get("input_offset_samples")
            .map(|v| {
                v.as_i64()
                    .filter(|n| *n == previous || n.unsigned_abs() <= limit as u64)
                    .ok_or("MIDI input calibration must be within the sync timing horizon")
            })
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
        if self.ui.clock_follow_input != source {
            self.ui.midi_sync.retain_calibrated_ports([source.as_deref(), self.ui.mtc_input_port.as_deref(), self.ui.mtc_output_port.as_deref()]);
        }
        if offset.is_some_and(|value| value != previous)
            && let Some(p) = &self.player
        {
            p.cancel_clock_resume();
        }
        if let (Some(offset), Some(name)) = (offset, &source) {
            if let Some(port) = self.ui.midi_sync.ports.get_mut(name) {
                port.in_offset_samples = offset;
            } else if offset != 0 {
                if self.ui.midi_sync.ports.len() >= 256 {
                    return Err("Too many calibrated MIDI inputs".into());
                }
                self.ui.midi_sync.ports.entry(name.clone()).or_default().in_offset_samples = offset;
            }
        }
        self.transition_clock_source(enabled, source.as_deref());
        self.ui.clock_follow_input = source;
        self.ui.clock_follow_stop_on_loss = stop;
        self.service_clock_follow();
        Ok(self.clock_follow_state())
    }

    pub(crate) fn transition_clock_source(&mut self, enabled: bool, source: Option<&str>) {
        if self.clock_follow.enabled != enabled || self.ui.clock_follow_input.as_deref() != source {
            // Disarm the old source before checking for a start submitted between host frames.
            if let Some(m) = &self.midi_inputs {
                m.feed().set_clock_follow(None, true, 0, false);
            }
            if self.clock_follow.enabled && (self.is_playing() || self.player.as_ref().is_some_and(|p| p.is_playing())) {
                self.stop_play();
            }
            self.engine.clear_follow_grid();
            self.clock_follow = Host { enabled, ..Host::default() };
            self.refresh_playback_session();
        }
    }

    fn clock_master_uses(&self, name: &str) -> bool {
        self.ui.midi_sync.enabled && self.ui.midi_sync.ports.get(name).is_some_and(|p| p.enabled)
    }

    fn reset_follow_grid(&mut self, pulse: i64, bpm: Option<f64>) {
        let tick = pulse.saturating_mul(TICKS_PER_PULSE);
        self.clock_follow.waiting_tick = tick;
        let score = &self.engine.session().tempo;
        let mut grid = TempoMap::default();
        for event in score.tempos().iter().take_while(|e| e.tick < tick) {
            let _ = grid.set_tempo(event.tick, event.bpm);
        }
        let _ = grid.set_tempo(tick, bpm.unwrap_or_else(|| score.tempo_at_tick(tick)));
        for meter in score.meters() {
            let _ = grid.set_meter(meter.tick, meter.numerator, meter.denominator);
        }
        self.engine.playback_tempo = Some(grid);
        self.clock_follow.grid_dirty = true;
    }

    pub(super) fn refresh_playback_session(&self) {
        if let Some(player) = &self.player {
            if let Some(grid) = &self.engine.playback_tempo {
                player.prepare_projected_clock_session(self.engine.session_arc(), grid, |doc| self.engine.prepare_clock_clips(doc));
            } else {
                player.update_session(self.engine.session_arc());
            }
        }
    }

    pub(super) fn prepare_located_clock_resume(&mut self) {
        let Some(pulse) = self.clock_follow.pending_cue.take() else { return };
        let (Some(player), Some(inputs), Some(name), Some(grid)) =
            (&self.player, &self.midi_inputs, &self.ui.clock_follow_input, &self.engine.playback_tempo)
        else {
            return;
        };
        if self.is_playing() || self.engine.transport.recording {
            return;
        }
        let sr = self.engine.session().sample_rate;
        let origin = grid.tick_to_samples(pulse.saturating_mul(TICKS_PER_PULSE), sr);
        if self.position() != origin {
            return;
        }
        let limit = soundcraft_playback::max_sync_offset_samples(sr);
        let offset = self.ui.clock_follow_input_offset().clamp(-limit, limit);
        // A previous grid may still be returning from the callback after rapid locates.
        // Prepare the exact settled grid here, before arming the next start.
        player.prepare_projected_clock_session(self.engine.session_arc(), grid, |doc| self.engine.prepare_clock_clips(doc));
        self.clock_follow.grid_dirty = false;
        let feed = inputs.feed();
        let location = soundcraft_playback::clock_follow::LocatedCue { pulse, origin, offset, rate: sr, revision: self.engine.prepared_revision() };
        player.arm_clock_resume(feed, feed.ports().assign(name), location);
    }

    pub(super) fn clock_start_pending(&self) -> bool {
        self.clock_follow.enabled
            && (self.clock_follow.waiting_start.is_some() || self.midi_inputs.as_ref().is_some_and(|m| m.feed().clock_start_pending()))
    }

    pub(crate) fn service_clock_follow(&mut self) {
        self.service_clock_follow_with_timing(|app| app.player.as_ref().map_or(Some(0), |p| p.clock_follow_output_lead_us()));
    }

    fn service_clock_follow_with_timing(&mut self, lead_us: impl Fn(&Self) -> Option<u64>) {
        if !self.clock_follow.enabled {
            return;
        }
        if self.engine.transport_requests.iter().any(|r| matches!(r, TransportRequest::ResetSession)) {
            if let Some(inputs) = &self.midi_inputs {
                inputs.feed().set_clock_follow(None, true, 0, false);
            }
            self.clock_follow = Host::default();
            self.engine.clear_follow_grid();
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
        let lead = lead_us(self);
        let waiting_for_output =
            lead.is_none() && !self.engine.transport.recording && !self.is_playing() && !self.player.as_ref().is_some_and(|p| p.is_playing());
        feed.set_clock_follow(Some(id), !self.ui.clock_follow_stop_on_loss, initial_pulse, waiting_for_output);
        let limit = soundcraft_playback::max_sync_offset_samples(sr);
        let offset = self.ui.clock_follow_input_offset().clamp(-limit, limit);
        let mut events = std::mem::take(&mut self.clock_follow.actions);
        let s = feed.clock_follow_status(&mut events);
        let waiting = self.clock_follow.waiting_start.take();
        if events.is_empty()
            && let Some((pulse, at)) = waiting
            && !self.is_playing()
            && !self.engine.transport.recording
            && self.engine.transport_requests.iter().all(|r| matches!(r, TransportRequest::AllNotesOff))
        {
            events.push(Action::Play { pulse, at });
        }
        self.clock_follow.snapshot = s;
        if self.engine.playback_tempo.is_none() {
            self.reset_follow_grid(initial_pulse, s.bpm);
        }
        if let Some(grid) = self.player.as_ref().and_then(|p| p.take_rejected_follow_grid()) {
            self.engine.playback_tempo = Some(grid);
            self.clock_follow.grid_dirty = false;
        }
        for event in events.drain(..) {
            self.clock_follow.pending_cue = None;
            self.clock_follow.waiting_start = None;
            match event {
                Action::Locate(pulse) => {
                    self.clock_follow.pending_cue = Some(pulse);
                    self.clock_follow.anchor = None;
                    self.engine
                        .transport_requests
                        .retain(|r| !matches!(r, TransportRequest::ClockPlaybackStarted(_) | TransportRequest::PlayFromClock { .. }));
                    if self.is_playing() || self.player.as_ref().is_some_and(|p| p.is_playing()) {
                        self.stop_play();
                    }
                    self.reset_follow_grid(pulse, s.bpm);
                    let at = self.engine.transport_tempo().tick_to_samples(pulse.saturating_mul(TICKS_PER_PULSE), sr);
                    // The normal locate command updates the stopped insertion and both transport cursors.
                    let _ = self.run("transport.locate", json!({"at":at}));
                }
                Action::Play { pulse, at } | Action::Started { pulse, at, .. } => {
                    self.clock_follow.anchor = Some((pulse, at));
                    let tick = pulse.saturating_mul(TICKS_PER_PULSE);
                    if !self.is_playing()
                        && let (Some(bpm), Some(grid)) = (s.bpm, &mut self.engine.playback_tempo)
                        && grid.tempo_at_tick(tick) != bpm
                    {
                        self.clock_follow.grid_dirty |= grid.set_tempo(tick, bpm).is_ok();
                    }
                    if let Action::Started { render_from, .. } = event
                        && self.player.as_ref().is_some_and(|p| p.is_playing())
                    {
                        self.engine.transport_requests.push(TransportRequest::ClockPlaybackStarted(render_from));
                        continue;
                    }
                    let origin = self.engine.transport_tempo().tick_to_samples(tick, sr);
                    let age = sr.samples(s.now_us.saturating_sub(at).max(0) as f64 / 1e6);
                    let Some(lead) = lead else {
                        self.clock_follow.waiting_start = Some((pulse, at));
                        self.clock_follow.anchor = None;
                        continue;
                    };
                    let lead = sr.samples(lead as f64 / 1e6);
                    let render_from = origin.saturating_add(age).saturating_add(lead).saturating_sub(offset).max(0);
                    self.engine.transport_requests.push(TransportRequest::PlayFromClock { origin, render_from });
                }
                Action::Stop => {
                    self.clock_follow.anchor = None;
                    self.engine.transport_requests.retain(|r| !matches!(r, TransportRequest::PlayFromClock { .. }));
                    self.engine.transport_requests.push(TransportRequest::Stop);
                }
            }
        }
        self.clock_follow.actions = events;
        if let Some(grid) = self.engine.playback_tempo.as_mut() {
            self.clock_follow.grid_dirty |= update_grid(
                grid,
                self.clock_follow.anchor,
                self.clock_follow.waiting_tick,
                s,
                sr,
                self.player.as_ref().filter(|p| p.is_playing()).map_or(0, |p| {
                    // Leave at least one external pulse and one device buffer for delivery.
                    let pulse = 2.5 / s.bpm.unwrap_or(120.);
                    let buffer = f64::from(p.device_buffer.unwrap_or(512)) / f64::from(p.device_rate.max(1));
                    p.position().saturating_add(sr.samples(pulse.max(buffer)))
                }),
            );
            if self.clock_follow.grid_dirty && self.player.as_ref().is_none_or(|p| p.update_follow_grid(grid)) {
                self.clock_follow.grid_dirty = false;
            }
        }
        self.clock_follow.transport_error_us = (s.state == State::Locked && self.is_playing()).then(|| {
            let heard = self.player.as_ref().map_or(self.engine.transport.position, |player| player.presented_position());
            let tick = s.pulse.saturating_mul(TICKS_PER_PULSE);
            let elapsed = sr.samples(s.now_us.saturating_sub(s.stamp_us).max(0) as f64 / 1e6);
            let master = self.engine.transport_tempo().tick_to_samples(tick, sr).saturating_add(elapsed).saturating_sub(offset);
            (sr.seconds(heard.saturating_sub(master)) * 1e6).round() as i64
        });
    }
    pub(crate) fn clock_follow_settings(&mut self, ui: &mut egui::Ui) {
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
            let mut offset = self.ui.midi_sync.ports.get(name).map_or(0, |p| p.in_offset_samples);
            let limit = soundcraft_playback::max_sync_offset_samples(self.engine.session().sample_rate);
            ui.horizontal(|ui| {
                let caption = ui.label(crate::localization::text("Input offset"));
                if ui.add(egui::DragValue::new(&mut offset).range(-limit..=limit).clamp_existing_to_range(false)).labelled_by(caption.id).changed()
                    && let Err(error) = self.clock_follow_command(&json!({"input_offset_samples":offset}))
                {
                    self.ui.status = crate::localization::error_text(&error);
                }
            });
            if offset.unsigned_abs() > limit as u64 {
                ui.label(format!("{} {}", crate::localization::text("Effective input offset (samples)"), offset.clamp(-limit, limit)));
            }
            ui.label(crate::localization::text("Negative values make SoundCraft respond earlier."));
        }
    }
}

// Only append tempo changes at or after the latest pulse. Elapsed playback and recorded
// note positions remain fixed; future beat spacing follows the incoming clock and phase.
fn update_grid(grid: &mut TempoMap, anchor: Option<(i64, i64)>, waiting_tick: i64, s: Snapshot, sr: SampleRate, render_until: i64) -> bool {
    let Some(bpm) = s.bpm else { return false };
    let Some((origin, stamp)) = anchor else {
        if s.state == State::Waiting && (grid.tempo_at_tick(waiting_tick) - bpm).abs() >= 0.05 {
            let _ = grid.set_tempo(waiting_tick, bpm);
            return true;
        }
        return false;
    };
    let tick = s.pulse.saturating_mul(TICKS_PER_PULSE);
    if grid.tempos().last().is_some_and(|event| event.tick > tick) {
        return false;
    }
    let before = grid.tempos().partition_point(|event| event.tick <= tick);
    let last_edit_tick = grid.tempos()[..before].last().map_or(0, |event| event.tick);
    let since_edit = tick.saturating_sub(last_edit_tick);
    let slope = grid.tempo_at_tick(tick);
    // A confirmed tempo step should reach the next unrendered beat even when a minor
    // correction was just published. Small jitter corrections retain the usual spacing.
    let tempo_step = (slope - bpm).abs() >= slope * 0.08;
    if s.state != State::Locked || s.pulse <= origin || (!tempo_step && since_edit < 6 * TICKS_PER_PULSE) {
        return false;
    }
    let start = grid.tick_to_samples(origin.saturating_mul(TICKS_PER_PULSE), sr);
    let elapsed = sr.samples(s.stamp_us.saturating_sub(stamp) as f64 / 1e6);
    let phase = sr.seconds(start.saturating_add(elapsed).saturating_sub(grid.tick_to_samples(tick, sr)));
    if (slope - bpm).abs() < 0.15 && phase.abs() < 0.003 {
        return false;
    }
    // Spread small deviations over eight beats to avoid adding redundant events during gradual
    // drift. Recover larger phase errors in four beats, with at most 5% change in beat length.
    // An event at this pulse leaves every earlier tick at precisely the same sample.
    let recovery_beats = if phase.abs() <= 0.006 { 8. } else { 4. };
    let target = bpm / (1. + (phase * bpm / (60. * recovery_beats)).clamp(-0.05, 0.05));
    (slope - target).abs() >= 0.15
        && grid.set_tempo(tick.max(grid.samples_to_ticks(render_until, sr).saturating_add(1)), target.clamp(5., 1000.)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_playback::midi::MidiInputs;

    // These transport tests exercise real command queues on a silent device, independent
    // of the workstation's audio driver and its timing-report warmup.
    fn simulated_player(engine: &soundcraft_engine::Engine) -> soundcraft_playback::Player {
        soundcraft_playback::Player::reconnect_output(engine.session_arc(), &Default::default(), "SoundCraft absent Clock test device")
    }

    #[test]
    fn cold_clock_follow_continues_only_the_current_start_on_the_first_valid_report() {
        for interrupt in ["none", "wire stop", "wire locate", "local stop", "local locate", "local play", "local record"] {
            let mut app = SoundApp::new(Default::default(), None, crate::Services::default());
            app.midi_inputs = Some(MidiInputs::with_port_names(vec![], vec!["DIN".into()]));
            let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
            let port = feed.ports().assign("DIN");
            app.run("setup.midi_clock_follow", json!({"input":"DIN","enabled":true})).unwrap();
            app.service_clock_follow_with_timing(|_| None);
            feed.receive(port, &[0xf2, 64, 0]);
            app.service_clock_follow_with_timing(|_| None);
            app.handle_transport(0.);
            feed.receive(port, &[0xfb, 0xf8]);
            app.service_clock_follow_with_timing(|_| None);
            assert!(app.engine.transport_requests.is_empty(), "no offset may be inferred from a missing render timestamp");
            match interrupt {
                "wire stop" => feed.receive(port, &[0xfc]),
                "wire locate" => feed.receive(port, &[0xf2, 32, 0]),
                "local stop" => app.engine.transport_requests.push(TransportRequest::Stop),
                "local locate" => app.engine.transport_requests.push(TransportRequest::Locate(12_000)),
                "local play" => app.engine.transport_requests.push(TransportRequest::Play),
                "local record" => app.engine.transport_requests.push(TransportRequest::Record),
                _ => {}
            }
            let local = app.engine.transport_requests.clone();
            app.service_clock_follow_with_timing(|_| Some(75_000));
            if !local.is_empty() {
                assert_eq!(app.engine.transport_requests, local, "leave the local transport request unchanged");
            }
            if interrupt == "none" {
                let lead = app.engine.session().sample_rate.samples(0.075);
                assert!(
                    matches!(app.engine.transport_requests.as_slice(), [TransportRequest::PlayFromClock {origin: 384_000, render_from}] if *render_from>=384_000+lead)
                );
            } else {
                assert!(
                    app.engine.transport_requests.iter().all(|r| !matches!(r, TransportRequest::PlayFromClock { .. })),
                    "{interrupt} must cancel the old onset"
                );
            }
            app.engine.transport_requests.clear();
            app.service_clock_follow_with_timing(|_| Some(0));
            assert!(app.engine.transport_requests.is_empty(), "recovery must never replay an onset");
            if interrupt == "wire locate" {
                feed.receive(port, &[0xfb, 0xf8]);
                app.service_clock_follow_with_timing(|_| Some(0));
                assert!(matches!(app.engine.transport_requests.as_slice(), [TransportRequest::PlayFromClock { origin: 192_000, .. }]));
            }
        }
    }

    #[test]
    fn clock_start_blocks_output_recovery_from_continue_until_acknowledged_or_stopped() {
        let mut app = SoundApp::new(Default::default(), None, crate::Services::default());
        app.midi_inputs = Some(MidiInputs::with_port_names(vec![], vec!["DIN".into()]));
        let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
        let port = feed.ports().assign("DIN");
        app.run("setup.midi_clock_follow", json!({"input":"DIN","enabled":true})).unwrap();
        app.service_clock_follow_with_timing(|_| None);
        app.unavailable_output = Some(crate::UnavailableOutput { name: "offline".into(), stalled_at_report: None, idle_status: None });
        feed.receive(port, &[0xf2, 32, 0]);
        assert!(!app.clock_start_pending(), "a stopped location can wait on a healthy replacement stream");
        app.service_clock_follow_with_timing(|_| None);
        app.handle_transport(0.);
        feed.receive(port, &[0xfb]);
        assert!(app.clock_start_pending(), "the host has not seen Continue yet");
        assert!(!app.timing_reconnect_allowed());
        feed.receive(port, &[0xf8]);
        assert!(app.clock_start_pending(), "first pulse before the host frame");
        app.service_clock_follow_with_timing(|_| None);
        assert!(app.clock_start_pending(), "waiting for timing after the first pulse");
        assert!(!app.timing_reconnect_allowed());
        feed.receive(port, &[0xfc]);
        app.service_clock_follow_with_timing(|_| None);
        app.handle_transport(0.);
        assert!(!app.clock_start_pending(), "Stop releases the switch");
        assert!(app.timing_reconnect_allowed());
        feed.receive(port, &[0xfb]);
        app.run("setup.midi_clock_follow", json!({"enabled":false})).unwrap();
        assert!(!app.clock_start_pending());
    }

    #[test]
    fn cold_start_superseded_between_host_frames_never_queues_its_first_note() {
        for command in [&[0xfc][..], &[0xf2, 32, 0]] {
            let mut app = SoundApp::new(Default::default(), None, crate::Services::default());
            app.midi_inputs = Some(MidiInputs::with_port_names(vec![], vec!["DIN".into()]));
            let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
            let port = feed.ports().assign("DIN");
            app.run("setup.midi_clock_follow", json!({"input":"DIN","enabled":true})).unwrap();
            app.service_clock_follow_with_timing(|_| None);
            feed.receive(port, &[0xfb, 0xf8]);
            feed.receive(port, command);
            app.service_clock_follow_with_timing(|_| Some(0));
            assert!(app.engine.transport_requests.iter().all(|r| !matches!(r, TransportRequest::PlayFromClock { .. })));
            assert!(app.clock_follow.waiting_start.is_none());
        }
    }

    #[test]
    fn located_continue_reaches_the_player_before_the_next_host_frame() {
        let engine = soundcraft_engine::Engine::default();
        let player = simulated_player(&engine);
        let mut app = SoundApp::new(engine, Some(player), crate::Services::default());
        app.midi_inputs = Some(MidiInputs::with_port_names(vec![], vec!["DIN".into()]));
        let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
        let port = feed.ports().assign("DIN");
        app.run("setup.midi_clock_follow", json!({"input":"DIN","enabled":true})).unwrap();
        feed.receive(port, &[0xf2, 64, 0]);
        let ctx = egui::Context::default();
        ctx.run_ui(Default::default(), |_| app.logic(&ctx)).textures_delta.clear();
        assert!(!app.player.as_ref().unwrap().is_playing());
        feed.receive(port, &[0xfb]);
        assert!(!app.player.as_ref().unwrap().is_playing(), "Continue alone must wait for a clock");
        feed.receive(port, &[0xf8]);
        assert!(app.player.as_ref().unwrap().is_playing(), "the first Clock must dispatch without a host frame");
        app.service_clock_follow();
        assert!(
            app.engine.transport_requests.iter().all(|r| !matches!(r, TransportRequest::PlayFromClock { .. })),
            "the host must not replay the attack"
        );
        app.handle_transport(0.);
        assert!(app.engine.transport.playing);

        // Editing the prepared document falls back to the host's ordinary start path.
        feed.receive(port, &[0xfc]);
        ctx.run_ui(Default::default(), |_| app.logic(&ctx)).textures_delta.clear();
        feed.receive(port, &[0xf2, 65, 0]);
        ctx.run_ui(Default::default(), |_| app.logic(&ctx)).textures_delta.clear();
        // The input may already own a temporary strong reference to the old gate.
        // A real edit must revoke that gate before it publishes any changed document.
        let observed = app.engine.prepared_revision().upgrade().unwrap();
        app.run("track.new", json!({"kind":"midi"})).unwrap(); // edit between host frames; no session refresh
        assert!(!*observed.lock().unwrap());
        feed.receive(port, &[0xfb]);
        feed.receive(port, &[0xf8]);
        assert!(!app.player.as_ref().unwrap().is_playing());
        app.service_clock_follow();
        assert!(matches!(app.engine.transport_requests.as_slice(), [TransportRequest::PlayFromClock { .. }]));

        // Taking follow offline after dispatch, before the host frame, stops that roll too.
        app.handle_transport(0.);
        feed.receive(port, &[0xfc]);
        ctx.run_ui(Default::default(), |_| app.logic(&ctx)).textures_delta.clear();
        feed.receive(port, &[0xf2, 64, 0]);
        ctx.run_ui(Default::default(), |_| app.logic(&ctx)).textures_delta.clear();
        feed.receive(port, &[0xfb]);
        feed.receive(port, &[0xf8]);
        assert!(app.player.as_ref().unwrap().is_playing());
        app.run("setup.midi_clock_follow", json!({"enabled":false})).unwrap();
        assert!(!app.player.as_ref().unwrap().is_playing());
    }

    #[test]
    fn ui_calibration_patch_cancels_only_a_cue_with_a_changed_input_offset() {
        for (before, after) in [(0, -480), (-480, 480), (480, 0), (240, 240)] {
            let engine = soundcraft_engine::Engine::default();
            let player = simulated_player(&engine);
            let mut app = SoundApp::new(engine, Some(player), crate::Services::default());
            app.midi_inputs = Some(MidiInputs::with_port_names(vec![], vec!["DIN".into()]));
            let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
            let port = feed.ports().assign("DIN");
            app.run("setup.midi_clock_follow", json!({"input":"DIN","enabled":true,"input_offset_samples":before})).unwrap();
            feed.set_clock(1_000_000);
            feed.receive(port, &[0xf2, 64, 0]);
            let ctx = egui::Context::default();
            ctx.run_ui(Default::default(), |_| app.logic(&ctx)).textures_delta.clear();
            let mut ports = json!({"other":{"in_offset_samples":120}});
            if after != 0 {
                ports["DIN"] = json!({"in_offset_samples":after,"out_offset_samples":60});
            }
            app.run("ui.set", json!({"midi_sync":{"ports":ports}})).unwrap();
            feed.receive(port, &[0xfb, 0xf8]);
            let player = app.player.as_ref().unwrap();
            assert_eq!(player.is_playing(), before == after, "prepared dispatch after {before} -> {after}");
            if before != after {
                // Check the host's new sample position without variable driver-reported lead.
                drop(app.player.take());
                app.service_clock_follow();
                assert!(
                    matches!(app.engine.transport_requests.as_slice(), [TransportRequest::PlayFromClock {origin, render_from}]
                    if *origin == 384_000 && *render_from == *origin - after),
                    "requests: {:?}",
                    app.engine.transport_requests
                );
            }
        }
    }

    #[test]
    fn ui_patch_cannot_feed_beat_clock_back_into_the_source() {
        let mut app = SoundApp::new(Default::default(), None, crate::Services::default());
        app.run("setup.midi_clock_follow", json!({"input":"DIN","enabled":true})).unwrap();
        let patch = |enabled: bool, port: &str| json!({"midi_sync":{"enabled":enabled,"ports":{(port):{"enabled":true}}}});
        assert!(app.run("ui.set", patch(true, "DIN")).is_err());
        assert!(!app.ui.midi_sync.enabled);
        app.run("ui.set", patch(false, "DIN")).unwrap();
        app.run("ui.set", patch(true, "OUT")).unwrap();
        assert!(app.ui.midi_sync.ports["OUT"].enabled);
    }

    #[test]
    fn ui_patch_clearing_the_clock_source_disarms_a_prepared_resume() {
        let engine = soundcraft_engine::Engine::default();
        let player = simulated_player(&engine);
        let mut app = SoundApp::new(engine, Some(player), crate::Services::default());
        app.midi_inputs = Some(MidiInputs::with_port_names(vec![], vec!["DIN".into()]));
        let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
        let port = feed.ports().assign("DIN");
        app.run("setup.midi_clock_follow", json!({"input":"DIN","enabled":true})).unwrap();
        feed.receive(port, &[0xf2, 64, 0]);
        let ctx = egui::Context::default();
        ctx.run_ui(Default::default(), |_| app.logic(&ctx)).textures_delta.clear();
        assert!(app.run("ui.set", json!({"clock_follow_input":" "})).is_err());
        assert_eq!(app.ui.clock_follow_input.as_deref(), Some("DIN"));
        app.run("ui.set", json!({"clock_follow_input":null})).unwrap();
        assert!(!app.clock_follow.enabled);
        assert!(app.engine.playback_tempo.is_none());
        feed.receive(port, &[0xfb]);
        feed.receive(port, &[0xf8]);
        assert!(!app.player.as_ref().unwrap().is_playing());
        app.run("ui.set", json!({"clock_follow_input":null})).unwrap();
    }

    #[test]
    fn replacement_player_handles_a_start_already_delivered_to_the_old_player() {
        let engine = soundcraft_engine::Engine::default();
        let player = simulated_player(&engine);
        let mut app = SoundApp::new(engine, Some(player), crate::Services::default());
        app.midi_inputs = Some(MidiInputs::with_port_names(vec![], vec!["DIN".into()]));
        let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
        let port = feed.ports().assign("DIN");
        app.run("setup.midi_clock_follow", json!({"input":"DIN","enabled":true})).unwrap();
        let ctx = egui::Context::default();
        feed.receive(port, &[0xf2, 64, 0]);
        ctx.run_ui(Default::default(), |_| app.logic(&ctx)).textures_delta.clear();
        feed.receive(port, &[0xfb, 0xf8]);
        assert!(app.player.as_ref().unwrap().is_playing());
        app.unavailable_output = Some(crate::UnavailableOutput { name: "offline".into(), stalled_at_report: None, idle_status: None });
        app.reconnect_audio();
        assert!(app.player.as_ref().unwrap().is_playing());
        app.player = Some(simulated_player(&app.engine));
        assert!(!app.player.as_ref().unwrap().is_playing());
        ctx.run_ui(Default::default(), |_| app.logic(&ctx)).textures_delta.clear();
        assert!(app.player.as_ref().unwrap().is_playing());
        assert!(app.is_playing());
        assert!(app.position() >= 384_000);
    }

    #[test]
    fn new_start_before_host_frame_cancels_the_acknowledgement_of_the_previous_roll() {
        let engine = soundcraft_engine::Engine::default();
        let player = simulated_player(&engine);
        let mut app = SoundApp::new(engine, Some(player), crate::Services::default());
        app.midi_inputs = Some(MidiInputs::with_port_names(vec![], vec!["DIN".into()]));
        let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
        let port = feed.ports().assign("DIN");
        app.run("setup.midi_clock_follow", json!({"input":"DIN","enabled":true})).unwrap();
        feed.receive(port, &[0xf2, 64, 0]);
        let ctx = egui::Context::default();
        ctx.run_ui(Default::default(), |_| app.logic(&ctx)).textures_delta.clear();
        feed.receive(port, &[0xfb, 0xf8]);
        assert!(app.player.as_ref().unwrap().is_playing());
        feed.receive(port, &[0xfa]);
        ctx.run_ui(Default::default(), |_| app.logic(&ctx)).textures_delta.clear();
        assert!(!app.player.as_ref().unwrap().is_playing());
        assert!(!app.is_playing());
        assert_eq!(app.position(), 0);
        feed.receive(port, &[0xf8]);
        ctx.run_ui(Default::default(), |_| app.logic(&ctx)).textures_delta.clear();
        assert!(app.is_playing());
    }

    #[test]
    fn follows_only_the_selected_port_and_retains_project_tempo() {
        let mut app = SoundApp::new(Default::default(), None, crate::Services::default());
        app.engine.session_mut().tempo.set_tempo(TICKS_PER_QUARTER, 60.).unwrap();
        let saved = app.engine.session().tempo.clone();
        app.run("transport.locate", json!({"at":72000})).unwrap();
        app.handle_transport(0.);
        app.midi_inputs = Some(MidiInputs::with_port_names(vec![], vec!["MPC".into(), "keys".into()]));
        app.run("setup.midi_clock_follow", json!({"input":"MPC","enabled":true,"input_offset_samples":-120})).unwrap();
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
        assert!(
            matches!(app.engine.transport_requests.as_slice(), [TransportRequest::PlayFromClock { origin: 72_000, render_from }] if *render_from>=72_120)
        );
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
        assert!(
            matches!(app.engine.transport_requests.as_slice(),[TransportRequest::PlayFromClock { origin, render_from }] if *origin==at && *render_from>=at+120)
        );
        app.handle_transport(0.);
        assert!(app.is_playing());
        feed.receive(src, &[0xfc]);
        app.service_clock_follow();
        app.handle_transport(0.);
        assert!(!app.is_playing());
        assert_eq!(app.run("setup.midi_clock_follow", json!({})).unwrap()["state"], "Stopped");
        app.run("setup.midi_clock_follow", json!({"enabled":false})).unwrap();
        assert!(app.engine.playback_tempo.is_none());
        assert_eq!(app.engine.session().tempo, saved);
    }

    #[cfg(not(target_arch = "wasm32"))]
    mod recording;

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
    fn continue_after_a_backward_spp_ignores_the_previous_pass_grid() {
        let mut app = SoundApp::new(Default::default(), None, crate::Services::default());
        app.midi_inputs = Some(MidiInputs::with_port_names(vec![], vec!["DIN".into()]));
        app.run("setup.midi_clock_follow", json!({"input":"DIN","enabled":true})).unwrap();
        let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
        let src = feed.ports().assign("DIN");
        // A previous pass continued beyond pulse 1000 and corrected its tempo there.
        app.engine.playback_tempo.as_mut().unwrap().set_tempo(1000 * TICKS_PER_PULSE, 90.).unwrap();
        feed.receive(src, &[0xf2, 40, 0]); // position 40 = pulse 240
        app.service_clock_follow();
        app.handle_transport(0.);
        feed.receive(src, &[0xfb]);
        feed.receive(src, &[0xf8]);
        app.service_clock_follow();

        let sr = app.engine.session().sample_rate;
        let grid = app.engine.playback_tempo.as_mut().unwrap();
        let origin = grid.tick_to_samples(240 * TICKS_PER_PULSE, sr);
        grid.set_tempo(240 * TICKS_PER_PULSE, 130.).unwrap();
        let mut peak = 0;
        for pulse in 241..=1050 {
            let elapsed = (pulse - 240) as f64 * 2.5 / 130.;
            let snap = Snapshot { state: State::Locked, bpm: Some(130.), pulse, stamp_us: (elapsed * 1e6).round() as i64, ..Default::default() };
            update_grid(grid, Some((240, 0)), 0, snap, sr, 0);
            peak = peak.max(grid.tick_to_samples(pulse * TICKS_PER_PULSE, sr).abs_diff(origin + sr.samples(elapsed)));
        }
        eprintln!("backward SPP/Continue: peak {peak} samples");
        assert!(peak < sr.samples(0.003) as u64, "previous-pass tempo produced {peak} samples of phase error");
    }

    #[test]
    fn input_and_output_calibration_of_a_same_named_port_are_independent() {
        let mut app = SoundApp::new(Default::default(), None, crate::Services::default());
        app.run("setup.midi_clock_follow", json!({"input":"DIN","input_offset_samples":-480})).unwrap();
        app.run("setup.midi_beat_clock", json!({"ports":{"DIN":{"enabled":false,"offset_samples":-200}}})).unwrap();
        let settings = app.run("setup.midi_sync_status", json!({})).unwrap()["settings"].clone();
        assert_eq!(settings["ports"]["DIN"]["in_offset_samples"], -480);
        assert_eq!(settings["ports"]["DIN"]["out_offset_samples"], -200);
        app.run("setup.midi_beat_clock", json!({"ports":{"DIN":null}})).unwrap();
        let settings = app.run("setup.midi_sync_status", json!({})).unwrap()["settings"].clone();
        assert_eq!(settings["ports"]["DIN"]["out_offset_samples"], 0);
        assert_eq!(app.run("setup.midi_clock_follow", json!({})).unwrap()["input_offset_samples"], -480);
        // Recalibrating then releasing the MTC Generator must preserve the same input.
        app.run("setup.mtc_output", json!({"port":"DIN","offset_samples":120})).unwrap();
        app.run("setup.mtc_output", json!({"offset_samples":0})).unwrap();
        app.run("setup.mtc_output", json!({"port":null})).unwrap();
        assert_eq!(app.run("setup.midi_clock_follow", json!({})).unwrap()["input_offset_samples"], -480);
        // An input-only calibration also survives when a different input is selected.
        app.run("setup.midi_clock_follow", json!({"input":"USB"})).unwrap();
        app.run("setup.midi_beat_clock", json!({"ports":{}})).unwrap();
        assert_eq!(app.run("setup.midi_clock_follow", json!({"input":"DIN"})).unwrap()["input_offset_samples"], -480);
    }

    #[test]
    fn legacy_clock_follow_offsets_migrate_with_the_sign_inverted() {
        let mut app = SoundApp::new(Default::default(), None, crate::Services::default());
        app.ui = serde_json::from_value(json!({"clock_follow_input":"DIN","clock_follow_input_offsets":{"DIN":480},
            "midi_sync":{"ports":{"DIN":{"offset_samples":-200}}}}))
        .unwrap();
        app.migrate_midi_sync();
        assert_eq!(app.run("setup.midi_clock_follow", json!({})).unwrap()["input_offset_samples"], -480);
        let saved = serde_json::to_value(&app.ui).unwrap();
        assert!(saved.get("clock_follow_input_offsets").is_none());
        assert_eq!(saved["midi_sync"]["ports"]["DIN"]["out_offset_samples"], -200);
        assert_eq!(saved["midi_sync"]["ports"]["DIN"]["in_offset_samples"], -480);
        app.ui = serde_json::from_value(saved).unwrap();
        assert_eq!(app.run("setup.midi_clock_follow", json!({})).unwrap()["input_offset_samples"], -480);
    }

    #[test]
    fn negative_input_offset_moves_the_follow_transport_earlier() {
        for offset in [0, -480] {
            let mut app = SoundApp::new(Default::default(), None, crate::Services::default());
            app.midi_inputs = Some(MidiInputs::with_port_names(vec![], vec!["DIN".into()]));
            app.run("setup.midi_clock_follow", json!({"input":"DIN","enabled":true,"input_offset_samples":offset})).unwrap();
            let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
            let src = feed.ports().assign("DIN");
            feed.receive(src, &[0xf2, 4, 0]);
            app.service_clock_follow();
            app.handle_transport(0.);
            feed.receive(src, &[0xfb]);
            feed.receive(src, &[0xf8]);
            app.service_clock_follow();
            let (pulse, stamp) = app.clock_follow.anchor.unwrap();
            let sr = app.engine.session().sample_rate;
            let age = sr.samples((app.clock_follow.snapshot.now_us - stamp).max(0) as f64 / 1e6);
            let anchor = app.engine.transport_tempo().tick_to_samples(pulse * TICKS_PER_PULSE, sr);
            assert!(
                matches!(app.engine.transport_requests.as_slice(), [TransportRequest::PlayFromClock { origin, render_from }] if *origin == anchor && *render_from == anchor + age - offset),
                "offset {offset}, reference {}, requests {:?}",
                anchor + age,
                app.engine.transport_requests
            );
        }
    }

    #[test]
    fn saved_input_calibration_survives_a_lower_rate() {
        let mut app = SoundApp::new(Default::default(), None, crate::Services::default());
        app.ui = serde_json::from_value(json!({"clock_follow_input":"DIN","clock_follow_input_offsets":{"DIN":-24000}})).unwrap();
        app.migrate_midi_sync();
        assert!(app.run("setup.midi_clock_follow", json!({"input_offset_samples":24000})).is_ok());
        assert!(app.run("setup.midi_clock_follow", json!({"input_offset_samples":24001})).is_err());
        assert_eq!(app.run("setup.midi_clock_follow", json!({})).unwrap()["input_offset_samples"], 24000);
    }

    #[test]
    fn sync_menu_and_peripherals_share_the_external_clock_controls() {
        use egui_kittest::{Harness, kittest::Queryable};
        let mut app = SoundApp::new(Default::default(), None, crate::Services::default());
        app.midi_inputs = Some(MidiInputs::with_port_names(vec![], vec!["MPC".into()]));
        app.run("setup.midi_clock_follow", json!({"input":"MPC"})).unwrap();
        let path = "Setup > MIDI > Sync";
        let cmd = crate::menus::command_for_path(path, &[]).unwrap();
        crate::menus::invoke_menu(&mut app, &cmd, path);
        assert_eq!(app.dialogs.open_name(), Some("synchronization"));
        let mut h = Harness::builder()
            .with_size(egui::vec2(1000., 800.))
            .with_context_setup(|ctx| crate::fonts::install(ctx, false))
            .build_ui_state(|ui, app: &mut SoundApp| crate::dialogs::show(app, ui.ctx()), app);
        h.run();
        h.get_by_label("External MIDI clock");
        h.get_by_label("MIDI clock input");
        h.get_by_label("Follow MIDI clock").click();
        h.run();
        h.get_by_label("Stop if MIDI clock is lost").click();
        h.run();
        let result = h.state_mut().run("setup.midi_clock_follow", json!({})).unwrap();
        assert_eq!((result["input"].as_str(), result["enabled"].as_bool(), result["loss_policy"].as_str()), (Some("MPC"), Some(true), Some("stop")));
        let app = h.state_mut();
        assert!(app.dialogs.open_for_command(&app.engine, "setup.peripherals"));
        assert_eq!(app.dialogs.open_name(), Some("synchronization"));
    }

    #[test]
    fn live_correction_keeps_notes_inside_already_rendered_audio() {
        let mut grid = TempoMap::default();
        let rate = SampleRate::new(48_000).unwrap();
        let at = 121 * TICKS_PER_PULSE;
        let before = grid.tick_to_samples(at, rate);
        let s = Snapshot { pulse: 120, stamp_us: 2_666_667, bpm: Some(90.), state: State::Locked, ..Default::default() };
        assert!(update_grid(&mut grid, Some((96, 2_000_000)), 0, s, rate, 121_200));
        assert_eq!(grid.tick_to_samples(at, rate), before);
        assert!(grid.tempo_at_tick(at + 2 * TICKS_PER_PULSE) < 120.);
    }

    #[test]
    fn chorus_tempo_step_keeps_the_next_backbeat_within_one_clock() {
        let sr = SampleRate::new(48_000).unwrap();
        for (first, second) in [(120., 100.), (100., 120.)] {
            let inputs = MidiInputs::with_port_names(vec![], vec!["drums".into()]);
            let feed = inputs.feed();
            let id = feed.ports().assign("drums");
            feed.set_clock_follow(Some(id), true, 0, false);
            let mut grid = TempoMap::new(first, 4, 4).unwrap();
            let mut actions = Vec::new();
            let mut us = 0.0_f64;
            feed.set_clock(0);
            feed.receive(id, &[0xfa]);
            let chorus = 16 * 24;
            for pulse in 0..chorus + 24 {
                if pulse > 0 {
                    us += 2_500_000. / if pulse <= chorus { first } else { second };
                }
                feed.set_clock(us.round() as i64);
                feed.receive(id, &[0xf8]);
                let snap = feed.clock_follow_status(&mut actions);
                update_grid(&mut grid, Some((0, 0)), 0, snap, sr, sr.samples(us / 1e6 + 0.030));
            }
            let tick = (chorus + 24) * TICKS_PER_PULSE;
            let expected = sr.samples(16. * 60. / first + 60. / second);
            let actual = grid.tick_to_samples(tick, sr);
            assert!(
                actual.abs_diff(expected) <= sr.samples(2.5 / second) as u64,
                "{first} to {second} BPM: backbeat error {} samples",
                actual - expected
            );
            assert_eq!(grid.tick_to_samples(chorus * TICKS_PER_PULSE, sr), sr.samples(16. * 60. / first));
        }
    }

    #[test]
    fn tempo_step_updates_future_beat_spacing_and_preserves_elapsed_bars() {
        let mut grid = TempoMap::default();
        let rate = SampleRate::new(48000).unwrap();
        let anchor = Some((96, 2_000_000));
        let s = Snapshot { pulse: 120, stamp_us: 2_666_667, bpm: Some(90.), state: State::Locked, ..Default::default() };
        assert!(update_grid(&mut grid, anchor, 0, s, rate, 0));
        assert_eq!(grid.tick_to_samples(96 * TICKS_PER_PULSE, rate), 96_000);
        // When the source stepped between observations, preserve the samples already played.
        assert_eq!(grid.tick_to_samples(120 * TICKS_PER_PULSE, rate), 120_000);
        for pulse in 121..=504 {
            let s = Snapshot { pulse, stamp_us: 2_000_000 + (pulse - 96) * 2_000_000 / 72, ..s };
            update_grid(&mut grid, anchor, 0, s, rate, 0);
        }
        assert_eq!(grid.tick_to_samples(120 * TICKS_PER_PULSE, rate), 120_000);
        let master = rate.samples(2. + 408. * 2.5 / 90.);
        assert!(grid.tick_to_samples(504 * TICKS_PER_PULSE, rate).abs_diff(master) < rate.samples(0.010) as u64);
    }

    #[test]
    fn two_hours_of_gradual_clock_drift_preserve_earlier_beat_positions() {
        let mut grid = TempoMap::default();
        let sr = SampleRate::new(48_000).unwrap();
        let anchor = Some((0, 0));
        let mut stamp = 0.0_f64;
        let mut pulse = 0_i64;
        let mut passed_bar = (0, 0);
        let mut first_rewrite = None;
        let mut published_cost = Vec::new();
        let mut peak_phase = 0_u64;
        let started = std::time::Instant::now();
        while stamp < 7_200_000_000. {
            let ramp = (stamp / 3_600_000_000.).min(2.);
            let bpm = 120. + 30. * (1. - (ramp - 1.).abs());
            stamp += 2_500_000. / bpm;
            pulse += 1;
            if pulse % 6 != 0 {
                continue;
            }
            let snapshot = Snapshot { state: State::Locked, bpm: Some(bpm), pulse, stamp_us: stamp.round() as i64, ..Default::default() };
            let update_started = std::time::Instant::now();
            if update_grid(&mut grid, anchor, 0, snapshot, sr, sr.samples(stamp / 1e6 + 0.025)) {
                published_cost.push(update_started.elapsed());
                let actual = grid.tick_to_samples(passed_bar.0, sr);
                if actual != passed_bar.1 && first_rewrite.is_none() {
                    first_rewrite = Some((passed_bar, actual));
                }
            }
            let at = grid.tick_to_samples(pulse * TICKS_PER_PULSE, sr);
            peak_phase = peak_phase.max(at.abs_diff(sr.samples(stamp / 1e6)));
            if pulse % 96 == 0 {
                let tick = pulse * TICKS_PER_PULSE;
                passed_bar = (tick, grid.tick_to_samples(tick, sr));
            }
        }
        published_cost.sort_unstable();
        let updates = published_cost.len();
        eprintln!(
            "two-hour 120–150–120 BPM simulation: {pulse} pulses, {updates} updates, {} events, peak phase {peak_phase} samples, update p95 {:?}, elapsed {:?}",
            grid.tempos().len(),
            published_cost[updates * 95 / 100],
            started.elapsed()
        );
        assert!(peak_phase < sr.samples(0.010) as u64);
        assert!(updates < 800, "excessive tempo events during a gradual drift");
        assert_eq!(first_rewrite, None, "a previously passed bar moved");
        let end = grid.tick_to_samples(pulse * TICKS_PER_PULSE, sr);
        assert!(end.abs_diff(sr.samples(stamp / 1e6)) < sr.samples(0.010) as u64, "lost phase");
    }
    /// Profile the lookup made by the callback with an eight-hour live tempo map.
    /// Run explicitly with --ignored --nocapture; wall-clock timing varies by host.
    #[test]
    #[ignore = "long-running external-clock performance profile"]
    fn eight_hours_of_clock_flutter_keep_the_live_grid_in_phase() {
        let rate = SampleRate::new(48_000).unwrap();
        let mut grid = TempoMap::default();
        let mut stamp = 0.0_f64;
        let mut pulse = 0_i64;
        let mut peak_phase = 0;
        let mut lookup_cost = Vec::new();
        while stamp < 28_800_000_000. {
            let bpm = 120. + 0.2 * (stamp / 30_000_000. * std::f64::consts::TAU).sin();
            stamp += 2_500_000. / bpm;
            pulse += 1;
            if pulse % 6 == 0 {
                let snapshot = Snapshot { state: State::Locked, bpm: Some(bpm), pulse, stamp_us: stamp.round() as i64, ..Default::default() };
                update_grid(&mut grid, Some((0, 0)), 0, snapshot, rate, rate.samples(stamp / 1e6 + 0.025));
                let now = std::time::Instant::now();
                let sample = std::hint::black_box(grid.tick_to_samples(pulse * TICKS_PER_PULSE, rate));
                lookup_cost.push(now.elapsed());
                peak_phase = peak_phase.max(sample.abs_diff(rate.samples(stamp / 1e6)));
            }
        }
        lookup_cost.sort_unstable();
        let count = lookup_cost.len();
        eprintln!(
            "8h ±0.2 BPM, {pulse} pulses, {} events, {count} lookups, p99 {:?}, max {:?}, peak phase {peak_phase} samples",
            grid.tempos().len(),
            lookup_cost[count * 99 / 100],
            lookup_cost[count - 1]
        );
        assert!(peak_phase < rate.samples(0.010) as u64);
    }
}
