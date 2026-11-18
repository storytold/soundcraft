# Command plug-ins

SoundCraft runs **command plug-ins**: WebAssembly modules that act on the open session through
the same commands as the menus, the CLI, the control channel and MCP. They are not audio
plug-ins: CLAP, VST3 and Audio Unit plug-ins are inserts and instruments in the mixer
([plugin-parity.md](plugin-parity.md)), and a command plug-in never runs on the audio thread.

The sandbox, the manifest (`craft-plugin.json`), the `.craftplugin` bundle, the per-user store,
catalogs and GitHub installs come from the shared `craft-plugin` crate in
[craft-libs](https://github.com/storytold/craft-libs/tree/main/crates/plugin) (RFC-0046); its
README describes the ABI and how to write and publish a plug-in. SoundCraft's side is
`crates/engine/src/command_plugin.rs` (with `command_plugin/`) and `crates/engine/src/cmd/command_plugins.rs`,
and the Command Plug-ins menu and window in `crates/ui-egui/src/command_plugins.rs`.

A plug-in calls commands in their JSON form, as `engine.execute` does; `soundcraft-cli commands`
lists them with their parameters. Positions go as the commands document them (samples,
`{seconds}`, a counter string). The selection a plug-in receives is
`{tracks: [ids], clips: [ids], start, end}` (the edit selection, in samples). A plug-in only
installs when its manifest names `soundcraft` in `apps`.

## Using command plug-ins

- The **Command Plug-ins** menu (after the catalog's menus) lists each installed plug-in's
  commands, then **Manage Command Plug-ins…**. A command with parameters asks for them in a dialog
  generated from its manifest. With the macOS menu bar, **SoundCraft ▸ Command Plug-ins…** opens
  the window, which lists the commands too.
- **Manage Command Plug-ins…** lists the installed plug-ins with their commands, and installs from
  a `.craftplugin` file, from a GitHub repository (`owner/repo` or `owner/repo@tag`: its release's
  `.craftplugin`, checked against the release's `SHA256SUMS`) or from a catalog (`index.json` on
  GitHub's hosts, or a `file://` one). **Check for Updates** looks at each plug-in's catalog or
  repository. Downloads run in the background.
- Every install shows a confirmation first: the plug-in's access, the commands it may call, its
  kind, where it comes from and its SHA-256, and says plainly when a repository is in no catalog
  you use. Nothing is written before **Install**.
- The desktop app keeps plug-ins in `Plug-ins/` in its settings folder, and their settings and
  catalogs in `ui.json` (`command_plugins`). Runs without preferences (`SOUNDCRAFT_NO_PREFS`), the
  CLI, MCP and the web build keep plug-ins for the session only, and install from GitHub or https
  catalogs only in the desktop app. A `path` and a `file://` catalog go through the automation
  read roots (`soundcraft-cli mcp --automation-read-root`).
- The menu's and the windows' fixed strings are English for now: the pt-BR catalog has no entries
  for them yet.

## Commands

| Command | Parameters |
|---|---|
| `plugin.list` | `{}` |
| `plugin.install` | `{path}`, `{data: base64, name?}`, `{repo: owner/repo[@tag]}` or `{catalog, id, version?}`; `review: true` checks and describes the bundle without installing; `sha256` installs only that bundle |
| `plugin.update` | `{id?, apply?}`: lists newer releases; `apply` installs those asking for no other permissions |
| `plugin.remove` | `{id}`: the plug-in and its settings |
| `plugin.run` | `{id, command?, params?, fuel?}` |
| `plugin.catalog.list` | `{url?}`: the packages SoundCraft can run |
| `plugin.catalog.add` | `{url, remove?}` |

Only `plugin.run` is journaled; the others manage the plug-in folder and the catalogs, outside the
session.

## What a run may do

- A run is **one undo step** named after the plug-in (Edit ▸ Undo takes the whole run back). A
  run that fails (a trap, an exhausted instruction budget, a non-zero status, a refusal it passes
  on) puts the session, the selection, the undo history, the clipboard and the transport requests
  back as they were. A run that changes nothing but the selection records no step. A run makes at
  most 1000 edits, and doesn't start while a take is being recorded.
- `permissions.access: "read"` plug-ins may call any allowed command, but a call that changes the
  session document (or records an undo step) is put back and refused. Selecting and inspecting are
  allowed. `permissions.commands` narrows the commands further.
- **Only listed commands** may be called (`ALLOWED` in `command_plugin.rs`): editing the session's
  tracks, clips, playlists, automation, MIDI, tempo, meter, markers and mixer, selecting, and
  reading. A command added to SoundCraft later is refused until it is reviewed and listed.
  Refused, whatever the manifest asks: everything `craft-plugin` refuses in every app
  (`plugin.*`, `plugins.*`, `edit.undo`, `edit.redo`, `file.*`, `app.*`, `pref.*`, `window.*`);
  the transport, playback and recording (`transport.*` but `transport.status`,
  `edit.play_timeline`, `edit.tab`, `markers.recall` / `next` / `previous`, `mix.record_arm`,
  `mix.input_monitor`, `event.step_input`, `event.retrospective_record`, `event.all_notes_off`);
  devices and options (`setup.*` but `setup.session` and `setup.current_timecode`, `options.*`,
  edit modes and tools); the view; other sessions and files (`session.*` but its queries, track
  presets, `mix.sampler_load`, `mix.insert_state`, `video.online`); the audio plug-in scan and the
  undo history (`engine.*` but `engine.commands`); and the clipboard (`edit.copy*` but the
  playlist copies, `edit.cut*`, `edit.paste*`).
- After every call, whatever the access, a call that sent a request to the transport or the audio
  engine, changed the clipboard, armed a track or monitored its input, or changed the session file
  is put back and refused.
- Parameters that name files (`path`, `paths`, `file`, `files`, `dir`, `folder`, `directory`,
  `destination`, `location` holding text, at any depth) are refused, and so are hosted audio
  plug-ins (`plugin` or `instrument` ids starting `clap:`, `vst3:` or `au:`): a command plug-in
  uses SoundCraft's built-in processors only. A plug-in gets no disk, network, clock or other
  session.
- Settings: each plug-in keeps up to 64 KB of its own settings (keys up to 128 characters), 1 MB
  for every plug-in together, for up to 64 plug-ins.

Not yet: plug-in panels (`ui_show` runs report that SoundCraft doesn't show them), offering files
to save (`saveFile`), translated strings for the menu and windows, and the installed plug-ins'
commands in the macOS menu bar (the window lists them).
