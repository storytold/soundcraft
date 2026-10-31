# SoundCraft — egui 0.37 migration plan

## Current qualification — 2026-10-25

The local implementation incorporates main through
`541096d2cb5f96a13d5b58935e45711530471480`. Managed job
`egui037-sound-main541-ci-20261025-0857` passed all seven stages: formatting,
strict workspace lint, tests, doctests, assets, layering and WASM. Across 52 test
summaries it recorded 1,420 passing executions, zero failures and one ignored.

The manifest and lock use canonical Git URLs and immutable revisions:
egui `6d518fc799681571963842f95b55a6b68431b15a`,
winit `a517b3846ee8457d84ee4975110bfda6797c44dc`,
craft-ui `650b1c0b058017c4eb3af06d5b39586d6732e07b`, and
craft-docpath/craft-media `16fb3d46e1eaa0d455465e4e66a3ac7b17b55bd3`.
The craft-ui commit is available on GitHub; it includes the newer docking
Workspace/DockCommand APIs required by this app. Local validation used the
retained task-scoped Git mirror. Remote CI must independently qualify fetching
the dependency graph.

The docking port preserves SoundCraft's default side beside the session canvas
when a saved panel anchor has disappeared. It probes recorded locations and
hidden-anchor chains before selecting that fallback. Regression coverage includes
a deleted anchor and a hidden floating chain. Earlier drag/float/close/reopen/reload
GUI evidence qualifies the prior candidate, not this final main reconciliation.

Final-source native/browser GUI, X11/Wayland, Windows/macOS, audio/plugin hardware,
packaging/licence coverage and paired app performance remain acceptance gates.
Human review and human merge are required. Historical pins, counts and qualification
claims below describe earlier states and are superseded by this section.

## Historical planning record

Status: **implementation in progress (local worktree only)**. The planning sections below remain
the contract; the local implementation amendments at the end record the current source and
dependency state. Nothing here changes a published branch, tag, PR or release, and the current
path-only lockfile/local integration configuration is **not** a publishable dependency state.
Every push or new PR requires fresh, explicit human approval after the exact contents and
validation results are shown; nothing here grants that approval. No agent/model/tool signature is
to be added when this text or its derived PR text is published.

Scope: the assigned checkout
`/home/codex/work/CraftApps/egui037-migration-20261022/soundcraft` only. The shared
`storytold/egui`, `storytold/winit`, `storytold/craft-libs`, `craft-ai-integrations` and sibling
applications are dependency/hand-off points, not repositories changed by this plan.

The plan is written against source, not against a generic migration template. Every claim names
the file and symbol it comes from. Historical compiler output and old audit logs are labelled as
clues, never as current acceptance.

## 1. Decision in one page

SoundCraft is not starting from a 0.36 `update`-only application. It already uses the root-`Ui`
lifecycle: `apps/soundcraft/src/main.rs:37-94` implements `eframe::App::logic`/`App::ui`, and
`crates/ui-egui/src/lib.rs:1136` / `:1232` already take `&egui::Context` for logic and
`&mut egui::Ui` for layout. Upstream 0.36.2 has the same `App::logic`/`App::ui` shape, so this
migration must **not** rewrite the app into `update` or introduce `Context::root_ui` bridges.

The committed dependency state is nevertheless inconsistent: `Cargo.toml:49-52` asks for
`egui`, `eframe` and `egui_kittest` 0.37 while `craft-ui` is pinned at
`5c874d51…`, whose `egui` dependency is 0.36.2. `Cargo.lock` therefore contains two egui families
(`egui`/`ecolor`/`emath`/`epaint` 0.36.2 and 0.37.0). The first deliverable is one coherent graph and
one coherent lifecycle, not another version bump.

Target release state:

1. One immutable egui family from the complete reviewed/published **storytold egui P8**
   hand-off and one immutable **completed storytold winit**, each at a storytold-owned full
   SHA. P8 is the E1–E21 base plus **P2b** (`E4` regular-clipboard/PRIMARY routing over the
   async W15 transfer path), **P7** held-key release pairing and `E21`, **P7b/E21b** outgoing
   surrounding text, and **P7c**'s selected `#8763` selection-action and `#8748`
   hidden-parent/deferred-child fixes, plus only justified performance fixes. Winit must
   contain **W15P**, the **W27 lifecycle revision** and the **bounded W15 async transfer
   API** (§5.1). `aiongg/craft/0.37` =
   `a4f9a17f64a6bae61350fb85804c9df0d9865916` and `aiongg/winit
   craft/0.30.13-lang` = `8b35388bd719e8cc86a95971ba0bc026c9a8f058` are **research inputs**
   at inspection, not publishable pins. If those organization-owned revisions are not
   reachable when implementation starts, wait for approved publication; never fill the gap
   with a movable branch, an invented SHA or an incomplete stage. Stock 0.37 is only a
   measurement control, not an intermediate production stage.
2. `craft-ui` and `craft-docpath` from one reviewed `storytold/craft-libs` revision on the
   reconciled merge base `3832c47ef68a0e6214f2c1e85f3526b6adcb686e`, which carries the intended
   egui-API port. The historical PR #199 hunks (`8614e4a`, README `1764bca`) and the earlier
   `4919088` main inspection describe how that base was assembled; they are no longer the base
   to apply. The stale PR head `b68de8c5f4f1c060904d211a7b1320aa0f7bce78` and audit merge
   `ec8840af8c5fda695cf195cdd05e5ae078415c07` must not be merged as trees: they omit current
   `craft-platform` and revert security/settings/docking/printer fixes. The exact reachable
   immutable publication SHA remains a central-publication prerequisite; it is not guessed here.
3. Source changes limited to: the real compile-error classes (`UserData`/`Event::Screenshot`,
   `MouseWheel.source`, `WidgetType`→`Role`, private `ScrollArea::State.offset`), deprecations that
   `clippy -D warnings` will reject (`Id::new`, `Ui::id`), the native `clipboard` feature that
   0.36 supplied implicitly, font/emoji feature and glyph-coverage fixes, accessibility names, and
   the lifecycle/input fixes in §6. No wholesale UI or command-layer rewrite.
4. A staged implementation: atomic fork/shared-library/API compatibility including callback and
   lifecycle correctness, then deeper accessibility/input/fonts and platform/performance evidence. Each stage is
   independently buildable, testable and revertible. See §14.

The largest behavioural risk is not the Cargo update. It is the interaction between egui 0.37's
multi-pass `Context::run_ui`, logic-only ticks (`Context::run_logic` / `App::logic` while hidden),
and SoundCraft's current per-frame side effects (control queue, autosave, transport, dropped files,
screenshots). That work is specified in §6.

## 2. Baselines, exact revisions and evidence

### 2.1 Checkouts and baseline revisions

| Role | Revision / value | Notes |
| --- | --- | --- |
| Assigned SoundCraft checkout (planning) | `52471b37e957feaea56dfd48932ef16d314cc74a` | Historical planning `HEAD`; superseded by the local implementation update below |
| Historical coherent 0.36.2 SoundCraft baseline | `e85a4801e9029c9a1f5ce536b11be65aea832ed8` (`Release 0.12.1`) | Direct `egui = "0.36"`, `eframe = "0.36"`, `egui_kittest = "0.36"`, `craft-ui` 0.36; one coherent family; the required performance baseline |
| Upstream egui 0.37.0 | `841a328ba2dd8f6996beb7189ebb7fc00e668a2a` | Peeled release commit; annotated tag object differs |
| Suite fork research input | `a4f9a17f64a6bae61350fb85804c9df0d9865916` | `aiongg/craft/0.37`, movable branch; E1–E21; not a production pin |
| Suite winit research input | `8b35388bd719e8cc86a95971ba0bc026c9a8f058` | `aiongg/winit craft/0.30.13-lang`; W17–W27; movable branch; no W15P |
| storytold winit base | `ffae5198770d7730e718043f035a772b7dbc3b1b` | `craft/0.30.13`, W1–W15 |
| Published storytold egui 0.36.2 | `767b1cad294120689a514bb4bd68c077e46679b9` | Existing `craft/0.36.2`; not the 0.37 target |
| craft-libs reconciliation base | `3832c47ef68a0e6214f2c1e85f3526b6adcb686e` | Current merged `craft-ui`/`craft-docpath` base for this migration; the exact publish pin is still a central-publication gate |
| craft-libs historical inspection | `49190884278f16c9a9f5823d7698a8ffb5deb062` / PR #199 `8614e4a` + README `1764bca` | Earlier base and hunks; retained as history only. Branch head `b68de8c…`/audit `ec8840a…` are stale trees and must not be merged |
| craft-libs audit merge | `ec8840af8c5fda695cf195cdd05e5ae078415c07` | Evidence only; not a target |

The exact `storytold/egui`, `storytold/winit` and reviewed `craft-libs` revision after merge are
**blocking unknowns** (§16). They must be supplied as full immutable SHAs before P1 manifests
are changed.

### 2.2 Historical compiler clues (not acceptance)

`/home/codex/work/ssd2/task-evidence/egui037-audit-20261022/{fork,stock}/soundcraft.errors`
reports nine error classes from a trial that patched 11 crates.io packages and both craft-libs
crates by local path:

- `control.rs:192`: `Event::MouseWheel` initializer lacks `source`.
- `control.rs:253`: `Event::Screenshot` no longer exists.
- `control.rs:268`: `egui::UserData` no longer exists.
- `panels.rs:69`: no `Role::SelectableLabel` (the current source actually uses the deprecated
  `WidgetType` alias at this line).
- `panels.rs:110`: no `Role::Checkbox` (current source `WidgetType::Checkbox`).
- `panels.rs:164,168,177,182`: `egui::scroll_area::State.offset` is private.

The audit's path patches and source state are historical. They prove the API delta is small and
show where to look; they do not prove the current tree compiles, and they do not cover tests or
warnings that `clippy -D warnings` will reject.

### 2.3 Dependency facts in the committed lockfile

`Cargo.lock` currently contains:

- `egui` 0.36.2 and 0.37.0; likewise `ecolor`, `emath`, `epaint` (0.36.2 and 0.37.0).
- `accesskit` 0.24.1 and 0.25.1; `accesskit_winit` 0.34.1; `kittest` 0.5.0.
- `craft-ui` at `git+…?rev=5c874d51…#5c874d51…`, depending on `egui 0.36.2`.
- `craft-docpath` at `git+…?rev=82a24b00…#82a24b00…`; no egui dependency.
- `eframe`, `egui-winit`, `egui-wgpu`, `egui_kittest` 0.37.0 from crates.io; `wgpu` 30.0.1;
  `winit` 0.30.13 from crates.io.
- No `[patch.crates-io]`, no `vendor/` directory, and no machine-local source replacement.
  `.cargo/config.toml` contains only the `xtask` alias.

A read-only offline `cargo tree` was attempted for planning and could not resolve the
workspace because the local registry index lacks `symphonia-common` (required by
`symphonia-bundle-flac 0.6.1`). The feature graph below therefore comes from manifests and the
lockfile, and must be rechecked with network/cache in a managed job. This is not a build result.

### 2.4 Non-UI audio prerequisite follow-up

The `symphonia` 0.6.1 and `cpal` 0.18.2 prerequisite port was reviewed against the registries. Two
audio-io defects and one playback semantic gap are repaired in source (no runtime claim here):

- **Audio probe candidate selection.** `crates/audio-io/src/symph.rs` no longer treats any
  `0xFFEx` pair as an MPEG/ADTS marker. It asks the registered readers for their exact
  `probe_data()` markers and scores the candidate with `Scoreable::score` over a
  `ScopedStream`, mirroring the private probe's rules: `Supported` selects, EOF/other scoring
  errors continue, and a non-EOF I/O error aborts. The scan remains the first actual candidate
  within the 1 MiB probe depth, so a false sync cannot suppress a later FLAC/CAF/MP4 check.
- **Regression coverage.** `crates/audio-io/tests/malformed.rs` now prefixes empty FLAC and the
  existing malformed CAF/MP4 fixtures with a false `0xFFE0` sync and checks both public `probe`
  and `decode`; a valid MP3 and a valid ADTS stream each carry incidental `caff`/`kuki` magic.
  The earlier-malformed-CAF-before-valid-CAF rejection stays covered.
- **ID3 footer.** `id3v2_len` applies the 10-byte footer only to ID3v2.4; v2.2/v2.3 bit `0x10`
  is reserved. An ID3-tag-only file remains `Malformed` (the truncated/oversized class), noted
  rather than called a restoration of every prior ID3 classification.
- **RF64/BW64 alignment.** `first_marker` documents that its WAV marker includes RF64/BW64 for
  `wav::check_info`, while symphonia 0.6's probe registers only `RIFF`. Native PCM RF64/BW64 is
  parsed by `wav::parse` before this path; foreign RF64/BW64 stays unsupported rather than
  silently misdetected.
- **Playback stream invalidation.** `crates/playback/src/devices.rs` maps `StreamInvalidated` to
  the existing lost-stream/`OutputTrouble::Gone` path (which offers Reconnect) instead of leaving
  a dead stream logging silently. `DeviceChanged` remains live and `Xrun` transient;
  `HostUnavailable`, `DeviceBusy` and `BackendError` remain log-only. This is a conservative
  policy mapping, not a rebuilt stream and not a hardware result.

No build, test, formatter, clippy, device, VM or hardware run is implied by this subsection.

## 3. Current implementation inventory

### 3.1 Workspace, packages and feature targets

Workspace root: `Cargo.toml:1-11` (`resolver = "3"`, members `crates/*`, `apps/*`, `xtask`);
`workspace.package.rust-version = "1.90"`; `crates/ui-egui/Cargo.toml:7` already declares
`rust-version = "1.95"` because the egui 0.37 family requires it. `docs/architecture.md:13-14`
records that the UI may require 1.95 without raising the core crates' 1.90 declaration.

Relevant package targets:

| Target | egui use | Current features / notes |
| --- | --- | --- |
| `crates/ui-egui` (library) | `egui`; `craft-ui` docking/buttons | `Cargo.toml:24`; dev `egui_kittest` with `wgpu,snapshot` at `:31`; `build.rs` bakes contributor credits (no asset change) |
| `apps/soundcraft` (desktop binary) | `eframe`, `egui` | `Cargo.toml:21` features `default_fonts,wgpu,accesskit,wayland,x11,links`; **missing `clipboard`** compared with 0.36's implicit native clipboard; Windows `winresource`; macOS `muda`/`objc2` |
| `apps/soundcraft-web` (wasm binary) | `eframe`, `egui` | `Cargo.toml:27` features `default_fonts,wgpu,accesskit`; `rfd` browser picker; `WebRunner` in `src/web.rs` |
| `apps/soundcraft-cli` | none | Headless engine/control/MCP; no egui dependency |
| `crates/{time,audio-io,midi,dsp,model,mix,engine,playback,automation}` | none | Must stay UI-free; `cargo xtask layers` enforces this |
| `xtask` | none | `ci`, `layers`, `assets`, `parity`, `wasm` gates |

`rustc 1.99.0` / `cargo 1.99.0` were present at inspection; the plan does not assume that this
toolchain is used by every CI lane. The 1.95 UI MSRV must be checked against each lane.

Nested workspaces `crates/clap-host/tests/fixture` and `crates/vst3-host/tests/fixture` are
independent roots with their own lockfiles and no egui/winit dependencies. They do not inherit the
root patch and need no dependency change; they still need to build as part of the plugin tests.

### 3.2 Entry points and actual lifecycle paths

| Path | Current shape | Migration relevance |
| --- | --- | --- |
| Native shell | `apps/soundcraft/src/main.rs:37-105` `impl eframe::App`: `logic`, `raw_input_hook`, `ui`, `on_exit`; `run_native` at `:266` | `App::logic` and `App::ui` both run inside `Context::run_ui`, once per layout pass; `run_logic` also calls `logic` while hidden. Do not model native logic as a once-per-frame hook outside `run_ui` |
| Native dropped files | `main.rs:52-58` calls `SoundApp::take_dropped_files(ctx)` from `logic`; `sampler_window.rs:88-100` clones `ctx.input().raw.dropped_files` and does not clear it | At the start of `App::ui` — before the `!fonts_ready` early return at `lib.rs:1232-1239` — capture and clear `raw.dropped_files` through `input_mut`, together with the drop-time pointer position, into a one-shot pending queue. Process that queue after layout and sampler drop targets exist, for every UI pass including a forced hidden paint, and once fonts are ready. A first-frame or first forced-hidden-paint drop must not be lost; `run_logic` leaves the last pass's input in place, so later hidden ticks must not replay it |
| Native control server | `apps/soundcraft/src/control_server.rs:35-140`; `crates/ui-egui/src/control.rs:49-65` drain | Queue/reply must execute once per consumed request, including hidden ticks and multiple passes |
| Native screenshots | `control.rs:221-234`, `:244-279`; `docs/control-protocol.md:25` | Old `Event::Screenshot`/`UserData` path must become callback based |
| Native separate Mix window | `mix_window.rs:25-38` `show_viewport_immediate`; `separate_closed` at `:41` | Child root `Ui`, focus/close, hidden-parent behaviour; never call `root_ui` from the child |
| Native macOS menu | `native_menu.rs`; `main.rs:43-50,286-301` | `muda` events processed in `logic`; no egui pass required |
| Native Windows renderer | `apps/soundcraft/src/graphics.rs:11-30`; `main.rs:263-265` | DX12/FXC defaults are deliberate app fixes; preserve and measure the 0.37 shader cost |
| Web shell | `apps/soundcraft-web/src/web.rs:194-218` `WebShell` `logic`/`ui`/`raw_input_hook`; `start` `:18-50`; `take_drops` at `:136-160` already clears the raw list | Same `SoundApp` lifecycle; route drop/picker completion from the known UI phase too, so both hosts share one rule; browser file picker/drop/download; no control channel |
| Headless render example | `crates/ui-egui/examples/ui_shot.rs` uses `Harness::builder().build_ui_state` and calls `app.logic` inside the UI closure at `:65-70` | Keep logic then UI inside the harness closure, matching both native and web; assert effects across discarded passes |
| UI tests | 54 integration files; 108 `#[test]` in `crates/ui-egui/src` plus 156 in `crates/ui-egui/tests` (264 total); 1,159 workspace `#[test]` occurrences at HEAD inspection | Existing geometry/roles/command tests are the regression base; counts are the comparison point for later selected-test evidence; no committed kittest snapshot files exist |

### 3.3 Platform cfgs

- `cfg(target_arch = "wasm32")` / `not`: `fonts.rs:46,82,106` (system font loading is native-only),
  `video_track.rs:93-105,217` (thread vs inline decoder), `bounce.rs:49-63` (time slice vs 32 blocks),
  `dialogs.rs:296-300` (native picker), `sampler_window.rs` `DroppedFileHandle` web bytes.
- `cfg(target_os = "macos")`: `main.rs:19,43-50,199-203,286-301`, `native_menu.rs`, icon include;
  `muda`, `objc2`, `objc2-app-kit`.
- `cfg(target_os = "windows")`: `main.rs:264`, `graphics.rs` (DX12/FXC), `build.rs` VERSIONINFO.
- `cfg(unix)`: `config.rs:299` (settings directory/permissions).
- `cfg(target_os = "linux")` / FreeBSD: `fonts.rs` fontconfig/`fc-match`, packaging; Linux CI is
  X11+Wayland.
- `cfg(not(target_arch = "wasm32"))` in `apps/soundcraft/Cargo.toml:26-27` gives native `rfd`.

These cfgs are part of acceptance. A green Linux check is not a macOS native-menu, Windows IME,
Wayland clipboard, or browser workflow result.

### 3.4 Existing tests and CI

`cargo xtask ci` (`xtask/src/main.rs:174-200`) runs `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`,
`cargo xtask assets`, `cargo xtask layers`, and `cargo xtask wasm`. It does not add `--locked`
to every step; a lockfile/graph check must be a separate accepted command.

Repository workflows at inspection: `freebsd.yml` (nightly/release, common action),
`windows-arm64.yml` (nightly/release, common action), `packaging-lint.yml`, `nix.yml` (common
action; there is no root flake in this checkout), `sonarcloud.yml`, `release.yml` (release branch
only). The primary PR test matrix is supplied by `storytold/craft-common-actions`, not by a
`ci.yml` in this repository.

Existing UI tests already exercise real frames and shapes rather than only state:

- `tests/docking_workflows.rs` (drag float/close/reopen, reload, command ownership),
- `tests/trackpad.rs` (`smooth_scroll_delta`, pinch zoom, shift pan),
- `tests/tracks_scroll_bar.rs` (custom gutter paging),
- `tests/side_lists_access.rs`, `tests/shared_controls.rs`, `tests/appearance.rs`,
  `tests/theme_docking.rs`, `tests/mix_instrument_contrast.rs` (roles, colors, scale, theme),
- `tests/save_changes.rs`, `tests/save_as_replace.rs`, `tests/recovery.rs`,
  `tests/bounce_dialog.rs`, `tests/edit_window_headers.rs`, `tests/midi_*`.
- `crates/ui-egui/src/lib.rs:1400-1545` contains lifecycle/automation/autosave tests,
  including `a_hidden_window_still_autosaves` using `Context::run_logic`.

Optional evidence hooks already exist: `CRAFT_UI_THEME_SHOTS`, `SOUNDCRAFT_MIX_PROOF_DIR`,
`ui_shot`, and the control channel's `ui.screenshot`. They are not a substitute for native
window evidence.

## 4. Actual egui 0.37 incompatibilities and required adaptations

These are released upstream changes, not fork features. The fork may contain more changes (E1–E21);
those are handled in §5 and §12.

| ID | Source anchor | Change | Required SoundCraft adaptation |
| --- | --- | --- | --- |
| A1 | `crates/ui-egui/src/control.rs:192` | `Event::MouseWheel` now has `source: MouseWheelSource` (`egui/src/data/input/event.rs` 0.37.0) | Set `MouseWheelSource::Wheel` for `unit:"line"`, `Trackpad` for point/trackpad; assert source and phase in `trackpad.rs` |
| A2 | `control.rs:253`, `:268`; `PendingShot` at `:32-39` | `Event::Screenshot` and `egui::UserData` removed; `ViewportCommand::Screenshot(ScreenshotCallback)` | Replace event polling with `ctx.request_screenshot`, a bounded weak completion slot per request, and callback correlation (§6.4) |
| A3 | `panels.rs:69`, `:98`, `:110`; all `WidgetType::*` occurrences | `WidgetType` is a deprecated alias for `accesskit::Role`; `SelectableLabel`/`Checkbox` are not accesskit roles | `SelectableLabel`→`Role::Button`, `Checkbox`→`Role::CheckBox`, `ComboBox`→`Role::ComboBox`, `Slider`→`Role::Slider`, `RadioButton`→`Role::RadioButton`; use `egui::Role` or `egui::accesskit::Role` consistently |
| A4 | `panels.rs:155-185` | `scroll_area::State.offset` is private; public reads are `clamped_offset()`/`unclamped_offset()` | Read `out.state.clamped_offset().y`; apply a pending target through `ScrollArea::vertical_scroll_offset` next frame (no private writes) |
| A5 | `Cargo.toml:50-52`; all `Id::new`/`ui.id()` uses | `Id::new` deprecated for `Id::unique`; `Ui::id`/`UiBuilder::id` renamed to `scope_id`; `AsId` is deprecated (`egui/src/id.rs:8-12`) but still present | Mechanical `Id::new`→`Id::unique`, `ui.id()`→`ui.scope_id()`, `UiBuilder::id`→`scope_id`; same hash, no state change. `clippy -D warnings` makes this blocking |
| A6 | `crates/ui-egui/Cargo.toml:24,31`; `apps/soundcraft/Cargo.toml:21`; `egui_kittest-0.37.0/Cargo.toml` (`[dependencies.egui]`, `[dev-dependencies.egui]`) | 0.37 `egui` declares `default = []`; `default_fonts`/`monochrome_emoji_fonts` are opt-in. eframe's selected `default_fonts` enables ordinary bundled faces for the binary; the new `egui/monochrome_emoji_fonts` must be selected explicitly. kittest's normal `egui` dependency is `default-features = false`; only kittest's OWN `[dev-dependencies]` enable the font features, and dev-dependencies do not propagate. An isolated `cargo test -p soundcraft-ui-egui` graph is therefore predicted to have no bundled fonts | Enable `default_fonts` and `monochrome_emoji_fonts` on SoundCraft's egui dependency for the UI/test root and prove unification with `cargo tree -e features`; treat the no-fonts prediction as a resolution claim until that command runs. See §9 |
| A7 | `control.rs` and `Cargo.toml:21` | 0.36 eframe forced `egui-winit/clipboard` in its native dependency; 0.37 gates it behind `eframe/clipboard` | Add `"clipboard"` to native `eframe` features. Without it, copy/paste silently uses egui's simulated clipboard instead of the OS clipboard |
| A8 | `crates/ui-egui/src/lib.rs:1136-1139` | After a completed `run_ui`, `current_pass_index()` is zero (`end_pass` takes the output), so the `>0` guard does not by itself skip an ordinary hidden tick. The real hazards are stale input/time in `run_logic` and duplicated non-idempotent work | Keep public host ordering; separate fresh input from background queues and monotonic deadlines (§6.2); test hidden and repeated passes |
| A9 | `main.rs:52-58`; `sampler_window.rs:88-100`; `lib.rs:1232-1239`; web `web.rs:136-160` | `ctx.input()` in `run_logic` is the last UI pass's input; native `dropped_files` is not consumed. Visibility does not identify a logic-only callback: `RequestPaintWhileHidden` can run `App::ui` while minimized/occluded. `ui` returns before layout while `!fonts_ready`, so a drop read only after layout is lost on the first frame | Capture and clear `raw.dropped_files` through `input_mut` at the top of `ui`, before the `!fonts_ready` return, into a one-shot queue; process it after layout for every UI pass including a forced hidden paint. Never gate on `visible()` |
| A10 | `Cargo.toml:50-52`; `crates/ui-egui/src/theme.rs:431-475` | 0.37 style defaults are roomier/larger corner radius; panel/menu geometry can shift | Keep explicit SoundCraft spacing/font/size overrides; assert geometry and exact custom faces; do not refresh screenshots first |
| A11 | `bounce.rs:68`; `shortcuts.rs:64-101`; `edit_window.rs:341` | 0.37 adds `MouseWheelSource`, kinetic scrolling and rubber-banding; smooth scroll values can change | Test wheel vs trackpad source, phase and momentum; make deterministic test styles (disable kinetic/rubber-band where exact offsets are asserted) |
| A12 | `egui_kittest` 0.37 `accessibility.rs` | Every visible input widget must have an accessible name; otherwise the harness panics after a settled step | Add names to all custom-painted controls and text fields; audit every window/dialog; do not disable the check globally |
| A13 | `egui_kittest` 0.37 `builder.rs` | Missing glyphs panic by default | Enable bundled fonts/emoji and fix the app's uncovered glyphs (§9); use `allow_missing_glyphs` only in a deliberately labelled tofu test, never for the suite |
| A14 | `apps/soundcraft-web/src/web.rs:194-218` | Same App lifecycle; browser has no control channel. `eframe`'s web runner also calls `logic` then `ui` inside `run_ui` per pass | Apply the same known-phase rules, not a visibility split; do not make browser pickers or downloads part of a multi-pass paint path |

### 4.1 Not incompatibilities

- `egui::Panel::top/bottom` at `edit_window.rs:109-113`, `menus.rs:224` already use the unified
  `Panel` API; 0.37 only changes the salt constraint to `impl AsIdSalt` and `show_inside` is
  deprecated but not used. Keep panel order and ids.
- `egui::Frame::NONE`, `CornerRadius`, `Modal`, `Popup::menu`, `Context::content_rect`,
  `Context::global_style`, `Context::set_theme(ThemePreference::System)`, and
  `ViewportBuilder`/`ViewportCommand` calls are already on the 0.37-compatible API surface.
- `TextEdit::load_state`/`store`, `CCursorRange`, `has_glyphs`, and `TextureHandle::set` remain
  available; their semantics need tests, not mechanical rewrites.
- `Window::new`'s title-derived id is not a 0.37 change: 0.36.2 already uses `IntoAtoms` and
  `Id::new(title.text())` (`egui 0.36.2 crates/egui/src/containers/window.rs:102-105`), and 0.37
  uses `Id::unique`. Keep the advice to pass a stable id for persistent windows, but do not list
  it as a new 0.37 incompatibility.
- No `update`→`ui` rewrite and no generic `Context` compatibility bridge is proposed.

## 5. Dependency target, fork provenance and publication rules

### 5.1 Coherent production target

P1 adopts the reviewed storytold egui family, companion winit and compatible
craft-libs revision atomically with all required source changes. The aiongg SHAs
are research inputs and patch provenance, not final production pins. Stock 0.37 may
be built as an isolated measurement control; it is not an intermediate shipping
stage. Wait for approved, reachable dependency publication rather than inventing a
SHA before implementation or publication.

The final production pin is the whole prerequisite set, not the E1–E21 base or an
intermediate stage:

- The complete **storytold egui P8** hand-off: E1–E21, **E4/P2b** regular-clipboard
  and PRIMARY routing (`get_primary_text`/`set_primary_text`, per-seat
  owner/offer/serial handling, no second regular `wl_data_device`, password
  exclusion, no unchanged-selection reclaim) over the bounded async transfer path,
  **P7** held-key release pairing plus `E21`, **P7b/E21b** outgoing surrounding
  text, and **P7c**'s selected `#8763`/`#8748` fixes, plus justified performance
  fixes. Incoming E21 deletion/commit alone is not surrounding-text support.
- The **completed storytold winit**: **W15P** PRIMARY on the existing wayland
  connection/per-seat lifecycle, the **bounded W15 async transfer API** (total
  deadline from admission, cumulative byte cap, bounded read/writer workers and
  queue depth, handle-owned result/cancel slot that cancels on drop, owner-specific
  completion waker; no unmanaged thread per request) and the **W27 lifecycle
  revision** with owner/focus epochs and pending preedit/commit/delete clearing.
  W15 is the regular clipboard path; it is not PRIMARY.
- The reviewed craft-libs **CL-1** revision for `craft-ui`/`craft-docpath` and the
  frozen shared capture helper (**CL-2F**). The eleven craft-ui consumers need CL-1
  and CL-2F at this stage; the DesignCraft/GridCraft pilot instead uses the
  provisional CL-2 API, and its adapters must precede CL-2F. Any incompatible
  existing craft-ui bump lands in this same atomic dependency stage. Reuse the API
  port hunks that now sit on the reconciled `3832c47` base (the earlier `8614e4a`/`1764bca`
  hunks on `4919088` are history); never merge PR #199's stale `b68de8c` or `ec8840a` trees.

P1 is blocked in full until the complete reviewed/published dependency set and
CL-2F exist: no root pin, lockfile change, partial manifest change or per-row P1
claim lands earlier, and there is no stock/intermediate production stage. The fork
SHAs cannot be filled in from a movable branch, an aiongg ref or this document.

### 5.2 Root patch shape (only after exact SHAs exist)

Cargo `[patch]` sections do not propagate from a dependency's workspace into a consumer root. A
consumer that patches only `egui` will still get registry `egui-winit`, `egui-wgpu` or winit. The
root `Cargo.toml` must name every used family member at the same full SHA (illustrative, not a
manifest to land with placeholders):

```toml
[patch.crates-io]
egui                 = { git = "https://github.com/storytold/egui",  rev = "<REVIEWED_EGUI_SHA>" }
ecolor               = { git = "https://github.com/storytold/egui",  rev = "<REVIEWED_EGUI_SHA>" }
emath                = { git = "https://github.com/storytold/egui",  rev = "<REVIEWED_EGUI_SHA>" }
epaint               = { git = "https://github.com/storytold/egui",  rev = "<REVIEWED_EGUI_SHA>" }
epaint_default_fonts = { git = "https://github.com/storytold/egui",  rev = "<REVIEWED_EGUI_SHA>" }
eframe               = { git = "https://github.com/storytold/egui",  rev = "<REVIEWED_EGUI_SHA>" }
egui-winit           = { git = "https://github.com/storytold/egui",  rev = "<REVIEWED_EGUI_SHA>" }
egui-wgpu            = { git = "https://github.com/storytold/egui",  rev = "<REVIEWED_EGUI_SHA>" }
egui_kittest         = { git = "https://github.com/storytold/egui",  rev = "<REVIEWED_EGUI_SHA>" }
winit                = { git = "https://github.com/storytold/winit", rev = "<REVIEWED_WINIT_SHA>" }
```

The table is illustrative, not an exhaustive resolved-family list. Include `egui_glow`,
`egui_system_font_provider` and any other egui workspace crate selected by native/web/dev
features after metadata inspection. `egui_extras` is not directly used by SoundCraft; inspect
transitive test/optional graphs before deciding whether its patch is needed. `egui_plot` is not part of
the 0.37 workspace and is not a SoundCraft dependency. `wgpu` stays at 30.0.1 unless a separately
reviewed change is needed; winit stays 0.30.13-compatible.

Workflow pins:

```toml
craft-ui     = { version = "0.1.0", git = "https://github.com/storytold/craft-libs", rev = "<REVIEWED_CRAFT_LIBS_SHA>", features = ["serde"] }
craft-docpath = { version = "0.1.0", git = "https://github.com/storytold/craft-libs", rev = "<REVIEWED_CRAFT_LIBS_SHA>" }
```

Use one craft-libs revision for both if the reviewed diff only changes the UI crate plus unrelated
main changes; otherwise retain the current `craft-docpath` rev and document why. Do not use two
craft-libs revisions merely to avoid reviewing the merged commit. Review PR #199's actual edits
line by line: its `WidgetType`→`Role` mappings, `UiBuilder::id`→`scope_id`, and the fixture
ellipsis change are evidence of the API port; its lockfile and stale base are not to be copied.
No machine-local path patch is allowed in the committed manifest or in `.cargo/config.toml`.

### 5.3 Lockfile and compatibility checks

Before accepting P1, in a managed build job with a complete registry index:

1. `cargo metadata --locked --format-version 1` and assert every egui family package has exactly
   one package id/source from the selected git SHA, and `craft-ui` resolves to the reviewed
   craft-libs SHA.
2. `cargo tree -d --locked` shows no 0.36.2 `egui`/`ecolor`/`emath`/`epaint`, no duplicate
   `winit`, and no second `accesskit` family.
3. `cargo tree -e features -p soundcraft --locked` shows `eframe/clipboard`,
   `egui/default_fonts`, `egui/monochrome_emoji_fonts`, and the intended winit/accesskit features.
   Repeat for `-p soundcraft-web` under the wasm target and check `eframe` web clipboard/file
   features. Also inspect the SoundCraft UI test graph with
   `cargo tree -e features -p soundcraft-ui-egui --locked` (dev-dependencies included) and the
   production-only graph with `cargo tree -e features,no-dev -p soundcraft-ui-egui --locked`:
   `egui/default_fonts`, `egui/monochrome_emoji_fonts` and `epaint_default_fonts` must appear in
   the test graph. Keep the three roots distinct: the production binary, the SoundCraft UI test
   target and kittest's own test root. kittest's normal `[dependencies] egui` is `default-features = false`; only its own
   `[dev-dependencies]` enable fonts, and a dependency's dev-dependencies do not propagate.
4. `cargo check --locked --target wasm32-unknown-unknown -p soundcraft-ui-egui -p soundcraft-web`.
5. `cargo check --locked` from a clean cache; no source replacement, no local path, no branch
   dependency, and all git revisions reachable by a clean clone.
6. After the pin, add Dependabot `ignore` entries for `egui`, `ecolor`, `emath`, `epaint`,
   `epaint_default_fonts`, `eframe`, `egui-winit`, `egui-wgpu`, `egui_kittest` and `winit` (with a
   comment pointing here), or require the dependency PR to update all pins together. A daily
   single-package bump would otherwise recreate the split this plan removes.
7. Record `git rev-parse HEAD`, full `cargo tree` output, `Cargo.lock` hash and selected feature
   list in the task evidence.

### 5.4 Vendors and existing fixes

SoundCraft has no `vendor/` directory and no `[patch]` section today. It must not add a vendored
copy of egui to solve compile errors. The relevant external fixes are:

- E2+E3 in the suite fork (`egui-wgpu` surface fit and error-scoped configure/renderer creation),
  relevant to `graphics.rs` and resize/GPU failure paths.
- E4 + winit W15 (regular Wayland clipboard) over the bounded async transfer API,
  the separate **W15P + E4/P2b** PRIMARY path, E19/E21 + W25/W26/W27 (non-Latin
  shortcuts, IME delete-surrounding), the **E21b/P7b** outgoing surrounding-text
  state and **P7c**'s selected `#8763`/`#8748` fixes, as one accepted **P8** fork
  set. A pin without the bounded W15 transfer, W15P/P2b, E21b/P7b or the P7c fixes
  supports none of the complete clipboard, PRIMARY, surrounding-text or
  selection/deferred-child acceptance rows.
- The app-local Windows DX12/FXC choice in `apps/soundcraft/src/graphics.rs:11-30` is a
  SoundCraft fix and must be preserved. Do not delete it because the dependency changed.
- `muda` 0.21.2, `rfd` 0.17.2 and `image` 0.25.10 are non-egui dependencies; keep them unless a
  separate, reviewed reason exists. `build.rs` Windows resources and `ATTRIBUTION.md` assets are
  unaffected.

No blanket vendor deletion is authorized. If a future consumer of SoundCraft carries a vendor,
its disposition is tracked in that consumer's plan, not here.

### 5.5 Language work (#198) integration decision

SoundCraft has no `craft-i18n` dependency and its menus/messages are English literals. The
language work tracked by craft-libs issue #198 is not a merged UI-localization contract. This
migration therefore consumes the fork's text/input mechanisms where they affect existing fields
(E5/E12–E16 bidi shaping and grapheme editing, E19/W25 keyboard layouts, E21/W27 IME
incoming deletion, E21b/P7b outgoing surrounding text),
but does **not** add a locale switch, a `craft-i18n` catalog, an RTL application layout, or a
second bidi pass. It also does not use E17 `Context::set_language` / E18
`Frame::set_ui_language` until a separate localization plan defines the UI language and its
AccessKit root language. Existing logical strings remain the save/clipboard/accessibility source
of truth; never feed already-reordered visual text back into a `TextEdit` or the document.

## 6. Root `Ui`, multi-pass and logic-only semantics

### 6.1 Actual host contract

Both native `epi_integration.rs::update` and web `app_runner.rs::logic` call
`App::logic` followed by `App::ui` inside the `Context::run_ui` closure, once per
layout pass. Both also call `App::logic` through `run_logic` while hidden. Native
logic is not a once-per-frame hook outside `run_ui`. Keep the existing public
`SoundApp::logic`/`ui` entry points and pass the existing root Ui; no recursive
`Context::root_ui` bridge is needed.

`run_logic` refreshes viewport/focus information but leaves events, time and UI
state from the last pass. After a completed `run_ui`, `current_pass_index()` is
zero because `end_pass` takes the platform output. The existing first-pass guard
does not, by itself, skip an ordinary hidden tick after a completed multipass frame.
Do not create a test that expects that false failure. Visibility also does not
identify the callback phase: `RequestPaintWhileHidden` can run UI while the window
remains minimized/occluded.

### 6.2 Background work and current input

Extract small private methods only where they clarify the existing responsibilities:

- `logic` polls consumed control/plugin/result queues, revision-based autosave,
  transport/device state and capture deadlines. Multiple calls are safe: one queued
  command is consumed once, one pending open is taken once, revision changes are
  applied once and monotonic deadlines determine timed work. A first-pass guard may
  remain around genuinely per-layout-frame diagnostics, not the entire async inbox.
- `ui` calls an input-preparation method before layout for pointer/keyboard
  release and shortcuts. Raw dropped files are captured and cleared at the top of
  `ui` (before the `!fonts_ready` early return) into a one-shot queue, then routed
  after the content has laid out the current sampler drop targets. Both are the
  known UI phase and run for every UI pass, including a forced hidden paint.
  Drain each input once; keep sampler hit routing and browser async read behavior.
  Native/web host `ui` methods perform platform-specific open/import routing from
  the resulting one-shot actions.
  Synthetic input requests while hidden request a UI paint or return the documented
  error; they never report an unperformed action.
- `App::raw_input_hook` runs once per host frame in `prepare_raw_input`, before
  `run_ui`, and also on logic-only ticks (`eframe/src/native/epi_integration.rs:315-340`;
  web `app_runner.rs` hidden branch), so it is not a UI-pass hook. A synthetic step
  drained there on a logic-only tick has not been applied by a UI pass. Keep exactly
  one unapplied gesture step pending across repeated hidden logic ticks until a full
  UI pass consumes it; later hidden ticks must neither apply nor discard it, and a
  press/release pair must stay together rather than collapsing into one event. Test
  repeated `run_logic` ticks with a queued click and key, then a full UI pass that
  applies each exactly once and in order.
- Replace transport simulation's `ctx.input().time` delta with a monotonic clock
  whose last instant is updated after each advance. A second pass observes only
  additional elapsed time, never another full frame delta. Autosave and screenshot
  deadlines use the same injectable clock concept. Audio position remains owned
  by the audio player; do not move real-time work onto the UI callback.
- Automation writes remain keyed by transport sample position and `last_write_at`.
  Split `write_automation(ctx)` so background work uses an explicit gesture state,
  not stale pointer input. Focus/visibility loss cancels the transient Touch drag;
  Latch/TouchLatch state continues according to its existing engine semantics.
  A current UI phase refreshes gesture state before processing new edits.
- Fonts/theme installation remains idempotent. Frame statistics count rendered
  frames rather than hidden ticks or layout passes. Bounce processing uses an
  elapsed-time work budget and cannot get an extra full budget for every discard.

Native/web hosts use the same logic/UI ordering. Test helpers must reproduce it:
call `logic` then `ui` inside the UI closure; call only `logic` from `run_logic`.
Do not simulate a fictional native host by calling background work once outside
`run_ui`. Existing tests may keep their public entry points. Add a small helper
for new phase-sensitive tests, without rewriting every test for style alone.

### 6.3 Side-effect placement

| Work | Current anchor | Class | Required change / regression |
| --- | --- | --- | --- |
| Font install and theme application | `lib.rs:1136-1154`; `fonts::install`; `theme::apply` (`theme.rs:453-480`) | `logic` host-once pre-pass, before any layout | Keep both in `logic` (or a named host-once pre-pass called before layout) and idempotent by `fonts_installed`/`theme_applied`. `ui` currently returns before layout while fonts are not ready (`lib.rs:1232-1239`), and `theme::apply` mutates global style branches before the root `ui.set_style(ctx.global_style())` call. Do not move this to `ui`: a hidden first tick must install fonts and a direct `logic` call must switch theme. Test hidden first frame, a multi-pass frame, and repeated direct `logic` calls. |
| Control queue drain | `lib.rs:1158`, `control.rs:49-65` | `logic` | Consume each queued request once; hidden ticks still answer requests. Test `ui.inspect`/`engine.execute` while minimized; two-pass frame executes command once. |
| Recent-session/prefs notification | `lib.rs:1159`, host `main.rs:71-74` | `logic` | Host save remains idempotent; hidden tick must not lose the pending recent change. Test after a hidden tick. |
| Plugin editor edits | `lib.rs:1160`, `apply_editor_edits` | `logic` | Native plugin editors idle between paints; run once per tick. Test with a fake editor queue if possible; otherwise document native-only. |
| Bounce rendering | `lib.rs:1161`, `bounce.rs:22-38` | `logic` | 30 ms slice per tick; hidden window must continue. `bounce::show`'s Escape check stays in `ui` (pass) or consumes current input. Test bounce progress across `run_logic`. |
| Pointer/gesture cleanup and automation merge | `lib.rs:1162-1170` | `logic` for writing/merge; `ui` input preparation for pointer release | Do not read stale `ctx.input` for pointer/keyboard in hidden ticks. Capture `pointer_down`/`keyboard_wanted` in `ui` input preparation, and clear a transient `Gesture::Fader`/pointer-down on entry to hidden logic so it cannot stick; `write_automation` uses the cached gesture/touched state, not stale input. Test a hidden tick does not write a spurious point. |
| Output/input device trouble | `lib.rs:1171-1172` | `logic` | Device errors arrive off-frame; one handling per tick. |
| Transport/player position | `lib.rs:1173-1180` | `logic` | Player position is read from the player; sim position only advances with real time. Hidden audio playback must continue. Test `transport.play` then `run_logic` with a fake/no-player path. |
| Open inserted plugin | `lib.rs:1181` | `logic` | Pending selection must open once, not per pass. Test a two-pass frame with `pending_open`. |
| Autosave revision/time | `lib.rs:1182-1184`, `672-708` | `logic` | `autosave_changes` is revision-idempotent; `autosave` is wall-clock based. Hidden window must still autosave. Existing `a_hidden_window_still_autosaves` must be extended to hidden-after-multipass. |
| Input recorder ensure | `lib.rs:1185-1189` | `logic` | One open attempt per tick; test no duplicate recorder on two passes. |
| Repaint deadlines | `lib.rs:1191-1195` | `logic` | Keep `request_repaint_after` for autosave/plugin editors. Add a short deadline while screenshots are pending. |
| Screenshot collection/issue | `control.rs:244-279` | `logic` | Callback + weak slot; explicit count/byte admission and in-flight budget; one reply. See §6.4. |
| Shortcuts | `ui` `lib.rs:1244`, `shortcuts.rs` | `ui` input preparation | Input-derived; hidden `run_logic` must not consume stale key events. |
| Dropped files | native `main.rs:52-58`, `sampler_window.rs:88-100`; web `web.rs:112-160` | known UI phase (`ui`); never `logic` and never gated on `visible()` | Capture and clear `raw.dropped_files` at the top of `ui`, before the `!fonts_ready` early return (`lib.rs:1232-1239`), together with the drop-time pointer position, into a one-shot pending queue; process it after layout once fonts are ready, on every UI pass including `RequestPaintWhileHidden`. Preserve sampler placement and web byte reads. Test a fonts-not-ready first frame (ordinary and forced-hidden), an ordinary paint and a forced hidden paint, each followed by hidden ticks, with exactly one import. |
| Native macOS menu | `main.rs:43-50` | `logic` | `muda` events are not egui input; sync is time-throttled. Hidden menu actions still apply. |
| Host title/window size/prefs | `main.rs:64-87` | `logic` or a host-once helper | Viewport info is current in `run_logic`; title/size updates are idempotent. Do not save prefs from every pass. |
| `ui_shot` test driver | `examples/ui_shot.rs:65-70` | test helper (optional) | The compatibility `logic` driver makes the current call safe; a helper that calls `logic`/`ui` input preparation explicitly is clearer and should be used for new scenarios. |
| `Harness` tests | all `build_ui_state` closures | test helper (recommended) | Add a helper matching native/web order: raw input hook, then logic/UI inside each pass; hidden tests call logic only. Assert side-effect outcomes rather than one hook call; no broad test rewrite is required. |

### 6.4 Screenshot callback correlation

Use the shared craft-ui design selected after review: a weak callback to a
cancelable, request-owned completion slot. Each admitted request owns its token,
reply/path, monotonic deadline, settle state and slot. The callback holds only a
weak slot reference; timeout/cancel/close drops the live slot so a late callback
releases its image without a file write or second reply. The live slot owns a
viewport wakeup handle; release its lock before waking the context/viewport. Never
strongly capture the Context in a callback queued inside that Context.

The eleven craft-ui consumers need CL-1 and the frozen CL-2F at this stage; the
DesignCraft/GridCraft pilot instead uses the provisional CL-2 API, and its adapters
must precede CL-2F. DeckCraft, FigCraft and ArtCraft Native use small
contract-compatible local adapters, and `craft-ai-integrations/ai-egui` has no
capture path, so the earlier "thirteen craft-ui consumers" shorthand is
superseded. SoundCraft is one of the eleven and keeps its own request IDs, paths,
settle delays, deadlines and replies outside the helper.

Admission is explicit and checked before a capture is issued. The shared helper's
default is **four outstanding captures, 128 MiB per image and 256 MiB aggregate**;
an application may choose stricter caps. SoundCraft keeps its own **8-request
pending queue**, **4 in-flight captures**, **64 MiB per captured image** and **192 MiB
aggregate encoding peak** (the image plus the raw/PNG/base64 copies built while encoding);
a further request is rejected with `"screenshot queue full"` and
writes no file. Before a request is issued, reserve a checked
`width_px * height_px * 4` from the capture-time surface/geometry estimate clamped
by `max_texture_side`, with overflow and aggregate checks; it is not a
post-completion guess. A request that cannot be reserved is rejected with
`"screenshot too large"`.

Bind the known issuance geometry and viewport/document generation (epoch) to the
request: viewport points, `pixels_per_point`, zoom and the surface-size estimate.
Revalidate that identity at issuance and completion rather than comparing current
settings. Any geometry/DPI/zoom change before the image is saved cancels and
retries within the deadline — even if the values change back before the save,
because the pixels were rendered for the earlier geometry. A second mismatch fails
with `"screenshot stale"` and never associates old pixels with new state. A request
that misses its deadline returns `"screenshot timed out"`. These errors travel
through the existing control-protocol error field; success still produces exactly
one reply and at most one file.

E2 `surface_fit` can render a smaller surface than the window
(`egui-wgpu/src/winit.rs:45,446`) and its `render_scale` is private (`:25`), so
returned image dimensions need not equal viewport points × `pixels_per_point`.
Derive the effective scale from the returned image dimensions plus the
capture-time logical viewport metadata, test a surface-reduced window, and accept
that capture instead of rejecting it as stale.

The callback delivers only pixels. The application owns the strong completion slot
and waker; the callback holds a `Weak` reference, releases the lock before waking,
and does not wake itself. `request_screenshot` sends the viewport command that
requests the paint; the callback itself requests no frame, and a post-completion
wake is only for collection/reply. A screenshot that is queued but not yet issued
— including a hidden request that is settling — needs an explicit
`RequestPaintWhileHidden` from `logic` so the request can reach the UI phase.
Encoding, file IO and replies happen after collection without holding the slot
lock. Keep a monotonic deadline if no frame is presented. No unbounded mpsc queue
is the default design; a bounded nonblocking channel is a measured fallback only if
the shared slot design fails a demonstrated requirement.

kittest completes every screenshot callback from **one** rendered image
(`egui_kittest/src/lib.rs:914-942` collects all viewport callbacks and completes
each with the same image), so it can test single-viewport request/cancel/lifetime
behavior only. Cross-viewport pixel correlation needs native two-window evidence
or a custom per-viewport renderer that renders each viewport separately.

Test 0, 1, N and N+1 requests; two large concurrent images; a resize race; forced
discard; cancellation; timeout; out-of-order completion; shutdown; hidden wakeup;
and retention returning to zero after drain. Each succeeds once or returns one of
the defined errors. A late callback cannot create a file or send a second reply.
Synthetic hidden input must visibly take effect on the requested UI paint or return
an explicit failure.

### 6.5 Lifecycle regressions and deliberate failure probes

- Complete a two-pass frame, then run multiple hidden ticks through the same public
  `logic` method. Verify autosave, control replies, bounce and transport continue;
  assert the actual pass index behavior rather than inventing a stale nonzero index.
- Drop one audio/session file through a real UI phase, then run hidden ticks and a
  restore. It imports exactly once. Repeat with a forced hidden UI paint
  (`RequestPaintWhileHidden`): the drop must be handled on that paint and not
  replayed afterwards. Repeat again on the very first frame, while fonts are not
  ready and `ui` returns before layout; the drop must survive in the one-shot queue
  and import once on the first laid-out pass. Gating on `visible()` must make the
  forced-hidden case fail; leaving the raw list uncleared must make the repeated-tick
  case fail; reading drops only after layout without pre-layout capture must make
  the first-frame case fail.
- Queue a command and force a discard. Both native-shaped and web-shaped drivers
  produce one mutation, one reply and one undo transaction. Deliberately replay the
  same consumed command to prove detection; merely moving a drained queue to UI is
  not guaranteed to duplicate anything and is not a valid mutation by itself.
- Advance an injected monotonic clock while egui input time remains unchanged.
  Hidden simulation advances by that elapsed interval; an extra layout pass cannot
  apply the same interval twice. Restoring the stale input-time clock must fail.
- Exercise Touch, Latch and TouchLatch across pointer release, focus loss, minimize,
  hidden playback and restore. Check exact automation points and merge boundaries.
- Request a hidden UI paint with synthetic input and capture. Current input executes
  once even though `visible()` is false; ordinary logic-only ticks cannot replay it.
- Complete a capture after timeout/cancel/shutdown: no file, no second reply and no
  retained image. Use a deliberately misrouted request slot to prove the pixel and
  reply assertions detect cross-request delivery.

## 7. Panels, docking, IDs, viewports and geometry

- Keep panel order: `menus::menu_bar` (`menus.rs:224`) before the Edit/Mix content; Edit's
  toolbar/status/lower-tabs/universe panels at `edit_window.rs:109-113`; the `DockArea` child at
  `docking.rs:410-434`; all floating windows/areas after the main content (`lib.rs:1254-1265`).
  `Panel` constructors accept `impl AsIdSalt` in 0.37; keep stable string salts and do not rename
  them during the mechanical migration.
- `DockArea::new(egui::Id::new(("soundcraft-docking", edit)))` and
  `cancel_drag::<Panel>(ctx, egui::Id::new(("soundcraft-docking", edit)))` at `docking.rs:332,410`
  must change together to `Id::unique`. A mismatch leaves a drag active after a workspace
  replacement; `tests/docking_workflows.rs` and craft-ui's cancellation tests are the regression.
- The custom Tracks/Groups scrollbar (`panels.rs:155-185`) must use
  `out.state.clamped_offset().y` for drawing and `ScrollArea::vertical_scroll_offset` for a
  pending target. Do not write the private `State.offset`. Because the target is applied next
  frame, `tests/tracks_scroll_bar.rs` needs a one-step settle after a page click; assert the
  offset is not overwritten by a wheel event in the intervening frame.
- `soundcraft-midi-body` temporary data (`docking.rs:429`, tests `pro_tools_keys.rs:216`,
  `midi_panel.rs:16`, `new_midi_clip.rs:133`) uses `Id::new`; convert all producers and readers to
  `Id::unique` with the same string. Do not leave a stale reader.
- Viewports: `mix_window::show_separate` gives the child its own `&mut Ui`; do not call
  `root_ui` there. Preserve `focus_mix`, close-request handling, and `separate_closed`. In headless
  tests `Context::embed_viewports` is true by default; native eframe sets it false. Headless tests
  can assert state transitions and no panic; native X11/Wayland/Windows evidence is required for
  real popout focus/close, mixed DPI, and parent-hidden behaviour. Upstream #8748 is
  **closed and unmerged**, not 0.37 behavior. The current Mix window uses immediate
  viewports (`mix_window.rs:25-38`), so it does not inherit the deferred-child fix.
  Keep the immediate-viewport limitation explicit. If the Mix window later moves to
  `show_viewport_deferred`, require the fork's tested hidden-parent/deferred-child
  fix plus open/update/close under an overdue or occluded parent before claiming it.
- Geometry/hit assertions: at 1600x1000 and 1400x900, with 1x/1.5x/2x, assert the menu bar height,
  timeline rect, track row heights, clips panel width, mix strip/narrow strip widths, dock tab
  rect, and the `ui.inspect` `edit_layout` values. Existing tests already derive many of these
  from painted shapes; retain that style.
- Clip/nesting assertions: docking bodies must clip to their panel rect intersected with the
  parent clip (`craft-ui` `surface.rs` uses `set_clip_rect`), and `minimum_body` must not paint
  outside its scroll viewport. Add a shape-visibility assertion for a floating panel overlapping
  the canvas and for the MIDI body at its minimum size.
- Persistence: `UiState`/`Workspace` remain the source of truth for layout, not egui ids. Test
  float/dock/close/reopen, reload, hidden panels and narrow Mix after the craft-ui update.

## 8. Input, IME, clipboard, PRIMARY and scrolling

- Enable `eframe/clipboard` on native. This is a real behavioural restoration, not a cosmetic
  feature: 0.36's eframe target dependency forced `egui-winit/clipboard`
  (`eframe 0.36.2 Cargo.toml` native dependency), while 0.37 exposes it as an optional
  `clipboard` feature. Without it, copy/paste of session names and text fields uses egui's
  in-app simulated clipboard.
- Test copy/paste from every text surface: session name, track/bus/marker/clip rename, Spot
  fields, search fields and plugin/bus search. Native external paste and middle-click PRIMARY
  require a real X11/Wayland session; headless tests only establish event routing.
- egui 0.37 has `Event::MiddleClickPaste` and `MouseWheelSource`. E4/W15 replaces smithay with
  winit's **regular** Wayland clipboard over the **bounded async W15 transfer API**: a total
  deadline from admission (2 s default, never reset per poll), a cumulative 16 MiB byte cap, at
  most 4 concurrent reads, 2 writer threads and queue depth 8, cancellation, stale-result
  discard and a live owner-specific completion waker. The transfer handle owns result/cancel/wake
  state and cancels on drop; no unmanaged thread is spawned per request. A bounded synchronous
  compatibility read may remain but must not run on the egui event path. PRIMARY is
  separate: it requires winit W15P plus egui E4/P2b, on the existing connection/per-seat
  lifecycle, with one regular
  `wl_data_device` per seat; a selection claim validates the triggering serial and seat
  synchronously while fresh, and only serving is asynchronous. A build with only W15/E4 has no
  native PRIMARY path (the candidate falls back to arboard/smithay). PRIMARY acceptance runs on
  wlroots/Hyprland, KWin and Mutter plus X11, and covers **keyboard- and pointer-triggered**
  selection serials and freshness per seat, owner/source/seat changes, no arbitrary first-seat
  fallback, empty/absent offers, self-paste, and slow/canceled/trickling/oversize transfers.
  SoundCraft has no password field today, so password exclusion is a fork-level requirement
  rather than an app workflow; the unchanged-selection non-reclaim rule still applies to every
  app selection. Record each compositor's capability rather than assuming parity.
- `ui.scroll` (`control.rs:187-197`) must set `source` (`Wheel` for line units, `Trackpad` for
  point units). Preserve `TouchPhase::Start/Move/End` and test that 0.37 kinetic/rubber-band
  behaviour does not turn a synthetic paging action into an unbounded scroll. For exact offset
  tests, set the test style's `spacing.scroll.kinetic.decay_time` to 0 (or assert direction and
  bounds instead of exact pixels).
- TextEdit click-away: upstream #8763 reports that a click outside a focused field collapses its
  selection before the clicked button runs; proposed PR #8764 is **open/unmerged** (head
  `6eb460a2a6d537d312a80579cb801895509deaed`), not released behavior. The suite fork owns the
  selected P7c fix with pointer and keyboard regressions. SoundCraft has inline rename fields
  (`panels.rs:411`, `dialogs.rs` path/name fields, `spot.rs`) and buttons beside them. Require
  the fork fix; permit an app-local workaround only with preserved selected range and undo
  semantics plus an explicit removal condition. Test select-then-click on a neighboring button
  through both pointer and keyboard paths.
- IME/E21/W27: test preedit, commit, replacement, cancellation and click-away in every rename path.
  Incoming deletion/commit is only half the contract. The accepted pin must satisfy the complete
  shared egui/winit **E21b/P7b** plus W27 lifecycle contract, not a weaker subset: an
  owner/context generation identity that is present even when surrounding text is
  **Unavailable**; a distinct **Available-empty** state sent explicitly as
  `set_surrounding_text("", 0, 0)` plus commit, while Unavailable omits the request after a
  committed clear boundary; a bounded <=4000-byte payload with UTF-8 byte cursor/anchor,
  preedit excluded and no full-document allocation for this new payload (the existing TextEdit
  change-detection/undo clone is recorded separately); clearing cached state and committing the
  disable/enable boundary on owner or available/unavailable transitions, since changing purpose
  or sending empty text is not a clear; cancelling pending preedit/commit/delete on Leave,
  disable, reset and seat removal; the final completed pass's `PlatformOutput` as authoritative
  **including `None`**, not `ime.or(self.ime)` accumulation; and a current-epoch `Done` with a
  mismatched serial applying incoming edits **once** while deferring outgoing state until a
  matching serial, with obsolete owner/epoch batches invalidated. SoundCraft's fields are
  built-in `TextEdit`s and inherit the producer; it has no custom document editor to adapt.
  Test outgoing text immediately after delete+commit and deletion-only batches, preedit
  exclusion, supplementary/combining/RTL text, field and two-window switches, focus
  loss/reset/late serial, current-epoch serial mismatch versus stale epoch,
  password/no-context clearing, and initially-empty/available-empty/unavailable transitions.
  Without this complete contract the "updated surrounding text" row is an unresolved gate, not
  a pass. The locked fork references `unicode-bidi 0.3.18` (Unicode 16) and
  `unicode-segmentation 1.13.3` (Unicode 17); conformance vectors must be pinned to those data
  versions. A similar Mozilla report (bug 2080478) is motivation, not proof of a SoundCraft
  defect.
- RTL/CJK names: the fork's E5/E12–E16 shape and reorder RTL text; SoundCraft's own layout remains
  LTR. Test Arabic/Hebrew session names and Korean/Japanese names using the pinned fixtures from
  §9.2 (Noto Sans CJK SC covers Hangul; BIZ UDPGothic covers Japanese), not a nondeterministic
  system fallback. Include combining marks and emoji clusters. Do not add an app-wide
  `right_to_left` style in this migration; the product UI is LTR and the fork's Style mirroring
  is not enabled.
- Non-Latin keyboard layouts: E19/W25. Test a Russian/Arabic/Hebrew layout on
  Windows and Linux: letter shortcuts must not fire from a non-Latin character; Ctrl/Cmd shortcuts
  still work. If P1 only, record this as an unresolved gap, not a pass.

## 9. Fonts, glyphs, accessibility and theme

### 9.1 Font feature decision

0.36's default fonts included emoji faces; 0.37 makes `default_fonts` opt-in and moves emoji
fonts behind `monochrome_emoji_fonts` (while eframe can add a system color provider). SoundCraft's
loader at `fonts.rs:109-139` starts from `FontDefinitions::default()`, prepends a system regular/
bold and Hangul fallback, and assumes bundled defaults exist. Therefore:

- Enable `default_fonts` and `monochrome_emoji_fonts` for the `egui` package used by `ui-egui`
  and the native app. `eframe/default_fonts` covers the binary, but not an isolated
  `cargo test -p soundcraft-ui-egui` graph. `egui` 0.37 declares `default = []`;
  `egui_kittest`'s normal `[dependencies] egui` is `default-features = false`, and only
  kittest's OWN `[dev-dependencies]` enable `default_fonts`/`monochrome_emoji_fonts`. A
  dependency's dev-dependencies do not propagate, so kittest supplies no fonts to SoundCraft's
  tests. Prove this per root with `cargo tree -e features` for the production binary, the
  SoundCraft UI test target and (separately, out of scope) kittest's own test root; treat the
  no-bundled-fonts prediction as a resolution claim until the command runs.
- Keep `eframe/system_fonts` **off** for this migration. That avoids eframe's new provider, not
  all font-discovery cost: SoundCraft's own loader still runs up to three `fc-match` calls
  (regular, bold, Hangul) and reads the returned files on Linux/FreeBSD (`fonts.rs:53-79`,
  `:109-139`). Measure `fc-match`, file reads and font install separately from egui work, cold
  and warm cache, and do not attribute the retained cost to egui. Re-evaluate a system provider
  only with startup measurements and coverage tests.
- Keep the runtime `SOUNDCRAFT_NO_SYSTEM_FONTS` escape hatch. It is useful for reproducible UI
  tests and for minimal systems.
- Keep the app's `FontId` contract (`theme.rs:439-451`: proportional, monospace and the explicit
  `"bold"` family) and its sizes; do not inherit a new 0.37 default text style for track headers,
  mixer labels or counters. The bold family must retain the same fallback order after the
  `FontDefinitions` feature change.
- `ATTRIBUTION.md` currently names Ubuntu-Light, Hack, Noto Emoji and emoji-icon-font as fonts
  shipped inside egui. `monochrome_emoji_fonts` keeps that statement true; if the feature is not
  enabled, the statement must be corrected. No new repository asset row is needed because these
  fonts remain inside the `epaint_default_fonts` dependency; its license texts travel with it.

### 9.2 Glyph coverage findings from the bundled font cmaps

The following is source/asset inspection of
`egui/crates/epaint_default_fonts/fonts/*.ttf` (cmap scan), not a rendered test. It identifies
specific risks for `egui_kittest`'s new `MissingGlyphPolicy::Panic`:

| Glyph / use | Anchor | Bundled coverage | Required action |
| --- | --- | --- | --- |
| `▸` U+25B8 in status text | `lib.rs:848,851,853,888`; docs text | Only Hack (monospace); proportional Ubuntu lacks it | Replace with `›`/`>` or draw it; add to a glyph test |
| `▾` U+25BE in menu labels | `panels.rs:477` (`Presets ▾`) | Only Hack (monospace); proportional Ubuntu lacks it | Replace with `›`/`v`/none, or draw the menu arrow; add to a glyph test |
| `✕` U+2715 close/delete buttons | `windows.rs:258,404`; `panels.rs:422` | emoji-icon-font, only with `monochrome_emoji_fonts` | Enable the feature; otherwise use `×` U+00D7 (Ubuntu has it) or a code-drawn close icon |
| `♯` U+266F sharp | `score_editor.rs:122` | None of the bundled fonts | Draw original sharp strokes in `icons.rs`/`score_editor.rs`, or bundle an OFL music font with an ATTRIBUTION row |
| `𝄞` U+1D11E treble clef | `score_editor.rs:67` | `emoji-icon-font` (present in its cmap) | Coverage exists when `monochrome_emoji_fonts` is enabled. Replacing it with a drawn original clef is a visual-quality choice, not a missing-glyph fix; assert whichever face is chosen |
| `⌘` U+2318 | `mix_window.rs:609` (mac tooltip) | `emoji-icon-font` (present in its cmap); mac system font also supplies it | Bundled coverage exists with `monochrome_emoji_fonts`. Keeping the mac-native glyph or drawing an original key is a visual-quality choice; in deterministic tests assert the bundled face. Do not globally allow missing glyphs |
| `📁`/`🔊`/`🔍`/`⬆` | `windows.rs:327,355`; `panels.rs:206` | Noto Emoji with `monochrome_emoji_fonts` | Enable the feature and test in every family |
| `♩` U+2669 | `panels.rs:352` | emoji-icon-font | Covered by `monochrome_emoji_fonts`; assert |
| `−`/`∞` | `mix_window.rs:1079,1088` | Ubuntu-Light | Covered; assert |

Add a `fonts::tests` (or a kittest-only test) that checks every literal glyph used by UI fixtures
in `FontFamily::Proportional`, `FontFamily::Monospace` and `FontFamily::Name("bold")`, with
`MissingGlyphPolicy::Panic` and a bundled-font-only font set. Use a dedicated test binary or a
`context_setup` that installs `FontDefinitions::default()` so system fonts cannot make the test
pass by accident. Do not call `HarnessBuilder::allow_missing_glyphs()` for the main suite; use it
only in one explicitly named test that demonstrates the fallback is intentional.

Separate the **bundled UI-glyph test** (`▸`, `▾`, `✕`, `♯`, `♩`, `𝄞`, `⌘`, `📁`/`🔊`/`🔍`/`⬆`)
from **document-text multilingual tests**. The latter use the pinned fixture set from the egui
plan §10.6; do not infer multilingual coverage from the bundled faces:

- craft-fonts at `2481a8a324b986a45954e90045ed1f8f2efaaf3c`, each OFL-1.1 with its
  adjacent `OFL.txt`:
  - Noto Sans Arabic `63111b5b2e074dd48cc67692e0a2726d86ee94c1c37fe8598257b7b4e87e869e`
  - Noto Sans Thai `5a1c559bb539583c8a1fd99d1c5b9491e5e14478c9cd2bd0970d5c3096cc9ef8`
  - Noto Sans CJK SC `2c76254f6fc379fddfce0a7e84fb5385bb135d3e399294f6eeb6680d0365b74b`
  - BIZ UDPGothic Japanese `258d7156c165f2ff774b6efee637c22c3b950de0d8a10e501137061bc8085d01`
- Noto Sans Hebrew variable font from google/fonts at
  `8b0a1d0f5983c89bc2b93f1b5fb55f9e252744b5`, SHA256
  `7ef36a2c3593758cdb622e1bdef4f84523e92fbc3ccc667438dd80ff54c2de88`, weight 400 /
  width 100, with its `OFL.txt`.
- Noto Color Emoji v2.047 from googlefonts/noto-emoji at
  `22e564626297b4df0a40570ad81d6c05cc7c38bd`, SHA256
  `39ee3c587e10e89669b9ff32703261d10d5f9c4dd5ad147b6b5a1c5200591817`, OFL-1.1.

Fetch them into the task's explicit fixture directory, keep them out of app source unless the
asset policy adds an `ATTRIBUTION.md` row, and record file hashes and licenses in the evidence
manifest. A missing fixture or unsupported glyph must fail the row, never skip it silently.

### 9.3 Accessibility

`egui_kittest` now checks every visible input for a name after a settled step
(`accessibility.rs:35-70`); the check treats a label or non-empty placeholder as a name. The app
has many custom-painted controls and bare fields:

- `text_toggle` (`widgets.rs:117-125`) draws a key and only calls `on_hover_text`; a tooltip is not
  an AccessKit name. Add `resp.widget_info(|| WidgetInfo::selected(Role::Button, enabled, on, tip))`.
- `fader`/`pan_knob`/`surround_panner`/`multi_meter` (`widgets.rs`) and custom track/timeline
  controls need `WidgetInfo` with `Role::Slider`/`Button`/`CheckBox`, label and value; output-only
  meters should not be advertised as inputs.
- `TextEdit`s with only an adjacent `ui.label` need `Response::labelled_by` or
  `Ui::label_inputs_by`; hint text is acceptable only when it is a real field name (search).
- `DragValue`/`Slider` instances without `.text(...)` in `ops_windows.rs`, `windows.rs`,
  `dialogs.rs`, `toolbar.rs` need a label.
- Group rows use `Role::CheckBox`; track rows use `Role::Button` with toggled state, matching
  `tests/side_lists_access.rs`. The role migration must keep those exact roles.
- Hidden controls must disappear from the tree; 0.37's #8621 makes invisible widgets hidden. Do not
  add placeholder nodes.
- Top-level `Window`/`Area`/`Popup` nodes need a stable accessible name where egui does not derive
  a useful one (0.37 added `Area::role`/`accessible_name` and improved popup/menu trees). Keep
  modal/dialog focus inside the modal and restore it to the invoking control on close.

`Harness::unnamed_widgets()` is a useful supplementary assertion, but it is not sufficient:
it walks only nodes that exist (`egui_kittest/src/accessibility.rs:14-21`), filters by
`is_input`, and a control that never calls `Response::widget_info` has no node at all. Roles
excluded from `is_input`, including `Role::Unknown`, also pass. A sweep can therefore be green
while custom buttons and faders are unreachable.

For every top-level window (Edit, Mix, MIDI Editor, Score, Video, Workspace, Appearance, About,
Session Info, Audio Health, Bounce modal, Save Changes, Spot and each popup), define an
**explicit expected node inventory**: role, accessible name, selected/checked, disabled and
value where applicable, plus absence where a control must not exist. Look nodes up with
`get_by_role`/`get_by_role_and_label` and assert counts so an absent node fails. Cover every
custom-painted control, not just the ones a workflow clicks. `unnamed_widgets().is_empty()` runs
as an additional check.

Mutation controls: (a) remove a `widget_info` call entirely; (b) set a role to `Role::Unknown`;
(c) remove an accessible name; (d) hide a control and assert it disappears. Each must fail its
inventory test; remove the deliberate defect afterwards. Add a test that opens a popup and
checks the popup tree separately from the window tree.

### 9.4 Theme and style

`theme::apply` (`theme.rs:458-476`) overwrites `Visuals`, item spacing, button padding and
tooltip delay for both dark/light style branches and pins `ThemePreference::System` so later OS
appearance changes still arrive. Preserve this contract. The 0.37 default style is roomier
(buttons/corner radius); if `theme::apply` does not override every changed field, explicit
geometry tests will catch it. Do not accept a global visual change because the upstream default
changed. Re-run light/dark/system at 1x/1.5x/2x and both `system_theme: Some` / `None`; keep the
existing `theme_docking.rs` and `appearance.rs` assertions.

## 10. Commands, undo, save/reopen and persistence

These contracts are below the UI and must not change:

- `SoundApp::run` (`lib.rs:418-453`) dispatches UI-layer commands via
  `menus::run_ui_command`, then `Engine::execute`; `app.quit` sets `quit_requested`.
- `Engine::execute` pushes one undo snapshot per changed document; continuous gestures use
  `execute_merged` (`mix.volume`, `mix.send_level`, automation pass). Every additional logic run
  or UI pass must not create a second undo step.
- `session.save*`/`session.open*` remain engine/io operations; UI changes must not intercept them
  or require a paint.
- `UiState` prefs (`window_size`, `window_maximized`, `ui_scale`, docking, recent sessions) are
  saved by the host; hidden logic must still update the recent flag.

Regression assertions:

1. A two-pass frame triggered by a popup/menu executes one command and one undo step.
2. Fader/send drag still produces one merged undo step; a pass with no pointer input does not
   close the merge prematurely.
3. Save → close → reopen preserves tracks, clips, fades, mixer levels, sends, automation, markers,
   groups, busses and selection; `save_changes.rs`, `save_as_replace.rs` and `recovery.rs` stay
   green and gain a richer fixture.
4. Close/reopen a floating dock panel and restore its workspace; `docking_workflows.rs` keeps
   command ownership and transport commands.
5. Recovery after a crash/exit without Quit still offers the autosave; hidden logic must not
   stop the autosaver.
6. `ui.screenshot`/`ui.inspect` control replies remain one request → one reply, including hidden
   ticks and timeouts.
7. A mutation probe that makes `run` execute twice must fail the undo assertion; a probe that
   moves autosave into `ui` must fail the hidden-autosave assertion.

## 11. GPU, video, textures and rendering

- Preserve `apps/soundcraft/src/graphics.rs:11-30`: Windows initializes only DX12 and defaults to
  FXC because GL initialization and missing DXC/DXIL break real machines. The 0.37 family keeps
  wgpu 30.0.1 and winit 0.30.13; do not change these defaults as part of the egui bump.
- E2+E3 (surface fit and error-scoped configure/renderer creation) should be required in the
  fork/release pin if SoundCraft keeps wgpu; verify resize, maximize, DPI changes, and device/
  surface failure paths return errors instead of panicking.
- `VideoPool` (`video_track.rs:281-430`) uses `load_texture("video-frame", ...)`,
  per-thumbnail names, and `TextureHandle::set` for the current frame. Tests set
  `max_texture_side = Some(8192)`; keep that and add a test for an oversized frame being rejected/
  clamped without a debug assertion.
- Screenshot callback images are CPU `Arc<ColorImage>`; do not upload them as textures. Encode PNG
  and write files only in `collect_screenshots` on the UI thread.
- `egui_kittest` default `RendererOptions::PREDICTABLE` gives deterministic test images; native
  eframe uses its own defaults. Record the renderer/backend/options for every screenshot and
  performance run. Software-rendered Windows and a real GPU are separate evidence rows.
- 0.37 upstream reports that need reproduction, not automatic acceptance:
  - #8731: plain-text tessellation regression from the color-glyph check. SoundCraft is text-heavy
    (menus, track headers, mixer labels). Measure text tessellation in the frame benchmark; if
    confirmed, adopt a row-level fast path in the fork with mixed/color-row tests. Do not use a
    clamp-only or output-changing shortcut.
  - #8736: DX12/FXC shader compile cost in `egui-wgpu` 0.37. SoundCraft explicitly selects FXC.
    Measure renderer construction and first frame separately from the rest of startup. A
    float/`select` wrap implementation or a pipeline-overridable constant is acceptable only with
    Repeat/MirroredRepeat/clamp pixel tests; a clamp-only change is not equivalent.
  - #8512: system-font discovery/startup cost. Keeping `system_fonts` off avoids eframe's provider
    only; SoundCraft's own loader still performs up to three `fc-match` calls and file reads.
    Measure those, and font install, separately from egui startup, cold and warm cache.
  - #8763/#8764: TextEdit click-away selection. #8764 is open/unmerged; the fork owns the fix.
    Test the inline rename and Spot paths through pointer and keyboard (§8).
  - #8748: closed and unmerged, not 0.37 behavior. Current SoundCraft uses immediate viewports;
    keep the limitation explicit and revisit only if the Mix window moves to deferred viewports.
  - #8657 and #8577: Wayland clipboard/PRIMARY and bidi. PRIMARY means W15P + E4/P2b, not W15
    alone; test native compositor workflows, not just compilation.

## 12. Native, web and platform workflows

### 12.1 Native (Linux, Windows, macOS, FreeBSD)

- Linux X11 and Wayland with a named compositor: launch with `--demo --control 0 --no-audio`,
  inspect and screenshot via the control channel, exercise track rename, clip import, mix
  fader/send, docking drag/float/close/reopen, MIDI/Score, save/reopen, and copy/paste.
  The planning server has no `DISPLAY`/`WAYLAND_DISPLAY` session; this can only be run in a
  managed native environment, and must not be reported as passed from headless renders.
  Read-only device inspection found an i915 render node (`/dev/dri/renderD128`) but no display
  session; that is availability evidence only, not a qualified Vulkan/OpenGL compositor result.
- Wayland PRIMARY matrix: wlroots/Hyprland, KWin and Mutter, plus X11 regular clipboard. With
  W15P + E4/P2b installed, verify keyboard- and pointer-triggered selection serials and freshness
  per seat, owner/source/seat changes, no arbitrary first-seat fallback, empty/absent offers,
  self-paste, unchanged-selection non-reclaim, and slow/canceled/trickling/oversize transfers.
  Record each compositor's capability/version and `WAYLAND_DEBUG=1` protocol trace showing one
  regular `wl_data_device` per seat. A wlroots-only pass is not the matrix.
- Two-viewport screenshot correlation: run native two-window evidence or a custom per-viewport
  renderer. kittest renders one image for all callbacks and cannot prove this.
- Windows: use a uniquely owned clone from `/home/codex/work/VMs/craft-windows` and its
  `ACCESS.txt`/`READY.json`; build with the reviewed `Build-DesignCraft`-style equivalent for
  SoundCraft or `cargo build --release --locked -p soundcraft`; verify FXC/DX12, mixed DPI,
  native file picker, save/reopen and screenshots. The baseline uses the software renderer, so
  physical-GPU/audio claims are separate and unresolved.
  Capacity gate before allocation: the default 128 GiB full clone does not fit the recorded
  allocator snapshot (about 23.6 GiB and 37.7 GiB allocatable on the two drives, not combinable).
  Inspect `task-storage.py status` first. Use a checksum-verified overlay clone with a reservation
  covering expected guest growth (at least the documented 10 GiB minimum) if it fits, or record
  the Windows row as blocked by capacity. Do not lower the reservation to pass the guard.
- macOS: native menu (`muda`), traffic lights, ⌘ shortcuts, Metal/wgpu, IME commit/deletion,
  file picker and termination need real macOS evidence. Cross-compilation is not acceptance.
  If unavailable, this is an explicit rollout blocker, not a pass.
- FreeBSD: existing nightly workflow; compile/packaging can be checked, but native UI/IME/audio
  acceptance needs an actual FreeBSD environment (not provided here).
- Native control channel: use `--control 0` and `SOUNDCRAFT_NO_PREFS=1`/a scratch
  `SOUNDCRAFT_CONFIG_DIR` for isolation; never arm/monitor/record in an automated UI run
  (`docs/control-protocol.md:22-25`).

### 12.2 Web (wasm)

- `cargo check --target wasm32-unknown-unknown -p soundcraft-ui-egui -p soundcraft-web`
  (part of `cargo xtask wasm`) is a compile gate only.
- `cd apps/soundcraft-web && trunk build --release` produces `dist/web`; use
  `packaging/web/package.sh` for the zip and relative-path checks. Record raw and compressed wasm
  size, first-interactive time, WebGPU and `?webgl` fallback, `?empty` and demo sessions.
- Browser workflows: open demo, use the file picker and drag/drop, import audio/MIDI, save a
  `.scraftz` download, play after a user gesture, resize, and inspect the page. Browser screenshots
  and console logs are evidence; a native screenshot is not browser evidence.
- `rfd` browser picker and `DroppedFileHandle::bytes_async` are asynchronous. The raw drop list
  is consumed in the known `ui` phase; completed byte reads are a separate consumed queue and may
  be handled in `logic`. Test that a picker/drop is handled once and not repeated on subsequent
  frames, including a forced hidden UI paint. The browser has no control channel, so browser UI
  testing needs a page driver (Playwright/Chromium or equivalent), not the TCP protocol.

## 13. Test and performance plan

### 13.1 Required gates

Run every new build/test through the managed server workflow (`maintenance/task.py run` /
`status` / `finish`) with a unique task, source and evidence path. Do not run directly or reuse
another worker's target. Proposed commands (implementation stage):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p soundcraft-ui-egui --locked
cargo test -p soundcraft-engine --locked
cargo test --workspace --all-targets --locked
cargo xtask assets
cargo xtask layers
cargo xtask wasm
cargo check --locked --target wasm32-unknown-unknown -p soundcraft-ui-egui -p soundcraft-web
cargo metadata --locked --format-version 1
cargo tree -d --locked
cargo tree -e features -p soundcraft --locked
```

On Windows add `cargo build --release --locked -p soundcraft -p soundcraft-cli`, the VM GUI
workflow, and the FXC startup capture. On Linux add X11 and Wayland native runs. Add
`trunk build --release` and a browser workflow. Selected optional features to check explicitly:
native `eframe/clipboard`, `accesskit`, `wgpu`, `x11`, `wayland`; web `wgpu` with WebGL2 fallback;
`egui_kittest/wgpu,snapshot`; fork-only `E17/E18` only if the app starts using a UI language.

`cargo xtask ci` is the repository gate but does not add `--locked` to each step; the implementation
PR must add or separately run the lock/graph checks above and record actual selected test counts,
not just exit codes.

### 13.2 Regression assertions

| Risk | Observable assertion |
| --- | --- |
| Root `Ui`/multi-pass | One command, one undo step in a forced two-pass frame; no `root_ui` recursion panic |
| Hidden logic | Autosave, control reply, player position and bounce progress continue; ordinary and forced-hidden paints each import a dropped file once and later hidden ticks do not replay it; paste does not repeat; screenshot timeout is clean |
| Docking | Float/dock/close/reopen/reload keeps panel identity, permissions, floating rect and command ownership |
| Panels/geometry | Timeline, track rows, strips, dock tabs and menu bar rects match the baseline within the stated tolerance |
| Accessibility | Explicit expected node inventory per window/popup with role/name/selected/disabled/value and count assertions; `unnamed_widgets().is_empty()` only as an extra check; missing `widget_info` and `Role::Unknown` mutations fail; hidden controls absent |
| Fonts/glyphs | Bundled UI glyphs have coverage in every family; pinned craft-fonts/Hebrew/Noto Color Emoji fixtures render document text; no tofu; no global `allow_missing_glyphs` |
| Text/IME | Preedit/commit/cancel/replacement, click-away selection, grapheme/UTF-8 boundaries, outgoing surrounding text after delete+commit via E21b/P7b with owner present when unavailable, available-empty versus unavailable, transition clear/disable-enable, final-pass `None`, serial-mismatch apply-once/defer, stale-epoch invalidation; focus/password/reset clearing |
| Scroll/input | Wheel vs trackpad source/phase, momentum/rubber-band policy, zoom anchor, shift-pan, gutter page |
| Clipboard/PRIMARY | Bounded regular clipboard transfer (deadline/byte cap/workers/cancel/stale discard), keyboard- and pointer-triggered PRIMARY serials, owner/seat changes, no first-seat fallback, self-paste |
| Screenshot | Two-viewport/request correlation in native/custom per-viewport evidence, exactly one reply/file, timeout/late callback inert, checked pre-issue reservation, geometry/epoch binding with cancel-on-any-change, surface-fit scale derivation, shared 4/128 MiB/256 MiB defaults with SoundCraft's stricter 4/64 MiB per image and 192 MiB encoding-peak caps, drain-to-zero |
| Commands/undo | Fader/send drag one merged step; command via menu/control/CLI same id and one mutation |
| Save/reopen | Round-trip document meaning, selection/workspace and autosave recovery; no dirty flag loss |
| GPU/video | Video texture update/resize, oversize rejection, surface errors return errors, no panic |
| Native/web | Native focus/IME/picker/clipboard/PRIMARY and browser picker/download/playback are actually exercised |

### 13.3 UI screenshot and pixel policy

- Capture baseline images from `e85a4801e9029c9a1f5ce536b11be65aea832ed8` and candidate images at the same fixture, window size
  (1600x1000 and 1400x900), DPI (1x/1.5x/2x), theme (dark/light/system), fonts (bundled-only
  test set) and renderer (`egui_kittest` wgpu with `PREDICTABLE`, or the named native backend).
  Store them under the task evidence directory, not as unreviewed committed goldens.
- Prefer semantics/geometry over images. Exact RGBA equality is required for flat palette fills,
  custom control faces, text colors and shape rects, as existing `mix_instrument_contrast.rs` and
  `shared_controls.rs` do.
- For full-window raster comparisons, use a documented mask: allow per-channel delta ≤3 in at
  most 0.5% of pixels, no delta >32 outside reviewed regions, and zero missing-glyph/tofu pixels.
  Any difference in panel/window geometry, focus ring, scroll thumb, or text position is a
  failure until reviewed. The only expected style changes are the reviewed 0.37 default
  button/corner differences; restore SoundCraft's explicit values where the product should not
  change.
- Never bulk-refresh a golden after seeing a failure. A changed image is accepted only with a
  written reason and a corresponding geometry/semantic assertion.
- Mutation controls: replace a `Role` with `Unknown`, remove the whole `widget_info` call, remove
  an accessible name, shift a panel rect, drop a screenshot callback token, and replay a consumed
  command; each must fail its intended test. Remove the defect after proving detection.
- kittest renders one image for every viewport screenshot callback
  (`egui_kittest/src/lib.rs:914-942`). Never use it to assert two-viewport pixel correlation or
  swapped-slot detection; those require a native two-window run or a custom per-viewport
  renderer.

### 13.4 Performance protocol

Record baselines before production edits. Four variants are distinct, and the report must not
collapse them into "three":

1. **Historical coherent baseline**:
   `e85a4801e9029c9a1f5ce536b11be65aea832ed8` (0.36.2 egui family, its own non-UI dependency set).
2. **Current-engine / 0.36-UI control**: current non-UI dependencies (the 11-package bump:
   `symphonia` 0.5→0.6, `cpal` 0.16→0.18, `zip` 9.0.1→9.0.5, `libloading` 0.8.9→0.9.0,
   `muda` 0.19.2→0.21.2 and the rest) with the egui family reverted to 0.36.2 and `craft-ui`
   back at `5c874d51eeb3de70e735c8ce87a59a7dedd47675`. Build it only if it actually compiles;
   if it does not, record it as an unavailable attribution control, not a pass or a fabricated
   result.
3. **Equivalent stock-0.37 control**: stock `841a328ba2dd8f6996beb7189ebb7fc00e668a2a`, with the same operation expressed in a
   pinned minimal adapter/harness where fork-only APIs are involved. Record any semantic
   differences.
4. **Fork candidate** at the exact reviewed P8/winit lock/SHA.

The stock-0.37 equivalent control is required, not optional: report it first in every
table and evaluate any regression against both the paired baseline and this control.
Report non-UI dependency deltas separately from egui-frame work. A `cpal`/`symphonia` startup or
audio-underrun difference must not be attributed to egui or the fork. Record one lockfile,
source SHA and feature list per variant, and note where an adapter is not bit-equivalent.

Use identical compiler, profile, features, font set, fixture, renderer/backend, driver, DPI and
viewport. Separate software renderer and real-GPU results, and keep **native eframe** timings in
a different row from **synthetic kittest/Harness** timings: they measure different event loops,
paint scheduling and input paths and are not interchangeable. Do not mix any of these rows.

Workloads:

1. **Startup**: 20 fresh processes, demo session, `--no-audio`, time to first painted frame; on
   Windows also time `egui_wgpu::Renderer::new`/pipeline creation separately (FXC, #8736) and
   font install/`fc-match` separately (#8512).
2. **Idle**: 60 s with no input; frames/repaints, CPU%, median frame interval, idle `request_repaint`
   count. Any new busy loop or audio underrun is a failure.
3. **Text/timeline**: 1,600x1,000, demo + 60 tracks, 5 clips each, 1,000 frames of horizontal pan
   and vertical scroll; `smooth_scroll_delta`, waveform shapes, text tessellation, frame p50/p95/
   p99.
4. **Mix**: 64 tracks, meters live (no-audio simulation and, separately, player), 60 Hz, 1,000
   frames; strip layout and meter paint.
5. **Windows/dialogs**: open/close MIDI, Score, Video, docking float/resize, About/Preferences;
   operation latency and frames to settle.
6. **Video**: one 1080p frame decode + texture update cycle, repeated; report upload bytes and
   frame time.
7. **Screenshots**: request-to-callback-and-file latency, pending queue depth, peak image bytes.
8. **Web**: wasm raw/compressed size, first interactive, demo load and a fixed interaction script;
   record browser/backend separately.

Proposed measurement harness: `crates/ui-egui/examples/frame_bench.rs` (a dev example, not
a shipped binary), driven by the same demo fixture and `Harness` renderer; `ui_shot` remains the
screenshot tool. The harness must not be used as a production compatibility layer.

Measurements: 10 warmups and at least 30 iterations for steady operations; at least 1,000 frames
for frame distributions (including p99); 20 fresh processes for startup. Report raw samples,
median/p95/p99 only where sample count supports it, peak RSS (`VmHWM` on Linux), allocation
count/bytes via a vetted dev-only profiler such as `dhat` or an external heap profiler, texture
set/upload counts and bytes from `FullOutput.textures_delta`, and idle CPU/repaint counts. Do not
add a local `#[global_allocator]`: a `GlobalAlloc` implementation needs `unsafe`, and the
workspace forbids it (`Cargo.toml:56-57`, applied to `ui-egui` via `[lints] workspace = true`).
Use paired baseline/candidate runs and alternate order; record host load and driver versions.

Predefined stop thresholds, frozen before seeing candidate results: investigate a repeatable
>5% median, >10% p95 or >5% peak-RSS regression, >5% startup median or >25 ms, any new idle
busy-loop/audio underrun, or any loss of an existing stricter app budget; every relative change
must also exceed the measured run-to-run noise floor. For sub-millisecond operations, require the
relative increase also to exceed 0.05 ms or be corroborated by instructions/allocations. Report
p99 from ≥1,000 frames and investigate a repeatable p99 regression even though it has no separate
fixed threshold. Reject noisy comparisons as inconclusive and rerun; do not widen thresholds
after seeing a failure. A confirmed #8731/#8736 regression needs a reviewed fork fix or a
documented exception, not a changed golden.

## 14. Reviewable implementation stages, DAG and rollback

Dependency DAG:

```text
P0 reviewed plans + SoundCraft baselines (e85a480..., current-engine/0.36-UI if it builds,
   stock-0.37 control, fork) and preserved baseline executable/fixture
   |
   +-- approved published storytold egui P8 (E1-E21 + P2b + P7 + P7b/E21b + P7c)
   +-- completed approved published storytold winit (W15P + bounded W15 transfer
       + W27 lifecycle revision)
   +-- reviewed craft-libs CL-1 (reconciled 3832c47 base)
   |        -> CL-2 capture prototype -> Design/Grid pilot adapters -> CL-2F API freeze
   |
   -> P1 atomic SoundCraft compatibility (root pins, lock, API/clipboard/fonts,
      lifecycle, capture adapter against the frozen CL-2F API)
        -> P1 rollback rehearsal
        -> P2 deeper UI assurance (accessibility inventory, scroll, glyphs, IME)
        -> P3 native/browser/performance acceptance + repeat rollback rehearsal
```

1. **P0 — plans and baselines.** Land all reviewed plans. Add
   regression/measurement support on the historical coherent baseline
   `e85a4801e9029c9a1f5ce536b11be65aea832ed8`; preserve the current mixed graph as a
   separately identified pre-existing failure. Record exact full SHAs, features and
   lockfiles for the four performance variants and raw baseline samples before
   production changes. Preserve the baseline executable and a saved fixture for
   rollback rehearsal.
2. **P1 — atomic compatibility.** Prerequisites are the complete approved,
   reachable **storytold egui P8** and **completed storytold winit** revisions
   (P2b over the bounded W15 transfer, P7, P7b/E21b, P7c; W15P, W15 transfer and
   W27 lifecycle/serial state), the reviewed craft-libs **CL-1** revision built by
   on the reconciled `3832c47` base (`8614e4a`/`1764bca` are the historical port hunks),
   and the **frozen CL-2F** capture-helper API. P1 waits for CL-2F; it does not land
   a provisional helper API. Then land all root family pins, lock changes,
   API/clipboard/font adaptations, callback replacement, any incompatible craft-ui
   bump and required lifecycle fixes together. A dependency-only or stock-first
   intermediate release is not planned. P1 is blocked in full until every
   prerequisite exists: a missing SHA or unpublished stage blocks all manifest
   changes, not just one PRIMARY/IME/capture row, and no partial P1 is claimed.
3. **P1 rollback rehearsal.** In a separate managed allocation, restore the
   previous coherent source/manifests/lockfiles/settings set at
   `e85a4801e9029c9a1f5ce536b11be65aea832ed8`, run the preserved baseline
   executable and reopen the saved fixture. Record the exact commands, identities
   and result. Do not restore only an old lockfile and call that a rehearsal.
4. **P2 — deeper UI assurance.** Add the explicit accessibility node inventories,
   remaining roles/names, scroll, glyph and **E21b outgoing-IME** tests and any
   measured fixes. The P1 commit must already pass the required tests; this stage
   expands cases rather than deferring broken behavior.
5. **P3 — native/browser/performance acceptance.** Qualify supported environments
   (including the W15P/E4/P2b PRIMARY matrix and two-viewport native capture),
   retain evidence, repeat the rollback rehearsal and perform a fresh adversarial
   implementation review. Regressions require app fixes or a reviewed shared-fork
   follow-up with downstream revalidation.

The first compatibility PR is a single coherent dependency unit. Subsequent commits
are individually reviewable and buildable. Rollback restores the entire accepted
source/manifests/lock/helper set in a new revision; do not restore only an old lock,
drop the fork while keeping fork-only code, or reset published history. Preserve all
evidence and source work. No stage authorizes publication without fresh approval.

## 15. Compatibility pin and lockfile acceptance details

- Patch section is in the **root** `Cargo.toml`, not a dependency's manifest.
- Every selected egui family member and winit is at one immutable full SHA; no branch names, no
  path dependencies, no machine-local source replacement.
- `Cargo.lock.version = 4` may remain; regenerate it through Cargo, do not hand-edit package
  identities. Record old and new lockfile hashes.
- The shared libraries are consumed by reviewed git revision for this migration. A separate
  crates.io publication or stock compatibility promise is not implied. SoundCraft must repeat
  required root patches and never depend on machine-local path overrides.
- `cargo package`/`cargo publish` is not applicable to `soundcraft` (`publish = false`); still
  inspect the generated `Cargo.lock` and metadata for accidental local paths.
- Dependabot must not silently re-split the family; add ignores or an all-family update rule.
- A clean clone with a cold cache must fetch every git revision and pass `--locked` without local
  overrides. Recheck remote heads before merge; a moved branch is not a revision.

## 16. Blocking unknowns and explicit acceptance criteria

Blocking unknowns:

1. **Exact reviewed `craft-libs` commit** after applying PR #199's intended hunks to current
   main. The local reconciliation base is `3832c47ef68a0e6214f2c1e85f3526b6adcb686e`;
   `b68de8c`/`ec8840a` are stale trees and must not be merged, and the historical
   `8614e4a`/`1764bca` hunks on `4919088` are no longer the base. Until a reviewed reachable SHA
   exists, P1 cannot pin it.
2. **Organization-owned egui/winit SHAs, including the required new work.** `aiongg/craft/0.37`
   and `aiongg/winit craft/0.30.13-lang` are movable research inputs. P1 requires approved,
   reachable `storytold` full SHAs for the complete **egui P8** set (E1–E21, **P2b**, **P7**,
   **P7b/E21b**, **P7c**) and the **completed winit** (**W15P**, bounded W15 transfer, W27
   lifecycle). A missing SHA blocks all P1 manifest changes; these SHAs cannot be invented
   before implementation and publication.
3. **CL-2F capture-helper API.** P1 sequences after the shared helper's approved freeze. The
   inspected `4919088` tree had no `crates/ui/src/capture.rs`, and the current source imports
   `craft_ui::capture::CaptureSlot` from the local craft-libs worktree. The reconciled `3832c47`
   base must be confirmed to carry the agreed CL-2F API when the final pin is chosen; a
   provisional API must not be pinned.
4. **Upstream #8731/#8736 disposition.** The fork does not yet contain either fix. SoundCraft's
   text-heavy UI and explicit FXC choice make both likely relevant; a measured result and a
   reviewed fork patch (or documented exception) are required before final performance acceptance.
5. **Native evidence environments.** No `DISPLAY`/Wayland session, no Windows clone allocated, no
   macOS hardware and no physical-GPU/audio qualification exists in this planning run. The
   Windows full-clone capacity gate is also unresolved. These rows remain unresolved until managed
   evidence exists.
6. **Browser workflow.** Compilation to wasm is not a browser test; a Chromium/Playwright-style
   driver, a running `trunk build --release` site and download/picker/audio checks must be
   available or the browser row is unresolved.
7. **Exact count of accessibility failures and scroll/multi-pass failures** is unknown until the
   candidate is built. The plan specifies the audit and tests; it does not claim a pass.

Acceptance for the whole migration:

- Exactly one immutable egui family and one winit revision in the accepted lock; no 0.36/0.37
  duplicate; reviewed craft-ui revision; no local/branch/path dependency.
- All repository gates plus the selected feature, native Windows/Linux, wasm compile and browser
  workflows pass with recorded counts and evidence.
- The lifecycle tests prove no duplicate or lost control/autosave/transport/undo/screenshot work in
  multi-pass and logic-only frames.
- UI geometry, screenshots, accessibility, fonts/glyphs, input/IME/clipboard, docking/viewport and
  save/reopen assertions pass with mutation controls shown to detect their intended defects.
- Performance meets the predefined thresholds with paired, same-environment measurements across
  the four distinct variants (historical coherent baseline, current-engine/0.36-UI control only
  if it compiles, equivalent stock-0.37 control, fork), with non-UI dependency deltas and
  native-versus-synthetic timings reported separately; any accepted regression is an explicit,
  reviewed tradeoff.
- The complete egui P8 and completed winit (W15P, bounded W15 transfer, W27 lifecycle) are
  present in the accepted pin; PRIMARY (keyboard and pointer serials), bounded clipboard
  transfer, outgoing-IME lifecycle, two-viewport capture and rollback-rehearsal gates have real
  evidence or remain explicit blockers.
- No publication occurs until the user has seen the exact commit range and validation and given
  fresh approval.

## 17. Evidence and limitations of this planning document

- Source inspected: all root/nested manifests and the root lockfile; `crates/ui-egui` source and
  tests; desktop/web/CLI entry points; build.rs; graphics/native-menu/config; xtask and workflow
  files; selected upstream 0.37.0 and fork source (`a4f9a17f` egui and `8b35388b` winit);
  `egui_kittest` accessibility/screenshot code and its normalized manifest feature split;
  eframe `prepare_raw_input`/`update_logic_only` and the web hidden branch; `egui-wgpu`
  `surface_fit`/`render_scale`; workspace lints; craft-libs PR #199/audit evidence; suite
  rollout notes and upstream discussion snapshots (`upstream-8764-fresh.json`).
- Static checks performed for this revision: `git diff e85a480..52471b3` (manifests/lock only);
  `#[test]` counts (108 src, 156 tests, 1,159 workspace); cmap scan of the
  `epaint_default_fonts` files; source presence/absence checks for `capture.rs`,
  `set_ime_surrounding_text`, W15P, `AsId`, 0.36.2 `Window::new`, and the kittest
  single-image screenshot completion. These are inspection results, not runtime evidence.
- No build, test, render, screenshot, benchmark, VM, push, PR or message was executed. The
  historical audit logs and the earlier fork/stock compiler errors are clues only.
- A read-only `cargo tree` was blocked offline by a missing `symphonia-common` registry index
  entry; feature/graph claims are from manifests/lockfile and must be rechecked in a managed job.
- The plan intentionally uses placeholder names for revisions and a test helper; it does not
  invent a merge SHA, an upstream API, or a benchmark result. Those placeholders must be replaced
  by exact reviewed values before implementation.


## Local implementation update — 2026-10-23

The user clarified that reviewed plans remain local and implementation proceeds before publication. The earlier requirement to publish/land this plan first is superseded; pushes and PR publication still require explicit approval.

Canonical `storytold/soundcraft` `main` resolved to `d47abe484d4f9ef0cc9f8e872e0b3880a61a4588` when the owned checkout was fast-forwarded on 2026-10-23. Earlier baseline SHAs and line anchors above are retained as historical review evidence.

Current local implementation update (2026-10-24): the owned worktree `HEAD` is
`6b84a4f0461f2467402eafe41dcc708c369fb045` on `codex/egui-037-soundcraft-20261023`, with the
uncommitted migration/upstream-reconciliation edits on top. The craft-libs reconciliation base
for those edits is `3832c47ef68a0e6214f2c1e85f3526b6adcb686e`. The dependency graph is currently
the local integration configuration/path-only lock, not a publishable manifest: one exact,
reachable, immutable craft-libs revision plus the complete egui/winit family remains the final
publication gate and must come from central local commits, not a guessed SHA.

No migration API redesign required by this delta; preserve the merged packaging, test and chrono changes where present.

The complete static delta review and exact old/new object identities are retained in the task evidence: `canonical-diffs-refresh-20261023.json` and `overnight-consumer-review-20261023.md`. This amendment claims no new build, native GUI or performance acceptance.

## Local implementation stage 1 — compatibility and lifecycle (source only)

This stage is implemented in the owned worktree only. No build, test, formatter, clippy, lockfile,
commit, push, PR or release was produced by the implementing worker; the parent runs the managed
`task.py` jobs and regenerates `Cargo.lock` for the local-path dependency set.

Implemented against the local pre-E22 `../egui-implementation` and the local
`../craft-libs-implementation` API:

- `MouseWheel.source` (`Wheel` for line units, `Trackpad` for point units), no `Event::Screenshot`/
  `UserData`, `Role` instead of `WidgetType` (`SelectableLabel`→`Button`, `Checkbox`→`CheckBox`),
  `Id::unique`, `Ui::scope_id`, `TextEdit::align(CENTER_TOP)` for the old horizontal center.
- Screenshot lifecycle: app-owned strong `craft_ui::capture::CaptureSlot` per request, weak renderer
  callback, 8 pending / 4 in flight, 64 MiB per image, 192 MiB aggregate **encoding peak** (image,
  raw copy, PNG buffer and base64 text), checked byte reservation from the capture-time viewport,
  8 s monotonic deadline checked before any write, geometry and session binding with one retry path,
  cancellation on session/focus change, and exactly one reply. `ui.screenshot` success now includes
  the request token; error strings are documented in `docs/control-protocol.md`.
- Lifecycle: `logic` no longer returns early on `current_pass_index`; frame pacing and the transport
  simulation use a monotonic `web_time::Instant`; `raw_input_hook` keeps exactly one unapplied
  synthetic batch across hidden ticks; a settled UI pass acknowledges it and issues captures after
  layout; dropped files are captured before the font gate and routed after layout, once.
- Control ordering: a scripted gesture's reply waits until a UI pass applied every event, later
  control requests behind it wait too, and an undeliverable gesture is canceled after 15 s. This
  makes a following inspect/engine command/screenshot see the post-gesture state. Scripted input
  does not publish OS clipboard or PRIMARY commands from its pass.
- Fonts/features: `egui/default_fonts` + `egui/monochrome_emoji_fonts` on the UI and app roots,
  `eframe/clipboard` on native, a bundled-face glyph test, and drawn/ASCII replacements for the
  uncovered `▸`, `▾` and `♯` glyphs.
- Accessibility names for the custom toggles/selectors/faders and bare text fields used by dialogs,
  inline rename, Spot and comments; repeated per-track Edit/Mix controls now carry track-qualified
  accessible names and their current value. This is not the full §9.3 node inventory.

Not included: E21b outgoing surrounding text (the sibling producer is under review), the full P2
accessibility inventory and mutation controls, native/browser/performance qualification, Windows
and macOS evidence, and any change to the shared dependency revisions.

Known local test-helpers: new phase-sensitive tests call `raw_input_hook(ctx, raw)` and then
`logic`/`ui` inside one `run_ui` closure, mirroring native and web eframe. The historical coherent
baseline `e85a480…` keeps its 0.36 test helpers until the parent backports this helper shape for
paired measurements.

## Post-port review repair (source only, 2026-10-23)

This round addressed the substantive findings of
`egui037-soundcraft-postport-review-20261023-root-v1.md` in the owned worktree; no build, test,
formatter, clippy, lockfile, commit, push or PR was produced.

- Synthetic input: `raw_input_hook` records the exact events it appended (batch plus the hover
  `PointerMoved`s) and removes that run before re-appending, so a host that retains its
  hook-mutated `RawInput` (eframe `pending_raw_input`, web `input.raw`) holds one copy, not one more
  per hidden tick. A pending batch now sends `RequestPaintWhileHidden`, the synthetic grace keeps
  being refreshed while a batch is pending, and a settled pass acknowledges the batch only if that
  pass actually received its events.
- Lifecycle: frame statistics are sampled once per settled UI pass from the monotonic clock (not
  per layout pass and never on hidden ticks); a bounce slice runs on the first pass of a host frame
  only; `last_frame`/record-hint freshness uses monotonic uptime; hidden logic ticks clear the
  cached pointer/keyboard state and a transient `Gesture::Fader`, and automation writing uses that
  cached state instead of stale input.
- Capture: `recover` and `dialogs::open_pending_session` go through `SoundApp::document_replaced`,
  so a pending capture is canceled for every document replacement path; the byte check accepts the
  same one-pixel tolerance as the dimension check; truncated images fail as malformed instead of
  succeeding; a missing `max_texture_side` falls back to the context's last known limit; a window
  with no known geometry reports `screenshot unavailable` rather than `too large`.
- Accessibility: I/O Setup bus names, the Score title/composer fields, per-strip faders and the
  inline pan/volume/send readouts now have accessible names; repeated controls are named per track.
- `ui.scroll` point events are trackpad-like; the deterministic control tests disable kinetic coast
  and rubber-banding, and `docs/control-protocol.md` documents the behavior.

Pending acceptance gates (not claimed by this repair): the E21b/P7b outgoing surrounding-text
producer is implemented in `../egui-ime-implementation` and is not integrated here — incoming IME
works but outgoing surrounding text remains unqualified; the local egui pin does not contain the
P7c selected `#8763` click-away/selection-action fix, so rename/Spot/path rename fields inherit the
unfixed behavior; native/browser GUI, IME and performance qualification still need the parent's
managed runs.
