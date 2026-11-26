//! App-owned nightly profile fences; no stable fallback, migration or process environment changes.

use std::ffi::OsString;

use crate::config::{PORTABLE_DATA_DIR, portable_marker};
use std::path::{Component, Path, PathBuf};

/// The Nightly Flatpak's own app id (packaging/linux/flatpak-bundle.sh). Inside that instance the
/// sandbox is the isolation, so it is the one Flatpak whose environment a Nightly launch accepts.
const NIGHTLY_FLATPAK_ID: &str = "ai.storyteller.soundcraft.nightly";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Platform {
    Windows,
    Linux,
    Mac,
}

impl Platform {
    fn current() -> Result<Self, String> {
        match std::env::consts::OS {
            "windows" => Ok(Self::Windows),
            "linux" => Ok(Self::Linux),
            "macos" => Ok(Self::Mac),
            _ => Err("nightly-v1 is not defined for this operating system".into()),
        }
    }

    fn ignores_case(self) -> bool {
        self != Self::Linux
    }
}

fn absolute(path: PathBuf, label: &str) -> Result<PathBuf, String> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("{label} must be absolute and contain no parent traversal"));
    }
    Ok(path)
}

fn required_base(env: &impl Fn(&str) -> Option<OsString>, key: &str) -> Result<PathBuf, String> {
    let value = env(key).filter(|v| !v.is_empty()).ok_or_else(|| format!("cannot establish the stable {key} boundary"))?;
    absolute(PathBuf::from(value), key)
}

fn xdg_base(env: &impl Fn(&str) -> Option<OsString>, key: &str, fallback: &str) -> Result<PathBuf, String> {
    match env(key).filter(|v| !v.is_empty()) {
        Some(value) => absolute(PathBuf::from(value), key),
        None => Ok(required_base(env, "HOME")?.join(fallback)),
    }
}

/// Resolve existing ancestors before creating anything. A dangling link or inaccessible ancestor
/// is not a missing directory that may be replaced or followed later.
fn canonical_future(path: &Path) -> Result<PathBuf, String> {
    let mut parent = path;
    let mut missing = Vec::new();
    loop {
        match std::fs::symlink_metadata(parent) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(parent.file_name().ok_or("the profile root has no existing ancestor")?.to_os_string());
                parent = parent.parent().ok_or("the profile root has no existing ancestor")?;
            }
            Err(error) => return Err(format!("cannot inspect the profile boundary: {error}")),
        }
    }
    let mut resolved = std::fs::canonicalize(parent).map_err(|e| format!("cannot resolve the profile boundary: {e}"))?;
    for part in missing.into_iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

fn overlaps(a: &Path, b: &Path, platform: Platform) -> bool {
    let parts = |p: &Path| {
        p.components()
            .map(|c| {
                let value = c.as_os_str().to_string_lossy().into_owned();
                if platform.ignores_case() { value.to_lowercase() } else { value }
            })
            .collect::<Vec<_>>()
    };
    let (a, b) = (parts(a), parts(b));
    a.starts_with(&b) || b.starts_with(&a)
}

fn installation_boundary(platform: Platform, exe: &Path) -> Result<PathBuf, String> {
    let parent = exe.parent().ok_or("cannot establish the installation boundary")?;
    if platform == Platform::Mac {
        // A profile in Resources is still inside the versioned install, not beside MacOS.
        if let Some(bundle) = exe.ancestors().find(|p| p.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("app"))) {
            return Ok(bundle.to_path_buf());
        }
    }
    Ok(parent.to_path_buf())
}

fn forbidden_roots(platform: Platform, env: &impl Fn(&str) -> Option<OsString>, exe: &Path) -> Result<Vec<PathBuf>, String> {
    let mut roots = match platform {
        Platform::Windows => {
            let mut roots = ["APPDATA", "LOCALAPPDATA"]
                .into_iter()
                .map(|key| required_base(env, key).map(|p| p.join("SoundCraft")))
                .collect::<Result<Vec<_>, _>>()?;
            // The MSI's default per-user and per-machine folders (install-policy.wxi). A custom
            // install folder is not known here; the executable's own folder is fenced below.
            roots.push(required_base(env, "LOCALAPPDATA")?.join("Programs").join("ArtCraft").join("SoundCraft"));
            for key in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
                if let Some(base) = env(key).filter(|v| !v.is_empty()) {
                    roots.push(absolute(PathBuf::from(base), key)?.join("ArtCraft").join("SoundCraft"));
                }
            }
            roots
        }
        Platform::Linux => {
            let mut roots =
                [("XDG_CONFIG_HOME", ".config"), ("XDG_DATA_HOME", ".local/share"), ("XDG_CACHE_HOME", ".cache"), ("XDG_STATE_HOME", ".local/state")]
                    .into_iter()
                    .map(|(key, fallback)| xdg_base(env, key, fallback).map(|p| p.join("soundcraft")))
                    .collect::<Result<Vec<_>, _>>()?;
            roots.extend(["/usr/bin/soundcraft", "/usr/local/bin/soundcraft", "/opt/soundcraft"].map(PathBuf::from));
            // The stable Flatpak keeps its data in its own per-app folder.
            roots.push(required_base(env, "HOME")?.join(".var/app/ai.storyteller.soundcraft"));
            roots
        }
        Platform::Mac => {
            let home = required_base(env, "HOME")?;
            let library = home.join("Library");
            vec![
                library.join("Application Support/SoundCraft"),
                library.join("Caches/SoundCraft"),
                library.join("Caches/ai.storyteller.soundcraft"),
                library.join("Preferences/SoundCraft"),
                library.join("Preferences/ai.storyteller.soundcraft.plist"),
                library.join("Saved Application State/ai.storyteller.soundcraft.savedState"),
                home.join("Applications/SoundCraft.app"),
                PathBuf::from("/Applications/SoundCraft.app"),
            ]
        }
    };
    roots.push(installation_boundary(platform, exe)?);
    roots.into_iter().map(|p| canonical_future(&p)).collect()
}

fn private_subdirectory(root: &Path, name: impl AsRef<Path>, label: &str) -> Result<PathBuf, String> {
    let chosen = canonical_future(&absolute(root.to_path_buf(), "the nightly profile root")?)?;
    let child = canonical_future(&chosen.join(name))?;
    if child == chosen || !child.starts_with(&chosen) {
        return Err(format!("the nightly {label} must stay inside its selected profile"));
    }
    Ok(child)
}

/// Every existing entry directly in the profile (Presets, logs, settings and so on) must resolve
/// inside it, so a link there cannot point Nightly's writes at another app's data.
fn entries_stay_inside(root: &Path) -> Result<(), String> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot inspect the nightly profile: {error}")),
    };
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot inspect the nightly profile: {e}"))?;
        private_subdirectory(root, entry.file_name(), "profile entry")?;
    }
    Ok(())
}

/// Recognise the shared portable data namespace in both the supplied and resolved path.
/// This is not an inventory of arbitrary custom Stable overrides or a Launcher ownership proof.
fn is_portable_layout(path: &Path, platform: Platform) -> bool {
    // A folder under a portable copy belongs to that copy too. Above the chosen folder itself, a
    // marker only counts beside a SoundCraft executable, so an unrelated portable.txt higher up
    // (in a user folder or at a drive root) does not refuse the profile.
    path.ancestors().enumerate().any(|(depth, ancestor)| {
        portable_marker(ancestor).is_some() && (depth == 0 || ["soundcraft.exe", "soundcraft"].iter().any(|exe| ancestor.join(exe).is_file()))
    }) || path.components().any(|component| match component {
        Component::Normal(name) if platform.ignores_case() => name.to_string_lossy().eq_ignore_ascii_case(PORTABLE_DATA_DIR),
        Component::Normal(name) => name == PORTABLE_DATA_DIR,
        _ => false,
    })
}

/// The writable hook is the app's existing exclusive probe. Keeping it injected lets filesystem
/// boundary tests exercise the real policy without starting the app or changing global state.
pub fn validate(
    root: &Path,
    env: impl Fn(&str) -> Option<OsString>,
    exe: Option<&Path>,
    writable: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<PathBuf, String> {
    validate_for(Platform::current()?, root, env, exe, writable)
}

fn validate_for(
    platform: Platform,
    root: &Path,
    env: impl Fn(&str) -> Option<OsString>,
    exe: Option<&Path>,
    writable: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<PathBuf, String> {
    absolute(root.to_path_buf(), "the nightly profile root")?;
    if env("SOUNDCRAFT_CONFIG_DIR").is_some_and(|v| !v.is_empty()) {
        // The Launcher protocol is the one root the app validates. A wrapper that isolates with
        // the stable override variable instead passes the profile explicitly; the message names
        // that, because this is what a Linux nightly launcher hits when it exports the variable.
        return Err(
            "nightly-v1 cannot be combined with SOUNDCRAFT_CONFIG_DIR; pass --launch-profile nightly-v1 --profile-root <dir> -- instead".into()
        );
    }
    // These rows are native tar/app-bundle launches, not a Flatpak package. Inheriting its
    // runtime socket namespace could point another profile at the stable app's socket. The
    // Nightly Flatpak is the exception: its own app id is the sandbox, so $HOME and the XDG
    // folders in there are already this package's own.
    if platform != Platform::Windows
        && let Some(instance) = env("FLATPAK_ID").filter(|v| !v.is_empty())
        && instance.to_string_lossy() != NIGHTLY_FLATPAK_ID
    {
        return Err("nightly-v1 native packages cannot inherit a Flatpak instance; launch through the native Launcher".into());
    }
    let chosen = canonical_future(root)?;
    if is_portable_layout(root, platform) || is_portable_layout(&chosen, platform) {
        return Err("the nightly root must not use a portable SoundCraft installation or its SoundCraftData settings folder".into());
    }
    let exe = absolute(exe.ok_or("cannot establish the installation boundary")?.to_path_buf(), "the executable")?;
    let forbidden = forbidden_roots(platform, &env, &exe)?;
    if forbidden.iter().any(|stable| overlaps(&chosen, stable, platform)) {
        return Err("the nightly root must not overlap stable data or an installation".into());
    }
    entries_stay_inside(&chosen)?;
    // Autosave is swept and rewritten on start and Presets are written in place: never let either
    // alias leave this private profile, including toward a Stable portable directory we cannot know.
    private_subdirectory(&chosen, "Autosave", "autosave folder")?;
    private_subdirectory(&chosen, "Presets", "presets folder")?;
    let mut create = std::fs::DirBuilder::new();
    create.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        create.mode(0o700);
    }
    create.create(&chosen).map_err(|e| format!("cannot create the nightly profile: {e}"))?;
    let confirmed = std::fs::canonicalize(&chosen).map_err(|e| format!("cannot resolve the created nightly profile: {e}"))?;
    if confirmed != chosen || forbidden.iter().any(|stable| overlaps(&confirmed, stable, platform)) {
        return Err("the nightly profile changed while it was being validated".into());
    }
    private_subdirectory(&confirmed, "Autosave", "autosave folder")?;
    private_subdirectory(&confirmed, "Presets", "presets folder")?;
    entries_stay_inside(&confirmed)?;
    writable(&confirmed).map_err(|e| format!("the nightly profile is not writable: {e}"))?;
    Ok(confirmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture {
        root: PathBuf,
        env: BTreeMap<String, OsString>,
        exe: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = loop {
                let candidate =
                    std::env::temp_dir().join(format!("soundcraft-profile-policy-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
                match std::fs::create_dir(&candidate) {
                    Ok(()) => break std::fs::canonicalize(candidate).unwrap(),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("cannot create owned fixture: {error}"),
                }
            };
            let home = root.join("home");
            let exe = root.join("install/soundcraft");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
            std::fs::write(&exe, b"fixture executable, never run").unwrap();
            let env = [("HOME", home), ("APPDATA", root.join("roaming")), ("LOCALAPPDATA", root.join("local"))]
                .into_iter()
                .map(|(key, value)| (key.into(), value.into_os_string()))
                .collect();
            Self { root, env, exe }
        }

        fn get(&self, key: &str) -> Option<OsString> {
            self.env.get(key).cloned()
        }

        fn check(&self, platform: Platform, root: &Path) -> Result<PathBuf, String> {
            validate_for(
                platform,
                root,
                |key| self.get(key),
                Some(&self.exe),
                |dir| {
                    let probe = dir.join("exclusive-test-probe");
                    let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&probe)?;
                    drop(file);
                    std::fs::remove_file(probe)
                },
            )
        }

        fn home(&self) -> PathBuf {
            PathBuf::from(self.get("HOME").unwrap())
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // Only the directory successfully created by this fixture is owned for removal.
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[test]
    fn portable_settings_from_another_install_are_refused_before_any_probe() {
        let fixture = Fixture::new();
        let stable_install = fixture.root.join("stable portable install");
        let stable_data = stable_install.join(PORTABLE_DATA_DIR);
        std::fs::create_dir_all(&stable_data).unwrap();
        std::fs::write(stable_install.join("portable.txt"), b"owned portable fixture").unwrap();
        std::fs::write(stable_data.join("settings.json"), b"stable settings must not change").unwrap();
        for platform in [Platform::Windows, Platform::Linux, Platform::Mac] {
            for candidate in [&stable_data, &stable_data.join("nested nightly")] {
                let probed = std::cell::Cell::new(false);
                let result = validate_for(
                    platform,
                    candidate,
                    |key| fixture.get(key),
                    Some(&fixture.exe),
                    |_| {
                        probed.set(true);
                        Ok(())
                    },
                );
                assert!(result.is_err(), "a different portable install is not a private Nightly profile");
                assert!(!probed.get(), "reject portable data before a writable probe");
                assert!(!stable_data.join("nested nightly").exists());
                assert_eq!(std::fs::read(stable_data.join("settings.json")).unwrap(), b"stable settings must not change");
                assert_eq!(std::fs::read_dir(&stable_data).unwrap().count(), 1);
            }
        }
        let private = fixture.root.join("SoundCraftData-backup-name/nightly");
        assert!(fixture.check(Platform::Linux, &private).is_ok(), "a merely similar name is not a portable settings component");
    }

    #[cfg(unix)]
    #[test]
    fn autosave_aliases_cannot_redirect_to_stable_missing_or_profile_root_data() {
        let fixture = Fixture::new();
        let stable = fixture.home().join(".local/state/soundcraft");
        std::fs::create_dir_all(&stable).unwrap();
        std::fs::write(stable.join("stable-autosave"), b"stable recovery bytes").unwrap();
        for (index, target) in [stable.clone(), fixture.root.join("missing recovery target")].into_iter().enumerate() {
            let private = fixture.root.join(format!("nightly-{index}"));
            std::fs::create_dir(&private).unwrap();
            std::os::unix::fs::symlink(&target, private.join("Autosave")).unwrap();
            let probed = std::cell::Cell::new(false);
            let result = validate_for(
                Platform::Linux,
                &private,
                |key| fixture.get(key),
                Some(&fixture.exe),
                |_| {
                    probed.set(true);
                    Ok(())
                },
            );
            assert!(result.is_err(), "Autosave must not escape the private profile or follow a dangling alias");
            assert!(!probed.get());
        }
        let private = fixture.root.join("nightly-self");
        std::fs::create_dir(&private).unwrap();
        std::os::unix::fs::symlink(&private, private.join("Autosave")).unwrap();
        assert!(fixture.check(Platform::Linux, &private).is_err(), "Autosave cannot treat all profile settings as autosaves");
        assert_eq!(std::fs::read(stable.join("stable-autosave")).unwrap(), b"stable recovery bytes");
        assert_eq!(std::fs::read_dir(stable).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn autosave_alias_to_a_private_subfolder_keeps_the_profile_usable() {
        let fixture = Fixture::new();
        let private = fixture.root.join("nightly-private");
        let autosave = private.join("private-recovery");
        std::fs::create_dir_all(&autosave).unwrap();
        std::os::unix::fs::symlink(&autosave, private.join("Autosave")).unwrap();
        assert!(fixture.check(Platform::Linux, &private).is_ok());
        assert_eq!(std::fs::canonicalize(private.join("Autosave")).unwrap(), autosave);
    }

    #[test]
    fn overlap_check_handles_ancestor_descendant_and_prefix_lookalikes() {
        let fixture = Fixture::new();
        let root = fixture.home().join("nightly/profiles/soundcraft");
        for platform in [Platform::Windows, Platform::Linux, Platform::Mac] {
            assert!(overlaps(&root, &root.join("Autosave"), platform));
            assert!(overlaps(&root.join("Autosave"), &root, platform));
            assert!(!overlaps(&root, &root.with_file_name("soundcraft-other"), platform));
        }
    }

    #[test]
    fn linux_fences_app_xdg_subtrees_but_allows_launcher_sibling() {
        let mut fixture = Fixture::new();
        for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME"] {
            fixture.env.insert(key.into(), fixture.root.join(key).into_os_string());
        }
        for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME"] {
            let stable = PathBuf::from(fixture.get(key).unwrap()).join("soundcraft");
            std::fs::create_dir_all(&stable).unwrap();
            std::fs::write(stable.join("stable-sentinel"), b"stable bytes").unwrap();
            for candidate in [stable.clone(), stable.join("nightly"), stable.parent().unwrap().to_path_buf()] {
                assert!(fixture.check(Platform::Linux, &candidate).unwrap_err().contains("overlap"));
            }
            assert_eq!(std::fs::read(stable.join("stable-sentinel")).unwrap(), b"stable bytes");
        }
        let data = PathBuf::from(fixture.get("XDG_DATA_HOME").unwrap());
        let nightly = data.join("ArtCraftLauncher/nightly/profiles/soundcraft");
        assert_eq!(fixture.check(Platform::Linux, &nightly).unwrap(), nightly);
        assert_eq!(std::fs::read_dir(nightly).unwrap().count(), 0);
    }

    #[test]
    fn mac_fences_only_app_library_subtrees_and_the_entire_bundle() {
        let mut fixture = Fixture::new();
        fixture.exe = fixture.root.join("install/SoundCraft Nightly.app/Contents/MacOS/SoundCraft");
        std::fs::create_dir_all(fixture.exe.parent().unwrap()).unwrap();
        std::fs::write(&fixture.exe, b"fixture executable, never run").unwrap();
        let library = fixture.home().join("Library");
        for suffix in [
            "Application Support/SoundCraft",
            "Caches/SoundCraft",
            "Caches/ai.storyteller.soundcraft",
            "Preferences/SoundCraft",
            "Saved Application State/ai.storyteller.soundcraft.savedState",
        ] {
            let stable = library.join(suffix);
            assert!(fixture.check(Platform::Mac, &stable.join("nightly")).unwrap_err().contains("overlap"));
            assert!(!stable.exists());
        }
        let inside_resources = fixture.exe.parent().unwrap().parent().unwrap().join("Resources/profile");
        assert!(fixture.check(Platform::Mac, &inside_resources).unwrap_err().contains("overlap"));
        assert!(!inside_resources.exists());
        let valid = library.join("Application Support/ArtCraftLauncher/nightly/profiles/soundcraft");
        assert_eq!(fixture.check(Platform::Mac, &valid).unwrap(), valid);
    }

    #[test]
    fn windows_roaming_local_and_install_fences_remain_active() {
        let fixture = Fixture::new();
        for key in ["APPDATA", "LOCALAPPDATA"] {
            let stable = PathBuf::from(fixture.get(key).unwrap()).join("SoundCraft");
            assert!(fixture.check(Platform::Windows, &stable.join("nightly")).unwrap_err().contains("overlap"));
            assert!(!stable.exists());
        }
        assert!(fixture.check(Platform::Windows, &fixture.exe.parent().unwrap().join("profile")).is_err());
        assert!(fixture.check(Platform::Windows, &fixture.root.join("Launcher/profiles/soundcraft")).is_ok());
    }

    #[test]
    fn windows_default_msi_install_folders_are_fenced() {
        let mut fixture = Fixture::new();
        let local = PathBuf::from(fixture.get("LOCALAPPDATA").unwrap());
        let per_user = local.join("Programs").join("ArtCraft").join("SoundCraft");
        assert!(fixture.check(Platform::Windows, &per_user.join("nightly")).unwrap_err().contains("overlap"));
        for key in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
            let base = fixture.root.join(format!("machine {key}"));
            fixture.env.insert(key.into(), base.clone().into_os_string());
            let machine = base.join("ArtCraft").join("SoundCraft");
            assert!(fixture.check(Platform::Windows, &machine.join("nightly")).unwrap_err().contains("overlap"));
            assert!(!machine.exists());
        }
        let sibling = local.join("Programs").join("ArtCraft").join("ArtCraftLauncher").join("nightly").join("soundcraft");
        assert!(fixture.check(Platform::Windows, &sibling).is_ok(), "the Launcher's own Programs sibling is not SoundCraft's install");
    }

    #[test]
    fn folders_under_a_stable_portable_copy_are_refused_before_any_probe() {
        let fixture = Fixture::new();
        let portable = fixture.root.join("D/PC");
        std::fs::create_dir_all(&portable).unwrap();
        std::fs::write(portable.join("portable.txt"), b"stable portable copy").unwrap();
        std::fs::write(portable.join("soundcraft.exe"), b"stable portable executable, never run").unwrap();
        for platform in [Platform::Windows, Platform::Linux, Platform::Mac] {
            for candidate in [portable.join("nightly"), portable.join("deeper/nightly")] {
                let probed = std::cell::Cell::new(false);
                let result = validate_for(
                    platform,
                    &candidate,
                    |key| fixture.get(key),
                    Some(&fixture.exe),
                    |_| {
                        probed.set(true);
                        Ok(())
                    },
                );
                assert!(result.unwrap_err().contains("portable"));
                assert!(!probed.get());
                assert!(!candidate.exists());
            }
        }
        // A portable.txt that no SoundCraft executable sits beside is someone else's file.
        let unrelated = fixture.root.join("user folder");
        std::fs::create_dir_all(&unrelated).unwrap();
        std::fs::write(unrelated.join("portable.txt"), b"another app's marker").unwrap();
        assert!(fixture.check(Platform::Windows, &unrelated.join("Launcher/nightly/soundcraft")).is_ok());
    }

    #[test]
    fn linux_fences_the_stable_flatpak_data_folder() {
        let fixture = Fixture::new();
        let flatpak = fixture.home().join(".var/app/ai.storyteller.soundcraft");
        for candidate in [flatpak.clone(), flatpak.join("data/nightly")] {
            assert!(fixture.check(Platform::Linux, &candidate).unwrap_err().contains("overlap"));
        }
        assert!(!flatpak.exists());
        assert!(fixture.check(Platform::Linux, &fixture.home().join(".var/app/ai.storyteller.launcher/nightly")).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn any_profile_entry_alias_must_stay_inside_the_profile() {
        let fixture = Fixture::new();
        let stable = fixture.home().join(".config/soundcraft/Presets");
        std::fs::create_dir_all(&stable).unwrap();
        std::fs::write(stable.join("stable-preset"), b"stable preset bytes").unwrap();
        for name in ["Presets", "logs", "settings.json"] {
            let private = fixture.root.join(format!("nightly-{name}"));
            std::fs::create_dir(&private).unwrap();
            std::os::unix::fs::symlink(&stable, private.join(name)).unwrap();
            let probed = std::cell::Cell::new(false);
            let result = validate_for(
                Platform::Linux,
                &private,
                |key| fixture.get(key),
                Some(&fixture.exe),
                |_| {
                    probed.set(true);
                    Ok(())
                },
            );
            assert!(result.is_err(), "{name} must not lead outside the profile");
            assert!(!probed.get());
        }
        let private = fixture.root.join("nightly-inside");
        std::fs::create_dir_all(private.join("real-presets")).unwrap();
        std::fs::write(private.join("settings.json"), b"{}").unwrap();
        std::os::unix::fs::symlink(private.join("real-presets"), private.join("Presets")).unwrap();
        assert!(fixture.check(Platform::Linux, &private).is_ok(), "regular files and links that stay inside are fine");
        assert_eq!(std::fs::read(stable.join("stable-preset")).unwrap(), b"stable preset bytes");
        assert_eq!(std::fs::read_dir(&stable).unwrap().count(), 1);
    }

    #[test]
    fn ambiguous_environment_and_non_native_sandbox_are_refused_before_creation() {
        let mut fixture = Fixture::new();
        let candidate = fixture.home().join("nightly/profiles/soundcraft");
        fixture.env.insert("XDG_CACHE_HOME".into(), "relative/cache".into());
        assert!(fixture.check(Platform::Linux, &candidate).unwrap_err().contains("absolute"));
        fixture.env.remove("XDG_CACHE_HOME");
        fixture.env.insert("SOUNDCRAFT_CONFIG_DIR".into(), fixture.home().join("custom-stable").into_os_string());
        assert!(fixture.check(Platform::Linux, &candidate).unwrap_err().contains("SOUNDCRAFT_CONFIG_DIR"));
        fixture.env.remove("SOUNDCRAFT_CONFIG_DIR");
        fixture.env.insert("FLATPAK_ID".into(), "ai.storyteller.soundcraft".into());
        assert!(fixture.check(Platform::Linux, &candidate).unwrap_err().contains("Flatpak"));
        // The Nightly Flatpak is its own instance: the profile inside it is a sandbox-local folder.
        fixture.env.insert("FLATPAK_ID".into(), "ai.storyteller.soundcraft.nightly".into());
        let sandboxed = fixture.home().join(".var/app/ai.storyteller.soundcraft.nightly/config/soundcraft-nightly");
        assert_eq!(fixture.check(Platform::Linux, &sandboxed).unwrap(), sandboxed);
        fixture.env.remove("FLATPAK_ID");
        fixture.env.remove("HOME");
        assert!(fixture.check(Platform::Linux, &candidate).unwrap_err().contains("HOME"));
        assert!(!candidate.exists());
    }

    #[test]
    fn non_directory_and_failed_write_probe_never_fall_back_to_stable() {
        let fixture = Fixture::new();
        let file = fixture.home().join("not-a-directory");
        std::fs::write(&file, b"do not replace me").unwrap();
        assert!(fixture.check(Platform::Linux, &file).is_err());
        assert_eq!(std::fs::read(file).unwrap(), b"do not replace me");
        let candidate = fixture.home().join("nightly");
        let calls = std::cell::Cell::new(0);
        let result = validate_for(
            Platform::Linux,
            &candidate,
            |key| fixture.get(key),
            Some(&fixture.exe),
            |_| {
                calls.set(calls.get() + 1);
                Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "injected write refusal"))
            },
        );
        assert!(result.unwrap_err().contains("not writable"));
        assert_eq!(calls.get(), 1);
        assert!(!fixture.home().join(".config/soundcraft").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_and_dangling_aliases_cannot_create_or_modify_stable_data() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let stable = fixture.home().join(".config/soundcraft");
        std::fs::create_dir_all(&stable).unwrap();
        std::fs::write(stable.join("sentinel"), b"untouched").unwrap();
        let alias = fixture.home().join("alias");
        symlink(&stable, &alias).unwrap();
        assert!(fixture.check(Platform::Linux, &alias.join("nightly")).unwrap_err().contains("overlap"));
        let missing = fixture.home().join("missing-target");
        let dangling = fixture.home().join("dangling");
        symlink(&missing, &dangling).unwrap();
        assert!(fixture.check(Platform::Linux, &dangling.join("nightly")).is_err());
        assert!(!missing.exists());
        assert!(!stable.join("nightly").exists());
        assert_eq!(std::fs::read(stable.join("sentinel")).unwrap(), b"untouched");
    }
}
