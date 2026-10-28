use super::audio::feed_meter;
use super::plugins::failed_plugin_status;
use super::{Gesture, MainWindow, MeterDisplay, Range, Samples, SoundApp, TrackId, TransportRequest, json};

/// Whether another automation-write pass is due: the transport moved at least
/// `step` since `last` in either direction. `last == i64::MIN` marks "never
/// written", so the first pass after (re)start is always due. Saturating
/// arithmetic keeps restarts and loop wraps overflow-free: the previous
/// `(pos - last).abs()` panicked in debug builds on the first playing frame,
/// when `last` is still `i64::MIN`.
pub(crate) fn automation_step_due(pos: Samples, last: Samples, step: Samples) -> bool {
    last == i64::MIN || pos.saturating_sub(last) >= step || last.saturating_sub(pos) >= step
}

impl SoundApp {
    pub fn is_playing(&self) -> bool {
        self.engine.transport.playing
    }

    /// Why the last Record press did nothing, for a few seconds and while it still holds.
    pub(crate) fn record_hint(&self) -> Option<&'static str> {
        let (at, why) = self.record_hint?;
        let fresh = self.last_frame.unwrap_or_default() - at < 4.0;
        (fresh && !self.engine.session().tracks.iter().any(|t| t.mixer.record_arm)).then_some(why)
    }

    /// Current transport position (playhead while playing, else the edit insertion).
    pub fn position(&self) -> Samples {
        if self.engine.transport.playing { self.engine.transport.position } else { self.engine.session().edit.selection.start }
    }

    fn start_play(&mut self, from: Samples, end: Option<Samples>, looped: Option<Range>) {
        if let Some(p) = &self.player {
            p.update_session(self.engine.session_arc());
            p.play(from, end, looped);
        } else {
            self.sim = Some((from, end, looped));
        }
        self.engine.transport.playing = true;
        self.engine.transport.position = from;
        self.last_write_at = i64::MIN;
        // A fresh start resumes the playhead follow: clear a manual hold and
        // re-sync the follow tracker so the current view is not mistaken for
        // an outside move on the first frame.
        self.edit_layout.follow_hold = false;
        self.edit_layout.last_scroll = self.engine.session().edit.zoom.scroll;
        self.edit_layout.last_follow_to = None;
    }

    pub(crate) fn stop_play(&mut self) {
        if let Some(p) = &self.player {
            // Half-speed is a separate command. Stop must not leave that rate for the next Play.
            p.set_speed(1.0);
            p.stop();
        }
        if self.engine.transport.recording {
            self.finish_recording();
        }
        self.sim = None;
        self.engine.transport.playing = false;
        self.engine.transport.recording = false;
        let stop_at = self.engine.transport.position;
        // Snapshot the session flags first: `session_mut` below needs a
        // mutable borrow, so no session borrow may span it.
        let (follow_insertion, after_playback) = {
            let s = self.engine.session();
            (s.edit.insertion_follows_playback, s.edit.scrolling == "after_playback")
        };
        if follow_insertion {
            let pos = self.engine.transport.position;
            self.engine.session_mut().edit.selection = Range::point(pos);
        }
        if after_playback {
            // After Playback never scrolls while playing, so bring the stopped
            // position into view now (only if it is off-screen); every other
            // mode keeps its view.
            let at = stop_at.max(0);
            let (scroll, spp) = {
                let z = &self.engine.session().edit.zoom;
                (z.scroll, z.samples_per_px.max(0.01))
            };
            let width = f64::from(self.edit_layout.timeline[2] - self.edit_layout.timeline[0]).max(0.0);
            let end = scroll.saturating_add((width * spp) as Samples);
            if at < scroll || at >= end {
                let _ = self.engine.execute("view.scroll", &json!({ "to": at }));
            }
        }
        self.edit_layout.follow_hold = false;
    }

    fn start_recording(&mut self) {
        let armed = self.engine.session().tracks.iter().any(|t| t.mixer.record_arm);
        if !armed {
            let why = if self.engine.session().tracks.is_empty() {
                "Nothing to record on yet: add a track with Track › New…, then click its R button."
            } else {
                "No track is record-enabled: click the R button on the track to record."
            };
            self.ui.status = why.into();
            self.record_hint = Some((self.last_frame.unwrap_or_default(), why));
            return;
        }
        self.ensure_input();
        let takes = match self.arm_recorder() {
            Ok(takes) => takes,
            Err(err) => {
                self.ui.status = format!("Cannot record: {err}");
                return;
            }
        };
        self.lost_shown = 0;
        if let Some(p) = &self.player {
            p.set_recording(true);
        }
        let already_playing = self.is_playing();
        self.engine.transport.recording = true;
        if already_playing {
            // Punch in on the fly at the current playhead.
            self.record_start = self.position();
            self.punch = None;
            self.plan_recording(takes, None);
            return;
        }
        let e = self.engine.session().edit.clone();
        let sel = e.selection;
        if e.loop_record && !sel.is_empty() {
            self.record_start = sel.start;
            self.punch = None;
            self.start_play(sel.start, None, Some(sel));
            self.plan_recording(takes, Some(sel.len()));
        } else {
            // Record into the selection when there is one (punch), with pre/post-roll if enabled;
            // the take is trimmed back to the punch range afterwards.
            let (pre, post) = if e.pre_post_roll { (e.pre_roll, e.post_roll) } else { (0, 0) };
            let from = (sel.start - pre).max(0);
            self.record_start = from;
            self.punch = (!sel.is_empty()).then_some(sel);
            let end = (!sel.is_empty()).then_some(sel.end + post);
            self.start_play(from, end, None);
            self.plan_recording(takes, None);
        }
    }

    /// Remember where the armed `takes` go (split into passes of `pass` when loop recording) and
    /// autosave that plan with the session.
    fn plan_recording(&mut self, takes: Vec<soundcraft_engine::io::Take>, pass: Option<Samples>) {
        let rate = self.recorder.as_ref().map_or(0, |r| r.sample_rate);
        self.take_plan = Some(soundcraft_engine::io::Recording { start: self.record_start, pass, punch: self.punch, rate, takes });
        if let Some(root) = self.autosave_dir.clone() {
            self.queue_autosave(root);
        }
    }

    /// Arm the input for the record-enabled tracks; a session never saved records into its
    /// autosave folder. Returns the takes it records.
    fn arm_recorder(&mut self) -> Result<Vec<soundcraft_engine::io::Take>, String> {
        let name = soundcraft_engine::io::sanitize_name(&self.engine.session().name);
        let unsaved_dir = self.autosave_dir.as_ref().map_or_else(std::env::temp_dir, |d| d.join(name));
        let Some(r) = &mut self.recorder else { return Err("no audio input device".into()) };
        let plan = soundcraft_engine::io::plan_takes(&self.engine, &unsaved_dir, r.channels);
        if plan.is_empty() {
            return Err("no audio track is record-enabled".into());
        }
        // The takes are unsaved work: the folder they go into under the autosave root is
        // owner-only, like the autosaves, before the recorder makes anything in it.
        if self.engine.path.is_none() && self.autosave_dir.is_some() {
            for dir in plan.iter().filter_map(|(_, _, path)| path.parent()) {
                soundcraft_engine::autosave::create_folder(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
        }
        let takes = plan
            .iter()
            .map(|(track, inputs, path)| soundcraft_engine::io::Take { track: *track, path: path.clone(), channels: inputs.len() })
            .collect();
        r.arm(plan)?;
        Ok(takes)
    }

    fn finish_recording(&mut self) {
        self.engine.transport.recording = false;
        if let Some(p) = &self.player {
            p.set_recording(false);
        }
        let plan = self.take_plan.take();
        let Some(r) = &mut self.recorder else { return };
        let (takes, problem) = r.finish();
        let tracks = takes.len().max(1);
        let plan = plan.unwrap_or_else(|| soundcraft_engine::io::Recording {
            start: self.record_start,
            pass: None,
            punch: self.punch,
            rate: 0,
            takes: Vec::new(),
        });
        self.punch = None;
        self.autosaved_rev = 0;
        let ids = soundcraft_engine::io::place_recording(&mut self.engine, &plan, takes);
        self.ui.status =
            if plan.pass.is_some() { format!("Loop-recorded {} take(s)", ids.len() / tracks) } else { format!("Recorded {} clip(s)", ids.len()) };
        if let Some(p) = problem {
            log::warn!("recording: {p}");
            self.ui.status = format!("{} ({p})", self.ui.status);
        }
    }

    /// While playing, record volume automation from fader positions for tracks in Write mode, and
    /// for Touch/Latch tracks whose fader is (or, for Latch, was) being moved. Each point replaces the
    /// automation since the previous one, and Stop writes a last point. A pass and whatever is
    /// dragged during it, its fader included, are one undo step.
    pub(crate) fn write_automation(&mut self) {
        use soundcraft_model::AutomationMode as M;
        const PASS: &str = "automation_pass";
        let playing = self.is_playing();
        let may_write =
            playing && self.engine.session().tracks.iter().any(|t| matches!(t.mixer.automation_mode, M::Write | M::Touch | M::Latch | M::TouchLatch));
        self.engine.hold_merge(may_write.then_some(PASS));
        if !playing && self.touched.is_empty() {
            self.last_write_at = i64::MIN;
            return;
        }
        let pos = self.engine.transport.position;
        let step = self.engine.session().sample_rate.samples(0.02);
        if playing && !automation_step_due(pos, self.last_write_at, step) {
            return;
        }
        let from = std::mem::replace(&mut self.last_write_at, if playing { pos } else { i64::MIN });
        // The gesture state cached from the known UI phase, never hidden-tick stale input.
        let dragging = self.pointer_down;
        let fader_track = match &self.gesture {
            Some(Gesture::Fader { track, .. }) => Some(*track),
            _ => None,
        };
        let writes: Vec<(TrackId, f32)> = self
            .engine
            .session()
            .tracks
            .iter()
            .filter_map(|t| {
                let m = t.mixer.automation_mode;
                let touched_now = dragging && fader_track == Some(t.id);
                let write = match m {
                    M::Write => true,
                    M::Touch => touched_now,
                    M::Latch | M::TouchLatch => touched_now || self.touched.contains(&t.id),
                    _ => false,
                };
                write.then_some((t.id, t.mixer.volume_db))
            })
            .collect();
        for (t, v) in writes {
            self.touched.insert(t);
            let mut point = serde_json::json!({"track": t.0, "param": "volume", "at": pos, "value": v});
            if from != i64::MIN {
                point["from"] = from.into();
            }
            let _ = self.engine.execute_merged("automation.set_point", &point, PASS);
        }
        if !playing {
            self.touched.clear();
            self.engine.end_merge();
        }
    }

    /// Play from the selection, honouring loop playback and selection end.
    pub fn play_from_selection(&mut self) {
        let s = self.engine.session();
        let sel = s.edit.selection;
        let preroll = if s.edit.pre_post_roll { s.edit.pre_roll } else { 0 };
        let postroll = if s.edit.pre_post_roll { s.edit.post_roll } else { 0 };
        if s.edit.loop_playback && !sel.is_empty() {
            self.start_play(sel.start, None, Some(sel));
        } else if !sel.is_empty() {
            self.start_play((sel.start - preroll).max(0), Some(sel.end + postroll), None);
        } else {
            self.start_play((sel.start - preroll).max(0), None, None);
        }
    }

    /// Apply engine transport requests to the player and recorder, read back the position and refresh meters. A reset
    /// request also clears per-document capture, edit-gesture and meter state.
    pub(crate) fn handle_transport(&mut self, dt: f32) {
        let reqs: Vec<TransportRequest> = std::mem::take(&mut self.engine.transport_requests);
        for r in reqs {
            match r {
                TransportRequest::ResetSession => {
                    if let Some(p) = &self.player {
                        p.stop();
                        p.set_recording(false);
                        p.set_speed(1.0);
                        p.locate(self.engine.transport.position);
                    }
                    self.sim = None;
                    // The old capture belongs to the replaced document, never to the new one.
                    self.recorder = None;
                    self.recorder_failed = false;
                    self.punch = None;
                    self.gesture = None;
                    self.touched.clear();
                    self.last_write_at = i64::MIN;
                    self.edit_layout.follow_hold = false;
                    self.meters.clear();
                    self.main_meter = MeterDisplay::default();
                }
                TransportRequest::Play => {
                    if !self.is_playing() {
                        self.play_from_selection();
                    }
                }
                TransportRequest::Stop | TransportRequest::Pause => self.stop_play(),
                TransportRequest::TogglePlay => {
                    if self.is_playing() {
                        self.stop_play();
                    } else {
                        self.play_from_selection();
                    }
                }
                TransportRequest::Record => {
                    if self.engine.transport.recording {
                        self.stop_play();
                    } else {
                        self.start_recording();
                    }
                }
                TransportRequest::Locate(at) => {
                    if self.is_playing() {
                        if let Some(p) = &self.player {
                            p.locate(at);
                        }
                        if let Some(sim) = &mut self.sim {
                            sim.0 = at;
                        }
                        self.last_write_at = i64::MIN;
                    }
                    self.engine.transport.position = at;
                }
                TransportRequest::PlaySelection => {
                    let sel = self.engine.session().edit.selection;
                    self.start_play(sel.start, Some(sel.end.max(sel.start + 1)), None);
                }
                TransportRequest::HalfSpeed => {
                    if let Some(p) = &self.player {
                        p.set_speed(0.5);
                    }
                    self.play_from_selection();
                }
                TransportRequest::Scrub(at) => self.engine.transport.position = at,
                TransportRequest::AllNotesOff => {}
            }
        }
        // Read back the position.
        if let Some(p) = &self.player {
            let playing = p.is_playing();
            let ended = self.engine.transport.playing && !playing;
            if ended {
                p.set_speed(1.0);
            }
            if self.engine.transport.playing {
                self.engine.transport.position = p.position();
            }
            let now = self.uptime;
            let meter_interval = if self.ui.window == MainWindow::Mix { 1.0 / 60.0 } else { 1.0 / 20.0 };
            if now - self.last_meter_poll >= meter_interval {
                p.take_meters(&mut self.meter_snapshot);
                self.last_meter_poll = now;
            }
            let snap = &self.meter_snapshot;
            for (id, m) in &snap.tracks {
                let d = self.meters.entry(*id).or_default();
                feed_meter(d, m.peak, m.gain_reduction_db, dt);
                if m.plugin_failures.count != d.plugin_failures {
                    d.plugin_failures = m.plugin_failures.count;
                    if let Some(msg) = failed_plugin_status(self.engine.session(), *id, m.plugin_failures.last) {
                        self.ui.status = msg;
                    }
                }
            }
            let main = snap.main.peak;
            feed_meter(&mut self.main_meter, main, 0.0, dt);
            if ended {
                self.stop_play();
            }
        } else if let Some((pos, end, looped)) = &mut self.sim {
            let sr = self.engine.session().sample_rate.as_f64();
            *pos += (f64::from(dt) * sr) as i64;
            if let Some(l) = looped
                && *pos >= l.end
            {
                *pos = l.start;
            }
            let stop = end.is_some_and(|e| *pos >= e);
            self.engine.transport.position = *pos;
            if stop {
                self.stop_play();
            }
        }
        if !self.meters_live() {
            for d in self.meters.values_mut() {
                feed_meter(d, [0.0, 0.0], 0.0, dt);
            }
            feed_meter(&mut self.main_meter, [0.0, 0.0], 0.0, dt);
        }
    }
}
