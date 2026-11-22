use super::{Action, PreparedResume, State};
use crate::{Cmd, Shared, callback::AudioState, midi::MidiFeed};
use soundcraft_dsp::{Plugin, PluginInfo};
use soundcraft_mix::{INSTRUMENT_SLOT, PreparedInstance};
use soundcraft_model::soundcraft_midi::{Note, Sequence};
use soundcraft_model::{ChannelFormat, Clip, Insert, Session, TrackKind};
use soundcraft_time::{Range, SampleRate, TICKS_PER_QUARTER, TempoMap};
use std::sync::{
    Arc, Mutex,
    atomic::Ordering,
    mpsc::{Sender, channel},
};

struct Rig {
    feed: Arc<MidiFeed>,
    tx: Arc<Sender<Cmd>>,
    audio: AudioState,
}

impl Rig {
    fn new(song: Session) -> Self {
        let shared = Arc::new(Shared { position: 0.into(), playing: false.into(), speed: 1000.into(), meters: Mutex::default() });
        let (tx, rx) = channel();
        let audio = AudioState::new(rx, shared, Arc::new(song), 48_000., channel().0);
        let feed = Arc::new(MidiFeed::default());
        feed.set_clock_follow(Some(0), true, 0, false);
        tx.send(Cmd::Midi(feed.clone())).unwrap();
        Self { feed, tx: Arc::new(tx), audio }
    }

    fn continue_at(&self, spp: u8) -> i64 {
        self.feed.set_clock(0);
        self.feed.receive(0, &[0xf2, spp, 0]);
        let mut events = Vec::new();
        self.feed.clock_follow_status(&mut events);
        let pulse = i64::from(spp) * 6;
        assert_eq!(events, [Action::Locate(pulse)]);
        let origin = self.audio.session.tempo.tick_to_samples(pulse * (TICKS_PER_QUARTER / 24), self.audio.session.sample_rate);
        let revision = Arc::new(Mutex::new(true));
        let output = Arc::default();
        assert!(self.feed.arm_clock_resume(
            0,
            PreparedResume {
                revision: Arc::downgrade(&revision),
                pulse,
                origin,
                offset: 0,
                sr: self.audio.session.sample_rate,
                shared: Arc::downgrade(&self.audio.shared),
                tx: Arc::downgrade(&self.tx),
                output: Arc::downgrade(&output),
                silent: true,
            }
        ));
        self.feed.receive(0, &[0xfb, 0xf8]);
        events.clear();
        assert_eq!(self.feed.clock_follow_status(&mut events).state, State::Locked);
        assert_eq!(events, [Action::Started { pulse, at: 0, render_from: origin }]);
        origin
    }

    fn position(&self) -> i64 {
        self.audio.shared.position.load(Ordering::Acquire)
    }
    fn render(&mut self) {
        self.audio.fill(&mut [0.; 256], 2);
    }
}

#[test]
fn external_spp_and_continue_ignore_loop_playback_inside_and_outside_the_selection() {
    let selection = Range { start: 24_000, end: 32_000 };
    for spp in [5, 8] {
        // 30,000 and 48,000 samples at 120 BPM.
        let mut song = Session { sample_rate: SampleRate::HZ_48000, tempo: TempoMap::new(120., 4, 4).unwrap(), ..Default::default() };
        song.edit.selection = selection;
        song.edit.loop_playback = true;
        let mut rig = Rig::new(song);
        // Establish that the selected range really loops under local transport.
        rig.tx.send(Cmd::Play { from: selection.start, end: None, looped: Some(selection) }).unwrap();
        for _ in 0..80 {
            rig.render();
        }
        assert!((selection.start..=selection.end).contains(&rig.position()));
        rig.tx.send(Cmd::Stop).unwrap();
        rig.render();

        let origin = rig.continue_at(spp);
        let mut previous = origin;
        for block in 0..96 {
            let sample = block * 128;
            if sample / 1_000 != (sample - 128).max(0) / 1_000 {
                rig.feed.set_clock(sample * 1_000_000 / 48_000);
                rig.feed.receive(0, &[0xf8]);
            }
            rig.render();
            let current = rig.position();
            assert!(current >= previous, "SPP {spp} wrapped from {previous} to {current}");
            previous = current;
        }
        assert_eq!(rig.position(), origin + 96 * 128);
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Event {
    On(u8),
    Off(u8),
    Reset,
}

struct Keys {
    held: [bool; 128],
    events: Arc<Mutex<Vec<Event>>>,
}

impl Plugin for Keys {
    fn info(&self) -> &'static PluginInfo {
        soundcraft_dsp::plugin_info("gain").unwrap()
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {
        self.events.lock().unwrap().push(Event::Reset);
        self.held.fill(false);
    }
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn note_on(&mut self, _: usize, n: u8, _: u8) {
        self.held[usize::from(n)] = true;
        self.events.lock().unwrap().push(Event::On(n));
    }
    fn note_off(&mut self, _: usize, n: u8) {
        self.held[usize::from(n)] = false;
        self.events.lock().unwrap().push(Event::Off(n));
    }
    fn all_notes_off(&mut self) {
        // The instrument adapter owns the set of active keys, as in the hosted formats.
        for n in 0..128u8 {
            if self.held[usize::from(n)] {
                self.note_off(0, n);
            }
        }
    }
}

#[test]
fn external_stop_releases_the_entire_held_clip_chord_before_reset() {
    let mut song = Session { sample_rate: SampleRate::HZ_48000, ..Default::default() };
    let id = song.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let cid = song.new_clip_id();
    let track = song.track_mut(id).unwrap();
    let mut notes = Sequence::default();
    for pitch in [60, 64, 67] {
        notes.notes.push(Note { start: 0, length: 960, channel: 0, pitch, velocity: 100, release_velocity: 64 });
    }
    track.instrument = Some(Insert::new("clap:clock-release"));
    track.playlist_mut().unwrap().clips.push(Clip::midi(cid, "held chord", 0, 48_000, notes));
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut rig = Rig::new(song);
    rig.audio.mix.set_external_instances(true);
    rig.audio.mix.adopt(vec![PreparedInstance {
        track: id,
        slot: INSTRUMENT_SLOT,
        id: "clap:clock-release".into(),
        ch: 2,
        plugin: Box::new(Keys { held: [false; 128], events: Arc::clone(&events) }),
    }]);
    rig.continue_at(0);
    rig.render();
    assert!(events.lock().unwrap().ends_with(&[Event::On(60), Event::On(64), Event::On(67)]));
    events.lock().unwrap().clear();
    rig.feed.receive(0, &[0xfc]);
    let mut actions = Vec::new();
    assert_eq!(rig.feed.clock_follow_status(&mut actions).state, State::Stopped);
    assert_eq!(actions, [Action::Stop]);
    rig.tx.send(Cmd::Stop).unwrap();
    rig.render();
    rig.render();
    assert_eq!(*events.lock().unwrap(), [Event::Off(60), Event::Off(64), Event::Off(67), Event::Reset]);
}
