# Command-line tool

`soundcraft-cli` runs SoundCraft without a window: it converts and inspects audio, runs engine
commands on a session, serves MCP, and calls a running app. Follows
[RFC-0012](https://github.com/storytold/craft-contrib/pull/18) (command-line conventions for the
Craft apps).

`soundcraft-cli --help` and `soundcraft-cli <subcommand> --help` are the source of truth for
flags, values and defaults. This page does not repeat them.

| Subcommand | What it does |
|---|---|
| `info` | Prints an audio file's format as JSON, or a `.scraft` session's summary |
| `convert` | Converts an audio file: resample, change the bit depth, normalize |
| `run` | Runs engine commands on a new, demo or saved session; can bounce, save and inspect |
| `script` | Runs a file of engine commands, one `ID JSON` per line |
| `commands` | Lists the engine commands |
| `describe` | Prints one engine command's details as JSON |
| `app` | Calls a running app's control channel (see [the control protocol](control-protocol.md)) |
| `mcp` | Serves MCP on stdin and stdout (see [the MCP guide](mcp.md)) |
| `parity` | Prints the parity report as markdown |
| `plugins` | Lists the built-in plugins |

```sh
soundcraft-cli --version                 # also -V; there is no `version` subcommand
soundcraft-cli help convert              # the same as `convert --help`
soundcraft-cli info session.scraft
soundcraft-cli convert in.wav out.flac --bit-depth 16 --rate 44100
soundcraft-cli run --demo --cmd 'mix.volume={"track":"Kick","db":-6}' --bounce mix.wav
soundcraft-cli script steps.txt --demo --keep-going --save out.scraft
soundcraft-cli commands bounce
soundcraft-cli describe file.bounce_mix
```

## Output and exit codes

Results go to standard output, as JSON where a program is meant to read them. `info` (on an audio
file), `describe`, `app` and the `--inspect` output of `run` and `script` pretty-print it; `--compact`
prints the same JSON on one line. The result lines of `run` and `script` (`ID: JSON`) and the
`--bounce` and `--save` results are always one line each. `commands`, `plugins`, `parity` and the
`.scraft` summary of `info` are text. Errors go to standard error as `error: <message>`. `mcp` writes only the protocol
to standard output.

| Exit code | Meaning |
|---|---|
| 0 | Success |
| 1 | The command failed (a missing file, an unknown engine command, an app that is not running) |
| 2 | Usage error |

Arguments are checked before any work starts. These are usage errors (exit 2): a flag the
subcommand does not take (`--fromat`), a flag given twice (only `--cmd` and the two automation
root flags repeat), a value of the wrong type or out of range, flags that conflict
(`--dither` with `--no-dither`, `--connect` with a root flag or `--demo`) and a missing value. A value flag
never takes the next flag as its value: write a value that starts with `-` as `--bounce=-x.wav`, or
give a file as `./-x.wav`. File names that are not valid Unicode reach the command unchanged.
The engine writes bounces and saved sessions through commands whose parameters are JSON text, so
those paths must be valid Unicode (`--bounce`, `--save`, and the `--in` session with
`--in-place`: exit 1).

An output that is one of the inputs is a usage error too, however the path is spelt (`a.wav`,
`./a.wav`, a symbolic or hard link):

| Subcommand | Inputs | Outputs | `--in-place` lets an output replace |
|---|---|---|---|
| `convert` | `IN` | `OUT` | `IN` |
| `run` | `--in` | `--bounce`, `--save` | the `--in` session, by `--save` only |
| `script` | `FILE`, `--in` | `--bounce`, `--save` | the `--in` session, by `--save` only; never `FILE` |

`run` and `script` take `--in-place` only with `--in`. A `--bounce` naming the `--in` session, and
`--bounce` and `--save` naming one file, are refused with or without `--in-place`. With
`--in-place`, an output that is a symbolic link to the input replaces the file it links to; the
link stays. An existing file that is not an input is replaced without asking.

## Environment variables

| Variable | Flag | Notes |
|---|---|---|
| `SOUNDCRAFT_CONTROL_PORT` | `app --port` (CLI), `--control` (desktop app) | A port or `host:port`, an IPv6 host in brackets (`[::1]:7979`). The desktop app takes the port of a loopback `host:port` (`127.0.0.1`, `localhost`). Its control server listens on `127.0.0.1` only, so it ignores another host from the variable, with a log warning, and starts no control server; given to `--control`, another host is a usage error |

- A flag wins over its variable.
- An empty variable counts as unset.
- A bad value is a usage error that names the variable.
- `<subcommand> --help` lists the variable.
- The automation roots (`--automation-read-root`, `--automation-write-root`) are flags only. No
  variable grants file access.

`SOUNDCRAFT_CONFIG_DIR`, `SOUNDCRAFT_NO_PREFS`, `SOUNDCRAFT_NO_SYSTEM_FONTS`, `SOUNDCRAFT_WAYLAND`
(Linux) and `RUST_LOG` configure the desktop app (see the [README](../README.md#logs) and
[the control protocol](control-protocol.md)), not the CLI. `soundcraft --help` lists them.
An empty variable counts as unset (RFC-0012 rule 1.7), except `SOUNDCRAFT_CONFIG_DIR`: empty means
no settings folder.

`SOUNDCRAFT_NO_PREFS`, `SOUNDCRAFT_NO_SYSTEM_FONTS` and `SOUNDCRAFT_WAYLAND` are on/off switches with
one rule: `1/on/true/yes` is on, `0/off/false/no` is off (any case, spaces ignored), and unset or
blank is the default (off). A value that isn't one of these spellings is ignored (the default applies) and logs a warning that
names the variable.

## Desktop app launch flags

`soundcraft [OPTIONS] [FILES]...` takes the same strict parsing and exit codes.
`soundcraft --help` lists the flags:

| Flag | Meaning |
|---|---|
| `FILES` | Sessions, audio and MIDI files to open; put `--` before a name that starts with `-` |
| `--demo` (alias `--sample`) | Start with the demo session |
| `--no-audio` | Don't open an audio device |
| `--control <PORT>` | Start the control server on this port (`0` picks a free port and prints it); falls back to `SOUNDCRAFT_CONTROL_PORT` |
| `--version`, `-V` | Print the version |
| `--help`, `-h` | Print the flags and the environment variables |

## Man pages and completions

The Linux `.deb` and `.rpm` packages install `man soundcraft-cli` (plus a page per subcommand, such
as `man soundcraft-cli-convert`) and completions for bash, zsh and fish. The Linux and FreeBSD
tarballs carry the same files. The pages list each flag's environment variable, as `--help` does.
The desktop app has neither.

Without a package, generate them from a source checkout:

```sh
cargo run -p soundcraft-cli --example man-and-completions -- out/
```

`out/` then holds `soundcraft-cli.bash`, `_soundcraft-cli` (zsh), `soundcraft-cli.fish`,
`soundcraft-cli.elv` (elvish), `_soundcraft-cli.ps1` (PowerShell) and the `.1` man pages. Copy
`soundcraft-cli.bash` to `~/.local/share/bash-completion/completions/soundcraft-cli`,
`soundcraft-cli.fish` into `~/.config/fish/completions/`, `_soundcraft-cli` into a directory on
zsh's `$fpath`, and read a page with `man -l out/soundcraft-cli.1`.

## RFC-0012

soundcraft-cli follows [RFC-0012](https://github.com/storytold/craft-contrib/pull/18): clap derive
types, thiserror errors, `run(args, out, err)` behind `main`, environment variables declared on
their flags, generated completions and man pages, tests that pin help and every usage error, no
output replacing an input without `--in-place`, and the ast-grep rules in `lint/ast-grep`. The shared automation flags are declared once in
`crates/automation/src/args.rs`.

Rules not followed yet:

- **JSON flag (rule 1.3).** `commands`, `plugins`, `parity` and the `info` summary of a `.scraft`
  session print text and have no `--json` flag. Whether such listings need one is an open question
  in the RFC itself.
- **`string-error` rule (section 3).** Its ast-grep rule is on for the CLI code and the shared flags,
  not yet for the rest of the workspace.
