//! Live MIDI input: events from MIDI devices, played on the instruments of record-enabled and
//! input-monitored MIDI and instrument tracks (MIDI thru).
//!
//! The audio host fills [`crate::MixEngine::midi`] before each block. Notes reach the instrument
//! at their sample offset; the sustain pedal (CC 64) holds note-offs back until it is released.
//! Other controllers, pitch bend, aftertouch and program changes go to the instrument as they
//! are (`Plugin::midi`), without their channel, like the ones in clips. When the track stops
//! playing live input, the instrument's controllers go back to what the clips say.
//!
//! Every byte comes from a device and is not trusted: data bytes are masked to 7 bits, an
//! offset past the block is pulled into it, and anything that is not a channel message is
//! ignored.
//!
//! # Who holds a note
//!
//! Instruments take notes without a channel, so a track that listens to several devices or
//! channels sends them all to one instrument. The strip still keeps the keys and the pedal of
//! every *owner* (an input port and a channel) apart: a pitch sounds until the last owner that
//! holds it lets go, and one owner's sustain pedal does not hold another's notes.
//!
//! A strip follows [`MAX_OWNERS`] owners at a time (an owner takes a place only while it holds a
//! key or its pedal). Further owners share the last place and act as one there: the first
//! release of a pitch among them ends it. That bound keeps the state a fixed size. While the
//! shared place is in use, an owner without a place of its own joins it even when another place
//! has come free, so an owner never holds keys in two places and its releases always find them.
//! Only the owners that put something into the shared place act on it: a release from a source
//! that never joined it, or the loss of a port that is not in it, leaves it alone.
//!
//! # Controllers
//!
//! The strip remembers the last value every owner gave each controller, the pitch bend, the
//! channel pressure and the program, and the order they came in. When an owner goes away (its
//! port lost input, or the track stopped listening to it) while the track keeps playing live
//! input, the instrument's controllers are set again from the clips and then from the owners
//! that are still there, oldest first: a bend left by a device that was unplugged does not
//! detune what another device plays, and a controller two devices had set goes back to the
//! value of the one that stays.
//!
//! That reset is MIDI's Reset All Controllers, which leaves the channel volume (CC 7) and the pan
//! (CC 10) as they are. So when the source that went away had set one of those two, and neither
//! the clips nor a source that is still there gives it a value, the instrument is sent the value
//! MIDI names as the default (volume 100, pan in the middle): a device that turned the volume
//! down to nothing and was unplugged does not leave the track silent. Other controllers the
//! reset leaves alone have no default to go back to, and stay.
//!
//! [`MAX_LIVE_CTRLS`] such values are kept; past that the oldest is forgotten. The strip still
//! knows which ports and channels it forgot a value of: when one of them goes away the
//! controllers are set again all the same, and the forgotten one falls back to the clips' value.
//!
//! # Lost events
//!
//! The queues between a device and the engine are bounded. When one drops events, the host sets
//! the port's bit in [`crate::MixEngine::midi_lost`] ([`midi_port_bit`]) for the next block: the
//! notes and the pedal that port holds are released there, since their note-off may be among
//! what was lost. A key that is still down stays silent until it is played again.

use soundcraft_dsp::Plugin;
use soundcraft_model::{MidiInput, Track};
use std::sync::OnceLock;

/// Most input ports one engine tells apart; later ones still reach tracks that listen to all.
pub const MAX_MIDI_PORTS: usize = 32;

/// A MIDI channel message from a device, for the next block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LiveMidi {
    /// Sample offset within the block.
    pub offset: u32,
    /// Index into [`MidiPorts`]; `u8::MAX` when the port has none.
    pub port: u8,
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
}

impl LiveMidi {
    /// MIDI channel 0..=15.
    pub fn channel(&self) -> u8 {
        self.status & 0x0F
    }
}

/// Names of the input ports, by the index events carry. Slots are written once and read without
/// locking, so the audio thread can look a name up.
#[derive(Debug)]
pub struct MidiPorts {
    names: Vec<OnceLock<String>>,
}

impl Default for MidiPorts {
    fn default() -> Self {
        MidiPorts { names: (0..MAX_MIDI_PORTS).map(|_| OnceLock::new()).collect() }
    }
}

impl MidiPorts {
    /// The index of `name`, giving it a free slot the first time; `u8::MAX` when all are taken.
    /// Not for the audio thread (allocates).
    pub fn assign(&self, name: &str) -> u8 {
        for (i, slot) in self.names.iter().enumerate() {
            if slot.get_or_init(|| name.to_string()) == name {
                return u8::try_from(i).unwrap_or(u8::MAX);
            }
        }
        u8::MAX
    }

    pub fn name(&self, port: u8) -> Option<&str> {
        self.names.get(usize::from(port)).and_then(OnceLock::get).map(String::as_str)
    }
}

/// Whether a track plays live MIDI input: record-enabled or input-monitored.
pub fn midi_thru(t: &Track) -> bool {
    !t.inactive && (t.mixer.record_arm || t.mixer.input_monitor)
}

/// The bit of `port` in [`crate::MixEngine::midi_lost`]. Ports without an index of their own
/// (`u8::MAX`) share the top bit.
pub fn midi_port_bit(port: u8) -> u64 {
    1u64 << u32::from(port).min(63)
}

/// 128 notes, one bit each.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct NoteSet([u64; 2]);

impl NoteSet {
    fn set(&mut self, note: u8, on: bool) {
        let Some(w) = self.0.get_mut(usize::from(note >> 6) & 1) else { return };
        let bit = 1u64 << (note & 63);
        if on {
            *w |= bit;
        } else {
            *w &= !bit;
        }
    }

    fn has(&self, note: u8) -> bool {
        self.0.get(usize::from(note >> 6) & 1).is_some_and(|w| w & (1u64 << (note & 63)) != 0)
    }

    fn is_empty(&self) -> bool {
        self.0 == [0, 0]
    }

    fn union(&self, other: &NoteSet) -> NoteSet {
        let ([a, b], [c, d]) = (self.0, other.0);
        NoteSet([a | c, b | d])
    }

    fn notes(self) -> impl Iterator<Item = u8> {
        (0..128u8).filter(move |n| self.has(*n))
    }
}

/// Owners (input port and channel) a strip keeps apart at a time; see the module text.
pub const MAX_OWNERS: usize = 16;

/// The keys and the pedal of one owner.
#[derive(Debug, Clone, Copy, Default)]
struct Owner {
    used: bool,
    /// Owners without a place of their own are in here too.
    shared: bool,
    port: u8,
    channel: u8,
    /// Every port ([`midi_port_bit`]) and channel (bit `n` for channel `n`) that put something
    /// in this place: one of each, unless the place is shared.
    ports: u64,
    channels: u16,
    /// Keys held down.
    down: NoteSet,
    /// Keys released while the sustain pedal was down.
    sustained: NoteSet,
    pedal: bool,
}

/// The port index a bit of [`midi_port_bit`] stands for.
fn bit_port(bit: u8) -> u8 {
    if bit >= 63 { u8::MAX } else { bit }
}

/// Whether `input` takes every pairing of the ports in `port_bits` ([`midi_port_bit`]) with the
/// channels in `channel_bits` (bit `n` for channel `n`).
fn all_accepted(port_bits: u64, channel_bits: u16, input: &MidiInput<'_>, ports: Option<&MidiPorts>) -> bool {
    (0..64u8).filter(|b| port_bits & (1u64 << b) != 0).all(|b| {
        let name = ports.and_then(|p| p.name(bit_port(b)));
        (0..16u8).filter(|c| channel_bits & (1u16 << c) != 0).all(|c| input.accepts(name, c))
    })
}

impl Owner {
    fn new(port: u8, channel: u8) -> Owner {
        let mut o = Owner { used: true, port, channel, ..Owner::default() };
        o.join(port, channel);
        o
    }

    /// `port` and `channel` keep something in this place.
    fn join(&mut self, port: u8, channel: u8) {
        self.ports |= midi_port_bit(port);
        self.channels |= 1u16 << (channel & 0x0F);
    }

    /// Whether `input` takes every owner that may hold something in this place. A shared place
    /// does not know which of its ports played on which of its channels, so each pairing counts.
    fn accepted(&self, input: &MidiInput<'_>, ports: Option<&MidiPorts>) -> bool {
        all_accepted(self.ports, self.channels, input, ports)
    }

    fn holds(&self, note: u8) -> bool {
        self.used && (self.down.has(note) || self.sustained.has(note))
    }

    fn is(&self, port: u8, channel: u8) -> bool {
        self.used && self.port == port && self.channel == channel
    }

    /// Whether `port` and `channel` may hold something in this place ([`Self::join`]).
    fn joined(&self, port: u8, channel: u8) -> bool {
        self.used && self.ports & midi_port_bit(port) != 0 && self.channels & (1u16 << (channel & 0x0F)) != 0
    }

    /// Nothing left to remember: the place is free again.
    fn settle(&mut self) {
        if self.down.is_empty() && self.sustained.is_empty() && !self.pedal {
            *self = Owner::default();
        }
    }
}

/// Controller values a strip remembers (one for each owner and controller); see the module text.
pub const MAX_LIVE_CTRLS: usize = 64;

/// The last value an owner gave a controller.
#[derive(Debug, Clone, Copy, Default)]
struct LiveCtrl {
    /// The message as it arrived: its port, and its channel in `status`. `status` 0: free.
    msg: LiveMidi,
    /// When it arrived, among the others ([`LiveNotes::ctrl_order`]).
    order: u64,
}

impl LiveCtrl {
    /// Whether this is the value `e` replaces: the same owner and the same controller (the
    /// controller number counts for control changes only).
    fn is(&self, e: &LiveMidi, data1: u8) -> bool {
        self.msg.status != 0 && self.msg.port == e.port && self.msg.status == e.status && (e.status & 0xF0 != 0xB0 || self.msg.data1 == data1)
    }
}

/// The controllers that Reset All Controllers leaves alone and that have a default in MIDI:
/// `(controller, default, bit in [`LiveNotes::defaults_owed`])`.
const KEPT_BY_RESET: [(u8, u8, u8); 2] = [(7, 100, 1), (10, 64, 2)];

/// The place of `msg` in [`KEPT_BY_RESET`], when it sets one of them.
fn kept_index(msg: &LiveMidi) -> Option<usize> {
    KEPT_BY_RESET.iter().position(|(cc, ..)| msg.status & 0xF0 == 0xB0 && *cc == msg.data1)
}

/// The bit of `msg` in [`LiveNotes::defaults_owed`], when it sets one of [`KEPT_BY_RESET`].
fn kept_by_reset(msg: &LiveMidi) -> u8 {
    if msg.status & 0xF0 != 0xB0 {
        return 0;
    }
    KEPT_BY_RESET.iter().find(|(cc, ..)| *cc == msg.data1).map_or(0, |(.., bit)| *bit)
}

/// Clip note-offs a strip can hold back in one block ([`LiveNotes::clip_off_held_back`]); past
/// that a live release in the same block ends the note where the key went up.
const MAX_CLIP_OFFS: usize = 16;

/// Per-strip state of the live notes, by owner.
#[derive(Debug)]
pub(crate) struct LiveNotes {
    owners: [Owner; MAX_OWNERS],
    /// Controllers, pitch bend, aftertouch or a program change were passed to the instrument.
    controllers: bool,
    /// The last value each owner gave each controller. See "Controllers" in the module text.
    ctrls: [LiveCtrl; MAX_LIVE_CTRLS],
    /// Counts the controller messages kept, to tell their order.
    ctrl_order: u64,
    /// The ports ([`midi_port_bit`]) and channels (bit `n` for channel `n`) of the values the
    /// full table forgot: the instrument may still have a value of theirs.
    forgotten_ports: u64,
    forgotten_channels: u16,
    /// For each of [`KEPT_BY_RESET`], the port and channel that set it last: that is the value
    /// the instrument has. The default is owed when that source goes, not when another does
    /// (one that set it earlier, or one whose value the full table forgot).
    kept_last: [Option<(u8, u8)>; 2],
    /// Note-offs of the clips that were held back in this block because a live key held the
    /// pitch: `(pitch, sample offset, where the last live owner let go of the pitch in this
    /// block)`, the last `usize::MAX` while it has not. What becomes of them is settled once the
    /// block's live events are all played ([`Self::settle_clip_offs`]).
    clip_offs: [(u8, usize, usize); MAX_CLIP_OFFS],
    clip_offs_len: usize,
    /// Which of [`KEPT_BY_RESET`] a live source that is gone had set: the next reset of the
    /// instrument's controllers does not undo that ([`Self::restore_defaults`]).
    defaults_owed: u8,
    /// Pitches the track's clips hold on the same instrument right now ([`Self::set_clip_notes`]).
    clip: NoteSet,
}

impl Default for LiveNotes {
    fn default() -> Self {
        LiveNotes {
            owners: [Owner::default(); MAX_OWNERS],
            controllers: false,
            ctrls: [LiveCtrl::default(); MAX_LIVE_CTRLS],
            ctrl_order: 0,
            forgotten_ports: 0,
            forgotten_channels: 0,
            kept_last: [None; 2],
            clip_offs: [(0, 0, usize::MAX); MAX_CLIP_OFFS],
            clip_offs_len: 0,
            defaults_owed: 0,
            clip: NoteSet::default(),
        }
    }
}

impl LiveNotes {
    /// Forget everything (the instrument was told `all_notes_off`).
    pub(crate) fn clear(&mut self) {
        // What live input did to the volume and the pan is still on the instrument.
        let owed = self.kept_set();
        *self = LiveNotes::default();
        self.defaults_owed = owed;
    }

    /// Which of [`KEPT_BY_RESET`] live input has set on the instrument, as far as is known.
    fn kept_set(&self) -> u8 {
        let last = KEPT_BY_RESET.iter().zip(&self.kept_last).filter(|(_, last)| last.is_some()).fold(0, |m, ((.., bit), _)| m | bit);
        self.ctrls.iter().filter(|c| c.msg.status != 0).fold(self.defaults_owed | last, |m, c| m | kept_by_reset(&c.msg))
    }

    /// The sources picked by `gone` (a port and a channel) went away: where one of them had set
    /// the volume or the pan last, the default of that controller is owed. True when there was
    /// one, so that the caller resets the controllers.
    fn kept_last_gone(&mut self, gone: impl Fn(u8, u8) -> bool) -> bool {
        let mut any = false;
        for ((.., bit), last) in KEPT_BY_RESET.iter().zip(self.kept_last.iter_mut()) {
            if last.is_some_and(|(port, channel)| gone(port, channel)) {
                self.defaults_owed |= bit;
                *last = None;
                any = true;
            }
        }
        any
    }

    /// A new block starts: no clip note-off is held back yet.
    pub(crate) fn begin_block(&mut self) {
        self.clip_offs_len = 0;
    }

    /// The clips' note-off for `pitch` at sample `offset` of this block was not sent, as a live
    /// key holds the pitch.
    pub(crate) fn clip_off_held_back(&mut self, pitch: u8, offset: usize) {
        if let Some(slot) = self.clip_offs.get_mut(self.clip_offs_len) {
            *slot = (pitch, offset, usize::MAX);
            self.clip_offs_len += 1;
        }
    }

    /// The last live owner let go of `note` at sample `at`: the note ends, unless a clip still
    /// holds it. When note-offs of the clips for it were held back in this block, where it ends
    /// is settled after the block's live events ([`Self::settle_clip_offs`]): the key may go
    /// down again before the clip's note would have ended.
    fn end_note(&mut self, inst: &mut dyn Plugin, at: usize, note: u8) {
        if self.sounding(note) {
            return;
        }
        let mut held_back = false;
        for entry in self.clip_offs.iter_mut().take(self.clip_offs_len).filter(|(pitch, ..)| *pitch == note) {
            entry.2 = at;
            held_back = true;
        }
        if !held_back {
            inst.note_off(at, note);
        }
    }

    /// A live key takes `note` at sample `at`. If no live owner held it since one let go
    /// earlier in the block, what happened in between is settled now: the clips' held back
    /// note-offs of that stretch are sent where they were (the clip's note ended there, and the
    /// instrument gets to release it before the new press), and when the clips held nothing by
    /// then, the note ends where the key went up. Held back note-offs that come later stay held
    /// back: the key is down again.
    fn press_settles_clip_offs(&mut self, inst: &mut dyn Plugin, at: usize, note: u8) {
        let n = self.clip_offs_len;
        let Some(released) = self.clip_offs.iter().take(n).filter(|(pitch, ..)| *pitch == note).map(|(.., r)| *r).filter(|r| *r != usize::MAX).max()
        else {
            return;
        };
        let (mut sent, mut later) = (false, false);
        for entry in self.clip_offs.iter_mut().take(n).filter(|(pitch, ..)| *pitch == note) {
            if entry.1 >= released && entry.1 < at {
                inst.note_off(entry.1, note);
                // Done with: no pitch has this number.
                entry.0 = u8::MAX;
                sent = true;
            } else {
                later |= entry.1 >= at;
                entry.2 = usize::MAX;
            }
        }
        if !sent && !later && !self.clip.has(note) {
            inst.note_off(released, note);
        }
    }

    /// The block's live events are played: send the note-offs that go with the clips' held back
    /// ones. A pitch a live key holds again, or a clip, gets none (that owner ends it). For a
    /// pitch nobody holds, the clips' note-offs from where the last key went up are sent where
    /// they were: the clip's note lasts as long as the clip says, and a gap between two clip
    /// notes stays. When they were all earlier than that, the note ends where the key went up.
    pub(crate) fn settle_clip_offs(&mut self, inst: &mut dyn Plugin) {
        let offs = self.clip_offs;
        let n = std::mem::take(&mut self.clip_offs_len);
        for (i, &(note, _, released)) in offs.iter().take(n).enumerate() {
            // Each pitch once, at its first entry; not one no live owner let go of.
            let first = offs.iter().take(i).all(|(pitch, ..)| *pitch != note);
            if !first || note == u8::MAX || released == usize::MAX || self.sounding(note) {
                continue;
            }
            // The last time a live owner let go of it in this block.
            let released =
                offs.iter().take(n).filter(|(pitch, ..)| *pitch == note).map(|(.., at)| *at).filter(|at| *at != usize::MAX).max().unwrap_or(released);
            let mut any = false;
            for &(_, off, _) in offs.iter().take(n).filter(|(pitch, off, _)| *pitch == note && *off >= released) {
                inst.note_off(off, note);
                any = true;
            }
            if !any {
                inst.note_off(released, note);
            }
        }
    }

    /// After the instrument's controllers were reset and set from the clips (`from_clips`: the
    /// controllers the clips gave a value, bit `n` for CC `n`): the volume and the pan that a
    /// live source which is gone had set go back to MIDI's defaults, unless the clips or a live
    /// source that is still there say otherwise. See "Controllers" in the module text.
    pub(crate) fn restore_defaults(&mut self, from_clips: u128, inst: &mut dyn Plugin) {
        let owed = std::mem::take(&mut self.defaults_owed);
        let live = self.ctrls.iter().filter(|c| c.msg.status != 0).fold(0, |m, c| m | kept_by_reset(&c.msg));
        for (cc, default, bit) in KEPT_BY_RESET {
            if owed & bit != 0 && live & bit == 0 && from_clips & (1u128 << cc) == 0 {
                inst.midi(0, [0xB0, cc, default]);
            }
        }
        // Where the source that set the volume or the pan last is gone and another one's value
        // is sent again ([`Self::resend_controllers`]), that one is the last setter from now on:
        // when it goes too, the default is owed.
        for ((cc, ..), last) in KEPT_BY_RESET.iter().zip(self.kept_last.iter_mut()).filter(|(_, last)| last.is_none()) {
            let newest = self.ctrls.iter().filter(|c| c.msg.status & 0xF0 == 0xB0 && c.msg.data1 == *cc).max_by_key(|c| c.order);
            *last = newest.map(|c| (c.msg.port, c.msg.channel()));
        }
    }

    /// Whether a live note may still sound, or a pedal is down.
    pub(crate) fn active(&self) -> bool {
        self.owners.iter().any(|o| o.used)
    }

    /// Whether live input left anything on the instrument: notes, a pedal, or controllers.
    pub(crate) fn touched(&self) -> bool {
        self.controllers || self.active()
    }

    /// Whether a live owner holds `note` (a key down or the pedal). The clips ask before they end
    /// a note of the same pitch: instruments release by pitch, so that note-off would silence
    /// the key that is still down.
    pub(crate) fn holds(&self, note: u8) -> bool {
        self.owners.iter().any(|o| o.holds(note))
    }

    /// The pitches the track's clips hold on the instrument, told before each block's live
    /// events. A live release of such a pitch is not sent on: the clip's note goes on sounding
    /// and ends with its own note-off.
    pub(crate) fn set_clip_notes(&mut self, held: &[u8]) {
        self.clip = NoteSet::default();
        for &note in held {
            self.clip.set(note & 0x7F, true);
        }
    }

    /// Whether anything still holds `note` on the instrument: a live owner or a clip.
    fn sounding(&self, note: u8) -> bool {
        self.clip.has(note) || self.holds(note)
    }

    /// The track's input changed: owners it no longer takes would never be heard releasing, so
    /// what they hold ends now. Owners it still takes stay. The shared place goes when the input
    /// no longer takes any one of the owners in it. True when a controller one of them set was
    /// forgotten ([`Self::forget_controllers`]).
    fn drop_unaccepted(&mut self, input: &MidiInput<'_>, ports: Option<&MidiPorts>, inst: &mut dyn Plugin) -> bool {
        if self.active() {
            self.drop_owners(inst, 0, |o| !o.accepted(input, ports));
        }
        // A source whose value the full table forgot may be among those the input dropped.
        let forgotten = self.forgotten_ports != 0 && !all_accepted(self.forgotten_ports, self.forgotten_channels, input, ports);
        let forgotten = self.kept_last_gone(|port, channel| !input.accepts(ports.and_then(|p| p.name(port)), channel)) || forgotten;
        self.forget_controllers(|c| !input.accepts(ports.and_then(|p| p.name(c.port)), c.channel())) || forgotten
    }

    /// Forget the controllers last set by the sources picked by `gone`. True when there were
    /// any: the instrument still has their values, and the caller sets its controllers again
    /// (from the clips, then [`Self::resend_controllers`]).
    fn forget_controllers(&mut self, gone: impl Fn(&LiveMidi) -> bool) -> bool {
        let mut any = false;
        for c in self.ctrls.iter_mut().filter(|c| c.msg.status != 0 && gone(&c.msg)) {
            *c = LiveCtrl::default();
            any = true;
        }
        any
    }

    /// Keep `e` (a controller, bend, channel pressure or program message with masked data
    /// `d1`, `d2`) as the last value its owner gave that controller. When the table is full
    /// the oldest value goes.
    fn keep_controller(&mut self, e: &LiveMidi, d1: u8, d2: u8) {
        if let Some(last) = kept_index(&LiveMidi { data1: d1, ..*e }).and_then(|i| self.kept_last.get_mut(i)) {
            *last = Some((e.port, e.channel()));
        }
        self.ctrl_order = self.ctrl_order.saturating_add(1);
        let order = self.ctrl_order;
        let free = self.ctrls.iter().position(|c| c.is(e, d1)).or_else(|| self.ctrls.iter().position(|c| c.msg.status == 0));
        let at = free.or_else(|| {
            // Full: the oldest value goes. Its source is remembered, so that the controllers are
            // still set again when that source goes away.
            let (i, oldest) = self.ctrls.iter().enumerate().min_by_key(|(_, c)| c.order)?;
            self.forgotten_ports |= midi_port_bit(oldest.msg.port);
            self.forgotten_channels |= 1u16 << oldest.msg.channel();
            Some(i)
        });
        if let Some(c) = at.and_then(|i| self.ctrls.get_mut(i)) {
            *c = LiveCtrl { msg: LiveMidi { offset: 0, data1: d1, data2: d2, ..*e }, order };
        }
    }

    /// Send the instrument the controller values of the live sources that are left, after its
    /// controllers were reset to what the clips say: in the order they arrived, so that a
    /// controller several sources set ends at the newest of their values.
    pub(crate) fn resend_controllers(&self, inst: &mut dyn Plugin) {
        let mut after = 0u64;
        // The table is small: take the next oldest each time instead of sorting a copy.
        while let Some(c) = self.ctrls.iter().filter(|c| c.msg.status != 0 && c.order > after).min_by_key(|c| c.order) {
            inst.midi(0, [c.msg.status & 0xF0, c.msg.data1, c.msg.data2]);
            after = c.order;
        }
    }

    /// Before the block's events are played: let go of what the ports in `lost` hold
    /// ([`Self::release_ports`]) and of the owners `t` no longer listens to. True when the
    /// instrument's controllers have to be set again, as one of those sources had set some.
    pub(crate) fn reconcile(&mut self, t: &Track, lost: u64, ports: Option<&MidiPorts>, inst: &mut dyn Plugin) -> bool {
        let lost = self.release_ports(lost, inst);
        let stale = self.drop_unaccepted(&MidiInput::of(&t.mixer.input), ports, inst) || lost;
        if stale {
            // The caller resets the instrument's controllers: no forgotten value is left on it,
            // but for the volume and the pan, which the reset does not touch (`kept_last` goes
            // on saying whose they are).
            self.forgotten_ports = 0;
            self.forgotten_channels = 0;
        }
        stale
    }

    fn shared_place(&self) -> Option<usize> {
        self.owners.iter().position(|o| o.used && o.shared)
    }

    /// The place of an owner: its own, else the shared one when the owner joined it. A source
    /// that put nothing there has nothing to release there: its note-off for a pitch another
    /// owner holds in the shared place must not end that note.
    fn find(&self, port: u8, channel: u8) -> Option<usize> {
        self.owners
            .iter()
            .position(|o| o.is(port, channel))
            .or_else(|| self.shared_place().filter(|i| self.owners.get(*i).is_some_and(|o| o.joined(port, channel))))
    }

    /// The place of an owner that has something to hold: its own; else the shared one while it
    /// is in use (the owner may hold keys there already, and must not get a second place); else
    /// a free one, or the last one, shared from then on.
    fn place(&mut self, port: u8, channel: u8) -> Option<usize> {
        if let Some(i) = self.owners.iter().position(|o| o.is(port, channel)) {
            return Some(i);
        }
        let i = match self.shared_place() {
            Some(i) => i,
            None => {
                if let Some(i) = self.owners.iter().position(|o| !o.used) {
                    *self.owners.get_mut(i)? = Owner::new(port, channel);
                    return Some(i);
                }
                self.owners.len().checked_sub(1)?
            }
        };
        let o = self.owners.get_mut(i)?;
        o.shared = true;
        o.join(port, channel);
        Some(i)
    }

    /// Let go of everything the owners picked by `which` hold; a pitch another owner still holds
    /// keeps sounding.
    fn drop_owners(&mut self, inst: &mut dyn Plugin, at: usize, which: impl Fn(&Owner) -> bool) {
        let mut freed = NoteSet::default();
        for o in self.owners.iter_mut().filter(|o| o.used && which(o)) {
            freed = freed.union(&o.down).union(&o.sustained);
            *o = Owner::default();
        }
        for note in freed.notes() {
            self.end_note(inst, at, note);
        }
    }

    /// Release what live input started, leaving notes from clips alone. True when live input
    /// also moved the instrument's controllers: the caller chases them back to the clips' values.
    pub(crate) fn release(&mut self, inst: &mut dyn Plugin) -> bool {
        self.drop_owners(inst, 0, |_| true);
        self.settle_clip_offs(inst);
        self.defaults_owed = self.kept_set();
        self.ctrls = [LiveCtrl::default(); MAX_LIVE_CTRLS];
        self.forgotten_ports = 0;
        self.forgotten_channels = 0;
        self.kept_last = [None; 2];
        std::mem::take(&mut self.controllers)
    }

    /// Events of the ports in `lost` ([`midi_port_bit`]) were dropped: release what those ports
    /// hold. The shared place goes too when one of them joined it, as it may hold their notes
    /// (it does not know whose are whose); the loss of a port that is not in it leaves the notes
    /// of the others there alone. True when a controller one of those ports set was forgotten
    /// ([`Self::forget_controllers`]).
    pub(crate) fn release_ports(&mut self, lost: u64, inst: &mut dyn Plugin) -> bool {
        if lost == 0 {
            return false;
        }
        if self.active() {
            self.drop_owners(inst, 0, |o| lost & o.ports != 0);
        }
        // The port may have set a controller whose value the full table forgot.
        let forgotten = lost & self.forgotten_ports != 0;
        let forgotten = self.kept_last_gone(|port, _| lost & midi_port_bit(port) != 0) || forgotten;
        self.forget_controllers(|c| lost & midi_port_bit(c.port) != 0) || forgotten
    }

    /// Play the block's events that `t` listens to, after [`Self::reconcile`].
    pub(crate) fn play(&mut self, t: &Track, inst: &mut dyn Plugin, events: &[LiveMidi], ports: Option<&MidiPorts>, frames: usize) {
        let input = MidiInput::of(&t.mixer.input);
        for e in events {
            let channel = e.channel();
            if !input.accepts(ports.and_then(|p| p.name(e.port)), channel) {
                continue;
            }
            let at = usize::try_from(e.offset).unwrap_or(0).min(frames.saturating_sub(1));
            let (d1, d2) = (e.data1 & 0x7F, e.data2 & 0x7F);
            match e.status & 0xF0 {
                0x90 if d2 > 0 => {
                    let Some(o) = self.place(e.port, channel).and_then(|i| self.owners.get_mut(i)) else { continue };
                    o.down.set(d1, true);
                    o.sustained.set(d1, false);
                    self.press_settles_clip_offs(inst, at, d1);
                    inst.note_on(at, d1, d2);
                }
                0x80 | 0x90 => {
                    let Some(o) = self.find(e.port, channel).and_then(|i| self.owners.get_mut(i)) else { continue };
                    if !o.down.has(d1) {
                        continue;
                    }
                    o.down.set(d1, false);
                    if o.pedal {
                        o.sustained.set(d1, true);
                        continue;
                    }
                    o.settle();
                    self.end_note(inst, at, d1);
                }
                // Sustain pedal down: the owner needs a place to keep it.
                0xB0 if d1 == 64 && d2 >= 64 => {
                    if let Some(o) = self.place(e.port, channel).and_then(|i| self.owners.get_mut(i)) {
                        o.pedal = true;
                    }
                }
                // Sustain pedal up: the notes it held end, unless another owner holds them.
                0xB0 if d1 == 64 => {
                    let Some(o) = self.find(e.port, channel).and_then(|i| self.owners.get_mut(i)) else { continue };
                    let freed = std::mem::take(&mut o.sustained);
                    o.pedal = false;
                    o.settle();
                    for note in freed.notes() {
                        self.end_note(inst, at, note);
                    }
                }
                // All Sound Off, All Notes Off: channel messages, so for this owner.
                0xB0 if d1 == 120 || d1 == 123 => {
                    if let Some(i) = self.find(e.port, channel) {
                        let shared = self.owners.get(i).is_some_and(|o| o.shared);
                        self.drop_owners(inst, at, |o| if shared { o.shared } else { o.is(e.port, channel) });
                    }
                }
                // Other controllers, aftertouch, program change and pitch bend: passed on.
                // Channel mode messages (120 and up) are not; the two that matter are above.
                status @ (0xA0 | 0xB0 | 0xC0 | 0xD0 | 0xE0) if status != 0xB0 || d1 < 120 => {
                    // Program change and channel pressure have one data byte.
                    let d2 = if matches!(status, 0xC0 | 0xD0) { 0 } else { d2 };
                    inst.midi(at, [status, d1, d2]);
                    self.controllers = true;
                    // Poly aftertouch belongs to a key and is not kept.
                    if status != 0xA0 {
                        self.keep_controller(e, d1, d2);
                    }
                }
                _ => {}
            }
        }
        self.settle_clip_offs(inst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_dsp::PluginInfo;

    /// An instrument that writes down the notes it is sent, `(on, pitch)`, and the other messages.
    #[derive(Default)]
    struct Log(Vec<(bool, u8)>, Vec<[u8; 3]>, Vec<usize>);

    impl Plugin for Log {
        fn info(&self) -> &'static PluginInfo {
            soundcraft_dsp::create("gain").unwrap().info()
        }
        fn prepare(&mut self, _: f32, _: usize, _: usize) {}
        fn reset(&mut self) {}
        fn set_param(&mut self, _: &str, _: f32) -> bool {
            false
        }
        fn param(&self, _: &str) -> Option<f32> {
            None
        }
        fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
        fn note_on(&mut self, _: usize, note: u8, _: u8) {
            self.0.push((true, note));
        }
        fn note_off(&mut self, at: usize, note: u8) {
            self.0.push((false, note));
            self.2.push(at);
        }
        fn midi(&mut self, _: usize, msg: [u8; 3]) {
            self.1.push(msg);
        }
    }

    fn ev(port: u8, status: u8, data1: u8, data2: u8) -> LiveMidi {
        LiveMidi { offset: 0, port, status, data1, data2 }
    }

    /// Plays `events` on a track that listens to everything and returns what the instrument got.
    fn play(live: &mut LiveNotes, events: &[LiveMidi]) -> Vec<(bool, u8)> {
        let mut s = soundcraft_model::Session::default();
        let id = s.add_track(soundcraft_model::TrackKind::Instrument, soundcraft_model::ChannelFormat::Stereo, None);
        let mut log = Log::default();
        live.play(s.track(id).unwrap(), &mut log, events, None, 512);
        log.0
    }

    #[test]
    fn changing_the_track_input_releases_the_owners_it_no_longer_takes() {
        let ports = MidiPorts::default();
        let (keys, pad) = (ports.assign("Keys"), ports.assign("Pad"));
        let mut s = soundcraft_model::Session::default();
        let id = s.add_track(soundcraft_model::TrackKind::Instrument, soundcraft_model::ChannelFormat::Stereo, None);
        let mut live = LiveNotes::default();
        let mut step = |s: &soundcraft_model::Session, events: &[LiveMidi]| {
            let mut log = Log::default();
            live.reconcile(s.track(id).unwrap(), 0, Some(&ports), &mut log);
            live.play(s.track(id).unwrap(), &mut log, events, Some(&ports), 512);
            log.0
        };
        // All devices: Keys holds 60 (with its pedal down, and 61 sustained), Pad holds 62 and 60.
        let held = [
            ev(keys, 0xB0, 64, 127),
            ev(keys, 0x90, 60, 100),
            ev(keys, 0x90, 61, 100),
            ev(keys, 0x80, 61, 0),
            ev(pad, 0x91, 62, 100),
            ev(pad, 0x91, 60, 100),
        ];
        assert_eq!(step(&s, &held).len(), 4);
        // The track now listens to Pad only. Keys' notes end at once, with no event at all; the
        // pitch Pad also holds stays.
        s.track_mut(id).unwrap().mixer.input = MidiInput { device: Some("Pad"), channel: None }.route();
        assert_eq!(step(&s, &[]), [(false, 61)]);
        // Keys' late releases are not for this track any more and change nothing.
        assert_eq!(step(&s, &[ev(keys, 0x80, 60, 0), ev(keys, 0xB0, 64, 0)]), []);
        // Pad on another channel: its channel-2 notes end.
        s.track_mut(id).unwrap().mixer.input = MidiInput { device: Some("Pad"), channel: Some(4) }.route();
        assert_eq!(step(&s, &[]), [(false, 60), (false, 62)]);
        // The same choice again releases nothing that is still taken.
        assert_eq!(step(&s, &[ev(pad, 0x94, 70, 100)]), [(true, 70)]);
        assert_eq!(step(&s, &[]), []);
        assert_eq!(step(&s, &[ev(pad, 0x84, 70, 0)]), [(false, 70)]);
    }

    #[test]
    fn a_live_release_leaves_a_pitch_that_a_clip_holds() {
        let mut live = LiveNotes::default();
        live.set_clip_notes(&[60]);
        assert_eq!(play(&mut live, &[ev(0, 0x90, 60, 100), ev(0, 0x90, 64, 100)]), [(true, 60), (true, 64)]);
        assert!(live.holds(60) && !live.holds(61));
        assert_eq!(play(&mut live, &[ev(0, 0x80, 60, 0), ev(0, 0x80, 64, 0)]), [(false, 64)], "the clip's note goes on");
        // The same for the pedal, All Notes Off and a port that lost its input.
        play(&mut live, &[ev(0, 0xB0, 64, 127), ev(0, 0x90, 60, 100), ev(0, 0x80, 60, 0)]);
        assert_eq!(play(&mut live, &[ev(0, 0xB0, 64, 0)]), []);
        play(&mut live, &[ev(0, 0x90, 60, 100), ev(0, 0x90, 67, 100)]);
        assert_eq!(play(&mut live, &[ev(0, 0xB0, 123, 0)]), [(false, 67)]);
        play(&mut live, &[ev(0, 0x90, 60, 100)]);
        let mut log = Log::default();
        live.release_ports(midi_port_bit(0), &mut log);
        assert!(log.0.is_empty());
        // Once the clip's note is over, a live release ends the pitch as usual.
        live.set_clip_notes(&[]);
        assert_eq!(play(&mut live, &[ev(0, 0x90, 60, 100), ev(0, 0x80, 60, 0)]), [(true, 60), (false, 60)]);
    }

    #[test]
    fn a_live_release_does_not_end_a_note_before_the_clips_held_back_note_off() {
        let at = |offset, status| LiveMidi { offset, ..ev(0, status, 60, 100) };
        let s = {
            let mut s = soundcraft_model::Session::default();
            s.add_track(soundcraft_model::TrackKind::Instrument, soundcraft_model::ChannelFormat::Stereo, None);
            s
        };
        let t = &s.tracks[0];
        // A key holds pitch 60, which a clip plays too. In this block the clip's note ends at
        // sample 400 (its note-off is held back, as the key is down) and the key goes up at
        // sample 32, or at sample 450: the note ends at the later of the two.
        for (release, ends) in [(32, 400), (450, 450)] {
            let (mut live, mut log) = (LiveNotes::default(), Log::default());
            live.set_clip_notes(&[60]);
            live.play(t, &mut log, &[at(0, 0x90)], None, 512);
            live.begin_block();
            live.clip_off_held_back(60, 400);
            live.set_clip_notes(&[]);
            live.play(t, &mut log, &[at(release, 0x80)], None, 512);
            assert_eq!((log.0.last(), log.2.as_slice()), (Some(&(false, 60)), &[ends][..]), "key up at {release}");
        }
        // What the clips' note-offs at `offs` and the live events of one block come to: the
        // offsets of the note-offs the instrument gets.
        let block = |offs: &[usize], events: &[LiveMidi]| {
            let (mut live, mut log) = (LiveNotes::default(), Log::default());
            live.set_clip_notes(&[60]);
            live.play(t, &mut log, &[at(0, 0x90)], None, 512);
            live.begin_block();
            offs.iter().for_each(|off| live.clip_off_held_back(60, *off));
            live.set_clip_notes(&[]);
            live.play(t, &mut log, events, None, 512);
            (log.2, live.holds(60))
        };
        // The key goes up and down again before the clip's note would have ended: no note-off,
        // or it would end the new press. The key going up once more ends the note there, or at
        // the clip's note-off when that is later.
        assert_eq!(block(&[400], &[at(32, 0x80), at(200, 0x90)]), (vec![], true));
        // The key goes up, the clip's note ends, and the key goes down again: the clip's
        // note-off is sent where it was, so the instrument lets go before the new press. When
        // the clip's note had ended before the key went up, the note ends where the key went up.
        assert_eq!(block(&[100], &[at(50, 0x80), at(200, 0x90)]), (vec![100], true));
        assert_eq!(block(&[20], &[at(50, 0x80), at(200, 0x90)]), (vec![50], true));
        assert_eq!(block(&[100], &[at(50, 0x80), at(200, 0x90), at(300, 0x80)]), (vec![100, 300], false));
        assert_eq!(block(&[400], &[at(32, 0x80), at(200, 0x90), at(300, 0x80)]), (vec![400], false));
        assert_eq!(block(&[400], &[at(32, 0x80), at(200, 0x90), at(450, 0x80)]), (vec![450], false));
        // Two clip notes of the pitch end in the block, at 100 and at 400 (the second started in
        // between). The key goes up at 300: the second clip note lasts to its end. The key goes
        // up at 50: both clip note-offs are sent, so the gap between the two notes stays.
        assert_eq!(block(&[100, 400], &[at(300, 0x80)]), (vec![400], false));
        assert_eq!(block(&[100, 400], &[at(50, 0x80)]), (vec![100, 400], false));
        // Nothing is carried into the next block: a release there ends the note where it is.
        let (mut live, mut log) = (LiveNotes::default(), Log::default());
        live.play(t, &mut log, &[at(0, 0x90)], None, 512);
        live.clip_off_held_back(60, 400);
        live.begin_block();
        live.play(t, &mut log, &[at(32, 0x80)], None, 512);
        assert_eq!(log.2, [32]);
    }

    #[test]
    fn a_pitch_held_by_two_owners_sounds_until_both_release_it() {
        // Two devices, and two channels of one device, on a track that takes all of them.
        for (a, b) in [(ev(0, 0x90, 60, 100), ev(1, 0x90, 60, 90)), (ev(0, 0x90, 60, 100), ev(0, 0x91, 60, 90))] {
            let off = |e: LiveMidi| LiveMidi { status: 0x80 | (e.status & 0x0F), ..e };
            let mut live = LiveNotes::default();
            assert_eq!(play(&mut live, &[a, b]), [(true, 60), (true, 60)]);
            assert_eq!(play(&mut live, &[off(a)]), [], "the other owner still holds the key");
            assert!(live.active());
            assert_eq!(play(&mut live, &[off(a)]), [], "a second release by the same owner changes nothing");
            assert_eq!(play(&mut live, &[off(b)]), [(false, 60)]);
            assert!(!live.active());
        }
    }

    #[test]
    fn each_owner_has_its_own_sustain_pedal() {
        let mut live = LiveNotes::default();
        // Port 0 has its pedal down; port 1 does not.
        assert_eq!(play(&mut live, &[ev(0, 0xB0, 64, 127), ev(0, 0x90, 60, 100), ev(1, 0x90, 64, 100)]), [(true, 60), (true, 64)]);
        assert_eq!(play(&mut live, &[ev(1, 0x80, 64, 0)]), [(false, 64)], "another owner's pedal does not hold this note");
        assert_eq!(play(&mut live, &[ev(0, 0x80, 60, 0)]), [], "its own pedal does");
        assert_eq!(play(&mut live, &[ev(1, 0xB0, 64, 127), ev(1, 0xB0, 64, 0)]), [], "and another owner's pedal up does not end it");
        assert_eq!(play(&mut live, &[ev(0, 0xB0, 64, 0)]), [(false, 60)]);
        assert!(!live.active());
        // A sustained pitch another owner holds down stays until that key goes up.
        assert_eq!(
            play(&mut live, &[ev(0, 0xB0, 64, 127), ev(0, 0x90, 60, 100), ev(0, 0x80, 60, 0), ev(1, 0x90, 60, 100)]),
            [(true, 60), (true, 60)]
        );
        assert_eq!(play(&mut live, &[ev(0, 0xB0, 64, 0)]), []);
        assert_eq!(play(&mut live, &[ev(1, 0x80, 60, 0)]), [(false, 60)]);
    }

    #[test]
    fn all_notes_off_and_lost_events_release_one_owner_only() {
        let mut live = LiveNotes::default();
        play(&mut live, &[ev(0, 0x90, 60, 100), ev(0, 0x90, 62, 100), ev(1, 0x90, 62, 100), ev(1, 0x90, 67, 100), ev(1, 0xB0, 64, 127)]);
        // All Notes Off on port 0, channel 1: 62 is still held by port 1.
        assert_eq!(play(&mut live, &[ev(0, 0xB0, 123, 0)]), [(false, 60)]);
        // Port 1 lost events: its keys and its pedal go, whatever was dropped.
        let mut log = Log::default();
        live.release_ports(midi_port_bit(0), &mut log);
        assert!(log.0.is_empty() && live.active(), "port 0 holds nothing");
        live.release_ports(midi_port_bit(1), &mut log);
        assert_eq!(log.0, [(false, 62), (false, 67)]);
        assert!(!live.active());
        // A port without an index is covered by the top bit.
        play(&mut live, &[ev(u8::MAX, 0x90, 40, 100)]);
        live.release_ports(midi_port_bit(u8::MAX), &mut log);
        assert!(!live.active());
    }

    #[test]
    fn owners_beyond_the_table_share_one_place_and_never_leave_a_note_on() {
        let mut live = LiveNotes::default();
        // 16 channels of port 0 fill the table; port 1 then shares the last place.
        let ons: Vec<LiveMidi> = (0..16u8).map(|ch| ev(0, 0x90 | ch, 60 + ch, 100)).chain([ev(1, 0x90, 100, 100), ev(2, 0x90, 100, 100)]).collect();
        assert_eq!(play(&mut live, &ons).len(), 18);
        // In the shared place the first release ends the pitch (the documented bound).
        assert_eq!(play(&mut live, &[ev(1, 0x80, 100, 0)]), [(false, 100)]);
        assert_eq!(play(&mut live, &[ev(2, 0x80, 100, 0)]), []);
        let offs: Vec<LiveMidi> = (0..16u8).map(|ch| ev(0, 0x80 | ch, 60 + ch, 0)).collect();
        assert_eq!(play(&mut live, &offs).len(), 16);
        assert!(!live.active(), "every place is free again");
        // Places are reused: a new owner gets its own.
        assert_eq!(play(&mut live, &[ev(5, 0x90, 50, 1), ev(6, 0x90, 50, 1), ev(5, 0x80, 50, 0)]), [(true, 50), (true, 50)]);
    }

    #[test]
    fn a_source_that_never_joined_the_shared_place_does_not_act_on_it() {
        let mut live = LiveNotes::default();
        // 16 channels of port 0 fill the table; ports 1 and 2 share the last place.
        let ons: Vec<LiveMidi> = (0..16u8).map(|ch| ev(0, 0x90 | ch, 60 + ch, 100)).chain([ev(1, 0x90, 100, 100), ev(2, 0x90, 101, 100)]).collect();
        assert_eq!(play(&mut live, &ons).len(), 18);
        // Port 7 holds nothing here (its key went down before the track listened): its note-off,
        // its pedal-up and its All Notes Off are not for the notes of ports 1 and 2.
        assert_eq!(play(&mut live, &[ev(7, 0x80, 100, 0), ev(7, 0xB0, 64, 0), ev(7, 0xB0, 123, 0)]), []);
        // Neither is another channel of port 1.
        assert_eq!(play(&mut live, &[ev(1, 0x83, 100, 0)]), []);
        assert!(live.holds(100) && live.holds(101));
        // Port 7 loses its input: the shared place, which it is not in, keeps its notes.
        let mut log = Log::default();
        assert!(!live.release_ports(midi_port_bit(7), &mut log));
        assert!(log.0.is_empty() && live.holds(100) && live.holds(101), "{:?}", log.0);
        // Port 1 is in it: the place goes, with what the others hold there (it does not know
        // whose is whose; the place was channel 16 of port 0 before it was shared).
        live.release_ports(midi_port_bit(1), &mut log);
        assert_eq!(log.0, [(false, 75), (false, 100), (false, 101)]);
        assert!(live.holds(60), "port 0 keeps its own places");
        // The owners that did join still release there.
        play(&mut live, &[ev(1, 0x90, 100, 100)]);
        assert_eq!(play(&mut live, &[ev(1, 0x80, 100, 0)]), [(false, 100)]);
    }

    #[test]
    fn an_owner_in_the_shared_place_gets_no_second_place_and_its_releases_are_found() {
        let mut live = LiveNotes::default();
        // Port 0 holds one key on each of its 16 channels: the table is full.
        let ons: Vec<LiveMidi> = (0..16u8).map(|ch| ev(0, 0x90 | ch, 60 + ch, 100)).collect();
        assert_eq!(play(&mut live, &ons).len(), 16);
        // Port 1 presses 100: the shared place. Port 0 then frees a place, and port 1 presses 101.
        assert_eq!(play(&mut live, &[ev(1, 0x90, 100, 100), ev(0, 0x80, 60, 0), ev(1, 0x90, 101, 100)]), [(true, 100), (false, 60), (true, 101)]);
        // Both of port 1's keys end with their releases.
        assert_eq!(play(&mut live, &[ev(1, 0x80, 100, 0)]), [(false, 100)]);
        assert_eq!(play(&mut live, &[ev(1, 0x80, 101, 0)]), [(false, 101)]);
        // The same for the pedal: down in the shared place, a place comes free, up.
        assert_eq!(play(&mut live, &[ev(0, 0x90, 60, 100), ev(1, 0xB0, 64, 127), ev(1, 0x90, 100, 100), ev(1, 0x80, 100, 0)]).len(), 2);
        // (Port 0's new key is in the shared place as well, which is in use: that pedal holds it.)
        assert_eq!(play(&mut live, &[ev(0, 0x80, 60, 0), ev(1, 0x90, 101, 100), ev(1, 0x80, 101, 0)]), [(true, 101)]);
        assert_eq!(play(&mut live, &[ev(1, 0xB0, 64, 0)]), [(false, 60), (false, 100), (false, 101)]);
        // Once nothing is shared any more, a new owner gets a place of its own again.
        let offs: Vec<LiveMidi> = (1..16u8).map(|ch| ev(0, 0x80 | ch, 60 + ch, 0)).collect();
        assert_eq!(play(&mut live, &offs).len(), 15);
        assert!(!live.active());
        assert_eq!(play(&mut live, &[ev(1, 0x90, 50, 1), ev(2, 0x90, 50, 1), ev(1, 0x80, 50, 0)]), [(true, 50), (true, 50)]);
    }

    #[test]
    fn an_input_change_releases_the_shared_place_when_it_drops_any_owner_in_it() {
        let ports = MidiPorts::default();
        let (keys, pad, wind) = (ports.assign("Keys"), ports.assign("Pad"), ports.assign("Wind"));
        let mut s = soundcraft_model::Session::default();
        let id = s.add_track(soundcraft_model::TrackKind::Instrument, soundcraft_model::ChannelFormat::Stereo, None);
        let mut live = LiveNotes::default();
        let mut step = |s: &soundcraft_model::Session, events: &[LiveMidi]| {
            let mut log = Log::default();
            live.reconcile(s.track(id).unwrap(), 0, Some(&ports), &mut log);
            live.play(s.track(id).unwrap(), &mut log, events, Some(&ports), 512);
            log.0
        };
        // Keys fills the table; the last place is its channel 16, and Pad then Wind share it.
        let ons: Vec<LiveMidi> =
            (0..16u8).map(|ch| ev(keys, 0x90 | ch, 40 + ch, 100)).chain([ev(pad, 0x9F, 100, 100), ev(wind, 0x9F, 101, 100)]).collect();
        assert_eq!(step(&s, &ons).len(), 18);
        // The same input again: the shared place stays.
        assert_eq!(step(&s, &[]), []);
        // Keys only, channel 16: the place's first owner is still taken, Pad and Wind are not.
        // Their releases would no longer be heard, so everything in the shared place ends now.
        s.track_mut(id).unwrap().mixer.input = MidiInput { device: Some("Keys"), channel: Some(15) }.route();
        let mut ended = step(&s, &[]);
        ended.sort_unstable();
        let mut want: Vec<(bool, u8)> = (40..56u8).chain([100, 101]).map(|n| (false, n)).collect();
        want.sort_unstable();
        assert_eq!(ended, want);
        assert!(!live.active());
    }

    #[test]
    fn controllers_of_a_source_that_went_away_are_forgotten_and_the_others_kept() {
        let ports = MidiPorts::default();
        let (keys, pad) = (ports.assign("Keys"), ports.assign("Pad"));
        let mut s = soundcraft_model::Session::default();
        let id = s.add_track(soundcraft_model::TrackKind::Instrument, soundcraft_model::ChannelFormat::Stereo, None);
        let mut live = LiveNotes::default();
        let mut log = Log::default();
        // Keys bends and moves the mod wheel; Pad then takes the mod wheel over and sets the volume.
        let events = [ev(keys, 0xE0, 0, 0x7F), ev(keys, 0xB0, 1, 20), ev(pad, 0xB1, 1, 90), ev(pad, 0xB1, 7, 100), ev(pad, 0xA1, 60, 9)];
        live.play(s.track(id).unwrap(), &mut log, &events, Some(&ports), 512);
        assert_eq!(log.1.len(), 5);
        // Pad lost input: Keys' bend and its mod wheel value are left. The strip resets the
        // instrument's controllers and sends those again.
        assert!(!live.reconcile(s.track(id).unwrap(), 0, Some(&ports), &mut log), "nothing went away");
        assert!(live.reconcile(s.track(id).unwrap(), midi_port_bit(pad), Some(&ports), &mut log));
        let mut log = Log::default();
        live.resend_controllers(&mut log);
        assert_eq!(log.1, [[0xE0, 0, 0x7F], [0xB0, 1, 20]], "the wheel goes back to the value Keys gave it");
        assert!(!live.reconcile(s.track(id).unwrap(), midi_port_bit(pad), Some(&ports), &mut log), "once");
        // Both set the mod wheel, Pad last. When Pad goes, the wheel is Keys' again; while
        // both are there, what is sent again ends at the newer value.
        let mut log = Log::default();
        live.play(s.track(id).unwrap(), &mut log, &[ev(keys, 0xB0, 1, 20), ev(pad, 0xB1, 1, 90)], Some(&ports), 512);
        let mut log = Log::default();
        live.resend_controllers(&mut log);
        assert_eq!(log.1, [[0xE0, 0, 0x7F], [0xB0, 1, 20], [0xB0, 1, 90]], "oldest first");
        assert!(live.reconcile(s.track(id).unwrap(), midi_port_bit(pad), Some(&ports), &mut log));
        let mut log = Log::default();
        live.resend_controllers(&mut log);
        assert_eq!(log.1, [[0xE0, 0, 0x7F], [0xB0, 1, 20]], "the value of the source that stays");
        // The track stops listening to Keys: its bend and its wheel go too.
        s.track_mut(id).unwrap().mixer.input = MidiInput { device: Some("Pad"), channel: None }.route();
        assert!(live.reconcile(s.track(id).unwrap(), 0, Some(&ports), &mut log));
        let mut log = Log::default();
        live.resend_controllers(&mut log);
        assert!(log.1.is_empty());
    }

    #[test]
    fn more_controller_values_than_the_table_holds_forget_the_oldest() {
        let mut live = LiveNotes::default();
        let mut log = Log::default();
        let mut s = soundcraft_model::Session::default();
        let id = s.add_track(soundcraft_model::TrackKind::Instrument, soundcraft_model::ChannelFormat::Stereo, None);
        // One more controller than fits (64, the sustain pedal, is not one of them: the host
        // handles it), then one of them again: it is kept once.
        let mut events: Vec<LiveMidi> = (0..=MAX_LIVE_CTRLS + 1).map(|n| ev(0, 0xB0, n as u8, 1)).collect();
        events.push(ev(0, 0xB0, 5, 99));
        live.play(s.track(id).unwrap(), &mut log, &events, None, 512);
        let mut log = Log::default();
        live.resend_controllers(&mut log);
        assert_eq!(log.1.len(), MAX_LIVE_CTRLS);
        assert!(!log.1.contains(&[0xB0, 0, 1]), "the oldest value went");
        assert_eq!(log.1.last(), Some(&[0xB0, 5, 99]), "the newest is sent last, and replaced its earlier value");
        assert!(!log.1.contains(&[0xB0, 5, 1]));
    }

    #[test]
    fn a_source_whose_controller_value_was_forgotten_still_has_the_controllers_reset_when_it_goes() {
        let ports = MidiPorts::default();
        let (keys, pad) = (ports.assign("Keys"), ports.assign("Pad"));
        let mut s = soundcraft_model::Session::default();
        let id = s.add_track(soundcraft_model::TrackKind::Instrument, soundcraft_model::ChannelFormat::Stereo, None);
        // Keys bends the pitch; Pad then sets enough controllers to push that value out.
        let filled = |live: &mut LiveNotes| {
            let mut events = vec![ev(keys, 0xE0, 0, 0x7F)];
            events.extend((0..MAX_LIVE_CTRLS).map(|n| ev(pad, 0xB1, n as u8, 1)));
            live.play(s.track(id).unwrap(), &mut Log::default(), &events, Some(&ports), 512);
        };
        let mut live = LiveNotes::default();
        filled(&mut live);
        let mut log = Log::default();
        live.resend_controllers(&mut log);
        assert!(!log.1.iter().any(|m| m[0] == 0xE0), "the bend is no longer in the table");
        // Keys is unplugged: the instrument still has its bend, so the controllers are reset.
        assert!(!live.reconcile(s.track(id).unwrap(), midi_port_bit(7), Some(&ports), &mut log), "another port: nothing to do");
        assert!(live.reconcile(s.track(id).unwrap(), midi_port_bit(keys), Some(&ports), &mut log));
        assert!(!live.reconcile(s.track(id).unwrap(), midi_port_bit(keys), Some(&ports), &mut log), "once");
        // The same when the track stops listening to Keys.
        let mut live = LiveNotes::default();
        filled(&mut live);
        let mut pad_only = s.clone();
        pad_only.track_mut(id).unwrap().mixer.input = MidiInput { device: Some("Pad"), channel: None }.route();
        assert!(!live.reconcile(s.track(id).unwrap(), 0, Some(&ports), &mut log), "the input still takes Keys");
        assert!(live.reconcile(pad_only.track(id).unwrap(), 0, Some(&ports), &mut log));
        assert!(!live.reconcile(pad_only.track(id).unwrap(), 0, Some(&ports), &mut log), "once");
    }

    #[test]
    fn ports_keep_their_index_and_run_out_gracefully() {
        let p = MidiPorts::default();
        assert_eq!((p.assign("Keys"), p.assign("Pad"), p.assign("Keys")), (0, 1, 0));
        assert_eq!((p.name(1), p.name(2), p.name(u8::MAX)), (Some("Pad"), None, None));
        for i in 2..MAX_MIDI_PORTS {
            p.assign(&format!("port {i}"));
        }
        assert_eq!(p.assign("one too many"), u8::MAX);
    }

    #[test]
    fn note_set_covers_all_128_notes() {
        let mut s = NoteSet::default();
        for n in [0u8, 63, 64, 127] {
            assert!(!s.has(n));
            s.set(n, true);
            assert!(s.has(n));
        }
        s.set(64, false);
        assert!(!s.has(64) && s.has(63) && !s.is_empty());
    }
}
