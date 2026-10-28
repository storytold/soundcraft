//! How `soundcraft-cli` parses its arguments and reports results today, run as the real binary.
//! A change to the command line's behaviour updates them on purpose.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};

/// What one run of the binary produced.
struct Invoke {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Runs the binary in `dir` with empty stdin and no control-port variable.
fn invoke(dir: &Path, args: &[&str]) -> Invoke {
    invoke_env(dir, args, None)
}

fn invoke_env(dir: &Path, args: &[&str], control_port: Option<&str>) -> Invoke {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_soundcraft-cli"));
    cmd.env("SOUNDCRAFT_NO_PREFS", "1").env_remove("SOUNDCRAFT_CONTROL_PORT");
    if let Some(port) = control_port {
        cmd.env("SOUNDCRAFT_CONTROL_PORT", port);
    }
    let out = cmd.current_dir(dir).args(args).stdin(Stdio::null()).output().unwrap();
    Invoke { code: out.status.code().unwrap(), stdout: String::from_utf8(out.stdout).unwrap(), stderr: String::from_utf8(out.stderr).unwrap() }
}

/// A fresh directory holding `in.wav` (200 samples, 16-bit mono, 48 kHz) and `s.txt` (a one-command script).
fn sandbox(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sc-cli-behaviour-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut buf = AudioBuffer::new(48_000, 1, 200);
    for (i, d) in buf.channels[0].iter_mut().enumerate() {
        *d = i as f32 / 400.0;
    }
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None };
    std::fs::write(dir.join("in.wav"), soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();
    std::fs::write(dir.join("s.txt"), "track.new {}\n").unwrap();
    dir
}

/// The sample rate of the WAV `name` in `dir`.
fn rate_of(dir: &Path, name: &str) -> u32 {
    soundcraft_audio_io::probe(&std::fs::read(dir.join(name)).unwrap(), None).unwrap().sample_rate
}

/// Bit depth of the WAV `name` in `dir`, as the probe reports the sample format.
fn format_of(dir: &Path, name: &str) -> String {
    format!("{:?}", soundcraft_audio_io::probe(&std::fs::read(dir.join(name)).unwrap(), None).unwrap().sample_format)
}

/// Exit 1, nothing on stdout, and exactly `error: {message}` on stderr.
#[track_caller]
fn assert_fails(got: &Invoke, message: &str) {
    assert_eq!(got.code, 1, "stderr: {}", got.stderr);
    assert_eq!(got.stdout, "");
    assert_eq!(got.stderr, format!("error: {message}\n"));
}

/// Exit 0 with nothing on either stream.
#[track_caller]
fn assert_silent_success(got: &Invoke) {
    assert_eq!((got.code, got.stdout.as_str(), got.stderr.as_str()), (0, "", ""));
}

/// Exit 2, nothing on stdout, and a clap usage error starting with `error: {message}` on stderr.
#[track_caller]
fn assert_usage_error(got: &Invoke, message: &str) {
    assert_eq!(got.code, 2, "stderr: {}", got.stderr);
    assert_eq!(got.stdout, "");
    assert!(got.stderr.starts_with(&format!("error: {message}")), "{}", got.stderr);
}

/// The OS's message for a missing file: error 2 is `ENOENT` on Unix and `ERROR_FILE_NOT_FOUND` on Windows.
fn not_found() -> std::io::Error {
    std::io::Error::from_raw_os_error(2)
}

/// Exit 1 with the refused-connection message for `port`; the OS's text for the refusal differs by platform.
#[track_caller]
fn assert_refused(got: &Invoke, port: u16) {
    assert_eq!((got.code, got.stdout.as_str()), (1, ""));
    let prefix = format!("error: cannot reach SoundCraft at 127.0.0.1:{port}: ");
    assert!(got.stderr.starts_with(&prefix) && got.stderr.contains(" (os error ") && got.stderr.ends_with(")\n"), "{}", got.stderr);
}

// ---- unknown flags ----

#[test]
fn unknown_flags_stop_the_work_before_it_starts() {
    let dir = sandbox("unknown-flag-reject");
    assert_usage_error(&invoke(&dir, &["convert", "in.wav", "out.wav", "--bogus"]), "unexpected argument '--bogus' found");
    assert_usage_error(&invoke(&dir, &["run", "--bogus", "--save", "s.scraft"]), "unexpected argument '--bogus' found");
    assert_usage_error(&invoke(&dir, &["script", "s.txt", "--bogus"]), "unexpected argument '--bogus' found");
    // A single-dash word is a flag too.
    assert_usage_error(&invoke(&dir, &["convert", "in.wav", "out.wav", "-x"]), "unexpected argument '-x' found");
    assert!(!dir.join("out.wav").exists() && !dir.join("s.scraft").exists());
}

#[test]
fn convert_flags_may_come_before_or_between_the_positionals() {
    let dir = sandbox("convert-order");
    let got = invoke(&dir, &["convert", "--rate", "44100", "in.wav", "a.wav"]);
    assert_eq!((got.code, got.stdout.as_str(), got.stderr.as_str()), (0, "wrote a.wav (1 ch, 44100 Hz, 0.00 s)\n", ""));
    let got = invoke(&dir, &["convert", "in.wav", "--rate", "44100", "b.wav"]);
    assert_eq!((got.code, got.stdout.as_str(), got.stderr.as_str()), (0, "wrote b.wav (1 ch, 44100 Hz, 0.00 s)\n", ""));
    assert_eq!((rate_of(&dir, "a.wav"), rate_of(&dir, "b.wav")), (44_100, 44_100));
}

// ---- value flags ----

#[test]
fn convert_value_flags_take_a_separate_or_inline_value() {
    let dir = sandbox("convert-values");
    assert_eq!(invoke(&dir, &["convert", "in.wav", "c.wav", "--rate", "44100"]).stdout, "wrote c.wav (1 ch, 44100 Hz, 0.00 s)\n");
    assert_eq!(invoke(&dir, &["convert", "in.wav", "d.wav", "--rate=44100"]).stdout, "wrote d.wav (1 ch, 44100 Hz, 0.00 s)\n");
    assert_eq!(rate_of(&dir, "c.wav"), 44_100);
    assert_eq!(rate_of(&dir, "d.wav"), 44_100);
    assert_eq!(invoke(&dir, &["convert", "in.wav", "e.wav", "--bit-depth", "16"]).code, 0);
    assert_eq!(invoke(&dir, &["convert", "in.wav", "f.wav", "--bit-depth=32f"]).code, 0);
    assert_eq!(format_of(&dir, "e.wav"), "Int16");
    assert_eq!(format_of(&dir, "f.wav"), "Float32");
    assert_eq!(invoke(&dir, &["convert", "in.wav", "h.wav", "--bit-depth", "32"]).code, 0);
    assert_eq!(format_of(&dir, "h.wav"), "Float32");
    assert_eq!(invoke(&dir, &["convert", "in.wav", "g.wav"]).code, 0);
    assert_eq!(format_of(&dir, "g.wav"), "Int24");
}

#[test]
fn convert_value_flag_without_a_value_or_with_a_bad_one_fails() {
    let dir = sandbox("convert-bad-values");
    assert_usage_error(&invoke(&dir, &["convert", "in.wav", "o.wav", "--rate"]), "a value is required for '--rate <HZ>' but none was supplied");
    assert_usage_error(&invoke(&dir, &["convert", "in.wav", "o.wav", "--bit-depth"]), "a value is required for '--bit-depth <16|24|32f>'");
    // A following flag is the next flag, not the value.
    assert_usage_error(&invoke(&dir, &["convert", "in.wav", "o.wav", "--rate", "--normalize"]), "a value is required for '--rate <HZ>'");
    assert_usage_error(&invoke(&dir, &["convert", "in.wav", "o.wav", "--normalize=1"]), "unexpected value '1' for '--normalize' found");
    assert!(!dir.join("o.wav").exists());
}

#[test]
fn repeated_flags_are_usage_errors_in_either_spelling() {
    let dir = sandbox("repeat");
    for args in [
        &["convert", "in.wav", "a.wav", "--rate", "44100", "--rate", "22050"][..],
        &["convert", "in.wav", "a.wav", "--rate=22050", "--rate", "44100"],
        &["convert", "in.wav", "a.wav", "--bit-depth", "16", "--bit-depth", "24"],
        &["convert", "in.wav", "a.wav", "--normalize", "--normalize"],
        &["run", "--save", "a.scraft", "--save", "b.scraft"],
        &["run", "--demo", "--demo"],
        &["run", "--demo", "--sample"],
        &["script", "s.txt", "--keep-going", "--keep-going"],
        &["app", "--port", "1", "--port", "2", "x"],
        &["mcp", "--demo", "--demo"],
    ] {
        let got = invoke(&dir, args);
        assert_usage_error(&got, "the argument '--");
        assert_eq!(got.code, 2, "{args:?}");
    }
    assert!(!dir.join("a.wav").exists() && !dir.join("a.scraft").exists());
}

#[test]
fn convert_input_and_output_errors() {
    let dir = sandbox("convert-io");
    assert_fails(&invoke(&dir, &["convert", "nope.wav", "o.wav"]), &format!("nope.wav: {}", not_found()));
    assert_fails(&invoke(&dir, &["convert", "in.wav", "o.xyz"]), "o.xyz: cannot write `.xyz` files (supported: wav, wave, bwf, aif, aiff, flac)");
}

#[test]
fn run_value_flags_take_a_separate_or_inline_value() {
    let dir = sandbox("run-values");
    let tracks = "track.new: {\"tracks\":[1]}\n";
    assert_eq!(invoke(&dir, &["run", "--cmd", "track.new"]).stdout, tracks);
    assert_eq!(invoke(&dir, &["run", "--cmd=track.new"]).stdout, tracks);
    let save = invoke(&dir, &["run", "--save", "s1.scraft"]);
    assert_eq!((save.code, save.stdout.as_str()), (0, "{\"audio_files_written\":0,\"path\":\"s1.scraft\"}\n"));
    let save = invoke(&dir, &["run", "--save=s2.scraft"]);
    assert_eq!((save.code, save.stdout.as_str()), (0, "{\"audio_files_written\":0,\"path\":\"s2.scraft\"}\n"));
    assert!(dir.join("s1.scraft").exists() && dir.join("s2.scraft").exists());
    // --in in both spellings loads the saved session.
    for args in [["run", "--in", "s1.scraft", "--inspect"], ["run", "--in=s1.scraft", "--inspect", ""]] {
        let args: Vec<&str> = args.into_iter().filter(|a| !a.is_empty()).collect();
        let got = invoke(&dir, &args);
        assert_eq!(got.code, 0, "{args:?}: {}", got.stderr);
        assert!(got.stdout.contains("\"dirty\": false"), "{args:?}: {}", got.stdout);
    }
    // --bounce with --start/--end in both spellings.
    let spaced = invoke(&dir, &["run", "--demo", "--bounce", "b1.wav", "--start", "0", "--end", "0.1"]);
    assert_eq!(spaced.code, 0, "{}", spaced.stderr);
    assert!(spaced.stdout.contains("\"path\":\"b1.wav\"") && spaced.stdout.contains("\"seconds\":0.1"), "{}", spaced.stdout);
    let inline = invoke(&dir, &["run", "--demo", "--bounce=b2.wav", "--start=0", "--end=0.1"]);
    assert_eq!(inline.code, 0, "{}", inline.stderr);
    assert!(inline.stdout.contains("\"path\":\"b2.wav\"") && inline.stdout.contains("\"seconds\":0.1"), "{}", inline.stdout);
    assert!(dir.join("b1.wav").exists() && dir.join("b2.wav").exists());
}

#[test]
fn run_cmd_repeats_and_runs_every_occurrence_in_order() {
    let dir = sandbox("run-repeat");
    assert_eq!(invoke(&dir, &["run", "--cmd", "track.new", "--cmd=track.new"]).stdout, "track.new: {\"tracks\":[1]}\ntrack.new: {\"tracks\":[2]}\n");
}

#[test]
fn negative_times_are_values_but_other_dash_words_are_not() {
    let dir = sandbox("negative-time");
    // The engine counts a position before 0, so the bounce starts a second ahead of the session.
    for args in [&["--start", "-1"][..], &["--start=-1"]] {
        let mut full = vec!["run", "--demo", "--end", "0.1", "--bounce", "b.wav"];
        full.extend(args);
        let got = invoke(&dir, &full);
        assert_eq!(got.code, 0, "{}", got.stderr);
        assert!(got.stdout.contains("\"seconds\":1.1"), "{}", got.stdout);
    }
    assert_usage_error(&invoke(&dir, &["run", "--bounce", "-x.wav"]), "unexpected argument '-x' found");
    assert_usage_error(&invoke(&dir, &["run", "--cmd", "-x"]), "unexpected argument '-x' found");
}

#[test]
fn script_runs_lines_and_takes_options_before_or_after_the_file() {
    let dir = sandbox("script");
    let got = invoke(&dir, &["script", "s.txt"]);
    assert_eq!((got.code, got.stdout.as_str(), got.stderr.as_str()), (0, "track.new: {\"tracks\":[1]}\n", ""));
    let got = invoke(&dir, &["script", "s.txt", "--save", "sv.scraft"]);
    assert_eq!(got.stdout, "track.new: {\"tracks\":[1]}\n{\"audio_files_written\":0,\"path\":\"sv.scraft\"}\n");
    assert!(dir.join("sv.scraft").exists());
    assert_eq!(invoke(&dir, &["script", "s.txt", "--keep-going"]).stdout, "track.new: {\"tracks\":[1]}\n");
    assert_eq!(invoke(&dir, &["script", "--keep-going", "s.txt"]).stdout, "track.new: {\"tracks\":[1]}\n");
    assert_fails(&invoke(&dir, &["script", "nope.txt"]), &format!("nope.txt: {}", not_found()));
    assert_fails(&invoke(&dir, &["script", "s.txt", "--in=nope.scraft"]), &format!("nope.scraft: {}", not_found()));
}

#[test]
fn script_line_errors_stop_unless_keep_going() {
    let dir = sandbox("script-errors");
    std::fs::write(dir.join("bad.txt"), "nosuch.cmd {}\ntrack.new {}\n").unwrap();
    std::fs::write(dir.join("badjson.txt"), "track.new {x\n").unwrap();
    // A stopping line error carries the `error:` prefix; a --keep-going one is a bare line.
    let stop = invoke(&dir, &["script", "bad.txt"]);
    assert_eq!((stop.code, stop.stdout.as_str()), (1, ""));
    assert_eq!(stop.stderr, "error: bad.txt:1: nosuch.cmd: unknown command `nosuch.cmd`\n");
    let keep = invoke(&dir, &["script", "bad.txt", "--keep-going"]);
    assert_eq!((keep.code, keep.stdout.as_str()), (0, "track.new: {\"tracks\":[1]}\n"));
    assert_eq!(keep.stderr, "bad.txt:1: nosuch.cmd: unknown command `nosuch.cmd`\n");
    assert_fails(&invoke(&dir, &["script", "badjson.txt"]), "badjson.txt:1: bad JSON: key must be a string at line 1 column 2");
}

#[test]
fn info_prints_json_for_audio_and_fails_for_missing_files() {
    let dir = sandbox("info");
    let got = invoke(&dir, &["info", "in.wav"]);
    assert_eq!((got.code, got.stderr.as_str()), (0, ""));
    assert!(
        got.stdout.contains("\"channels\": 1")
            && got.stdout.contains("\"sample_rate\": 48000")
            && got.stdout.contains("\"sample_format\": \"Int16\""),
        "{}",
        got.stdout
    );
    assert_fails(&invoke(&dir, &["info", "nope.wav"]), &format!("nope.wav: {}", not_found()));
}

/// The extension decides a session in any case, as the desktop app's `open_path` does.
#[test]
fn info_reads_a_session_whatever_the_extension_case() {
    let dir = sandbox("info-session-case");
    assert_eq!(invoke(&dir, &["run", "--demo", "--save", "S.SCRAFT"]).code, 0);
    let got = invoke(&dir, &["info", "S.SCRAFT"]);
    assert_eq!((got.code, got.stderr.as_str()), (0, ""));
    assert!(got.stdout.starts_with("SESSION NAME:"), "{}", got.stdout);
}

#[test]
fn info_compact_is_the_same_json_on_one_line() {
    let dir = sandbox("info-compact");
    let (pretty, compact) = (invoke(&dir, &["info", "in.wav"]), invoke(&dir, &["info", "in.wav", "--compact"]));
    assert_eq!((compact.code, compact.stderr.as_str()), (0, ""));
    assert_eq!(compact.stdout.trim_end().lines().count(), 1, "{}", compact.stdout);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&pretty.stdout).unwrap(),
        serde_json::from_str::<serde_json::Value>(&compact.stdout).unwrap()
    );
}

#[test]
fn parity_writes_a_report_with_write_in_both_spellings() {
    let dir = sandbox("parity");
    let stdout_report = invoke(&dir, &["parity"]);
    assert_eq!(stdout_report.code, 0);
    assert!(stdout_report.stdout.starts_with("# Parity report\n"), "{}", stdout_report.stdout);
    for (flag, name) in [(vec!["--write", "p1.md"], "p1.md"), (vec!["--write=p2.md"], "p2.md")] {
        let mut args = vec!["parity"];
        args.extend(flag);
        let got = invoke(&dir, &args);
        assert_eq!(got.code, 0, "{}", got.stderr);
        assert!(got.stdout.starts_with(&format!("wrote {name}: ")), "{}", got.stdout);
        assert!(std::fs::read_to_string(dir.join(name)).unwrap().starts_with("# Parity report\n"));
    }
    assert_usage_error(&invoke(&dir, &["parity", "--write"]), "a value is required for '--write <PATH>' but none was supplied");
}

// ---- outputs that replace an input ----

/// `in.wav`, `s.txt` and a saved session `s.scraft`.
fn sandbox_with_session(tag: &str) -> PathBuf {
    let dir = sandbox(tag);
    assert_eq!(invoke(&dir, &["run", "--save", "s.scraft"]).code, 0);
    dir
}

#[test]
fn an_output_that_is_an_input_is_refused_before_any_work() {
    let dir = sandbox_with_session("out-is-in");
    std::fs::hard_link(dir.join("in.wav"), dir.join("hard.wav")).unwrap();
    let (wav, session, script) =
        (std::fs::read(dir.join("in.wav")).unwrap(), std::fs::read(dir.join("s.scraft")).unwrap(), std::fs::read(dir.join("s.txt")).unwrap());
    let in_wav = dir.join("in.wav");
    let absolute = in_wav.to_str().unwrap();
    for out in ["in.wav", "./in.wav", "../{}/in.wav", absolute, "hard.wav"] {
        let out = out.replace("{}", dir.file_name().unwrap().to_str().unwrap());
        let got = invoke(&dir, &["convert", "in.wav", &out, "--rate", "44100"]);
        assert_usage_error(&got, &format!("<OUT> {out} is the <IN> file, which it would replace: choose another file, or pass --in-place"));
    }
    // Nothing ran: no --cmd result, no bounce, no save.
    let refused: &[(&[&str], &str)] = &[
        (&["run", "--in", "s.scraft", "--cmd", "track.new", "--save", "s.scraft"], "--save s.scraft is the --in session"),
        (&["run", "--in=./s.scraft", "--cmd", "track.new", "--bounce", "s.scraft"], "--bounce s.scraft is the --in session"),
        // --in-place lets only --save replace the session.
        (
            &["run", "--in", "s.scraft", "--in-place", "--cmd", "track.new", "--bounce", "./s.scraft"],
            "--bounce ./s.scraft is the --in session, which it would replace with audio: choose another file",
        ),
        (&["script", "s.txt", "--in", "s.scraft", "--in-place", "--bounce", "s.scraft"], "--bounce s.scraft is the --in session"),
        (&["script", "s.txt", "--in", "s.scraft", "--save", "./s.scraft"], "--save ./s.scraft is the --in session"),
        (&["script", "s.txt", "--save", "./s.txt"], "--save ./s.txt is the script file, which it would replace: choose another file"),
        (&["script", "s.txt", "--in", "s.scraft", "--in-place", "--bounce", "s.txt"], "--bounce s.txt is the script file"),
        (&["run", "--demo", "--cmd", "track.new", "--bounce", "b.wav", "--save", "./b.wav"], "--bounce and --save both name ./b.wav"),
    ];
    for (args, message) in refused {
        let got = invoke(&dir, args);
        assert_usage_error(&got, message);
        assert!(got.stderr.contains("\nUsage: soundcraft-cli "), "{}", got.stderr);
    }
    assert_eq!(std::fs::read(dir.join("in.wav")).unwrap(), wav);
    assert_eq!(std::fs::read(dir.join("s.scraft")).unwrap(), session);
    assert_eq!(std::fs::read(dir.join("s.txt")).unwrap(), script);
    assert!(!dir.join("b.wav").exists());
    assert_usage_error(
        &invoke(&dir, &["run", "--in-place", "--save", "s.scraft"]),
        "the following required arguments were not provided:\n  --in <S.scraft>",
    );
}

#[cfg(unix)]
#[test]
fn an_output_linked_to_the_input_is_refused() {
    let dir = sandbox_with_session("out-is-in-link");
    std::os::unix::fs::symlink("in.wav", dir.join("link.wav")).unwrap();
    std::os::unix::fs::symlink(".", dir.join("here")).unwrap();
    assert_usage_error(&invoke(&dir, &["convert", "in.wav", "link.wav"]), "<OUT> link.wav is the <IN> file");
    assert_usage_error(&invoke(&dir, &["run", "--in", "s.scraft", "--save", "here/s.scraft"]), "--save here/s.scraft is the --in session");
    // Two outputs that don't exist yet, one through a linked folder.
    assert_usage_error(&invoke(&dir, &["run", "--bounce", "here/b.wav", "--save", "b.wav"]), "--bounce and --save both name b.wav");
    assert_eq!(rate_of(&dir, "in.wav"), 48_000);
}

#[test]
fn in_place_replaces_the_input_and_other_files_are_still_replaced() {
    let dir = sandbox_with_session("in-place");
    let got = invoke(&dir, &["convert", "in.wav", "./in.wav", "--rate", "44100", "--in-place"]);
    assert_eq!(got.code, 0, "{}", got.stderr);
    assert_eq!(rate_of(&dir, "in.wav"), 44_100);
    let got = invoke(&dir, &["run", "--in", "s.scraft", "--cmd", "track.new", "--save", "s.scraft", "--in-place"]);
    assert_eq!(got.stdout, "track.new: {\"tracks\":[1]}\n{\"audio_files_written\":0,\"path\":\"s.scraft\"}\n", "{}", got.stderr);
    // The saved session holds that track, so the next one is the second.
    assert_eq!(invoke(&dir, &["run", "--in", "s.scraft", "--cmd", "track.new"]).stdout, "track.new: {\"tracks\":[2]}\n");
    // An existing file that is no input is replaced without --in-place.
    for rate in ["32000", "22050"] {
        let got = invoke(&dir, &["convert", "in.wav", "o.wav", "--rate", rate]);
        assert_eq!(got.code, 0, "{}", got.stderr);
    }
    assert_eq!(rate_of(&dir, "o.wav"), 22_050);
    // other.scraft gets the script's track, then is replaced by s.scraft as it is.
    let next_track = |session: &str| invoke(&dir, &["run", "--in", session, "--cmd", "track.new"]).stdout;
    let got = invoke(&dir, &["script", "s.txt", "--in", "s.scraft", "--save", "other.scraft"]);
    assert_eq!(got.code, 0, "{}", got.stderr);
    assert_eq!(next_track("other.scraft"), "track.new: {\"tracks\":[3]}\n");
    let got = invoke(&dir, &["run", "--in", "s.scraft", "--save", "other.scraft"]);
    assert_eq!(got.code, 0, "{}", got.stderr);
    assert_eq!(next_track("other.scraft"), "track.new: {\"tracks\":[2]}\n");
}

/// A write that fails leaves the input as it was: the new bytes go to a new file first.
#[cfg(unix)]
#[test]
fn failed_in_place_convert_keeps_the_input() {
    use std::os::unix::fs::PermissionsExt;
    let dir = sandbox("in-place-fails");
    let before = std::fs::read(dir.join("in.wav")).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let can_create = std::fs::File::create(dir.join("probe")).is_ok();
    if !can_create {
        let got = invoke(&dir, &["convert", "in.wav", "in.wav", "--rate", "44100", "--in-place"]);
        assert_eq!(got.code, 1, "{}", got.stderr);
        assert_eq!(std::fs::read(dir.join("in.wav")).unwrap(), before);
    }
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(std::fs::read_dir(&dir).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".part")));
}

/// An in-place output that is a link to the input replaces the input, and the link stays a link.
#[cfg(unix)]
#[test]
fn in_place_through_a_link_replaces_the_linked_input() {
    let dir = sandbox_with_session("in-place-link");
    std::os::unix::fs::symlink("in.wav", dir.join("link.wav")).unwrap();
    std::os::unix::fs::symlink("s.scraft", dir.join("link.scraft")).unwrap();
    let got = invoke(&dir, &["convert", "in.wav", "link.wav", "--rate", "44100", "--in-place"]);
    assert_eq!(got.stdout, "wrote link.wav (1 ch, 44100 Hz, 0.00 s)\n", "{}", got.stderr);
    assert_eq!(rate_of(&dir, "in.wav"), 44_100);
    let got = invoke(&dir, &["run", "--in", "s.scraft", "--cmd", "track.new", "--save", "link.scraft", "--in-place"]);
    assert_eq!(got.code, 0, "{}", got.stderr);
    // s.scraft holds that track, so the next one is the second.
    assert_eq!(invoke(&dir, &["run", "--in", "s.scraft", "--cmd", "track.new"]).stdout, "track.new: {\"tracks\":[2]}\n");
    for link in ["link.wav", "link.scraft"] {
        assert!(dir.join(link).symlink_metadata().unwrap().file_type().is_symlink(), "{link}");
    }
}

// ---- app ----

#[test]
fn app_port_comes_from_the_flag_then_the_environment() {
    let dir = sandbox("app-env");
    assert_refused(&invoke_env(&dir, &["app", "session.inspect"], Some("1")), 1);
    assert_refused(&invoke_env(&dir, &["app", "--port", "2", "session.inspect"], Some("1")), 2);
    // The flag wins before the variable is read, so a bad one doesn't matter.
    assert_refused(&invoke_env(&dir, &["app", "--port", "2", "session.inspect"], Some("abc")), 2);
}

#[test]
fn app_takes_an_empty_port_variable_as_unset() {
    if std::net::TcpStream::connect("127.0.0.1:7979").is_ok() {
        eprintln!("skipped: something listens on 127.0.0.1:7979, the default port");
        return;
    }
    let dir = sandbox("app-env-empty");
    for empty in ["", "  "] {
        assert_refused(&invoke_env(&dir, &["app", "session.inspect"], Some(empty)), 7979);
    }
}

#[test]
fn a_bad_port_variable_is_a_usage_error_naming_it() {
    let dir = sandbox("app-env-bad");
    for bad in ["abc", "70000", ":80"] {
        assert_usage_error(
            &invoke_env(&dir, &["app", "session.inspect"], Some(bad)),
            &format!("invalid value '{bad}' for 'SOUNDCRAFT_CONTROL_PORT': expected a port, 0-65535, or host:port"),
        );
    }
}

#[test]
fn app_help_names_the_port_variable() {
    let got = invoke(&sandbox("app-help"), &["app", "--help"]);
    assert_eq!(got.code, 0, "{}", got.stderr);
    assert!(got.stdout.contains("[env: SOUNDCRAFT_CONTROL_PORT=]\n          [default: 7979]\n"), "{}", got.stdout);
}

// ---- mcp ----

#[test]
fn mcp_accepts_its_flags_in_both_spellings_and_exits_cleanly_on_empty_stdin() {
    let dir = sandbox("mcp-ok");
    for args in [vec!["mcp"], vec!["mcp", "--demo"], vec!["mcp", "--connect", "1"], vec!["mcp", "--connect=1"]] {
        assert_silent_success(&invoke(&dir, &args));
    }
    // A root prints the confinement line to stderr (never stdout).
    for args in [&["mcp", "--automation-read-root", "."][..], &["mcp", "--automation-read-root=."][..]] {
        let got = invoke(&dir, args);
        assert_eq!((got.code, got.stdout.as_str()), (0, ""));
        assert!(got.stderr.starts_with("soundcraft-cli: files confined: read ") && got.stderr.ends_with("; write none\n"), "{}", got.stderr);
    }
    // A repeated write-root flag is accepted in either spelling.
    let twice = invoke(&dir, &["mcp", "--automation-write-root=.", "--automation-write-root", "."]);
    assert_eq!(twice.code, 0);
    assert!(
        twice.stderr.starts_with("soundcraft-cli: files confined: read none; write ") && !twice.stderr.contains("; write none"),
        "{}",
        twice.stderr
    );
}

#[test]
fn mcp_demo_with_connect_is_a_usage_error() {
    let dir = sandbox("mcp-demo-connect");
    assert_usage_error(&invoke(&dir, &["mcp", "--demo", "--connect", "1"]), "the argument '--demo' cannot be used with '--connect <PORT>'");
}

#[test]
fn mcp_roots_with_connect_are_a_usage_error() {
    let dir = sandbox("mcp-roots-connect");
    // Refused before any folder is opened, so a missing one isn't reported.
    assert_usage_error(
        &invoke(&dir, &["mcp", "--connect", "1", "--automation-write-root", "missing"]),
        "the argument '--connect <PORT>' cannot be used with '--automation-write-root <DIR>'",
    );
    assert_usage_error(
        &invoke(&dir, &["mcp", "--automation-read-root=.", "--connect=1"]),
        "the argument '--automation-read-root <DIR>' cannot be used with '--connect <PORT>'",
    );
}

/// A name that isn't valid Unicode works wherever the tool itself reads or writes the file
/// (`convert`); a session or bounce the engine opens or writes by text name is refused (exit 1),
/// naming the file. Run in-process: the names need no shell.
#[cfg(unix)]
#[test]
fn non_unicode_file_names() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let dir = sandbox("nonunicode");
    let latin1 = |ext: &str| {
        let mut n = b"caf\xe9.".to_vec();
        n.extend_from_slice(ext.as_bytes());
        dir.join(OsString::from_vec(n))
    };
    let run = |args: Vec<OsString>| {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = soundcraft_cli::run(args, &mut out, &mut err);
        (code, String::from_utf8_lossy(&err).into_owned())
    };
    let (code, err) = run(vec!["convert".into(), dir.join("in.wav").into(), latin1("flac").into()]);
    assert_eq!(code, 0, "{err}");
    assert!(std::fs::metadata(latin1("flac")).unwrap().len() > 0);
    // A session opens by path; only paths that go into the engine's JSON must be Unicode.
    let (code, err) = run(vec!["run".into(), "--demo".into(), "--save".into(), dir.join("s.scraft").into()]);
    assert_eq!(code, 0, "{err}");
    std::fs::rename(dir.join("s.scraft"), latin1("scraft")).unwrap();
    let (code, err) = run(vec!["info".into(), latin1("scraft").into()]);
    assert_eq!(code, 0, "{err}");
    let (code, err) = run(vec!["run".into(), "--demo".into(), "--bounce".into(), latin1("wav").into()]);
    assert_eq!(code, 1);
    assert!(err.contains("caf\u{FFFD}.") && err.contains("not valid Unicode"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}
