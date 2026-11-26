# MIDI input: playing and recording a keyboard

SoundCraft plays MIDI keyboards and pads through its instruments and records what is played into
MIDI clips.

**Where it works.** Linux (ALSA sequencer), macOS (CoreMIDI) and Windows (WinMM). It has been
played and recorded on Linux only; on macOS and Windows the code builds but has not been tried
with a device yet. There is no MIDI input on FreeBSD or in the web version. MIDI output and MIDI
clock are not there yet (see [gaps.md](gaps.md), G-02). Loop and merge recording: section 5.

## 1. Switch a device on

Plug the device in. SoundCraft opens every MIDI input it finds, and looks for new ones every two
seconds, so there is usually nothing to do.

To see the devices, or to switch one off, open **Setup › Playback Engine…** and go to the
**MIDI inputs** section at the bottom. **Setup › MIDI › MIDI Input Devices…** opens the same
window.

- Each device has a checkbox with its name. Untick it and SoundCraft stops listening to that device.
- Next to it is the state: *connected*, *off*, or *could not be opened*.
- **Test** shows the last message that arrived from the device, to check that a keyboard gets
  through. Press a key and it reads, for example, *note C3 on, velocity 100, channel 1*.

On Windows many MIDI drivers let only one program use a device at a time. While SoundCraft has
a device open, another program may not be able to use it: untick the device here to give it up.

The switches are app preferences: they are kept on this computer, for every session, and a
device that was switched off stays off the next time it is plugged in. They are not part of the
session file.

## 2. Choose what a track listens to

A MIDI or instrument track listens to **all devices on all channels** until told otherwise; its
input selector reads *All MIDI*.

Click the track's input selector (in the Mix window, or the I/O column of the Edit window) to
choose one device, one channel (1–16), or both. A device the track listens to stays in the list
while it is unplugged, marked *not connected*.

Scripts and agents set the same thing with the `track.input` command. The input is written
`MIDI:<channel>:<device>`, with `*` for all:

| Input | The track listens to |
|---|---|
| `none` or `MIDI:*:*` | every device, every channel |
| `MIDI:10:*` | channel 10 of every device |
| `MIDI:*:Launchkey MK3 MIDI 1` | every channel of that device |
| `MIDI:1:Launchkey MK3 MIDI 1` | channel 1 of that device |

```json
{"command": "track.input", "params": {"tracks": [3], "input": "MIDI:10:Drum Pad"}}
```

The device name is the one shown in Setup › Playback Engine. A channel outside 1–16 means every
channel.

## 3. Play

Record-enable the track (its **R** button) or switch its input monitor on. The keyboard now plays
the track's instrument, with the transport stopped or running.

- Notes, the sustain pedal, the other controllers, pitch bend, aftertouch and program changes
  reach the instrument.
- Several devices or channels can play one track. A note held on two of them sounds until both
  let go, and each has its own sustain pedal.
- Taking the track out of record, or switching its monitor off, ends the notes that are held.

## 4. Record

Record-enable one or more MIDI or instrument tracks and record as usual (**Record**, then
**Play**). No audio input is needed for a pass that records MIDI only.

When the pass stops, each track that was record-enabled and active when it started gets a MIDI
clip with what its input received: notes with their velocities, controllers, pitch bend, aftertouch and
program changes. What a pass records is settled when it starts: a track put in record while it
runs is recorded from the next pass, and one taken out of record keeps what it recorded. A track
that received nothing gets no clip. The clip covers the pass, the punch range, or the loop.
One **Undo** removes the take: **Edit › Undo MIDI Recording**, as in Pro Tools (Reference Guide
p.820). A pass that also recorded audio is one **Undo Record** step for both.

**Setup › MIDI › MIDI Input Filter…** keeps kinds of messages (for example aftertouch) out of
recordings.

## 5. Loop and merge recording

What a pass does with the MIDI already on a track depends on the record mode, as in Pro Tools
(Reference Guide p.773–774, p.813–814, p.819–826).

| Mode | What the pass does |
|---|---|
| **MIDI Merge** off (Replace) | What was played replaces what the pass covers: one clip over the punch range or the pass. |
| **MIDI Merge** on | What was played is merged into the MIDI clips already there. They keep their place and length; new clips are made only for the parts of the range no clip covers. A note that runs on past the end of its clip (for example from a new clip into the next existing one) is stored whole, but playback and MIDI export end every note at its clip's end, so its tail is not heard yet; Pro Tools allows notes to extend past a clip boundary (p.819), which is a separate change. |
| **Loop Playback** on, Loop Record off, only MIDI tracks record-enabled | Record (from stop) loops the selection, after the pre-roll. With MIDI Merge each round is merged into what the rounds before it recorded, so you can play the hi-hats in one round and the kick in the next. Without it the last round in which you played something replaces the others; a round in which nothing was played changes nothing, and a round cut short by Stop (the first one too, when you stop before the loop goes round) replaces only up to where it stopped. |
| **Loop Playback** on with an audio track record-enabled | The pass runs in a line, as audio recording does. |
| Record pressed while a loop plays (punch-in) | The pass records as a normal pass over the loop, merged or replacing as **MIDI Merge** says, even with Loop Record on ("Looping playback does not enable loop recording", p.753). Its first round starts at the punch-in point, so with Replace that round leaves what lies before the punch-in point; a later round covers the whole loop and, with Replace, may replace it. |
| **Loop Record** | Every round of the loop in which a track received new material (a key pressed or a controller moved) is a take of its own, the length of the loop. The first replaces what the loop covers on the track's playlist; each further one goes on a new playlist named after the track (*Keys.02*, *Keys.03*…), which becomes the track's. Earlier takes stay on their playlists. MIDI Merge has no effect, and its button in the Transport window is dimmed. A last round cut short by Stop is kept when something was played in it. |

- While **Options › Loop Record** is on, the Record buttons (Edit window toolbar and Transport
  window) show a loop symbol and their tooltip says *Loop Record mode* (p.771, p.826): when
  stopped, and during a pass started in Loop Record mode. While playing, and during a punch-in
  pass, they show the normal mode and **MIDI Merge** is not dimmed.
- The **MIDI Merge** button's tooltip says what Merge and Replace do and that a right-click
  chooses the release mode.
- **MIDI Merge** can be switched during a pass. Each round (the whole pass, when it does not
  loop) follows the button as it is at the end of that round.
- A key held over the loop point belongs to the round it was pressed in. In Loop Record it also
  sounds from the start of the next round's take, but a round with only that held key and
  nothing new played makes no take.
- A note played over a note of the same key and channel already on the track ends the earlier
  of the two where the later starts.
- **Edit-locked clips** are not written into: what is played over one goes into a new clip laid
  over it.

**MIDI Merge Release Mode** decides how controllers recorded with MIDI Merge replace the values
already there. It applies to control changes (the sustain pedal, the mod wheel…), pitch bend and
channel aftertouch; program changes and polyphonic aftertouch are merged as they are. Set it in
**Setup › Preferences › MIDI**, or right-click the **MIDI Merge** button in the Transport window
(View › Transport › MIDI Controls), also while it is dimmed in Loop Record. It is kept on this
computer, not in the session. The mode set when a pass stops applies to the whole pass, also to
the rounds recorded before it was changed.

| Release mode | Old values of a controller you move give way… |
|---|---|
| **Latch Until Next Event** (default) | from its first to its last new value; after that the old values play on. |
| **Latch** | from its first new value until the pass stops: the last value holds. In a loop it goes on over the later rounds. |

One **Undo** (**Undo MIDI Recording**) takes back the whole pass: every take, playlist and
merged note.

## If something is lost

The path from a device to the engine has a fixed size. If a device sends more than it holds
(thousands of messages in a few milliseconds), SoundCraft drops the excess and then ends every
note that device is holding, so nothing is left sounding. Press the keys again to go on. The
same happens when a device is unplugged with keys down.

If that happens during a record pass, the status line says the take is incomplete, and the notes
that were held end at that point in the clip. The status line also says so when the audio engine
did not answer at the end of the pass (an audio device that stopped): the last notes played may
then be missing, and a new pass that records MIDI does not start until the audio engine answers. The MIDI inputs section of Setup › Playback Engine
shows how many messages were dropped.

## For agents

`ui.inspect` reports `midi_inputs` (name, enabled, open), `midi_events` (messages received so far)
and `midi_events_dropped`. See [control-protocol.md](control-protocol.md).

The record modes are commands: `options.midi_merge {value?: bool}`, `options.loop_record
{value?: bool}` and `options.loop_playback {value?: bool}` (stored in the session), and
`ui.midi_merge_release {mode?: "latch" | "latch_until_next_event"}` (an app preference; without
`mode` it reports the current one).
