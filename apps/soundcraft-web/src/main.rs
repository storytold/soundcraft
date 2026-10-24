//! SoundCraft in the browser.
//!
//! Runs the same [`soundcraft_ui_egui::SoundApp`] as the desktop app through eframe's web runner
//! (wgpu: WebGPU where available, WebGL2 otherwise), on the synthesised demo session, with
//! playback through WebAudio (cpal's `wasm-bindgen` backend). Build with `trunk build --release`
//! from this directory (output in `dist/web`).
//!
//! Differences from the desktop app:
//! - no TCP control channel (browsers can't listen on sockets);
//! - no file system: File > Open Session…, Import > Audio…/MIDI… and files dropped on the page are
//!   read as bytes through the browser's file picker; a session is saved as ONE bundle file
//!   (`<name>.scraftz`: the `.scraft` and its `Audio Files/`), and every save or export arrives as a
//!   download (the demo session is still loaded on start);
//! - browsers only start audio after a user gesture, so the first click may be needed before
//!   playback is heard.
//!
//! URL query flags: `?webgl` forces the WebGL2 backend instead of WebGPU; `?empty` starts with an
//! empty session instead of the demo; `?host=parent` hands open and save to a same-origin parent
//! page (`host`).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_arch = "wasm32")]
mod host;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(target_arch = "wasm32")]
fn main() {
    web::start();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("soundcraft-web only runs in the browser: build it with `trunk build --release` in apps/soundcraft-web");
}
