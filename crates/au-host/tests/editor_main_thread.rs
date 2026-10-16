//! Audio Unit editors on the process main thread (`harness = false`: the standard harness never
//! runs tests there), against the units Apple ships with macOS. A view only ever lives inside a
//! host window (a Cocoa view outside one can crash the process once its unit is gone), so the
//! size probe puts it in a window that is never shown. Opening the editor on screen is checked by
//! hand. Skips (passes) when a unit is missing or there is no graphical session.

#[cfg(target_os = "macos")]
fn main() {
    use soundcraft_dsp::Plugin;

    const DELAY: &str = "au:aufx:dely:appl";
    const REVERB2: &str = "au:aufx:rvb2:appl";

    let all = soundcraft_au_host::scan();
    if ![DELAY, REVERB2].iter().all(|id| all.iter().any(|d| d.id == *id)) {
        eprintln!("editor_main_thread: Apple units missing; skipping");
        return;
    }
    // Windows need a login (Aqua) session; ssh and background jobs run in another one.
    let session =
        std::process::Command::new("launchctl").arg("managername").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).ok();
    if session.as_deref() != Some("Aqua") {
        eprintln!("editor_main_thread: no graphical session ({session:?}); skipping");
        return;
    }

    // A unit without a Cocoa view has no editor: the plugin window keeps the generic sliders.
    let mut reverb = soundcraft_au_host::instantiate_plugin(REVERB2).expect("AUReverb2");
    assert!(reverb.editor().is_none(), "AUReverb2 has no view of its own");
    assert!(reverb.open_editor().is_err());

    let mut p = soundcraft_au_host::instantiate_plugin(DELAY).expect("AUDelay");
    let mut ed = p.editor().expect("AUDelay has a Cocoa view");
    assert!(!ed.is_open());
    assert_eq!(p.editor_size(), Ok((484, 255)));
    assert!(!ed.is_open(), "the probe shows no window");
    assert_eq!(p.editor_size(), Ok((484, 255)), "probing twice");

    // Views and windows are main-thread only.
    let off_main = std::thread::spawn(move || {
        let r = ed.open();
        (r, ed)
    });
    let (r, mut ed) = off_main.join().expect("thread");
    assert!(r.is_err_and(|e| e.contains("main thread")));
    assert!(!ed.is_open());

    assert!(ed.idle().is_empty());
    ed.set_param("0", 0.25);
    assert!(ed.idle().is_empty(), "set_param is not reported back as an edit");

    drop(p);
    assert!(!ed.is_open());
    assert!(ed.open().is_err(), "the plugin is gone");
    assert!(ed.idle().is_empty());
    ed.set_param("0", 0.5);
    ed.close();
    println!("editor_main_thread: ok");
}

#[cfg(not(target_os = "macos"))]
fn main() {}
