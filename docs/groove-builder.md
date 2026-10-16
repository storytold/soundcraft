# Groove Builder

Groove Builder creates editable MIDI from a six-row step worksheet. Open **Track > New Groove...**, or choose **Build a groove pattern...** from New Tracks. **Create groove track** adds one instrument track using SoundCraft's existing Drum Synth and a MIDI clip in one undo step.

Reopen a saved pattern by double-clicking its region with the Grabber tool, using **Edit Groove...** in its track/clip context menu, or choosing it in **Window > Groove Builder**.

## Pattern construction

- **Steps:** 1–32 cells per cycle. **Cycles:** repeat that cycle 1–32 times. The cycle length is independent of the session's bar size.
- **Step size:** eighth or sixteenth notes. Notes use the session's tempo map.
- Each row distributes **Pulses** over its steps with integer arithmetic: `(step * pulses) % steps < pulses`, then applies **Rotate**. No performance corpus or pattern table is used.
- Click a cell to edit the row manually. Editing Pulses/Rotate or clicking **Manual** returns that row to its pulse distribution. Muting a row silences its cells and ending hits.
- **Swing** delays alternate cells by the displayed percentage of one step. It is a direct timing control, not a model of a performer.
- **Velocity spread** and **Timing spread** add deterministic bounded offsets. **New variation** changes their seed; with both spreads at zero it leaves the output unchanged.
- **Ending steps** adds alternating snare/tom hits to the final cycle, respecting row mutes.

The six voices use the existing Drum Synth note map: kick 36, snare 38, closed hat 42, clap 39, tom 45, ride 51, on MIDI channel 10. All sound is synthesized by the existing instrument. No new samples, images, presets or external data are bundled.

## Editing and persistence

Changes are a draft until **Apply changes**; each apply is one undo step. The clip stores its versioned recipe and generated MIDI. Opening the editor does not regenerate notes. Undo reloads the saved recipe into the editor. **Make ordinary MIDI** removes the recipe and opens the MIDI Editor without changing any notes.

If a generated clip has been trimmed or its notes edited outside the builder, updating it is rejected to preserve those edits. Detach its recipe to continue in the MIDI Editor. Updates also reject locked clips and overlaps with neighboring clips. Existing sessions without recipes continue to load.

## Commands

| Command | Parameters | Result |
|---|---|---|
| `groove.create` | `name?`, `track?`, `start?`, `config?` | Create a clip; omitted track creates a Drum Synth track. |
| `groove.update` | `clip`, `config?` | Apply a complete or partial configuration to an unmodified generated clip. |
| `groove.detach` | `clip` | Keep MIDI and discard the recipe. |
| `groove.open` (UI) | `clip?` | Open a recipe, using the selected clip when omitted. |
| `groove.new` (UI) | none | Open an empty builder. |
| `window.groove` (UI) | `value?` | Show/hide the builder. |

`config` contains `steps`, `repeats`, `division` (`eighth`/`sixteenth`), `velocity` (1–127), `swing` (0–75), `velocity_spread` (0–40), `timing_spread` (0–20), `ending` (0–steps), `seed`, and six `lanes`. Each lane contains `enabled`, `pulses` (0–steps), `rotate` (0–steps−1), and `cells` (null for arithmetic distribution, otherwise a 32-bit step mask). Unknown configuration keys and out-of-range values are rejected.

## Scope

This is a generic step generator. There are no performer families, style catalogues, guide-follow detector, articulation emulation, or live parameter regeneration. The UI uses native SoundCraft widgets and an original icon drawn in `icons.rs`. The feature does not establish parity with another application's generator. Native platform and accessibility coverage must be assessed separately from compilation and automated UI tests.
