# MCP server

SoundCraft speaks the [Model Context Protocol](https://modelcontextprotocol.io) over stdio, so
Claude and other agents can make music with it.

```sh
soundcraft-cli mcp                    # headless engine (no window, no audio device)
soundcraft-cli mcp --demo             # headless, starting from the demo session
soundcraft-cli mcp --automation-read-root DIR --automation-write-root DIR   # headless, confined to folders
soundcraft --control 7801 &           # the desktop app…
soundcraft-cli mcp --connect 7801     # …driven over MCP: same tools plus screenshots and clicks
```

Claude Code:

```sh
claude mcp add soundcraft -- soundcraft-cli mcp --demo
```

## Confining file access

```sh
soundcraft-cli mcp --automation-read-root /work/project --automation-write-root /work/project/out
soundcraft-cli mcp --automation-read-root /work/project --automation-read-root /work/shared-samples \
  --automation-write-root /work/project/out --automation-write-root /tmp/scratch
```

```json
{"mcpServers": {"soundcraft": {"command": "soundcraft-cli",
  "args": ["mcp", "--automation-read-root", "/work/project", "--automation-write-root", "/work/project/out"]}}}
```

The two flags (VectorCraft and the other Craft apps take the same ones, so one client config
works across the suite) keep the agent to the folders you give it:

- Every file a command reads must lie inside a **read root**: `open_session`, `session.open`,
  `import_audio` and `file.import_audio`, `file.import_midi`, `file.import_video`,
  `file.import_clip_groups`, `file.import_session_data`, `track.load_preset` and
  `mix.sampler_load`. The media a session file names is read too, so an opened session whose
  audio paths lead outside the read roots loads without that audio and lists it under `missing`.
- Every file a command writes must lie inside a **write root**: `save_session`, `session.save`,
  `session.save_as`, `session.save_copy`, `session.save_template`, `bounce_mix` and
  `file.bounce_mix`, `file.bounce_stems`, `file.export_clips`, `file.export_midi`,
  `file.export_session_text`, `file.export_selected_tracks_as_session`, `file.export_sibelius`,
  `file.send_to_sibelius`, `file.print_score`, `file.export_clip_groups` and
  `track.save_preset`, together with what a session save writes beside the session file
  (`Audio Files/`, `Backups/`) and its temporary file. A score command given no `path` writes to
  the temp folder, which is outside the write roots unless you made it one, so it is refused.
- Each flag may be given more than once (#874): a path inside any of the read roots may be read,
  one inside any of the write roots written, so one session can read a project folder and a shared
  samples folder, or write to an output folder and a scratch folder, without widening a root to
  their common parent. A folder given twice counts once.
- Read and write access are separate. Give the same folder to both flags for both; a kind left out grants
  none of its access (with only `--automation-read-root`, nothing can be written to disk). Without
  either flag nothing changes.
- A path is made absolute against the server's working directory, then its links are followed: a
  file (or folder) link that leads out of a root is outside it. For a file that doesn't exist yet,
  its nearest existing folder's links are followed and the rest must be plain names, so `..` below a
  folder that doesn't exist is refused, as is a link that leads nowhere (writing through it would
  create its target). The result is compared with the root by whole folder names: `/work2/x` is not
  inside `/work`. On Windows case is ignored, UNC paths (`\\server\share\...`) are resolved like
  drive-letter ones, and device names (`CON`, `NUL`, `COM1`...) are refused.
- A refused path is an ordinary tool error the agent can read and correct, for example
  `automation path rejected: outside the write root /work/project/out: /etc/x.wav` (with several,
  `outside the write roots /work/project/out, /tmp/scratch: ...`) or
  `automation filesystem access is not granted: write authority is absent: /work/project/out/x.wav`.
  A refused save path is reported as refused, never as "already exists".
- The roots must be existing folders; a missing one stops the server with an error. Each flag takes
  one folder per use (`--flag <dir>` or `--flag=<dir>`) and may be used again for more, and a
  misspelled `--automation-...` flag is an error rather than an unconfined server.

The flags confine the in-process server, so `--connect` together with them is an error: the server
would otherwise talk to an app that isn't confined. SoundCraft's desktop app has no equivalent flags
yet, so a running app (`soundcraft --control`) is not confined. `--demo` is the operator's own
choice and is built before the roots apply.

Playback and bounce need no check of their own. SoundCraft decodes audio into the session when it
enters it (import, open, sampler load), so rendering reads nothing from disk afterwards. Movies are
the exception: a session only references them by path and the desktop app decodes pictures from
that file, but the headless server never does, and the movie's path is checked when it is imported
and when a session naming it is opened.

What the roots do not cover: commands that take no path still read and change the open session,
and `inspect_session` returns it. Plug-in discovery scans the system's plug-in folders and loads
plug-in binaries, which are fixed locations and native code, not agent paths. Recording is only
reachable in the desktop app. Anything that reads or writes a path goes through one place in the
engine (`io::read_file`, `io::write_file` and the helpers beside them in `crates/engine/src/io.rs`);
a test keeps the rest of the engine from touching the file system on its own.

The checks hold for every thread of the server process. A path is checked, then opened: another
program that swaps a link in between can still win that race, so the roots keep an agent to the
files it was given rather than guarding against other software on the computer.

## Tools

| Tool | Purpose |
|---|---|
| `list_commands` | Discover every command (id, label, menu path, shortcut, params doc, enabled). |
| `execute` | Run one command with JSON params. |
| `batch` | Run up to 1000 commands in order (`keep_going` optional). Larger batches return `isError: true` before executing any command. Any failed call makes the tool result `isError: true`; with `keep_going` the remaining calls still run and every result is listed. |
| `inspect_session` | Verify work: tracks, clips, mixer, plugins, sends, automation, markers, selection; each `sources[]` entry includes `loaded` (decoded audio in the pool). |
| `new_session`, `open_session`, `save_session` | Session files (`.scraft`, audio in `Audio Files/`). `open_session` returns `missing`: the media files that could not be loaded (empty when all are found). `save_session` won't save over another existing file unless `replace` is true. |
| `import_audio` | WAV, AIFF, FLAC, MP3, OGG, AAC/M4A, ALAC, CAF onto a new or existing track. |
| `bounce_mix` | Render to WAV/AIFF/FLAC; returns peak dBFS and integrated LUFS. |
| `parity` | Feature-parity report. |
| `screenshot`, `ui_inspect`, `ui_set`, `menu_invoke`, `click`, `drag`, `key` | App only (with `--connect`). |

Resources: `soundcraft://session` (full session JSON), `soundcraft://commands`.

## Conventions

- Programmatic calls never open dialogs; empty params act on the current selection.
- Positions: samples, `{"seconds": x}`, or `"bars_beats:5|1|000"`-style strings.
- Tracks by name or id; clips by id.
- Every command is undoable through `edit.undo` unless it is a query or a view setting.

The acceptance test `crates/automation/tests/it/agent_tasks.rs` drives a realistic session end to end
through MCP only (mixing, plugins, routing, editing with undo, markers, tempo, automation, bounce,
save and reopen).
