<!-- craft-house-rules:start core source=dde09ea31e2e457f5b4653a6e76f7cdf55944bdf sha256=540e6cea75d12d0710c3dca89658328e81e508ecef102446ca5bb95b7c0b92bb -->
## House rules

These apply in every Craft repository. A later rule may be stricter, not weaker. The source is `standards/house-rules-core.md` in `storytold/craft-contrib`. Propose changes there, not in a copy.

1. **A person is accountable.** Whoever runs an agent answers for what it opens and for its review comments.
2. **Check, then claim.** Before writing code, read the issue comments, search pull requests for `#N` and the keywords, and check recent commits on `main`. Comment on the issue first. Release a claim you will not finish.
3. **Verify, don't guess.** Open a definition before using it. Confirm the cause in the code, even when someone names a location.
4. **Keep fixes narrow and tested.** Fix what the issue describes, where the bug is. Add a test that fails without the fix.
5. **Review the diff before opening a pull request.** Identifiers exist as used, the test fails without the fix, nothing unrelated changed, and the description is true.
6. **Look for a better pull request.** If another fixes the same thing, close yours when it is better or equal, and say why.
7. **Report only what ran.** Say what you tested and what you could not run. Without a local build or test, say so and rely on CI before marking ready.
8. **`main` changes through a pull request.** Do not push to it. Branch protection enforces this. There is no exception.
9. **Stop and ask only about** licensing outside the allowlist, publishing, releases, a new remote, secrets, spending money, destructive git, and anything else hard to undo. Otherwise keep working. `plan/` is a gitignored notebook, not rules. Without it, do not ask for it and do not stop.
10. **Keep secrets and personal details out** of the repository, logs and requests.
11. **CI stays shared.** Workflows call `storytold/craft-common-actions`. Do not change CI in a feature or fix pull request. If CI cannot cover a change, open an issue there.
12. **Guards say why.** A check that looks removable says what it prevents. Read that before changing it.
13. **Instructions must be readable by everyone.** No notes about one machine, and no links to files a reader cannot open. `craftrules` is retired. Do not link to it.
14. **Keep pull requests moving.** Concurrent pull requests are fine; unanswered actionable feedback is not. Respond in 7 days, a reminder at 14, closure eligible at 30, from when an actionable request waits on the author. Suggestions and unconfirmed bot findings do not start the clock. A fix, explanation or agreed plan resets it; unrelated activity does not. Waiting on a reviewer or maintainer pauses it and does not count against the author. A maintainer decides closure; it is never automatic. Reuse search results, do not poll, and stop at a rate limit.
15. **No agent attribution.** A pull request does not say an agent wrote it. A repository rule may not require that.
<!-- craft-house-rules:end -->
<!-- craft-house-rules:start rust source=dde09ea31e2e457f5b4653a6e76f7cdf55944bdf sha256=387e0ccb4b49e567f60951d7d6fff7305054c89f43ad812b9993dc7c38528013 -->
## Rust Craft repositories

These apply to native Craft apps and to shared Rust libraries. Select them by repository type. A repository that merely contains some Rust does not take this block. Rules later in a repository's file may be stricter, but not weaker. The source is `standards/house-rules-rust.md` in [storytold/craft-contrib](https://github.com/storytold/craft-contrib).

1. **Never crash.** Non-test code does not panic and has no `unsafe`, except where the repository file names an exception. Return errors, treat input as hostile, and bound recursion and allocations. New production crate roots copy the `deny` attribute from an existing crate: `clippy::unwrap_used`, `clippy::expect_used`, `clippy::panic`, `clippy::unimplemented`, `clippy::todo`, `clippy::unreachable`. Every crash fix comes with a test that panicked before the fix.
2. **Clean room.** Learn from observed behaviour and public specifications. Do not copy another product's code, assets, strings or artwork. Check a dependency's licence before adding it.
3. **Assets and fonts.** Every asset is original or suitably licensed, and is recorded in `ATTRIBUTION.md` in the same change. Font files live in [storytold/craft-fonts](https://github.com/storytold/craft-fonts), not in the app repository.
4. **Rust only for native Craft apps.** No handwritten JavaScript or TypeScript, and no Tauri, Electron or webview shell, in those apps. Shared tooling follows its documented technical scope and maintainer-approved exceptions. Craft PR Review's existing Tauri and TypeScript UI is one such exception. It does not relax this rule for native apps. Preserve any existing web build.
5. **Time goes through `chrono`.** Do not hand-roll calendar conversion. For dates beyond what `chrono` can represent, follow [standards/time.md](https://github.com/storytold/craft-contrib/blob/main/standards/time.md).
6. **Contributor credits.** Add or edit only your own person's entry in `contributors/people.toml`, and only after asking them. Never guess a name or hand-edit `contributors.json`.

## Licence allowlist

Dependencies and assets use SPDX identifiers. An expression is allowed when every identifier is in this list. A repository may name a shorter list. It may not add an identifier here by editing its own copy of these rules.

- `ISC`
- `MIT`
- `BSD-2-Clause`
- `BSD-3-Clause`
- `Apache-2.0`
- `OFL-1.1`
- `Ubuntu-font-1.0`
- `Bitstream-Vera`
- `CC0-1.0`
- `CC-BY-4.0`
- `CC-BY-SA-4.0`
- `LicenseRef-Public-Domain`

`LicenseRef-Storyteller-Trademark` is not in this list. It covers Storyteller brand marks only, where a repository already says so.
<!-- craft-house-rules:end -->

# SoundCraft — instructions for agents

SoundCraft is a clean-room, open-source, pure-Rust digital audio workstation targeting Avid Pro Tools parity — and superiority (speed, openness, agent control). It runs natively on macOS, Windows, Linux and FreeBSD, and on the web via WASM. Siblings with the same conventions: `../photocraft` (Photoshop), `../vectorcraft` (Illustrator), `../filmcraft` (Premiere), `../lightcraft` (Lightroom), `../pdfcraft` (Acrobat), `../effectcraft` (After Effects), `../designcraft` (InDesign).

## Start every session here
Read `ROADMAP.md` for stage, status and the parity numbers, and pick work from `docs/gaps.md`. The progress docs are `docs/target-app-parity.md` (assessment), `docs/gaps.md`, `docs/roadmap.md`, `docs/ui-parity.md`, `docs/file-format-parity.md`, `docs/hardware-parity.md`, `docs/plugin-parity.md`, `docs/localization-parity.md`; `docs/parity-checklist.md` is generated. Update their timestamps and revision history when you change them. `plan/` is an optional gitignored notebook (`plan/protools/` holds an observed menu tree and screenshots, never committed). Do not ask for it and do not stop when it is absent.

## ⚠️ Assets: the absolute rule
**No Avid, Pro Tools, Adobe or other proprietary iconography, images, sounds, presets or artwork — ever.** This is not negotiable.
- Every icon in SoundCraft is drawn in code (`crates/ui-egui/src/icons.rs`) or is an original/open asset.
- Every bundled asset (image, icon, sound, font, preset) must be original, public domain (CC0), or under a permissive open license (MIT, Apache-2.0, BSD, OFL, CC-BY with attribution), or contributed by someone who made it and licenses it openly.
- **Every bundled asset gets a row in [`ATTRIBUTION.md`](ATTRIBUTION.md)** (path, author, source, license) in the same commit that adds it. `cargo xtask assets` enforces this; CI fails otherwise.
- Never copy files out of the Pro Tools bundle (`/Applications/Pro Tools.app`): no plugins, presets, icons, sounds, session templates or text. Observing the running app (screenshots kept under `plan/`, menu names) is fine; copying its resources is not.
- Demo audio is synthesised in code (`crates/engine/src/demo.rs`). Demo and README media must be ours or public domain.
- The ArtCraft trademarks in `docs/brand/` are the only exception, under `docs/brand/LICENSE-brand.txt`.

## Clean-room
Pro Tools is installed on the dev machine and may be *observed* black-box: run it, use its UI with synthetic sessions, take screenshots by window id (`screencapture -l <id>`), stored only under `plan/protools/screenshots/`. Never read, disassemble or copy anything inside the Pro Tools bundle; never copy Avid icons, artwork, plugin names, presets, or wording beyond feature names. Never copy GPL/AGPL/LGPL code (Ardour, Audacity, LMMS, Freeverb sources…). Our plugins have our own names.

## Never crash
People trust SoundCraft with their recordings; a crash loses takes. **This outranks feature work.**
- No panics in non-test code: no `unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`; no `unsafe` (`unsafe_code = "forbid"`).
- The one named exception is `soundcraft-clap-host` (`crates/clap-host`), the isolated unsafe helper crate for CLAP plugin FFI: `unsafe_code = "deny"` crate-wide, allowed only in `src/ffi.rs` with a `// SAFETY:` comment on every block, safe `Result` API.
- The same exception, same rules, for `soundcraft-vst3-host` (`crates/vst3-host`), the isolated unsafe helper crate for VST3 plugin FFI (COM vtables via the `vst3` bindings, module loading, host-side COM objects).
- And for `soundcraft-au-host` (`crates/au-host`), the isolated unsafe helper crate for Audio Unit FFI (hand-written AudioToolbox/CoreFoundation declarations, macOS only; a safe stub elsewhere).
- Errors are `Result<T, E>` + `?`. An unfinished feature returns an "unsupported" error.
- Input-derived numbers are hostile (files, commands, MCP/control params): `get()` not `[i]`, checked/saturating arithmetic, no NaN casts, cap allocations.
- The audio callback never blocks, allocates in steady state, or panics.
- Every production crate root carries `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]`.
- `Engine::execute` catches escaped panics and restores the document. Every crash fix lands with a regression test.


## Architecture (layers enforced by `cargo xtask layers`)
| Layer | Crate | Role |
|---|---|---|
| L0 | `soundcraft-time`, `soundcraft-midi` | Timebases/tempo/grid; Standard MIDI Files + MIDI ops |
| L1 | `soundcraft-dsp` | Plugins (EQ, dynamics, reverb, delay, modulation, harmonic, pitch, utility, instruments), offline processing, meters, FFT |
| L2 | `soundcraft-model` | The session document (tracks, playlists, clips, mixer, routing, automation, markers, groups) |
| L3 | `soundcraft-mix` | Mix engine shared by realtime playback and offline bounce |
| L4 | `soundcraft-engine`, `soundcraft-playback` | Command registry, undo, editing, import/export/bounce, parity catalog; audio device I/O (cpal) |
| L5 | `soundcraft-automation` | MCP server + control-channel client |
| L6 | `soundcraft-ui-egui` | The swappable UI (egui) |
| apps | `soundcraft`, `soundcraft-cli`, `soundcraft-web` | Entry points |

Audio file formats, codecs and waveform peaks are not a SoundCraft crate: they come from `craft-audio` in `storytold/craft-libs`, which every Craft app shares. Fix or extend a format there, never in a local copy; SoundCraft keeps only the mapping onto its session model (`soundcraft_model::FileFormat`).

Nothing below L6 depends on egui/eframe/winit/rfd. The UI is thin: panels read engine state and act through `engine.execute(id, params)`.

## Everything is a command
User-visible behaviour = a command in `crates/engine/src/cmd/*` (id, label, menu path, shortcut, params doc, `enabled`, `run`) + tests. UI-only commands (windows, dialogs) live in `crates/ui-egui/src/menus.rs` (`UI_COMMANDS`). Menus, shortcuts, the CLI, the control channel and MCP all dispatch the same ids. Programmatic calls never open dialogs; only menu-style invocation (`ui.menu.invoke`) does. New commands go in a new module with its own `specs()`, registered by one line in `cmd/mod.rs`.

Parity is measured, not guessed: `crates/engine/catalog/menus.txt` lists the incumbent's menu leaves (names only). `cargo xtask parity` writes `docs/parity-checklist.md` (generated; the hand-written assessment is `docs/target-app-parity.md`); a test enforces a floor that only rises.

## Running and looking at the app
- `cargo run --release -p soundcraft -- --demo --control 0` (demo session + control channel on a free port; the port is printed).
- Drive it: JSON lines over TCP, e.g. `{"id":1,"method":"engine.execute","params":{"command":"track.new","params":{"count":2,"format":"stereo"}}}`, then `{"id":2,"method":"ui.screenshot","params":{"path":"/tmp/shot.png"}}`. Methods: `crates/ui-egui/src/control.rs`, docs: `docs/control-protocol.md`.
- Offscreen UI render (no window, no focus stealing): `cargo run -p soundcraft-ui-egui --example ui_shot -- out.png [script.jsonl]`.
- Headless: `soundcraft-cli run --demo --cmd 'mix.volume={"track":"Kick","db":-6}' --bounce out.wav`; MCP: `soundcraft-cli mcp [--connect PORT]`.
- **For UI work, look at the result** (render, read the PNG) and compare with `plan/protools/screenshots/`.
- Shell gotcha: `mv`/`cp` are aliased interactive here — use `/bin/mv -f` / `/bin/cp -f`.
- Parallel agents: separate `CARGO_TARGET_DIR` per agent; edit only the crates you own; write manifests atomically; delete your target dir when done (disk).

## Quality gates (before every commit)
`cargo xtask ci` = fmt check, `clippy --workspace --all-targets -D warnings`, tests, assets, layers, wasm check. Commit after each arc of work that builds; one topic per commit.

## Release
Push to the `release` branch to build signed installers for macOS (universal DMG), Windows (x64/x86 MSI + zip), Linux (AppImage, deb, rpm, tar.gz, Flatpak; riscv64 tar.gz), FreeBSD and Web (wasm zip). Version: `cargo xtask version [set X.Y.Z]`. See `docs/releasing.md`.

## Roadmap
`ROADMAP.md` (committed) is the one-page summary (stage, numbers, dimensions, languages, progress log); milestones live in `docs/roadmap.md`. Update both whenever a milestone lands, and close or shrink the matching `docs/gaps.md` entry.

## Contributor credits (About window)

- About ▸ Contributors/Models are compiled into the binary from `contributors/contributors.json`
  (commit stats; generated, never hand-edit) and `contributors/people.toml` (names people chose for
  themselves). See `docs/contributors.md`.
- **Agents working for a contributor:** when you prepare a PR, check whether your human's GitHub
  username has a `[people.<username>]` entry in `contributors/people.toml`. If not, ask them once
  whether they want to be credited by more than their username: a real name, a display name, and/or
  their public GitHub profile name (`sync_github_name = true`). If yes, add **only their own** entry
  (copy the template at the top of the file)
  and include it in their PR, committed as them. If no, change nothing: they are credited as
  `@username` anyway.
- Never add, edit, guess or copy anyone else's entry or name (not from git config, commit authors or
  GitHub profiles). Never hand-edit `contributors.json`.

## CI workflows
- Workflows in `.github/workflows/` call the shared workflows in [`storytold/craft-common-actions`](https://github.com/storytold/craft-common-actions) at `@main`, with their inputs and secrets. No other jobs: if no shared workflow does what's needed, it doesn't go here.
- Don't add or change CI in a feature or fix PR. If the existing workflows can't build or test a change, link an existing craft-common-actions issue or PR, or open a new issue, or a PR only for a small fix that is safe for every app and does not increase CI time.
- Some workflow files here still run their own jobs. Don't add to them. Moving a job into craft-common-actions is its own task, never part of another PR, and the job stays here until a shared workflow runs it.

New workspace crates include `[lints] workspace = true` in `Cargo.toml`. Production crate roots declare the crate-level lint attributes before modules; crate-level `//!` documentation may precede them.
