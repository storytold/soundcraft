use super::services::Services;
use super::state::MainWindow;
use super::{
    ControlRequest, Engine, Gesture, HashMap, MeterDisplay, OutputTrouble, Player, Range, Receiver, Samples, SoundApp, TrackId, TransportRequest,
    Trouble, UiState, Value, VecDeque, bounce, dialogs, edit_window, extra_windows, fonts, import, json, menus, midi_editor, mix_window, ops_windows,
    palette, panels, recovery, sampler_window, score_editor, shortcuts, theme, updates_ui, video_track, video_window, windows,
};

mod audio;
mod commands;
mod control;
mod files;
mod input;
mod playback;
mod plugins;

#[cfg(test)]
pub(crate) use playback::automation_step_due;
#[cfg(test)]
pub(crate) use plugins::{editor_changes, editor_states_due, failed_plugin_status};

impl SoundApp {
    /// Whether fonts installed from the system are used besides the bundled ones (on by default).
    /// Takes effect only before the first frame installs the fonts.
    pub fn set_system_fonts(&mut self, on: bool) {
        self.system_fonts = on;
    }

    /// Build an app around an engine and optional player, initialising UI, recording, autosave, control and
    /// synthetic-input state.
    pub fn new(engine: Engine, player: Option<Player>, services: Services) -> Self {
        SoundApp {
            recent_seen: engine.path.clone(),
            recent_changed: false,
            recent_checked: None,
            engine,
            player,
            recorder: None,
            record_start: 0,
            punch: None,
            recorder_failed: false,
            lost_shown: 0,
            take_plan: None,
            ui: UiState::default(),
            audio_health_report: None,
            services,
            native_menu_bar: false,
            autosave_dir: None,
            autosaver: None,
            autosaved_rev: 0,
            leftovers: Vec::new(),
            panic_rev: 0,
            preset_dir: None,
            last_autosave: None,
            dialogs: dialogs::Dialogs::default(),
            updates_rt: updates_ui::Runtime::default(),
            meters: HashMap::new(),
            main_meter: MeterDisplay::default(),
            meter_snapshot: soundcraft_playback::MeterSnapshot::default(),
            gesture: None,
            edit_layout: edit_window::EditLayout::default(),
            midi: midi_editor::MidiEditorState::default(),
            ops: ops_windows::OpsState::default(),
            extra: extra_windows::ExtraState::default(),
            palette: palette::PaletteState::default(),
            focus_mix: false,
            edit_scroll_into_view: None,
            mix_scroll_into_view: None,
            synthetic: Vec::new(),
            synthetic_batch: None,
            synthetic_pointer_active: false,
            cancel_synthetic_pointer: false,
            cancel_synthetic_keys: false,
            cancel_synthetic_scroll: false,
            input_before_scroll: None,
            injected_events: Vec::new(),
            injected_at: 0,
            host_keys_down: None,
            pointer_down: false,
            wants_keyboard: false,
            sample_drop_targets: Vec::new(),
            pending_drops: Vec::new(),
            host_drops: Vec::new(),
            sample_reads: Vec::new(),
            synthetic_grace: 0,
            control_rx: None,
            gesture_acks: VecDeque::new(),
            pending_shots: Vec::new(),
            doc_epoch: 0,
            host_geometry: None,
            last_tick: None,
            clock_offset: std::time::Duration::ZERO,
            test_clock: None,
            uptime: 0.0,
            last_rendered_tick: None,
            pending_open: None,
            last_rev: 0,
            fonts_ready: false,
            fonts_installed: false,
            system_fonts: true,
            theme_applied: None,
            docking_context: None,
            sim: None,
            record_hint: None,
            quit_requested: false,
            arrange_request: false,
            touched: std::collections::HashSet::new(),
            touched_sends: std::collections::HashSet::new(),
            last_write_at: i64::MIN,
            editor_states: Vec::new(),
            editor_poll_due: true,
            #[cfg(test)]
            editor_poll_fixture: None,
            frame_ms: 0.0,
            last_frame: None,
            last_meter_poll: f64::NEG_INFINITY,
            video: video_track::VideoPool::default(),
            bounce: None,
            imports: import::Imports::default(),
            #[cfg(test)]
            _palette_lock: theme::lock_test_palette(),
            #[cfg(test)]
            synthetic_applied_batches: 0,
        }
    }

    pub fn with_control(mut self, rx: Receiver<ControlRequest>) -> Self {
        self.control_rx = Some(rx);
        self
    }

    /// Per-frame logic: control channel, transport, document sync.
    ///
    /// eframe calls this once per layout pass and also on logic-only ticks while hidden. Consumed
    /// queues are one-shot, autosave is revision/instant based, and the monotonic clock decides
    /// timed work. Bounce and import polling run only on pass zero, including hidden logic ticks.
    /// Each host iteration starts with `raw_input_hook`; its editor-poll admission prevents an
    /// empty queue on a discarded pass from ending a still-active plugin gesture.
    pub fn logic(&mut self, ctx: &egui::Context) {
        // Fonts set now take effect next frame, so draw only from the frame after.
        if !self.fonts_ready {
            if self.fonts_installed {
                self.fonts_ready = true;
            } else {
                fonts::install(ctx, self.system_fonts);
                self.fonts_installed = true;
                ctx.request_repaint();
            }
        }
        let light = theme::wants_light(ctx, self.ui.theme);
        if self.theme_applied != Some(light) || theme::is_light() != light {
            theme::apply(ctx, light);
            self.theme_applied = Some(light);
        }
        // A logic-only tick while hidden gets no fresh events: `ctx.input` still says what the last
        // completed pass saw. Do not let that stale pointer/keyboard state keep writing automation
        // or hold a transient fader gesture (loss of visibility cancels a Touch drag by design).
        // `ui` (the known UI phase) caches this pass's real state for the next `logic` call.
        let hidden = !ctx.input(|i| i.viewport().visible()).unwrap_or(true);
        if hidden {
            self.pointer_down = false;
            self.wants_keyboard = false;
            if matches!(self.gesture, Some(Gesture::Fader { .. } | Gesture::SendFader { .. })) {
                self.gesture = None;
            }
        }
        // UI input time is stale on logic-only ticks and identical across passes of one frame.
        // Frame pacing and the transport simulation use a monotonic host clock instead.
        let dt = self.tick_dt();
        self.last_frame = Some(self.uptime);
        crate::control::expire_gestures(self);
        crate::control::drain(self, ctx);
        self.note_recent_session();
        self.apply_editor_edits();
        // A multi-pass frame runs this logic once per pass; bounce slices and background-import
        // polling are per host frame.
        if ctx.current_pass_index() == 0 {
            bounce::step(self, ctx);
            import::step(self, ctx);
            updates_ui::tick(self, ctx);
        }
        if !self.pointer_down {
            if matches!(self.gesture, Some(Gesture::Fader { .. } | Gesture::SendFader { .. })) {
                self.gesture = None;
            }
            // Keep a running automation pass merged until playback stops.
            if !self.wants_keyboard && self.touched.is_empty() && self.touched_sends.is_empty() {
                self.engine.end_merge();
            }
        }
        self.watch_output();
        self.watch_input();
        // Stop the old playback before sending a replacement session to the audio thread.
        self.handle_transport(dt);
        if self.engine.revision != self.last_rev {
            self.last_rev = self.engine.revision;
            if let Some(p) = &self.player {
                p.update_session(self.engine.session_arc());
            }
        }
        self.open_inserted();
        self.autosave_changes();
        self.write_automation();
        self.autosave();
        if self.recorder.is_none()
            && !self.recorder_failed
            && self.engine.session().tracks.iter().any(|t| t.mixer.input_monitor || t.mixer.record_arm)
        {
            self.ensure_input();
        }
        // Autosave and the macOS menu bar only run on frames, and plugin editors are native windows egui never hears from.
        // egui 0.37 schedules the delay as given (it no longer subtracts the predicted frame time),
        // so ask for the real interval instead of adding that time back.
        let editors = self.player.as_ref().is_some_and(Player::any_editor_open);
        ctx.request_repaint_after_secs(if editors { 0.03 } else { 1.0 });
        crate::control::drain_screenshots(self, ctx);
        if crate::control::screenshots_need_paint(self) {
            // An admitted capture that has not reached a UI pass needs an explicit hidden paint;
            // the screenshot command itself only paints once it has been issued.
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            ctx.request_repaint();
        }
        if self.synthetic_batch.is_some() {
            // A batch waiting for a settled pass (or the UI pass that would apply it) needs a real
            // UI paint, not just logic ticks: a hidden window runs no layout otherwise.
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        }
        if self.synthetic_batch.is_some() || !self.synthetic.is_empty() {
            ctx.request_repaint();
        }
    }

    /// Monotonic seconds since the previous tick, clamped to a sane frame step.
    fn tick_dt(&mut self) -> f32 {
        let now = self.clock_now();
        let elapsed = self.last_tick.replace(now).map_or(1.0 / 60.0, |prev| now.duration_since(prev).as_secs_f64());
        self.uptime += elapsed;
        (elapsed as f32).clamp(0.0, 0.25)
    }

    /// Lay out the whole window.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // egui just processed the scripted key events. A scripted release while the host still
        // physically holds the key would report it up; put the host-only state back before any
        // shortcut or widget reads it.
        if let Some(host_down) = self.host_keys_down.take() {
            ctx.input_mut(|i| i.keys_down = host_down);
        }
        // Dropped files arrive in a real UI pass. Capture and clear them before the font gate, so a
        // first frame that returns before layout keeps them in a one-shot queue for the next
        // laid-out pass, and a forced hidden paint handles them like any other.
        let dropped: Vec<(egui::DroppedFileHandle, Option<egui::Pos2>)> = ctx.input_mut(|i| {
            let pos = i.pointer.latest_pos();
            i.raw.dropped_files.drain(..).map(|f| (f, pos)).collect()
        });
        self.pending_drops.extend(dropped);
        if self.cancel_synthetic_pointer {
            // The deadline may have crossed in logic after this frame's raw-input hook. Do not
            // dispatch its stale press/release to widgets; the next hook cancels it safely before
            // egui computes interactions. Captured drops remain queued for that drawn pass.
            ctx.request_repaint();
            return;
        }
        // A discard rerun gets a fresh `RawInput`, so it never contains the injected run. Only
        // replay/remove when this pass really received it; otherwise a later pass would dispatch
        // the earlier pass's host key/text events twice and roll its time back.
        let received_injected = self.pass_received_injected(&ctx);
        if std::mem::take(&mut self.cancel_synthetic_keys) {
            // A key/text batch crossed its deadline in logic, after the hook had already handed it
            // to this pass. Drop that copy before widgets, shortcuts and egui's end-of-pass
            // quit/zoom shortcuts can execute it. A discard rerun did not receive the batch, so it
            // has nothing to remove and must keep its own input.
            if received_injected && !self.remove_received_injected(&ctx) {
                // The stale copy could not be identified. Do not lay out a pass that might execute
                // it; the next hook drops the retained raw copy.
                ctx.request_repaint();
                return;
            }
            // This real pass consumed the hook-mutated raw. Keep drawing when the stale copy was
            // removed so unrelated host key/text events in the same pass still reach shortcuts and
            // widgets; clearing the recorded run also stops the next hook stripping a genuine
            // identical host event.
            self.injected_events.clear();
            ctx.request_repaint();
        }
        if std::mem::take(&mut self.cancel_synthetic_scroll) {
            if received_injected {
                // A scroll/zoom batch expired after the hook injected it. Rebuild this pass's input
                // without it before widgets read scroll aggregates, then keep drawing so a
                // simultaneous real host key, wheel or pointer event is still dispatched.
                self.rebuild_input_without_expired_scroll(&ctx);
            } else {
                // The batch was not injected into this pass (for example a discarded rerun after a
                // font-gated pass). Replaying the earlier pass's host raw would dispatch its key
                // and text events twice and roll back this pass's time.
                self.input_before_scroll = None;
                self.injected_events.clear();
                ctx.request_repaint();
            }
        }
        // Input preparation: remember this pass's real pointer/keyboard state, which `logic` (run
        // before `ui` in each pass and alone on hidden ticks) uses for gestures and automation.
        self.pointer_down = ctx.input(|i| i.pointer.any_down());
        self.wants_keyboard = ctx.egui_wants_keyboard_input();
        // The integration creates this Ui before logic resolves the current frame's palette.
        ui.set_style(ctx.global_style());
        if !self.fonts_ready {
            ctx.request_repaint();
            return;
        }
        if self.arrange_request {
            self.arrange_request = false;
            ctx.memory_mut(|m| m.reset_areas());
        }
        // Capture whether this pass received the batch *before* menus and widgets run. egui widgets
        // legitimately consume matching events from `InputState.events` (`consume_key` in docking,
        // `DragValue` and text completion), so checking afterwards would see them missing, leave
        // the batch pending, and inject the same edit/navigation/cut again next frame.
        let received_synthetic = self.pass_received_synthetic(&ctx);
        shortcuts::handle(self, &ctx);
        self.sample_drop_targets.clear();
        if !self.native_menu_bar {
            menus::menu_bar(self, ui);
        }
        // With the Mix window in its own native window, the main window keeps Edit.
        match if self.ui.mix_separate { MainWindow::Edit } else { self.ui.window } {
            MainWindow::Edit => edit_window::show(self, ui),
            MainWindow::Mix => mix_window::show(self, ui),
        }
        if self.ui.mix_separate {
            mix_window::show_separate(self, &ctx);
        }
        panels::floating(self, &ctx);
        windows::show(self, &ctx);
        video_window::show(self, &ctx);
        ops_windows::show(self, &ctx);
        extra_windows::show(self, &ctx);
        recovery::show(self, &ctx);
        score_editor::show(self, &ctx);
        palette::show(self, &ctx);
        dialogs::show(self, &ctx);
        updates_ui::show(self, &ctx);
        // The sampler's drop rects are current now, so route any captured drop in this pass.
        self.route_dropped_files();
        // The pass that actually received the injected events has already dispatched their widget
        // side effects, even when it then asks for another layout pass. `received_synthetic` was
        // captured before menus and widgets, so a widget that consumed the event from InputState
        // still counts as received. Acknowledge the batch here, before the discard check: leaving
        // it pending would make the next host frame inject the same click/key again and run the
        // command twice. A font-gated pass returns above, and a pass without the events never
        // acknowledges, so those batches stay pending.
        if self.synthetic_batch.is_some() && received_synthetic {
            let delivered = self.synthetic_batch.as_ref().map_or(0, Vec::len);
            if self.synthetic_batch.as_ref().is_some_and(|batch| batch.iter().any(|e| matches!(e, egui::Event::PointerButton { pressed: false, .. })))
            {
                self.synthetic_pointer_active = false;
            }
            self.synthetic_batch = None;
            // The pass consumed this copy; nothing is left for a later host tick to strip.
            self.injected_events.clear();
            // A scripted gesture is not a user clipboard action. Do not let a synthetic copy or
            // PRIMARY selection reach the OS clipboard through the host's platform output.
            ctx.output_mut(|o| {
                o.commands.retain(|c| {
                    !matches!(c, egui::OutputCommand::CopyText(_) | egui::OutputCommand::CopyImage(_) | egui::OutputCommand::TextSelectionSettled(_))
                });
            });
            crate::control::synthetic_delivered(self, delivered);
            // The next control request queued behind this gesture (inspect, engine command or
            // screenshot) needs a logic tick to be drained now that the barrier is gone.
            ctx.request_repaint();
            #[cfg(test)]
            {
                self.synthetic_applied_batches += 1;
            }
        }
        if !ctx.will_discard() {
            // Frame statistics describe rendered frames: one sample per settled host frame, not one
            // per layout pass and never on a hidden logic-only tick.
            let now = self.clock_now();
            let dt = self.last_rendered_tick.replace(now).map_or(1.0 / 60.0, |prev| now.duration_since(prev).as_secs_f64()) as f32;
            self.frame_ms = self.frame_ms.mul_add(0.9, dt.clamp(0.0, 0.25) * 1000.0 * 0.1);
            crate::control::issue_screenshots(self, &ctx);
        }
        // Here, not in logic(): a covered or minimized window runs only logic(), with no vsync to pace it.
        if self.meters_live() {
            let hz = if self.ui.window == MainWindow::Mix { 60 } else { 30 };
            ctx.request_repaint_after(std::time::Duration::from_millis(1000 / hz));
        } else if self.meters.values().any(|m| m.level[0] > 0.0001) {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        } else if !self.pending_shots.is_empty() || !self.synthetic.is_empty() {
            ctx.request_repaint();
        }
    }
}
