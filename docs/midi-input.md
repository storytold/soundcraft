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

## Send MIDI Beat Clock to external hardware

In **Setup › MIDI › MIDI Beat Clock**, enable the clock and check each destination that should follow session tempo. Each port has a signed offset in samples. To calibrate a device, record its beat into an audio track, measure the first transient’s delay relative to the grid, and enter that sample count with a minus sign to advance the port. A positive offset delays delivery. Port calibration is limited to ±250 ms at the session rate. When a different session rate puts a saved sample offset outside that range, playback caps its effective value and preserves the saved calibration. Available lookahead limits how early a newly started transport can send. Settings persist on this computer across projects. Opening an old project imports its offset if the port has no saved calibration.

At the session origin SoundCraft sends Start. A locate during playback sends Stop, Song Position Pointer and Continue in sequence. The first subsequent clock aligns to the encoded sixteenth. Stopped locates send no F8 clock pulses. Position Pointer covers 0 through 16,383 sixteenths; a continuous pass can run beyond that range. The worker retries unplugged ports. If names are duplicated, it opens the first matching device and reports the ambiguity.

**Preferences › MIDI › Delay Compensation for External Devices** controls the use of the audio device’s reported output lead for Beat Clock and generated timecode. Per-port calibration applies in addition. The Notes and Controllers preference is saved for external note output. SoundCraft's current MIDI-input instruments render inside the audio engine.

Use `setup.midi_beat_clock` to read settings or apply a patch, for example `{"enabled":true,"ports":{"USB MIDI":{"enabled":true,"offset_samples":-200}}}`. Supplying `ports` replaces the active Beat Clock destinations. Omitted ports stop sending Beat Clock and retain their saved sample offsets for other sync outputs. An empty object disables all Beat Clock destinations and preserves their offsets. The desktop control channel’s `setup.midi_sync_status` and the dialog’s **Copy diagnostics** report host-side send timing, callback gaps and port offsets. USB buffering and wire latency require an external measurement. Hardware verification of this revision is pending.

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


## Generate or follow MIDI Time Code

Select the **MTC Reader Port** and **MTC Generator Port** in **Setup › Peripherals › Synchronization**. Enable the reader’s input under Playback Engine. **Options › Transport Online** and the transport’s **Online** switch engage chase. The **Gen MTC** switch enables transmission on the selected output. Generator and chase are mutually exclusive.

Set the session start label, timecode rate and signed external offset (frames) under **Setup › Session**. A new project starts at 01:00:00:00; opening an existing document preserves its stored start. The external offset shifts incoming positions in the session. Output calibration is a separate, per-port sample offset on the machine.

A stopped full-frame message locates the cursor. Forward quarter frames establish lock. A corroborated stopped locate preserves the first attack, and playback then runs on the device’s sample clock at normal pitch. Persistent drift greater than one frame for one second causes a relocate. A drifting master can therefore cause an audible jump. No continuous speed correction is applied. Freewheel allows 4–40 frames of missing timecode (default 8); Jam Sync keeps the transport rolling through timecode loss. Explicit MIDI Stop stops playback. Take chase offline to record. An external sample-clock reference may be appropriate when a production requires continuous, tight synchronization.

The generator sends quarter frames while rolling and full-frame messages on locates with Gen MTC enabled. Both directions support 23.976, 24, 25, 29.97 non-drop, 29.97 drop-frame and 30 fps. Pull-down sessions interpret the 24/30 wire codes using the corresponding fractional rates. Timecode calculations use the project rate and preserve drop-frame labels.

**For control clients.** `setup.mtc` reads or sets `input`, `online`, `freewheel_frames` and `jam_sync`. `setup.mtc_output` reads or sets `port` and `enabled`; `list_ports:true` enumerates destinations. Disabling the generator retains its selected port. `setup.session` accepts `start_timecode` and `external_offset_frames`. The engine without a desktop host reports Transport Online as unsupported. Updated hardware verification is pending.
