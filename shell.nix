# Development shell for NixOS / Nix users: `nix-shell`, then `cargo run`.
# Other distros: see the README for the packages a build needs.
{ pkgs ? import <nixpkgs> { } }:

let
  # Loaded at runtime by winit/eframe (dlopen), so they must be on the library path.
  runtimeLibs = with pkgs; [
    alsa-lib
    # libjack.so.0 for the JACK host: PipeWire's JACK server library (talks to the running
    # PipeWire). With a real jackd, put `libjack2` here instead.
    pipewire.jack
    wayland
    libxkbcommon
    libGL
    vulkan-loader
    libx11
    libxcursor
    libxi
    libxrandr
  ];
in
pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    pkg-config
    # `cargo xtask ci` (fmt check, clippy).
    rustfmt
    clippy
    # bindgen, for the PipeWire host.
    rustPlatform.bindgenHook
  ];
  buildInputs = runtimeLibs ++ (with pkgs; [
    libjack2 # jack.pc, for the JACK host
    pipewire # libpipewire-0.3 and its headers, for the PipeWire host
  ]);
  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibs;
}
