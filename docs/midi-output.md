# MIDI output: playing external synths and sound modules

MIDI and instrument tracks can play their MIDI clips to a hardware synth, a sound module or
another program through a MIDI output port.

**Where it works.** Linux (ALSA sequencer), macOS (CoreMIDI) and Windows (WinMM). It has been
played on macOS, to a hardware synth through a USB MIDI interface and to the IAC bus, and on
Windows, to a virtual port and to a TASCAM Model 12 over USB; on Linux the code builds but has
not been tried with a device yet. There is no MIDI output on
FreeBSD or in the web version: the selector says so.

## 1. Choose an output

Each MIDI and instrument track has a **MIDI output** selector:

- in the Mix window, at the bottom of the I/O section;
- in the Edit window's I/O column (**View › Edit Window Views › I/O**), under the audio output;
- for an instrument track, also in the Edit window's Instruments column (**View › Edit Window
  Views › Instruments**), under the instrument, as in Pro Tools.

Click it and choose a port, then a channel from 1 to 16. **No MIDI output** stops the track
sending. A new track sends nothing until you choose.

- Everything the track plays goes out on the chosen channel, whatever channel it was recorded on.
- A channel that another track already plays to on the same port is marked **in use**.
- Ports with the same name get numbered names, such as "DIN" and "DIN (2)". SoundCraft remembers
  which physical port each name stood for, the ports of the same name beside a port a track uses
  included (also one connected while that port is open), so the numbers stay with the same ports
  after a restart: with "DIN" unplugged, the other interface stays "DIN (2)" and the tracks on
  "DIN" read (unavailable) rather than playing to it. SoundCraft keeps up to 256 such names: near
  that limit the other ports' names ("DIN (2)") are saved before the name in use, as they are what
  keeps it apart; with that many already saved, a name newly used is not saved, so after a restart
  a port of the same name could take it.
- Ports are listed by the names the MIDI input list uses: on Linux without the ALSA client prefix
  and sequencer address ("UM-ONE MIDI 1", not "UM-ONE:UM-ONE MIDI 1 24:0").
- A port that comes back with a new system identity (CoreMIDI gives a port that is created again a
  new one; ALSA a device that comes back as another client) keeps its tracks when no other port has
  its name. Ports that share a name are told
  apart only by their identity, so one of them back with a new identity is listed as a new
  port, such as "DIN (3)", and its tracks need choosing again.
- A port that is unplugged stays chosen and reads **(unavailable)** on the selector, in the
  warning colour (orange) the Playback Engine window uses for a MIDI input that could not be
  opened ("unavailable" is the word Pro Tools uses for missing hardware, and SoundCraft for a
  missing audio device). Where the strip is too narrow for the whole text, it ends in "…" and
  hovering the selector shows the port, channel and **unavailable** in full. The status line
  says the same, in the current language, and names the tracks that play to the port, such as
  "MIDI output UM-ONE unavailable for Synth 1, Pad". The message goes away when the port is
  back, or when no active track uses it any more (an inactive track sends nothing, so its port
  is not opened).
- SoundCraft checks the ports it has open against the port list once a second, so an unplugged
  port is noticed within about a second, playing or stopped (one unplugged and plugged back in
  within that second can go unnoticed). When the port comes back, SoundCraft reconnects within a
  second or two and sends sustain pedal up and All Notes Off on the channels played on since the
  port was last opened, in case the device still holds notes it never heard end (a Stop's
  Note-Offs sent while the port was failing are lost), and again at the next reconnect if the
  port failed before those went out; while playing, it then carries on from the play position
  with the controllers and program chased. The log records each port opened, lost and
  reconnected.
- When a name is bound to another physical port while it is open (a recalled window
  configuration can do that), the notes and sustain pedals the old port holds are released on it
  before it is closed, then the new port is opened and quieted as above.

Choosing an output does not change the track's instrument plug-in or audio output. A MIDI track
with an instrument and an external output plays both, the same notes on each.

To hear an external synth whose audio comes back into the interface, add an Aux Input track whose
input is the synth's return (Pro Tools Reference Guide p740, step 3). An instrument track has no
audio input in SoundCraft yet, so the Pro Tools way of monitoring the return on the instrument
track itself (p740, step 2) is not available.

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

- **MIDI Track Offsets** move a track's notes earlier or later, for the instrument and the
  external output alike. Use a negative offset for a device that is slow to sound. The offsets are
  set with the `event.midi_track_offsets` command for now: the Event › MIDI Track Offsets window
  (Pro Tools p1061) is not there yet, and the menu item opens nothing.
- **Delay compensation**: Setup › Preferences › MIDI › Delay Compensation for External Devices ›
  **MIDI Notes and Controllers** (on by default) delays the output by the audio interface's output
  latency and the mixer's plug-in delay, so the notes sound with the audio. Off, they leave as soon
  as the audio is rendered, ahead of it by the output latency.
- If the audio device has reported no output timing since it was opened, compensated output
  cannot be timed, so it is sent uncompensated, as with the preference off, and while a track
  plays to an external port the status line says "MIDI output is not delay-compensated: the
  audio device reports no output timing."
  Compensation comes back on its own once the device reports its timing. A device that stops
  reporting after it did keeps the last measured latency, and the status line says that instead.
  Without an audio device (SoundCraft's silent clock), the output is not delayed.

## 4. Stop, locate, mute and All MIDI Notes Off

- **Stop** sends a Note-Off for each note that is sounding, sustain pedal up on channels where the
  clips left it down, then All Notes Off on each channel used.
- **Locate and loop** while playing send the Note-Offs and pedal up, but no All Notes Off.
- A note whose clip is edited or deleted while it sounds still ends when it was due to.
- **Mute** on a MIDI track, a solo on another track (unless this one is Solo Safe) and mute
  automation stop the track's output and end its sounding notes. On an instrument track, Mute and
  solo act on the audio only, so its MIDI output keeps playing; set its output to **No MIDI
  output** to silence it.
- **Changing a track's port or channel while playing**, and muting it, making it inactive or
  deleting it, ends its sounding notes on the old port and channel; if no other track plays
  there, the sustain pedal goes up there too. A track that starts playing to a port and channel
  while playing (routed there, unmuted or made active) first sends its controllers, pitch bend,
  pressure and program at the play position, as on a locate.
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
Playback Offset, the MIDI Track Offsets window, an audio input on instrument tracks, and device
names of your own. See [gaps.md](gaps.md), G-02.
