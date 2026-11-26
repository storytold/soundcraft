# MIDI output: playing external synths and sound modules

MIDI and instrument tracks can play their MIDI clips to a hardware synth, a sound module or
another program through a MIDI output port.

**Where it works.** Linux (ALSA sequencer), macOS (CoreMIDI) and Windows (WinMM). It has been
played on macOS, to a hardware synth through a USB MIDI interface and to the IAC bus; on Linux
and Windows the code builds but has not been tried with a device yet. There is no MIDI output on
FreeBSD or in the web version: the selector says so.

## 1. Choose an output

Each MIDI and instrument track has a **MIDI output** selector:

- in the Mix window, at the bottom of the I/O section;
- in the Edit window's I/O column (**View › Edit Window Views › I/O**), under the audio output.

Click it and choose a port, then a channel from 1 to 16. **No MIDI output** stops the track
sending. A new track sends nothing until you choose.

- Everything the track plays goes out on the chosen channel, whatever channel it was recorded on.
- A channel that another track already plays to on the same port is marked **in use**.
- Ports with the same name get numbered names, such as "DIN" and "DIN (2)". SoundCraft remembers
  which physical port each name stood for, so the numbers stay with the same ports after a restart.
- A port that is unplugged stays chosen and reads **(not connected)** on the selector; the status
  line names it. When it comes back, SoundCraft reconnects within a second or two and sends All
  Notes Off on the channels in use, in case the device still holds notes it never heard end.

Choosing an output does not change the track's instrument plug-in or audio output. A MIDI track
with an instrument and an external output plays both, the same notes on each.

For an external synth whose audio comes back into the interface, use an instrument track: set its
MIDI output to the synth's port and channel and its audio input to the synth's return.

## 2. What is sent

- Notes, control changes, pitch bend, channel and key pressure, and program changes from the
  track's MIDI clips, with the track's MIDI Real-Time Properties applied (as the instrument hears them).
- **When playback starts or jumps** (play, locate, loop), each controller, pitch bend, channel
  pressure and program change that has an event before the play position is sent with its last
  value, bank select before the program. Controllers without an event are left alone, and no Reset
  All Controllers is sent, so the device keeps its own setup.
- Notes that started before the play position do not sound (no note chasing yet).
- At one position, releases go first, then control changes, program changes, bend and pressure,
  then new notes.

## 3. Timing

- **MIDI Track Offsets** (Event › MIDI Track Offsets) move a track's notes earlier or later, for
  the instrument and the external output alike. Use a negative offset for a device that is slow to
  sound.
- **Delay compensation**: Setup › Preferences › MIDI › Delay Compensation for External Devices ›
  **MIDI Notes and Controllers** (on by default) delays the output by the audio interface's output
  latency and the mixer's plug-in delay, so the notes sound with the audio. Off, they leave as soon
  as the audio is rendered, ahead of it by the output latency.
- If the audio driver reports no output timing, compensated output cannot be timed: the status
  line says so. Switch compensation off or check the audio device. Without an audio device
  (SoundCraft's silent clock), the output is not delayed.

## 4. Stop, locate, mute and All MIDI Notes Off

- **Stop** sends a Note-Off for each note that is sounding, sustain pedal up on channels where the
  clips left it down, then All Notes Off on each channel used.
- **Locate and loop** while playing send the Note-Offs and pedal up, but no All Notes Off.
- A note whose clip is edited or deleted while it sounds still ends when it was due to.
- **Mute** on a MIDI track, a solo on another track (unless this one is Solo Safe) and mute
  automation stop the track's output and end its sounding notes. On an instrument track, Mute and
  solo act on the audio only, so its MIDI output keeps playing; set its output to **No MIDI
  output** to silence it.
- **Event › All MIDI Notes Off** (Cmd+Shift+.) sends, on every MIDI output port in use, a Note-Off
  for each sounding note, then sustain pedal up, All Notes Off and All Sound Off on all 16
  channels. Playback carries on.

## 5. Troubleshooting

- **Doubled or stuck notes on a keyboard synth**: switch its Local Control off if you also play its
  own keys while SoundCraft plays it.
- **Windows**: a WinMM port can usually be opened by one program at a time. Close other programs
  that use the port.
- **A port is missing from the list**: SoundCraft lists ports every two seconds. A port whose driver
  gives two ports the same identity is not offered, because SoundCraft could not tell them apart.

## 6. Automation interface

- `track.midi_output {tracks?, output: "MIDI:<channel 1-16>:<device>" | "none"}` sets a track's
  output; see [control-protocol.md](control-protocol.md).
- `ui.midi_output_compensation {value?: bool}` reads or sets the delay compensation preference.
- `ui.inspect` reports the ports (`midi_output_ports`), each track's output (`midi_outputs`) and
  the output worker's diagnostics (`midi_output_status`, `midi_output_error`).

## Not there yet

MIDI Thru to external gear, a default program and bank per track, note chasing, more than one
destination per track, a separate MIDI mute and solo on instrument tracks, the Global MIDI
Playback Offset, and device names of your own. See [gaps.md](gaps.md), G-02.
