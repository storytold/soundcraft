# SoundCraft on Android

SoundCraft on Android: a small `apps/` crate that hosts the existing `soundcraft_ui_egui` interface in an
`android-activity` (GameActivity) window, on top of `soundcraft_engine`. The desktop entry crate is not
compiled, so `rfd`, `arboard` and the macOS/Windows/Linux platform code never enter the dependency
graph.

## What the shell does

- **Entry point** — builds as `libmain.so` (`[lib] name = "main"`, `crate-type = ["cdylib"]`), loaded
  with `System.loadLibrary`; `android_main` is the usual `android-activity` entry.
- **Storage** — `AndroidApp::internal_data_path()` is the data directory (`dirs` answers nothing on Android). SoundCraft's `ui.json`/`prefs.json` pair is *not* loaded or saved yet, so UI state starts from the defaults each launch.
- **File pickers** — SoundCraft has no `file.open`: a DAW opens a *session* and otherwise imports material.
  `pick_open` receives a `purpose` and maps it to the matching engine command
  (`session.open`, `file.import_audio`, `file.import_midi`); the picked file is copied
  into `<filesDir>/import/` and handed back over the control channel. `pick_save` uses
  the same two-stage scratch-file-then-publish path as the other apps. `pick_folder` is
  left `None`.
- **Fonts** — Android does not expose the Unix font directories, so the CJK face from `/system/fonts`
  is appended to every family *after* SoundCraft installs its theme (any earlier, and the theme's named
  families would be dropped).
- **Touch** — hit targets are raised to 40 pt, sliders widened and hover tooltips suppressed, applied
  after the theme's style exists. The layout is not zoomed: it is designed for a desktop window and a
  phone gives ~411 pt, so the next step is a touch rework, not a scale factor.
  `crates/ui-egui/src/menus.rs` also grows its menu bar with the style's `interact_size`:
  the desktop keeps the 24 pt row, but a touch style's 40 pt row would clip the labels.

## Known gaps

- Audio output is not wired (`SoundApp::new` gets no player), so playback is silent.
- Sessions open from the app's own import copy; "save back to the original" is not wired.
- Printing has no backend.

- The system clipboard is not wired (`arboard` has no Android backend); SoundCraft's own clipboard is
  still used.
- The launcher icons are the Android Studio template's placeholder art.

## Build

```bash
cargo ndk -t arm64-v8a -o apps/soundcraft-android/jniLibs build --release -p soundcraft-android
cd apps/soundcraft-android/android && ./gradlew assembleRelease
```

`jniLibs/` and the Gradle output are gitignored; the APK is not part of this change.
