# UI parity

> **Last reviewed:** 2026-11-05 · **Last updated:** 2026-11-05 · **Change:** trivial (narrow dock seams) · **Target:** Avid Pro Tools Ultimate 2026.4.1

How SoundCraft feels in the hands of a Pro Tools user: tools, modes, modifiers, shortcuts,
windows, meters. Measured from `crates/model/src/session.rs` (tools, modes), the command
registry (shortcuts), `crates/ui-egui/src` (windows, gestures) and user issues; compared with
reference screenshots kept locally under `plan/protools/screenshots/` (16, never committed).

**UI/UX fidelity: ~55 % ready** (estimated). Remaining ≈ 60–100 h.

Legend: ✅ matches · 🟡 partial · ❌ missing.

## Tools and modes

| Feature | Status | Notes |
|---|---|---|
| Tools: Zoom, Trim, Selector, Grabber, Scrubber, Pencil, Smart (7) | ✅ present | Trim variants (TCE, loop, scrub trim) and Grabber variants (separation, object) partial; Multitool fade/trim handles don't adjust audio (#5) |
| Smart-tool Fade In / Fade Out drag bounds and preview | ✅ | One thin curve replaces the dragged side's previous curve and matches its appearance after release; bounded to the clip, including full-clip fades. Hovering the curve's upper endpoint shows the fade cursor and lets the existing fade be dragged |
| Fade midpoint curvature | ✅ | Visible fades retain a small midpoint handle in Smart mode, including after release and extreme bends. Hover shows the vertical resize cursor; repeated drags change curvature without changing length. Curves sample at physical-pixel resolution; preview, playback and bounce share the envelope. Length drags retain curvature |
| Live waveform fade preview | ✅ | Waveform amplitude updates during fade-length and midpoint-curvature drags, sharing the curve's temporary envelope. Mono/stereo, waveform styles, zoom and clip gain remain effective; release preserves the displayed waveform with one undo step and cached peaks |
| Edit modes: Shuffle, Slip, Spot, Grid (absolute/relative) | ✅ | Spot dialog present |
| Grid and nudge values, all timebases | ✅ | Bars/Beats, Min:Secs, Timecode, Feet+Frames, Samples |
| Zoom Toggle | 🟡 | E (Commands focus) and the button zoom to the edit selection with Fit To Window heights and back; Alt+Shift+E and Alt-click cancel it and keep the view. The Zoom Toggle preferences (Vertical Zoom, Horizontal Zoom, Track Height, Track View, Separate Grid Settings When Zoomed In, Zoom Toggle Follows Edit Selection, Remove Range Selection After Zooming In) are not present |
| Keyboard focus modes (Commands, Clip List, Groups) | 🟡 | Commands focus single-key editing ✅; list focus partial |
| Link timeline/edit, link track/edit, Tab to transient, insertion follows playback | ✅ | |
| Mirrored MIDI, MIDI merge, layered editing | 🟡 | Stored and partly honoured |

## Keyboard and mouse

| Feature | Status | Notes |
|---|---|---|
| Default shortcuts | 🟡 146 bound (measured: distinct `Some("…")` shortcuts in the registry) | Pro Tools' default set is several hundred incl. numeric keypad and focus keys. Collisions: Mix/Edit toggle vs zoom (#144), Heal (#140), Export (#7) |
| Ctrl as Cmd on Windows/Linux | ✅ since #15, #41 still open in some builds | #62 shows Ctrl in labels |
| Numeric keypad transport and memory-location recall | 🟡 | Classic / Transport / Shuttle modes not all present |
| Modifier gestures (Alt for all tracks, Shift-Alt for selected, Cmd-click fine faders, Ctrl-click defaults) | 🟡 | Main ones on faders and pan; not across every control |
| Scroll/zoom: wheel, trackpad pinch | 🟡 | Anchored trackpad zoom in open PRs #112, #137 |
| Drag and drop: Clip List → tracks, files → tracks | ✅ macOS/Windows; 🟡 Wayland (PR #150) | |
| Context menus (clip, track name, ruler, Clip List) | 🟡 | Clip and track menus exist; depth lower than Pro Tools'; Clip List delete (#4); track name ▸ Scroll Into View (Edit top, Mix left) |
| Drag across checkboxes to check or uncheck several | ✅ | #399; every checkbox, as in the other Craft apps |

## Windows and panels

| Window | Status | Notes |
|---|---|---|
| Edit window: rulers, track headers, view columns, Clip List, Groups, Tracks list | ✅ | Track Pin (new in 2026.4) ✅ (`track.pin`); Tracks and Groups and the Clip List sit in full-height dock columns at the window's sides |
| Mix window: strips, views (inserts, sends, I/O, comments, EQ curve, preamps, instruments, object) | ✅ | Narrow Mix ✅; Mix and Edit on screen at once (Window › Mix in Separate Window, its own native window) ✅; Delay Compensation view and HEAT missing |
| MIDI Editor, Score Editor, Event List | 🟡 | Dock height bug fixed (#113); notation editing shallow |
| Transport, Big Counter, Memory Locations, Automation, Video, Universe, Renderer, UI Customization | ✅ present | |
| Workspace / sound browser with previews | ❌ | G-16 |
| I/O Setup | 🟡 | Busses and paths; no hardware channel matrix (G-01) |
| Preferences | 🟡 | Many settings stored, fewer honoured |
| Window configurations, Tile/Cascade | 🟡 | Configurations ✅; Tile/Cascade in PR #151 |
| Plugin windows | 🟡 | Generic parameter sliders; Pro Tools plugins have bespoke GUIs (#138 asks for skeuomorphic design); multi-page plugin views (#6) |
| A plugin opens when it's inserted | ✅ | #486; a hosted plugin's own editor, else its plugin window |
| Command palette (Search) | ✅ | Over every command |

## Look and meters

| Feature | Status | Notes |
|---|---|---|
| Compact Edit toolbar | ✅ layout; 🟡 full behavior parity | Flat joined tool and zoom keys, four edit modes in a black bezel, action-and-return transport glyphs, main/selection/cursor readouts, independent Grid/Nudge values and stereo output. Quantize Grid/Strength/Swing and Input Quantize dispatch `event.input_quantize`; Q applies the displayed values through `event.quantize`. Tempo/Meter/Count Off are available from View › Transport › MIDI Controls and the toolbar gear menu. Existing time-scale menus and narrow-window scrolling are retained. All glyphs are original procedural artwork |
| Edit column titles | ✅ | Comments precedes Instruments, Inserts, Sends and I/O in captions and per-track controls. Flat faces share one-point black gaps on every edge; an accessible procedural Views selector preserves column visibility controls |
| Tracks and Groups presentation | ✅ layout; 🟡 full behavior parity | Compact contiguous rows align visibility/status, color, type/ID and names. Selected tracks fill every column in blue; all track kinds have original symbols. Groups includes an italic purple `<ALL>` row, options menu and command-backed keyboard-focus control; per-letter shortcuts remain partial. Selection, visibility, activation, reordering and gutter scrolling retain existing commands |
| Panel separation | ✅ | Two-point black seams separate dock panes, including Tracks from the ruler headers and the docked MIDI Editor. Major horizontal sections (toolbar/body and Tracks/Groups) keep four-point gaps; full-height toolbar columns use 1.5-point gaps. Counter, Grid/Nudge, quantize and output wells have no gray outer rim; Grid/Nudge and output wells are centered. Dock and gutter edge strokes do not double the seams, in both themes |
| macOS title alignment and dialog movement | ✅ | Public AppKit accessories center the Edit/Mix main-window title while retaining the actual title for accessibility. Shared forms, recovery and the web update notice start centered and can be moved by their title bars without changing form or session state |
| Dark theme close to Pro Tools' | ✅ | Light theme (#32) and live system appearance (#85) beyond Pro Tools |
| Interface scaling | 🟡 | Slider in PR #147 |
| Meter types and ballistics (peak, RMS, VU, K-scales, gain reduction) | 🟡 | Peak/RMS; meter type choice and GR meters partial |
| Clip indicators, track colours, waveform views (peak/power/rectified/outlines) | ✅ | |
| Fader resolution, fine adjust, numeric entry | 🟡 | |
| macOS system menu bar | ✅ | #10 |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-05 | trivial | Narrow dock seams from four to two points, including the vertical divider before the ruler headers; keep the four-point toolbar/body and Tracks/Groups section gaps. Readiness estimates unchanged |
| 2026-11-05 | minor | Refine the two-row Edit toolbar, original procedural glyphs, Comments-first column titles, compact Tracks/Groups, centered macOS titles and draggable forms. Share four-point black panel gaps without doubled dock edges; keep readiness estimates unchanged |
| 2026-10-25 | minor | Zoom Toggle row: Alt+Shift+E and Alt-click cancel Zoom Toggle and keep the view. Readiness estimate unchanged |
| 2026-10-25 | minor | Edit and Mix windows: Tracks and Groups and Clips in full-height dock columns. Readiness estimate unchanged |
| 2026-10-20 | minor | Preferences uses one fixed non-scrolling size with compact stacked choices, removes the duplicate meter-type list, and command-backs visual-effects reduction and undo depth alongside the UI theme |
| 2026-10-19 | minor | Preferences opens from Setup with nine searchable pages, compact grouped controls, staged apply/cancel, platform tab shortcuts and disabled placeholders for unavailable settings |
| 2026-10-16 | minor | Track name menu: Scroll Into View brings the track to the top of the Edit window and the left of the Mix. Readiness estimate unchanged |
| 2026-10-13 | minor | A plugin opens when it's inserted (#486) |
| 2026-10-13 | minor | Drag across checkboxes (#399) |
| 2026-10-13 | minor | Waveforms and fade curves share temporary envelopes during drag, without rebuilding media or editing the document before release. Readiness estimate unchanged |
| 2026-10-13 | minor | Fade curves use bounded physical-pixel sampling; midpoint handles stay visible and inside the clip, with repeat-drag coverage after extreme bends. Readiness estimate unchanged |
| 2026-10-13 | minor | Continuous fade curvature from a midpoint drag, shared UI/audio envelope, one-step undo and saved session data. Readiness estimate unchanged |
| 2026-10-13 | minor | Existing fades have draggable upper endpoints with matching hover/drag cursors, including at adjacent clip boundaries. Readiness estimate unchanged |
| 2026-10-13 | minor | Fade drags use the same thin S-curve renderer as committed fades, replacing the dragged side's old curve. Readiness estimate unchanged |
| 2026-10-13 | minor | Bounded Smart-tool fade previews and commits; full-clip fades target only the dragged clip and side. Readiness estimate unchanged |
| 2026-10-12 | minor | Mix window: Mix in Separate Window (Edit and Mix on screen at once) |
| 2026-10-10 | major | Created: tools, modes, shortcuts, windows and meters checklist against Pro Tools 2026.4.1 |
