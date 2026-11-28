//! Windows only: embed the app icon and version info (VERSIONINFO) into `soundcraft.exe`, so it
//! shows in Explorer, the taskbar, the Start menu and Alt-Tab.
//!
//! On every other target this does nothing. A missing resource compiler is a warning, so a
//! cross-compile from macOS or Linux still links, unless `SOUNDCRAFT_REQUIRE_WINRES=1` turns it
//! into an error (for release builds).
//!
//! `SOUNDCRAFT_BUILD_CHANNEL=nightly-v1` compiles the isolated Nightly flavor: `cfg(soundcraft_nightly_v1)`,
//! the Nightly icon, `ProductName "SoundCraft Nightly"` and the exact feed version as
//! `ProductVersion` (the Launcher checks that field, not FileVersion). An unset, empty or `stable`
//! channel is the stable build; anything else is a build error.

#[path = "src/build_channel.rs"]
mod build_channel;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=SOUNDCRAFT_BUILD_CHANNEL");
    println!("cargo:rustc-check-cfg=cfg(soundcraft_nightly_v1)");
    let nightly = match std::env::var("SOUNDCRAFT_BUILD_CHANNEL").as_deref() {
        Ok("nightly-v1") => {
            println!("cargo:rustc-cfg=soundcraft_nightly_v1");
            true
        }
        // An empty value (an unset CI input) means the default stable build.
        Ok("stable" | "") | Err(std::env::VarError::NotPresent) => false,
        _ => {
            eprintln!("unsupported SOUNDCRAFT_BUILD_CHANNEL (expected stable or nightly-v1)");
            std::process::exit(1);
        }
    };
    println!("cargo:rerun-if-changed=../../assets/app-icon/soundcraft.ico");
    println!("cargo:rerun-if-changed=../../assets/app-icon/soundcraft-nightly.ico");
    println!("cargo:rerun-if-env-changed=SOUNDCRAFT_REQUIRE_WINRES");
    println!("cargo:rerun-if-env-changed=SOUNDCRAFT_VERSION");
    println!("cargo:rerun-if-env-changed=SOUNDCRAFT_OVERRIDE_VERSION");
    let built_version = if nightly {
        // The feed identity is SOUNDCRAFT_VERSION. The packaging scripts (packaging/env.sh and the
        // Linux package.sh, inherited from PhotoCraft) set the same value as
        // SOUNDCRAFT_OVERRIDE_VERSION, which is accepted as the one alias so both agree.
        let identity = std::env::var("SOUNDCRAFT_VERSION").or_else(|_| std::env::var("SOUNDCRAFT_OVERRIDE_VERSION"));
        match identity {
            Ok(version) if build_channel::is_nightly_version(&version) => version,
            _ => {
                eprintln!("nightly-v1 requires an explicit SOUNDCRAFT_VERSION feed identity (SOUNDCRAFT_OVERRIDE_VERSION is accepted)");
                std::process::exit(1);
            }
        }
    } else {
        std::env::var("CARGO_PKG_VERSION").unwrap_or_default()
    };
    println!("cargo:rustc-env=SOUNDCRAFT_BUILT_VERSION={built_version}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon(build_channel::windows_icon_path(nightly))
        .set("ProductName", if nightly { "SoundCraft Nightly" } else { "SoundCraft" })
        .set("FileDescription", if nightly { "SoundCraft Nightly digital audio workstation" } else { "SoundCraft digital audio workstation" })
        .set("CompanyName", "Learning Machines LLC")
        .set("LegalCopyright", "Copyright (c) the SoundCraft authors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "soundcraft.exe")
        .set("InternalName", "soundcraft");
    if nightly {
        // The Launcher verifies the PE ProductVersion against the exact feed identity, not FileVersion.
        res.set("ProductVersion", &built_version);
    }
    if let Err(e) = res.compile() {
        if std::env::var_os("SOUNDCRAFT_REQUIRE_WINRES").is_some() {
            panic!("embedding Windows resources failed: {e}");
        }
        println!("cargo:warning=soundcraft.exe built without icon/version resources: {e}");
    }
}
