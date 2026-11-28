# MIDI input: playing and recording a keyboard

SoundCraft plays MIDI keyboards and pads through its instruments and records what is played into
MIDI clips.

**Where it works.** Linux (ALSA sequencer), macOS (CoreMIDI) and Windows (WinMM). It has been
played and recorded on Linux only; on macOS and Windows the code builds but has not been tried
with a device yet. There is no MIDI input on FreeBSD or in the web version. To play external
synths from MIDI and instrument tracks, see [midi-output.md](midi-output.md). MIDI Thru to
external gear and loop or merge recording are not there yet (see [gaps.md](gaps.md), G-02).
See [Send MIDI Beat Clock to external hardware](#send-midi-beat-clock-to-external-hardware) for clock output.

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
while it is unplugged, marked *unavailable*, the word the MIDI output selector and the audio
device messages use too.

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

## Send MIDI Beat Clock to external hardware

In **Setup › MIDI › MIDI Beat Clock**, enable the clock and check each destination that should follow session tempo. Each port has a signed offset in samples. To calibrate a device, record its beat into an audio track, measure the first transient’s delay relative to the grid, and enter that sample count with a minus sign to advance the port. A positive offset delays delivery. Port calibration is limited to ±250 ms at the session rate. When a different session rate puts a saved sample offset outside that range, playback caps its effective value and preserves the saved calibration. The audio callback limits how far a message can be advanced. For more advance, increase the audio buffer or delay the other devices. Measure several transients at different beat positions and average their sample offsets to reduce MIDI timing jitter. Settings persist on this computer across projects. Opening an old project imports its offset if the port has no saved calibration.

At the session origin SoundCraft sends Start. A locate during playback sends Stop, Song Position Pointer and Continue in sequence. The first subsequent clock aligns to the encoded sixteenth. Moving the stopped playhead sends its Song Position Pointer immediately. Clock and Continue wait until playback resumes. Position Pointer covers 0 through 16,383 sixteenths; a continuous pass can run beyond that range. The worker notices an unplugged port within about a second (it checks the ports it has open against the port list once a second) and opens it again when it is listed again. When several outputs fail, the status line and the `output.error` field of `setup.midi_sync_status` name each one, separated by semicolons. Identically named outputs appear as separate destinations (for example, DIN and DIN (2)), each with its own calibration. Existing name-only settings select the first occurrence. Once a device ID is saved, unplugging that device leaves its destination unavailable until the same ID returns, or until a port of that name returns under a new ID while no other port or saved destination has that name (see [midi-output.md](midi-output.md)).

**Preferences › MIDI › Delay Compensation for External Devices** controls the use of the audio device’s reported output lead and, while Delay Compensation is enabled, the mixer’s compensated plug-in delay for Beat Clock. A plug-in delay change during playback takes effect with the audio callback that renders the new session revision. MIDI already delivered to a device keeps its original timing. Per-port calibration applies in addition. **MIDI Notes and Controllers** does the same for the tracks' external note output (see [midi-output.md](midi-output.md)). Beat Clock and the track output share one connection per port.

Use `setup.midi_beat_clock` to read settings or apply a patch, for example `{"enabled":true,"ports":{"USB MIDI":{"enabled":true,"offset_samples":-200}}}`. Supplying `ports` replaces the active Beat Clock destinations. Omitted ports stop sending Beat Clock and retain their saved sample offsets for other sync outputs. Passing `{"ports":{}}` disables every Beat Clock destination and preserves calibrated offsets. Passing `{}` alone reads the settings. Use `{"ports":{"Old name":null}}` to remove a saved port. The desktop control channel’s `setup.midi_sync_status` and the dialog’s **Copy diagnostics** report host-side send timing, callback gaps and port offsets. USB buffering and wire latency require an external measurement.

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
