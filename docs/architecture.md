# Architecture

SoundCraft is a Cargo workspace of small crates with enforced layering (`cargo xtask layers`).
Nothing below the UI knows about egui, so the interface can be replaced.

```
L0  time        audio-io        midi            (standalone: no workspace deps)
L1  dsp         plugin-scan     clap-host · vst3-host · au-host   (plugins; the hosts isolate the unsafe FFI)
L2  model                                       (the session document)
L3  mix                                         (the mix engine)
L4  engine      playback                        (commands, undo, I/O · audio devices)
L5  automation                                  (MCP server, control-channel client)
L6  ui-egui                                     (the user interface)
    apps: soundcraft (desktop) · soundcraft-cli · soundcraft-web (wasm)
```

## The document

`soundcraft_model::Session` is plain data (serde): tracks with playlists of clips, mixer state
(inserts, sends, routing, automation lanes), tempo and meter maps, memory locations, groups,
busses, and `EditState` (selection, tools, modes, view flags). Decoded audio lives in a
`SourcePool` of `Arc`s that is not serialised; cloning a session is cheap. Sessions save as
`.scraft` JSON plus an `Audio Files/` folder.

## Commands and undo

Every user action is a `CommandSpec` (id, label, menu path, shortcut, params doc, enabled, run)
registered in `crates/engine/src/cmd/`. `Engine::execute(id, params)` runs it, catches any escaped
panic, and pushes an undo snapshot (the previous `Arc<Session>`) when the document changed.
Continuous gestures use `execute_merged` so a fader drag is one undo step. Menus are built from
the incumbent's menu catalog (`crates/engine/catalog/menus.txt`); a menu item lights up when a
command has the same menu path and label (or an alias maps it), which also drives the parity
report.

## Audio

`soundcraft_mix::MixEngine::render(session, pos, frames, out)` renders one block: clips (gain,
fades, clip effects) → trim → inserts → pre-fader sends → fader/mute (automation, VCA, trim
automation) → post-fader sends → pan → busses → aux inputs → master faders. Independent strips
process in parallel; plugin delay compensation aligns every path. The same engine renders
bounces offline (`render_range`) and realtime playback (`soundcraft_playback::Player`, which owns
it on the audio thread and receives new session snapshots through a channel).

The cpal callbacks mark their thread (`soundcraft_playback::mark_audio_thread`). Code that may run
there can ask `on_audio_thread()` before doing anything that blocks; the desktop app's logger does,
so a `log::` record from the audio thread (a stream error, a full synth event queue, a hosted
plugin's failed `process`, a CLAP plugin's own log call) is kept without waiting and written later
by the UI thread (see README › Logs).

## Third-party plugins

CLAP, VST3 and Audio Unit plugins are hosted by `clap-host`, `vst3-host` and `au-host`, which keep
all `unsafe` code in their `ffi` modules. Reading what a plugin file holds means running its
code, so scanning happens out of process (`plugin-scan`): the app starts a copy of itself
(`--scan-plugin <format> <path>`) for each file, a few at a time; the child prints what the file
holds and exits without running the plugin's teardown code. A plugin that crashes, throws or
hangs costs only its child, and is listed by `engine.plugin_scan_failures` and under the plugin
menus. Results are cached per user by file fingerprint, so only new or updated plugins are read
again, and only the plugins the user inserts are ever loaded into the app. Audio Units are listed
from the system's component registry, which runs no plugin code. Scanning stays off until an app
turns it on (`soundcraft_engine::plugins`), so tests and tools never load the machine's plugins.

## Agent control

The desktop app's control channel (JSON lines over TCP) exposes engine commands, inspection,
synthetic input and screenshots; `soundcraft-cli mcp` wraps either a headless engine or a running
app as an MCP server. See `control-protocol.md` and `mcp.md`.
