//! Audio devices that stopped working: the Reconnect offer, and reopening a device that went away
//! once it is back, or an output whose settings another app or the system changed.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use super::SoundApp;

/// How often the device list is read while a device is away.
const POLL: Duration = Duration::from_secs(2);
/// Readings in a row a device must be listed in before it is reopened: a device that was just
/// plugged in can be listed before it opens, and one that is going away can still be listed.
const STEADY: u32 = 2;

/// Reads whether the audio system (first argument, `None` for the default one) lists the output
/// and the input device named (a device not asked for counts as listed).
pub(crate) type Probe = Arc<dyn Fn(Option<&str>, Option<&str>, Option<&str>) -> (bool, bool) + Send + Sync>;

/// The audio devices need reopening: the output or input reported trouble. Devices that went away,
/// and an output whose settings changed, are waited for on a background thread that reads the
/// device list, never on the audio thread.
pub(crate) struct LostAudio {
    /// The audio system the devices belong to (`None`: the default one).
    system: Option<String>,
    /// The output and input device waited for: one that went away, or (`output_changed`) an
    /// output whose stream was invalidated while it stayed.
    pub(crate) output: Option<String>,
    pub(crate) input: Option<String>,
    /// The output did not go away: another app or the system changed its settings, which
    /// invalidated the stream. It is reopened once it is listed, and said to be reopened rather
    /// than back.
    pub(crate) output_changed: bool,
    /// The output is still there but stopped working (stalled, failed, or could not be
    /// reopened): only Reconnect reopens it, so the offer stays when nothing is waited for.
    pub(crate) by_hand: bool,
    /// The same for the input. Kept until an input opens again (`input_opened`) or Reconnect,
    /// so that reopening only the output does not take the offer away.
    pub(crate) input_by_hand: bool,
    /// Readings in a row that listed the output and the input.
    steady: (u32, u32),
    /// What the background thread reads; `None` while nothing is waited for. Dropping it ends
    /// the thread at its next reading.
    list: Option<Receiver<(bool, bool)>>,
    probe: Probe,
    every: Duration,
}

impl LostAudio {
    pub(crate) fn new(system: Option<String>) -> Self {
        Self::with_probe(system, Arc::new(soundcraft_playback::devices::devices_present), POLL)
    }

    /// Reads the device list with `probe` every `every`.
    pub(crate) fn with_probe(system: Option<String>, probe: Probe, every: Duration) -> Self {
        LostAudio {
            system,
            output: None,
            input: None,
            output_changed: false,
            by_hand: false,
            input_by_hand: false,
            steady: (0, 0),
            list: None,
            probe,
            every,
        }
    }

    /// Wait for the output (or input) device `name`, which went away, to come back.
    pub(crate) fn away(&mut self, name: String, output: bool) {
        if output {
            self.output_changed = false;
        }
        let slot = if output { &mut self.output } else { &mut self.input };
        if slot.as_deref() == Some(name.as_str()) && self.list.is_some() {
            return;
        }
        *slot = Some(name);
        self.watch();
    }

    /// The output device `name` stayed, but its stream was invalidated (another app or the
    /// system changed its settings): reopen it once it is listed, like a device that came back.
    pub(crate) fn changed(&mut self, name: String) {
        self.away(name, true);
        self.output_changed = true;
    }

    /// Stop waiting for the output device (it was reopened, or reopening it failed).
    fn output_done(&mut self) {
        self.output = None;
        self.output_changed = false;
        self.watch();
    }

    /// The output opened again: its trouble is settled, not the input's, which stays until an
    /// input opens (the input is only retried on its next use).
    fn output_reopened(&mut self) {
        self.by_hand = false;
    }

    /// Stop waiting for the input device.
    fn input_done(&mut self) {
        self.input = None;
        self.watch();
    }

    /// Whether a device is waited for (one that went away, or an output whose settings changed).
    fn waiting(&self) -> bool {
        self.list.is_some()
    }

    /// Whether nothing is left to reconnect: no device is waited for and none needs Reconnect.
    fn settled(&self) -> bool {
        !self.waiting() && !self.by_hand && !self.input_by_hand
    }

    /// (Re)start reading the device list for the devices away; the old reader stops.
    fn watch(&mut self) {
        self.steady = (0, 0);
        self.list = None;
        if self.output.is_none() && self.input.is_none() {
            return;
        }
        let (tx, rx) = channel();
        let (probe, every) = (Arc::clone(&self.probe), self.every);
        let (system, output, input) = (self.system.clone(), self.output.clone(), self.input.clone());
        let reader = std::thread::Builder::new().name("audio device watch".into()).spawn(move || {
            while tx.send(probe(system.as_deref(), output.as_deref(), input.as_deref())).is_ok() {
                std::thread::sleep(every);
            }
        });
        match reader {
            Ok(_) => self.list = Some(rx),
            Err(e) => log::warn!("cannot watch for the audio device to come back: {e}"),
        }
    }

    /// Takes in the readings since the last call. Returns whether the output and the input that
    /// went away are back to stay.
    fn back(&mut self) -> (bool, bool) {
        let Some(list) = &self.list else { return (false, false) };
        for (output, input) in list.try_iter() {
            self.steady.0 = if output { self.steady.0 + 1 } else { 0 };
            self.steady.1 = if input { self.steady.1 + 1 } else { 0 };
        }
        (self.output.is_some() && self.steady.0 >= STEADY, self.input.is_some() && self.steady.1 >= STEADY)
    }
}

impl SoundApp {
    /// The audio devices need reopening: from now on until they are, the status line offers
    /// Reconnect.
    pub(super) fn audio_lost(&mut self) -> &mut LostAudio {
        let system = self.ui.audio.system.clone();
        self.lost_audio.get_or_insert_with(|| LostAudio::new(system))
    }

    /// Whether the audio devices need reopening (the status line then offers Reconnect).
    pub(crate) fn offers_reconnect(&self) -> bool {
        self.lost_audio.is_some()
    }

    /// Reopen a device that went away once it is back, or an output whose settings changed once it
    /// is listed, while the transport is stopped: the output with the rest of the audio devices, or
    /// an input alone, by its name, on its next use.
    pub(super) fn watch_replug(&mut self) {
        let Some(lost) = &mut self.lost_audio else { return };
        let (output, input) = lost.back();
        if !(output || input) || self.engine.transport.playing || self.engine.transport.recording {
            return;
        }
        if output && let Some(name) = lost.output.clone() {
            // Back too (usually the same interface): it opens again with the output, by its name.
            let input_back = if input { lost.input.take() } else { None };
            let changed = lost.output_changed;
            lost.output_done();
            let input_away = lost.input.is_some();
            // The device that was lost, by name: with the system default chosen, the default
            // may still be another device.
            let cfg = soundcraft_playback::AudioConfig { output: Some(name.clone()), ..self.ui.audio.clone() };
            self.reopen_audio(&cfg);
            self.input_pin = input_back;
            // An input still away opens again when it is back, not on the next record.
            self.recorder_failed = input_away;
            if self.player.as_ref().is_some_and(|p| !p.silent && p.device_name == name) {
                if let Some(l) = &mut self.lost_audio {
                    l.output_reopened();
                }
                self.ui.status = if changed {
                    crate::localized_format!("SoundCraft reopened “{name}” after its settings changed.", name = name)
                } else {
                    crate::localized_format!("“{name}” is back: SoundCraft reconnected it.", name = name)
                };
            } else {
                self.ui.status = if changed {
                    crate::localized_format!(
                        "SoundCraft could not reopen “{name}” after its settings changed. Use Reconnect to try again.",
                        name = name
                    )
                } else {
                    crate::localized_format!("“{name}” is back, but SoundCraft could not open it. Use Reconnect to try again.", name = name)
                };
                if let Some(l) = &mut self.lost_audio {
                    l.by_hand = true;
                }
                return;
            }
        } else if input
            && lost.output.is_none()
            && let Some(name) = lost.input.clone()
        {
            lost.input_done();
            // It opens on the next record or monitor (this frame, if one waits for it), by its
            // name: with the system default chosen, the default may now be another device.
            self.recorder = None;
            self.recorder_failed = false;
            self.input_pin = Some(name.clone());
            self.ui.status = crate::localized_format!("“{name}” is back.", name = name);
        }
        if self.lost_audio.as_ref().is_some_and(LostAudio::settled) {
            self.lost_audio = None;
        }
    }

    /// The input device `name` opened: stop waiting for it.
    pub(super) fn input_opened(&mut self, name: &str) {
        let Some(l) = &mut self.lost_audio else { return };
        // An input that stopped working without going away is settled by any input opening.
        l.input_by_hand = false;
        if l.input.as_deref() == Some(name) {
            l.input_done();
        }
        if l.settled() {
            self.lost_audio = None;
        }
    }

    /// Reopen the audio devices (Reconnect audio device), keeping a take in progress, and say
    /// which output opened.
    pub fn reconnect_audio(&mut self) {
        let cfg = self.ui.audio.clone();
        self.reopen_audio(&cfg);
        let Some(p) = &self.player else { return };
        if p.silent {
            self.ui.status = crate::localization::text("Reconnect opened no audio device: SoundCraft runs on a silent clock.").to_string();
            return;
        }
        let name = p.device_name.clone();
        self.ui.status = crate::localized_format!("Reconnected to “{name}”.", name = name);
        // A device that went away and is still not back is still waited for; an input that is
        // back stops being waited for when it opens (`input_opened`).
        let still_away = self.lost_audio.as_mut().is_some_and(|l| {
            l.by_hand = false;
            l.input_by_hand = false;
            if l.output.as_ref().is_some_and(|away| *away == name) {
                l.output_done();
            }
            l.waiting()
        });
        if !still_away {
            self.lost_audio = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LostAudio, Probe, STEADY};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// A device list the test plugs devices in and out of.
    #[derive(Clone, Default)]
    struct Devices(Arc<Mutex<Vec<String>>>);

    impl Devices {
        fn set(&self, names: &[&str]) {
            *self.0.lock().unwrap() = names.iter().map(|n| n.to_string()).collect();
        }

        fn probe(&self) -> Probe {
            let list = self.clone();
            Arc::new(move |_system, output, input| {
                let listed = list.0.lock().unwrap();
                let has = |want: Option<&str>| want.is_none_or(|w| listed.iter().any(|n| n == w));
                (has(output), has(input))
            })
        }
    }

    fn lost(devices: &Devices) -> LostAudio {
        LostAudio::with_probe(None, devices.probe(), Duration::from_millis(5))
    }

    /// Calls `back` until it says something is back, or a while has passed.
    fn wait_back(l: &mut LostAudio) -> (bool, bool) {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let back = l.back();
            if back != (false, false) || Instant::now() > until {
                return back;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_device_that_went_away_is_back_once_it_stays_listed() {
        let devices = Devices::default();
        let mut l = lost(&devices);
        l.away("UFX".into(), true);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(l.back(), (false, false), "not listed: not back");
        devices.set(&["Speakers", "UFX"]);
        assert_eq!(wait_back(&mut l), (true, false));
        assert!(l.steady.0 >= STEADY, "listed in several readings in a row, not just one");
    }

    #[test]
    fn the_input_is_waited_for_on_its_own() {
        let devices = Devices::default();
        devices.set(&["UFX"]);
        let mut l = lost(&devices);
        l.away("Mic".into(), false);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(l.back(), (false, false), "the output was not lost, so it is never 'back'");
        devices.set(&["UFX", "Mic"]);
        assert_eq!(wait_back(&mut l), (false, true));
    }

    #[test]
    fn the_device_list_is_not_read_once_nothing_is_waited_for() {
        let read = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&read);
        let probe: Probe = Arc::new(move |_, _, _| {
            seen.store(true, Ordering::Relaxed);
            (false, false)
        });
        let mut l = LostAudio::with_probe(None, probe, Duration::from_millis(5));
        assert!(!l.waiting(), "a stalled device is reconnected by hand; nothing to wait for");
        std::thread::sleep(Duration::from_millis(30));
        assert!(!read.load(Ordering::Relaxed));
        l.away("UFX".into(), true);
        assert!(l.waiting());
        l.output_done();
        assert!(!l.waiting());
    }

    /// A device name no test machine has, so that reopening it never depends on the devices there.
    const AWAY: &str = "SoundCraft test device that is never present";

    /// A SoundApp whose output device `AWAY` went away while stopped, waited for on `devices`.
    fn app_with_lost_output(devices: &Devices) -> crate::SoundApp {
        let mut app = crate::SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.lost_audio = Some(lost(devices));
        app.audio_lost().away(AWAY.into(), true);
        app.ui.status = format!("Audio output: “{AWAY}” is unavailable");
        app
    }

    #[test]
    fn a_returning_output_is_not_reopened_while_the_transport_runs() {
        let devices = Devices::default();
        let mut app = app_with_lost_output(&devices);
        devices.set(&[AWAY]);
        app.engine.transport.playing = true;
        let until = Instant::now() + Duration::from_millis(200);
        while Instant::now() < until {
            app.watch_replug();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.player.is_none(), "nothing was reopened while playing");
        assert!(app.offers_reconnect());
        assert!(app.ui.status.contains("unavailable"));
    }

    #[test]
    fn a_returning_output_is_reopened_and_the_unavailable_message_replaced() {
        let devices = Devices::default();
        let mut app = app_with_lost_output(&devices);
        // Its input went away with it.
        app.audio_lost().away(AWAY.into(), false);
        app.recorder_failed = true;
        app.watch_replug();
        assert!(app.player.is_none(), "not back yet");
        devices.set(&[AWAY]);
        let until = Instant::now() + Duration::from_secs(5);
        while app.player.is_none() && Instant::now() < until {
            app.watch_replug();
            std::thread::sleep(Duration::from_millis(5));
        }
        // No machine has `AWAY`: the reopen falls back to another device or the silent
        // clock, which the status says instead of claiming success.
        assert!(app.player.is_some(), "the devices were reopened");
        assert!(!app.ui.status.contains("unavailable"), "{}", app.ui.status);
        assert!(app.ui.status.contains("could not open"), "{}", app.ui.status);
        assert!(app.offers_reconnect(), "the message asks for Reconnect, so the status line offers it");
        assert!(!app.recorder_failed, "the input that came back with it opens again on its next use");
    }

    /// A device whose stream another app invalidated (it changed the sample rate) never left: it
    /// is reopened, and neither message says it went away or came back.
    #[test]
    fn an_output_whose_settings_changed_is_reopened_without_saying_it_was_away() {
        let devices = Devices::default();
        devices.set(&[AWAY]);
        let mut app = crate::SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.lost_audio = Some(lost(&devices));
        app.audio_lost().changed(AWAY.into());
        let until = Instant::now() + Duration::from_secs(5);
        while app.player.is_none() && Instant::now() < until {
            app.watch_replug();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.player.is_some(), "the device was reopened");
        // No machine has `AWAY`, so the reopen does not get it back.
        assert_eq!(app.ui.status, format!("SoundCraft could not reopen “{AWAY}” after its settings changed. Use Reconnect to try again."));
        assert!(!app.ui.status.contains("is back"), "{}", app.ui.status);
        assert!(app.offers_reconnect());
    }

    #[test]
    fn an_input_that_comes_back_opens_again_on_its_next_use() {
        let devices = Devices::default();
        let mut app = crate::SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.lost_audio = Some(lost(&devices));
        app.audio_lost().away("Mic".into(), false);
        app.recorder_failed = true;
        app.ui.status = "Audio input: “Mic” is unavailable".into();
        devices.set(&["Mic"]);
        let until = Instant::now() + Duration::from_secs(5);
        while app.recorder_failed && Instant::now() < until {
            app.watch_replug();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!app.recorder_failed, "the input may open again");
        assert!(!app.offers_reconnect());
        assert!(!app.ui.status.contains("unavailable"), "{}", app.ui.status);
        // With the system default chosen, the default may now be another device: the input
        // opens by the name that came back, as the output does.
        assert_eq!(app.ui.audio.input, None);
        assert_eq!(app.input_config().input.as_deref(), Some("Mic"));
        // Reconnect opens what the audio choice says.
        app.reconnect_audio();
        assert_eq!(app.input_config().input, None);
    }

    /// The status line offers Reconnect while the devices need reopening; using it says what
    /// opened instead of leaving the old message, and the offer goes away.
    #[test]
    fn the_status_line_reconnects_and_says_so() {
        use egui_kittest::{Harness, kittest::Queryable};
        let app = crate::SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_ui_state(
            |ui, app: &mut crate::SoundApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
        h.run_steps(2);
        assert!(h.query_by_label("Reconnect").is_none(), "nothing to reconnect");
        let stale = "Audio output: “UFX” stopped asking for audio Use Reconnect audio device in Setup › Playback Engine.";
        h.state_mut().audio_lost();
        h.state_mut().ui.status = stale.into();
        h.run_steps(2);
        h.get_by_label("Reconnect").click();
        h.run_steps(2);
        assert!(h.state().player.is_some(), "the devices were reopened");
        assert_ne!(h.state().ui.status, stale);
        // Without an output device the reopen runs on the silent clock, which is still trouble.
        let opened = h.state().player.as_ref().is_some_and(|p| !p.silent);
        assert_eq!(h.state().offers_reconnect(), !opened, "{}", h.state().ui.status);
        assert_eq!(h.query_by_label("Reconnect").is_none(), opened, "the offer goes with the trouble");
    }

    /// An output that stalled is reopened only by Reconnect: an input that goes away and comes
    /// back meanwhile does not take the offer away.
    #[test]
    fn a_returning_input_keeps_the_offer_for_a_stalled_output() {
        let devices = Devices::default();
        let mut app = crate::SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.lost_audio = Some(lost(&devices));
        app.audio_lost().by_hand = true;
        app.audio_lost().away(AWAY.into(), false);
        app.recorder_failed = true;
        devices.set(&[AWAY]);
        let until = Instant::now() + Duration::from_secs(5);
        while app.recorder_failed && Instant::now() < until {
            app.watch_replug();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!app.recorder_failed, "the input may open again");
        assert!(app.offers_reconnect(), "the stalled output still needs Reconnect");
    }

    /// An input that stopped working without going away keeps the Reconnect offer when only the
    /// output is reopened: it is settled when an input opens, not before.
    #[test]
    fn a_reopened_output_keeps_the_offer_for_a_failed_input() {
        let devices = Devices::default();
        let mut app = crate::SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.lost_audio = Some(lost(&devices));
        app.audio_lost().input_by_hand = true;
        app.audio_lost().away(AWAY.into(), true);
        app.audio_lost().output_done();
        app.audio_lost().output_reopened();
        assert!(!app.audio_lost().settled(), "the input still needs Reconnect");
        app.watch_replug();
        assert!(app.offers_reconnect(), "reopening the output does not take the offer for the input away");
        app.input_opened("Mic");
        assert!(!app.offers_reconnect(), "an input opened: nothing is left to reconnect");
    }

    /// Reconnect while an input is still away keeps waiting for it, so that it opens again once
    /// it is back even if opening it failed in between.
    #[test]
    fn reconnect_keeps_waiting_for_an_input_still_away() {
        let devices = Devices::default();
        let mut app = crate::SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.lost_audio = Some(lost(&devices));
        app.audio_lost().away(AWAY.into(), false);
        app.reconnect_audio();
        assert_eq!(app.lost_audio.as_ref().and_then(|l| l.input.as_deref()), Some(AWAY));
        assert!(app.offers_reconnect());
        // Opening the input failed while it was away.
        app.recorder_failed = true;
        devices.set(&[AWAY]);
        let until = Instant::now() + Duration::from_secs(5);
        while app.recorder_failed && Instant::now() < until {
            app.watch_replug();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!app.recorder_failed, "the input that came back may open again");
        assert!(app.lost_audio.as_ref().is_none_or(|l| l.input.is_none()));
    }
}
