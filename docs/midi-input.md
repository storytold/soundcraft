# MIDI input: playing and recording a keyboard

SoundCraft plays MIDI keyboards and pads through its instruments and records what is played into
MIDI clips.

**Where it works.** Linux (ALSA sequencer), macOS (CoreMIDI) and Windows (WinMM). It has been
played and recorded on Linux only; on macOS and Windows the code builds but has not been tried
with a device yet. There is no MIDI input on FreeBSD or in the web version. MIDI output, MIDI
clock and loop or merge recording are not there yet (see [gaps.md](gaps.md), G-02).

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
One **Undo** removes the take.

**Setup › MIDI › MIDI Input Filter…** keeps kinds of messages (for example aftertouch) out of
recordings.

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
