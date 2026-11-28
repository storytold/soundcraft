# MIDI output: playing external synths and sound modules

MIDI and instrument tracks can play their MIDI clips to a hardware synth, a sound module or
another program through a MIDI output port, and play what you play on a keyboard to it live
(MIDI Thru, section 5).

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
  second or two and, as soon as the port is open (before anything you play through MIDI Thru goes
  out there), sends sustain pedal up and All Notes Off on the channels played on since the
  port was last opened (by the clips or through MIDI Thru), in case the device still holds notes
  it never heard end: on Windows, a device being added or removed can break every open output
  until it is reopened, so the Note-Offs sent meanwhile are lost. If the port fails again before
  that quieting went out, the next reconnect sends it again. While playing, it then carries on
  from the play position with the controllers and program chased. The log records each port
  opened, lost and reconnected.
- When a name is bound to another physical port while it is open (a recalled window
  configuration can do that), the notes and sustain pedals the old port holds, from the clips and
  through MIDI Thru, are released on it before it is closed, then the new port is opened and
  quieted as above.

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
  **MIDI Notes and Controllers** (on by default) delays what the clips play by the audio
  interface's output latency and the mixer's plug-in delay, so the notes sound with the audio.
  MIDI Thru is never delayed (section 5). Off, they leave as soon
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

## 5. MIDI Thru: playing the synth from a keyboard

Record-enable the track (or switch its input monitor on) and play: what the track's MIDI input
takes goes out on its MIDI output at once, stopped or playing, so you hear the external synth as
you play it. This is what Pro Tools calls MIDI Thru (Options › MIDI Thru, on by default).

- Everything goes out on the track's **output channel**, whatever channel the keyboard sends on.
  The track's MIDI input selector decides which devices and channels it takes (see
  [midi-input.md](midi-input.md)).
- Notes, the sustain pedal and other controllers, pitch bend, aftertouch and program changes go
  through. System exclusive and clock do not.
- Thru goes out as soon as the message arrives: MIDI Track Offsets and the delay compensation
  preference do not apply to it (they are for what the clips play), as in Pro Tools.
- Several record-enabled tracks that take the same input each send it to their own output. Two
  tracks on the same port and channel send it there once.
- The track's clips keep playing while you play along (they are merged, as on an instrument track).
  When a clip and your keyboard sound the same key on the same port and channel, the device hears
  the Note-Off only when the later of the two ends, since a device ends notes by key. The
  sustain pedal is shared the same way: it goes up only when the later of the two lifts it.
- What you play is recorded as usual whether MIDI Thru is on or off. The MIDI Input Filter acts
  on the recording only.
- No hardware keyboard: open **Window › MIDI Keyboard** (Shift+K), set the track's MIDI input to
  All devices, record-enable it and click the keys. A key sounds while you hold it down, on
  channel 1 (thru sends it on the output channel), and is recorded like a hardware key. With
  no MIDI or instrument track record-enabled, a click step-enters the note into the selected clip
  instead. The computer keyboard does not play it yet.
- **Options › MIDI Thru** switches thru off for every track: the keyboard then plays neither the
  instruments nor the external outputs.
- Notes you hold through thru are ended (Note-Off, then sustain pedal up) when the track is taken
  out of record or its monitor goes off, its output or input is changed, it is made inactive or
  deleted, a MIDI track is muted (or soloed out), MIDI Thru is switched off, the keyboard is
  unplugged, the transport stops, the output port is unplugged or its name is bound to another
  port, or with **Event › All MIDI Notes Off**. Letting go of the key afterwards sends nothing
  more. A port that is unplugged gets none of these: a device that missed the Note-Off can keep
  the note sounding until the port is back and SoundCraft quiets that channel (see above).
- When the clips stop on a port while you hold a key or the sustain pedal there through thru
  (their output offset is changed, say), their notes end one by one and no pedal up or All Notes
  Off is sent on that channel, so what you hold keeps sounding.
- A **mute** on an instrument track acts on its audio only, as for its clips: its MIDI output
  keeps playing what you play.
- **MIDI loops**: when an output feeds back into an input of the same device (an IAC bus used
  as both, or a device that echoes what it receives), a message that comes back on that input
  within 20 ms of being sent is taken as its echo, once per message sent. What MIDI Thru sent is
  dropped when it comes back, so it is neither played again nor recorded twice. What the clips
  sent is not sent on through MIDI Thru, and the track that played it does not play or record it
  again; another track that takes that input records it as usual, so you can still record one
  track's output onto another through a loopback. `ui.inspect` counts these echoes
  (`midi_output_status.thru.echoes_dropped`).
- Input from another device is never taken for an echo: a keyboard on the same channel as the
  synth it plays can repeat a key, a pad or a controller value as fast as it likes. SoundCraft
  tells devices apart by name: an input and an output with the same name (apart from a number
  the list adds, such as `Keys (2)`) are one device.
- After MIDI Thru ends a note or lifts the pedal itself (Stop, All MIDI Notes Off, a change of
  channel or device), the key's own release sends nothing more, but the pedal's own release
  still goes through when you let go of it: a second pedal-up is harmless.

## 6. Troubleshooting

- **Doubled or stuck notes on a keyboard synth**: switch its Local Control off if you also play its
  own keys while SoundCraft plays it, and when you play it through MIDI Thru from its own keys
  (Pro Tools Reference Guide p.811): otherwise each key sounds twice.
- **Windows**: a WinMM port can usually be opened by one program at a time. Close other programs
  that use the port.
- **A port is missing from the list**: SoundCraft lists ports every two seconds. A port whose driver
  gives two ports the same identity is not offered, because SoundCraft could not tell them apart.

## 7. Automation interface

- `track.midi_output {tracks?, output: "MIDI:<channel 1-16>:<device>" | "none"}` sets a track's
  output; see [control-protocol.md](control-protocol.md).
- `ui.midi_output_compensation {value?: bool}` reads or sets the delay compensation preference.
- `options.midi_thru {value?: bool}` sets Options › MIDI Thru, or toggles it without `value`;
  the answer is the new state. It is on by default.
- `ui.inspect` reports the ports (`midi_output_ports`), each track's output (`midi_outputs`) and
  the output worker's diagnostics (`midi_output_status`, `midi_output_error`);
  `midi_output_status.thru` has the messages sent through, the echoes dropped and the time from
  a message's arrival to its hand-off to the port (`latency_p50_us`, `latency_p95_us`,
  `latency_max_us`). The percentiles are estimated within the fixed buckets of
  `latency_buckets` and never read above the maximum.

## Not there yet

A default program and bank per track, note chasing, more than one destination per track, a
separate MIDI mute and solo on instrument tracks, the Global MIDI Playback Offset, the MIDI Track
Offsets window, an audio input on instrument tracks, device names of your own, system exclusive
through MIDI Thru, and Real-Time Properties on what MIDI Thru plays. See [gaps.md](gaps.md), G-02.
