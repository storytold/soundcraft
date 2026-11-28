//! Audio devices that stopped working: the Reconnect offer, and reopening a device that went away
//! once it is back, or an output whose settings another app or the system changed (or, where the
//! audio system does not say which, an output that stopped). Also following the system default
//! where the stream open on it is not told when the default moves (`DefaultWatch`).

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use super::SoundApp;
use soundcraft_playback::OutputLoss;
use soundcraft_playback::devices::Wanted;

/// How often the device list is read while a device is away (and the system default's name while
/// it is watched).
const POLL: Duration = Duration::from_secs(2);
/// Readings in a row a device must be listed in before it is reopened: a device that was just
/// plugged in can be listed before it opens, and one that is going away can still be listed.
const STEADY: u32 = 2;

/// Reads whether the audio system (first argument, `None` for the default one) lists the output
/// and the input device wanted (see [`soundcraft_playback::devices::devices_present`]).
pub(crate) type Probe = Arc<dyn Fn(Option<&str>, Wanted<'_>, Wanted<'_>) -> (bool, bool) + Send + Sync>;

/// The audio devices need reopening: the output or input reported trouble. Devices that went away,
/// and an output whose settings changed or that stopped for a reason the audio system does not
/// say, are waited for on a background thread that reads the device list, never on the audio
/// thread.
pub(crate) struct LostAudio {
    /// The audio system the devices belong to (`None`: the default one).
    system: Option<String>,
    /// The output and input device that was lost and is waited for (by its name, unless
    /// `follows_default`): one that went away, or (`output_loss`) an output whose stream was
    /// invalidated while it stayed, or that stopped for a reason the audio system does not say.
    pub(crate) output: Option<String>,
    pub(crate) input: Option<String>,
    /// Why the output waited for was lost. `SettingsChanged` (Core Audio only): another app or
    /// the system changed its settings, which invalidated the stream; it is said to be reopened
    /// rather than back. `Unknown` (every other audio system): it is said to be reopened, with
    /// no cause. Either way it is reopened once it is listed.
    pub(crate) output_loss: OutputLoss,
    /// The output is still there but stopped working (stalled, failed, or could not be
    /// reopened): only Reconnect reopens it, so the offer stays when nothing is waited for.
    pub(crate) by_hand: bool,
    /// The same for the input. Kept until an input opens again (`input_opened`) or Reconnect,
    /// so that reopening only the output does not take the offer away.
    pub(crate) input_by_hand: bool,
    /// Whether the output and the input chosen are the system default (no device named): the
    /// one waited for is then any default device, not the device that was lost, and the reopen
    /// opens the current default.
    follows_default: (bool, bool),
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
            output_loss: OutputLoss::Left,
            by_hand: false,
            input_by_hand: false,
            follows_default: (false, false),
            steady: (0, 0),
            list: None,
            probe,
            every,
        }
    }

    /// Wait for the output (or input) device `name`, which went away, to come back (with the
    /// system default chosen, for any default device: `follows_default`).
    pub(crate) fn away(&mut self, name: String, output: bool) {
        if output {
            self.output_loss = OutputLoss::Left;
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
        self.output_loss = OutputLoss::SettingsChanged;
    }

    /// The output device `name` stopped, and the audio system does not say whether it went away
    /// or its settings changed (every system other than Core Audio): reopen it once it is listed,
    /// as before, without naming a cause.
    pub(crate) fn stopped(&mut self, name: String) {
        self.away(name, true);
        self.output_loss = OutputLoss::Unknown;
    }

    /// Stop waiting for the output device (it was reopened, or reopening it failed).
    fn output_done(&mut self) {
        self.output = None;
        self.output_loss = OutputLoss::Left;
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

    /// Whether a device is waited for (one that went away, or an output that stopped).
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
        let (system, output, input, follows) = (self.system.clone(), self.output.clone(), self.input.clone(), self.follows_default);
        let reader = std::thread::Builder::new().name("audio device watch".into()).spawn(move || {
            while tx.send(probe(system.as_deref(), wanted(&output, follows.0), wanted(&input, follows.1))).is_ok() {
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

/// Reads the names of the system default output and input of an audio system (first argument,
/// `None` for the default one; see [`soundcraft_playback::devices::default_names`]).
pub(crate) type DefaultNames = Arc<dyn Fn(Option<&str>) -> (Option<String>, Option<String>) + Send + Sync>;

/// What a [`DefaultWatch`] watches: the audio system, and the output and input open on the
/// system default, by their names (`None`: not watched).
type Watched = (Option<String>, Option<String>, Option<String>);

/// Follows the system default where the stream opened on it does not: one pinned to the device
/// that was the default when it opened (`pinned_default`; WASAPI when its default handle cannot
/// open, and Core Audio's default input). A background thread reads the names of the current
/// defaults, never the device lists, and the app reopens on a default that moved to another
/// device.
pub(crate) struct DefaultWatch {
    /// What the background thread reads for, `None` while nothing is watched.
    watched: Option<Watched>,
    /// The readings; dropping it ends the thread at its next reading.
    readings: Option<Receiver<(Option<String>, Option<String>)>>,
    /// Readings in a row that named another default output and input than the one open.
    moved: (u32, u32),
    /// The default output and input the last reading named.
    now: (Option<String>, Option<String>),
    names: DefaultNames,
    every: Duration,
}

impl DefaultWatch {
    pub(crate) fn new() -> Self {
        Self::with_names(Arc::new(soundcraft_playback::devices::default_names), POLL)
    }

    /// Reads the default devices' names with `names` every `every`.
    pub(crate) fn with_names(names: DefaultNames, every: Duration) -> Self {
        DefaultWatch { watched: None, readings: None, moved: (0, 0), now: (None, None), names, every }
    }

    /// Stop reading the defaults.
    fn stop(&mut self) {
        self.watched = None;
        self.readings = None;
        self.moved = (0, 0);
    }

    /// Watch the system default of audio system `system` while the output and the input named
    /// (`None`: not open on a pinned default) are open on it. Returns the default output and input
    /// that replaced them, once another device was named the default in several readings in a row.
    fn moved(&mut self, system: Option<String>, output: Option<String>, input: Option<String>) -> (Option<String>, Option<String>) {
        if output.is_none() && input.is_none() {
            self.stop();
            return (None, None);
        }
        let watched = (system, output, input);
        if self.watched.as_ref() != Some(&watched) {
            self.stop();
            let (tx, rx) = channel();
            let (names, every, system) = (Arc::clone(&self.names), self.every, watched.0.clone());
            let reader = std::thread::Builder::new().name("audio default watch".into()).spawn(move || {
                while tx.send(names(system.as_deref())).is_ok() {
                    std::thread::sleep(every);
                }
            });
            match reader {
                Ok(_) => self.readings = Some(rx),
                Err(e) => log::warn!("cannot watch the system default audio device: {e}"),
            }
            self.watched = Some(watched);
        }
        let (Some(readings), Some((_, output, input))) = (&self.readings, &self.watched) else { return (None, None) };
        for (out, inp) in readings.try_iter() {
            self.moved.0 = if moved_from(output, &out) { self.moved.0 + 1 } else { 0 };
            self.moved.1 = if moved_from(input, &inp) { self.moved.1 + 1 } else { 0 };
            self.now = (out, inp);
        }
        (self.now.0.clone().filter(|_| self.moved.0 >= STEADY), self.now.1.clone().filter(|_| self.moved.1 >= STEADY))
    }
}

/// Whether the system default is another device (`default`) than the one open on it (`open`).
fn moved_from(open: &Option<String>, default: &Option<String>) -> bool {
    open.is_some() && default.is_some() && open != default
}

/// What to wait for: nothing, the device `name` that was lost, or (`default`) any system default.
fn wanted(name: &Option<String>, default: bool) -> Wanted<'_> {
    match name.as_deref() {
        None => Wanted::Nothing,
        Some(_) if default => Wanted::Default,
        Some(name) => Wanted::Named(name),
    }
}

impl SoundApp {
    /// The audio devices need reopening: from now on until they are, the status line offers
    /// Reconnect.
    pub(super) fn audio_lost(&mut self) -> &mut LostAudio {
        let system = self.ui.audio.system.clone();
        let follows = (self.ui.audio.output.is_none(), self.ui.audio.input.is_none());
        let lost = self.lost_audio.get_or_insert_with(|| LostAudio::new(system));
        lost.follows_default = follows;
        lost
    }

    /// Whether the audio devices need reopening (the status line then offers Reconnect).
    pub(crate) fn offers_reconnect(&self) -> bool {
        self.lost_audio.is_some()
    }

    /// Reopen a device that went away once it is back, or an output that stopped (settings
    /// changed, or a cause the audio system does not say) once it is listed, while the transport
    /// is stopped: the output with the rest of the audio devices, or an input alone on its next
    /// use. A device the user chose is waited for and reopened by its name; with the system
    /// default chosen, the current default is (any default device counts as back), so a default
    /// that moved to another device is followed.
    pub(super) fn watch_replug(&mut self) {
        let Some(lost) = &mut self.lost_audio else { return };
        let (output, input) = lost.back();
        if !(output || input) || self.engine.transport.playing || self.engine.transport.recording {
            return;
        }
        if output && let Some(name) = lost.output.clone() {
            // An input that is back too (usually the same interface) opens again with the output.
            if input {
                lost.input = None;
            }
            let loss = lost.output_loss;
            lost.output_done();
            let input_away = lost.input.is_some();
            // What the audio choice says: the device the user chose, by its name, or the current
            // system default when none is chosen (the default may be another device by now).
            let follows_default = self.ui.audio.output.is_none();
            let cfg = self.ui.audio.clone();
            // A device chosen by name that is gone again by now is not replaced by the default.
            let required = self.reconnect_requirement();
            self.reopen_audio(&cfg, required.as_deref());
            // An input still away opens again when it is back, not on the next record.
            self.recorder_failed = input_away;
            self.input_returned = input;
            let opened = self.player.as_ref().filter(|p| !p.silent).map(|p| p.device_name.clone());
            if opened.as_deref() == Some(name.as_str()) {
                if let Some(l) = &mut self.lost_audio {
                    l.output_reopened();
                }
                self.ui.status = match loss {
                    OutputLoss::SettingsChanged => crate::localized_format!("SoundCraft reopened “{name}” after its settings changed.", name = name),
                    OutputLoss::Unknown => crate::localized_format!("SoundCraft reopened “{name}”.", name = name),
                    OutputLoss::Left => crate::localized_format!("“{name}” is back: SoundCraft reconnected it.", name = name),
                };
            } else if let Some(now) = opened.filter(|_| follows_default) {
                if let Some(l) = &mut self.lost_audio {
                    l.output_reopened();
                }
                self.ui.status = crate::localized_format!("SoundCraft opened the system default output, “{name}”.", name = now);
            } else {
                self.ui.status = if follows_default {
                    crate::localization::text("SoundCraft could not open the system default output. Use Reconnect to try again.").to_string()
                } else {
                    match loss {
                        OutputLoss::SettingsChanged => crate::localized_format!(
                            "SoundCraft could not reopen “{name}” after its settings changed. Use Reconnect to try again.",
                            name = name
                        ),
                        OutputLoss::Unknown => {
                            crate::localized_format!("SoundCraft could not reopen “{name}”. Use Reconnect to try again.", name = name)
                        }
                        OutputLoss::Left => {
                            crate::localized_format!("“{name}” is back, but SoundCraft could not open it. Use Reconnect to try again.", name = name)
                        }
                    }
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
            // It opens on the next record or monitor (this frame, if one waits for it), as the
            // audio choice says: the device chosen by its name, else the current system default.
            self.recorder = None;
            self.recorder_failed = false;
            self.input_returned = true;
            self.ui.status = if self.ui.audio.input.is_none() {
                crate::localization::text("The system default input is available again.").to_string()
            } else {
                crate::localized_format!("“{name}” is back.", name = name)
            };
        }
        if self.lost_audio.as_ref().is_some_and(LostAudio::settled) {
            self.lost_audio = None;
        }
    }

    /// Follow the system default where the stream open on it is pinned to the device that was
    /// the default then (see [`DefaultWatch`]): with the system default chosen (no device by
    /// name), once another device is the default, reopen the output on it, or let the input open
    /// it on its next use. Like a device that comes back (`watch_replug`), nothing is reopened
    /// while the transport runs, so the default is not read then; nor while the devices need
    /// reopening for another reason (the Reconnect offer), which `watch_replug` handles.
    pub(super) fn watch_default(&mut self) {
        let running = self.engine.transport.playing || self.engine.transport.recording;
        if running || self.lost_audio.is_some() {
            self.default_watch.stop();
            return;
        }
        let output = self.player.as_ref().filter(|p| p.pinned_default && !p.silent && self.ui.audio.output.is_none()).map(|p| p.device_name.clone());
        let input = self.recorder.as_ref().filter(|r| r.pinned_default && self.ui.audio.input.is_none()).map(|r| r.device_name.clone());
        let (output, input) = self.default_watch.moved(self.ui.audio.system.clone(), output, input);
        if output.is_some() {
            self.default_watch.stop();
            // The input reopens with it, on its next use, on the current default input.
            let cfg = self.ui.audio.clone();
            self.reopen_audio(&cfg, None);
            self.ui.status = match self.player.as_ref().filter(|p| !p.silent) {
                Some(p) => crate::localized_format!("SoundCraft opened the system default output, “{name}”.", name = p.device_name),
                None => {
                    self.audio_lost().by_hand = true;
                    crate::localization::text("SoundCraft could not open the system default output. Use Reconnect to try again.").to_string()
                }
            };
        } else if let Some(name) = input {
            self.default_watch.stop();
            self.recorder = None;
            self.recorder_failed = false;
            self.ui.status = crate::localized_format!("SoundCraft follows the system default input, now “{name}”.", name = name);
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

    /// The output Reconnect must open by its name, else it runs the silent clock: the output
    /// chosen in Playback Engine, when it is a device by name. With the system default chosen
    /// (`output` is `None`), none: Reconnect opens the audio system's current default, as Apply
    /// does, which may be another device by now. What "current default" is depends on the audio
    /// system: Core Audio and WASAPI report the OS default output; cpal's ASIO host has no default
    /// and takes the first driver it lists.
    fn reconnect_requirement(&self) -> Option<String> {
        self.ui.audio.output.clone()
    }

    /// Reopen the audio devices (Reconnect audio device), keeping a take in progress, and say
    /// which output opened. An output or input chosen by name that is not to be found is not
    /// replaced by the system default: the output runs on the silent clock and the input stays
    /// closed, each waited for by its name and reopened when it is back.
    pub fn reconnect_audio(&mut self) {
        let cfg = self.ui.audio.clone();
        let required = self.reconnect_requirement();
        self.reopen_audio(&cfg, required.as_deref());
        let Some(p) = &self.player else { return };
        let opened = (!p.silent).then(|| p.device_name.clone());
        let missing = required.filter(|_| opened.is_none());
        // An input chosen by name that is still away: not opened (as the system default) on its
        // next use, but when it is back (`watch_replug`).
        let input_away = self.ui.audio.input.as_ref().and(self.lost_audio.as_ref().and_then(|l| l.input.clone()));
        self.recorder_failed = input_away.is_some();
        self.input_returned = false;
        self.ui.status = reconnect_status(missing.as_deref(), opened.as_deref(), input_away.as_deref());
        if let Some(name) = missing {
            // Not to be found: waited for by its name, and reopened once it is back.
            self.audio_lost().away(name, true);
            return;
        }
        let Some(name) = opened else { return };
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

/// What Reconnect says: that the output chosen by name (`missing`) is not to be found and the
/// silent clock runs, else which output opened (`opened`, `None`: the silent clock, with the system
/// default chosen), and that an input chosen by name (`input_away`) is still away.
fn reconnect_status(missing: Option<&str>, opened: Option<&str>, input_away: Option<&str>) -> String {
    let output = match (missing, opened) {
        (Some(name), _) => crate::localized_format!(
            "“{name}” is unavailable: SoundCraft runs on a silent clock and reconnects it when it is back, or use Reconnect.",
            name = name
        ),
        (None, Some(name)) => crate::localized_format!("Reconnected to “{name}”.", name = name),
        (None, None) => crate::localization::text("Reconnect opened no audio device: SoundCraft runs on a silent clock.").to_string(),
    };
    // The input is usually on the same interface as the output: named once.
    match input_away.filter(|input| missing != Some(*input)) {
        Some(input) => {
            let input =
                crate::localized_format!("“{name}” is unavailable: SoundCraft reconnects it when it is back, or use Reconnect.", name = input);
            format!("{output} {input}")
        }
        None => output,
    }
}

#[cfg(test)]
mod tests {
    use super::{DefaultNames, DefaultWatch, LostAudio, Probe, STEADY, Wanted};
    use std::sync::atomic::AtomicUsize;
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
                let has = |want: Wanted<'_>| match want {
                    Wanted::Nothing => true,
                    Wanted::Named(w) => listed.iter().any(|n| n == w),
                    // The first device listed is the system default.
                    Wanted::Default => !listed.is_empty(),
                };
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

    /// A SoundApp with `AWAY` chosen by name as its output and input.
    fn app_on_away(devices: &Devices) -> crate::SoundApp {
        let mut app = crate::SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.ui.audio.output = Some(AWAY.into());
        app.ui.audio.input = Some(AWAY.into());
        app.lost_audio = Some(lost(devices));
        app
    }

    /// A SoundApp whose output device `AWAY`, chosen by name, went away while stopped, waited for
    /// on `devices`.
    fn app_with_lost_output(devices: &Devices) -> crate::SoundApp {
        let mut app = app_on_away(devices);
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
        // No machine has `AWAY`: the reopen runs the silent clock rather than another device,
        // and the status says so instead of claiming success.
        assert!(app.player.as_ref().is_some_and(|p| p.silent), "the devices were reopened, on the silent clock");
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
        let mut app = app_on_away(&devices);
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

    /// An output chosen by name and lost where the audio system does not say why (WASAPI, ASIO,
    /// ALSA, ...) is reopened by its name, as before, and neither message claims a cause.
    #[test]
    fn an_output_lost_for_an_unknown_reason_is_reopened_without_naming_a_cause() {
        let devices = Devices::default();
        devices.set(&[AWAY]);
        let mut app = app_on_away(&devices);
        app.audio_lost().stopped(AWAY.into());
        let until = Instant::now() + Duration::from_secs(5);
        while app.player.is_none() && Instant::now() < until {
            app.watch_replug();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.player.is_some(), "the device was reopened");
        // No machine has `AWAY`, so the reopen does not get it back.
        assert_eq!(app.ui.status, format!("SoundCraft could not reopen “{AWAY}”. Use Reconnect to try again."));
        assert!(app.offers_reconnect());
    }

    #[test]
    fn an_input_that_comes_back_opens_again_on_its_next_use() {
        let devices = Devices::default();
        let mut app = crate::SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.ui.audio.input = Some("Mic".into());
        app.lost_audio = Some(lost(&devices));
        app.audio_lost().away("Mic".into(), false);
        app.recorder_failed = true;
        app.ui.status = "Audio input: “Mic” is unavailable.".into();
        devices.set(&["Mic"]);
        let until = Instant::now() + Duration::from_secs(5);
        while app.recorder_failed && Instant::now() < until {
            app.watch_replug();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!app.recorder_failed, "the input may open again");
        assert!(!app.offers_reconnect());
        assert_eq!(app.ui.status, "“Mic” is back.");
    }

    /// With the system default chosen for the output, a lost output is not waited for by its
    /// name: once the system has a default output again, the current default opens (seen on
    /// Windows: the default moved to another device, and the old one was reopened by its name).
    #[test]
    fn an_output_on_the_system_default_reopens_on_the_current_default() {
        let devices = Devices::default();
        devices.set(&["Another output"]);
        let mut app = crate::SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.lost_audio = Some(lost(&devices));
        app.audio_lost().stopped(AWAY.into());
        let until = Instant::now() + Duration::from_secs(5);
        while app.player.is_none() && Instant::now() < until {
            app.watch_replug();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.player.is_some(), "reopened, though `AWAY` never came back");
        assert!(!app.ui.status.contains(AWAY), "{}", app.ui.status);
        assert!(app.ui.status.contains("system default output"), "{}", app.ui.status);
        assert_eq!(app.ui.audio.output, None, "still the system default");
    }

    /// An output the user chose by name is waited for by its name, while another device is the
    /// system default.
    #[test]
    fn a_chosen_output_is_waited_for_by_its_name() {
        let devices = Devices::default();
        devices.set(&["Another output"]);
        let mut app = app_on_away(&devices);
        app.audio_lost().stopped(AWAY.into());
        for _ in 0..40 {
            app.watch_replug();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.player.is_none(), "not reopened on another device");
        assert!(app.offers_reconnect());
    }

    /// With the system default chosen for the input, a lost input is settled once the system has
    /// a default input again: it opens the current default on its next use, not the device that
    /// was lost.
    #[test]
    fn an_input_on_the_system_default_comes_back_as_the_current_default() {
        let devices = Devices::default();
        devices.set(&["Another microphone"]);
        let mut app = crate::SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.lost_audio = Some(lost(&devices));
        app.audio_lost().away("Mic".into(), false);
        app.recorder_failed = true;
        let until = Instant::now() + Duration::from_secs(5);
        while app.recorder_failed && Instant::now() < until {
            app.watch_replug();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!app.recorder_failed, "the input may open again, though “Mic” never came back");
        assert!(!app.offers_reconnect());
        assert_eq!(app.ui.status, "The system default input is available again.");
        assert_eq!(app.ui.audio.input, None, "still the system default");
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

    /// Reconnect with the output chosen by name and not to be found opens no other device in its
    /// place, whether the output was lost (went away) or only stopped working (Reconnect by
    /// hand): the silent clock runs, the status names the device, and it is waited for.
    #[test]
    fn reconnect_does_not_open_the_system_default_in_place_of_an_output_chosen_by_name() {
        for went_away in [true, false] {
            let devices = Devices::default();
            let mut app = app_on_away(&devices);
            if went_away {
                app.audio_lost().away(AWAY.into(), true);
            } else {
                app.audio_lost().by_hand = true;
            }
            app.reconnect_audio();
            let player = app.player.as_ref().expect("the devices were reopened");
            assert!(player.silent, "went away: {went_away}; opened “{}” in place of “{AWAY}”", player.device_name);
            assert_eq!(
                app.ui.status,
                format!("“{AWAY}” is unavailable: SoundCraft runs on a silent clock and reconnects it when it is back, or use Reconnect."),
                "went away: {went_away}"
            );
            assert_eq!(app.lost_audio.as_ref().and_then(|l| l.output.as_deref()), Some(AWAY), "went away: {went_away}");
            assert!(app.offers_reconnect());
        }
    }

    /// Reconnect requires the output chosen by name; with the system default chosen it opens
    /// whichever device is the default now.
    #[test]
    fn reconnect_requires_only_an_output_chosen_by_name() {
        for chosen in [None, Some("Model 12"), Some("Fireface UFX (23693403)")] {
            let mut app = crate::SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
            app.ui.audio.output = chosen.map(str::to_owned);
            assert_eq!(app.reconnect_requirement().as_deref(), chosen);
        }
    }

    /// What Reconnect says, from the output chosen by name that was not found, the output that
    /// opened, and an input chosen by name still away.
    #[test]
    fn reconnect_says_which_device_is_unavailable_by_name() {
        let unavailable = "“Model 12” is unavailable: SoundCraft runs on a silent clock and reconnects it when it is back, or use Reconnect.";
        // (missing, opened, input away, status)
        for (missing, opened, input, status) in [
            (Some("Model 12"), None, None, unavailable.to_string()),
            (Some("Model 12"), None, Some("Model 12"), unavailable.to_string()),
            (
                Some("Model 12"),
                None,
                Some("USB Mic"),
                format!("{unavailable} “USB Mic” is unavailable: SoundCraft reconnects it when it is back, or use Reconnect."),
            ),
            (None, Some("Speakers (Realtek(R) Audio)"), None, "Reconnected to “Speakers (Realtek(R) Audio)”.".to_string()),
            (
                None,
                Some("Model 12"),
                Some("USB Mic"),
                "Reconnected to “Model 12”. “USB Mic” is unavailable: SoundCraft reconnects it when it is back, or use Reconnect.".to_string(),
            ),
            (None, None, None, "Reconnect opened no audio device: SoundCraft runs on a silent clock.".to_string()),
        ] {
            assert_eq!(super::reconnect_status(missing, opened, input), status);
        }
    }

    /// Reconnect with an input chosen by name still away does not let its next use open the
    /// system default input in its place: it opens once it is back.
    #[test]
    fn reconnect_keeps_an_input_chosen_by_name_closed_until_it_is_back() {
        let devices = Devices::default();
        let mut app = app_on_away(&devices);
        app.ui.audio.output = None;
        app.audio_lost().away(AWAY.into(), false);
        app.reconnect_audio();
        assert!(app.recorder_failed, "the input is not opened on its next use while it is away");
        assert_eq!(app.lost_audio.as_ref().and_then(|l| l.input.as_deref()), Some(AWAY));
        devices.set(&[AWAY]);
        let until = Instant::now() + Duration::from_secs(5);
        while app.recorder_failed && Instant::now() < until {
            app.watch_replug();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!app.recorder_failed, "the input that came back opens on its next use");
    }

    /// An input that came back but fails to open on its next use keeps the Reconnect offer, as
    /// an output that came back and failed to reopen does: otherwise every later use is refused
    /// with nothing on the status line to retry it.
    #[test]
    fn an_input_that_came_back_but_fails_to_open_keeps_the_reconnect_offer() {
        let devices = Devices::default();
        let mut app = app_on_away(&devices);
        app.ui.audio.output = None;
        app.audio_lost().away(AWAY.into(), false);
        app.reconnect_audio();
        devices.set(&[AWAY]);
        let until = Instant::now() + Duration::from_secs(5);
        while app.recorder_failed && Instant::now() < until {
            app.watch_replug();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!app.recorder_failed, "the input that came back opens on its next use");
        // Listed again, it still fails to open (the error `ensure_input` gets).
        app.input_failed("the device is busy");
        assert!(app.recorder.is_none() && app.recorder_failed);
        assert!(app.offers_reconnect(), "the failed reopen leaves Reconnect on the status line");
        assert!(app.lost_audio.as_ref().is_some_and(|l| l.input_by_hand));
        // Opening it by hand later settles the offer (`input_opened`).
        app.input_opened(AWAY);
        assert!(!app.offers_reconnect());
    }

    /// An input that fails on its first use, with nothing lost before, is not a reopen: no
    /// Reconnect offer, which would also stop following the system default.
    #[test]
    fn an_input_that_fails_on_its_first_use_offers_no_reconnect() {
        let devices = Devices::default();
        let mut app = app_on_away(&devices);
        app.ui.audio.output = None;
        app.lost_audio = None;
        app.reconnect_audio();
        app.input_failed("no audio input device");
        assert!(app.recorder_failed);
        assert!(!app.offers_reconnect());
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

    /// System defaults the test moves, and how often they were read.
    #[derive(Clone, Default)]
    struct Defaults(Arc<Mutex<(Option<String>, Option<String>)>>, Arc<AtomicUsize>);

    impl Defaults {
        fn set(&self, output: Option<&str>, input: Option<&str>) {
            *self.0.lock().unwrap() = (output.map(str::to_owned), input.map(str::to_owned));
        }

        fn reads(&self) -> usize {
            self.1.load(Ordering::Relaxed)
        }

        fn watch(&self) -> DefaultWatch {
            let d = self.clone();
            let names: DefaultNames = Arc::new(move |_system| {
                d.1.fetch_add(1, Ordering::Relaxed);
                d.0.lock().unwrap().clone()
            });
            DefaultWatch::with_names(names, Duration::from_millis(5))
        }
    }

    /// The name of an output open on the system default and pinned to that device, which no
    /// machine has.
    const PINNED: &str = "SoundCraft test output pinned to the default";

    /// A SoundApp with the system default chosen, its output open on the default as the device
    /// `PINNED` (as WASAPI opens it when its default handle cannot open: `pinned_default`), with
    /// the defaults read from `defaults`.
    fn app_on_pinned_default(defaults: &Defaults) -> crate::SoundApp {
        let mut app = crate::SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let mut player = soundcraft_playback::Player::with_config(app.engine.session_arc(), &app.ui.audio);
        player.device_name = PINNED.into();
        player.silent = false;
        player.pinned_default = true;
        app.player = Some(player);
        app.default_watch = defaults.watch();
        defaults.set(Some(PINNED), None);
        app
    }

    /// Runs `watch_default` until the output open is no longer `PINNED`, or a while has passed.
    fn follow(app: &mut crate::SoundApp) {
        let until = Instant::now() + Duration::from_secs(5);
        while app.player.as_ref().is_some_and(|p| p.device_name == PINNED) && Instant::now() < until {
            app.watch_default();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Seen on Windows (WASAPI, Model 12): the output on the system default was reopened as the
    /// device that was the default then; when the default later moved (Model 12 plugged back in
    /// and made the default), SoundCraft stayed on the old device until Reconnect. It now reopens
    /// on the new default by itself, and says so.
    #[test]
    fn an_output_pinned_to_the_system_default_follows_the_default_when_it_moves() {
        let defaults = Defaults::default();
        let mut app = app_on_pinned_default(&defaults);
        for _ in 0..20 {
            app.watch_default();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(app.player.as_ref().map(|p| p.device_name.as_str()), Some(PINNED), "the default did not move: not reopened");
        assert!(defaults.reads() > 0, "the default is watched");
        defaults.set(Some("Model 12"), None);
        follow(&mut app);
        let player = app.player.as_ref().expect("an output is open");
        assert_ne!(player.device_name, PINNED, "reopened on the current default");
        assert!(app.ui.status.contains("system default output"), "{}", app.ui.status);
        assert_eq!(app.ui.audio.output, None, "still the system default");
    }

    /// An output chosen by name stays on it, whatever the system default is.
    #[test]
    fn an_output_chosen_by_name_does_not_follow_the_system_default() {
        let defaults = Defaults::default();
        let mut app = app_on_pinned_default(&defaults);
        app.ui.audio.output = Some(PINNED.into());
        defaults.set(Some("Model 12"), None);
        follow(&mut app);
        assert_eq!(app.player.as_ref().map(|p| p.device_name.as_str()), Some(PINNED));
        assert_eq!(defaults.reads(), 0, "nothing to watch");
    }

    /// While the transport runs, the devices are not reopened (as for a device that comes back),
    /// so the default is not read either; it is followed once the transport stops.
    #[test]
    fn the_system_default_is_not_followed_while_the_transport_runs() {
        let defaults = Defaults::default();
        let mut app = app_on_pinned_default(&defaults);
        defaults.set(Some("Model 12"), None);
        for running in [(true, false), (false, true)] {
            (app.engine.transport.playing, app.engine.transport.recording) = running;
            follow(&mut app);
            assert_eq!(app.player.as_ref().map(|p| p.device_name.as_str()), Some(PINNED), "{running:?}");
            assert_eq!(defaults.reads(), 0, "{running:?}");
        }
        (app.engine.transport.playing, app.engine.transport.recording) = (false, false);
        follow(&mut app);
        assert_ne!(app.player.as_ref().map(|p| p.device_name.as_str()), Some(PINNED), "followed once stopped");
    }

    /// A stream that follows the system default by itself (WASAPI's default handle, Core Audio's
    /// default output) is left to it: the default is not watched.
    #[test]
    fn a_stream_that_follows_the_default_itself_is_not_watched() {
        let defaults = Defaults::default();
        let mut app = app_on_pinned_default(&defaults);
        if let Some(p) = &mut app.player {
            p.pinned_default = false;
        }
        defaults.set(Some("Model 12"), None);
        follow(&mut app);
        assert_eq!(app.player.as_ref().map(|p| p.device_name.as_str()), Some(PINNED));
        assert_eq!(defaults.reads(), 0);
    }

    /// The input open on a pinned default is followed the same way: once another device is the
    /// default input in several readings in a row, not while it is the device open.
    #[test]
    fn an_input_pinned_to_the_system_default_is_told_when_the_default_moves() {
        let defaults = Defaults::default();
        let mut watch = defaults.watch();
        defaults.set(Some("Speakers"), Some("Mic A"));
        let read = |watch: &mut DefaultWatch| {
            let mut moved = (None, None);
            let until = Instant::now() + Duration::from_millis(300);
            while moved == (None, None) && Instant::now() < until {
                moved = watch.moved(None, None, Some("Mic A".into()));
                std::thread::sleep(Duration::from_millis(5));
            }
            moved
        };
        assert_eq!(read(&mut watch), (None, None), "the default input is the one open");
        defaults.set(Some("Speakers"), Some("Mic B"));
        assert_eq!(read(&mut watch), (None, Some("Mic B".into())));
        assert!(watch.moved.1 >= STEADY, "named in several readings in a row, not just one");
    }
}
