//! The CLI run in-process through `soundcraft_cli::run`: the tests that need no files, working
//! directory, environment or standard input. Those that do run the real binary (`behaviour.rs`).
//! A change to the command line's behaviour updates them on purpose.

use soundcraft_cli::run;

/// What [`run`] returned and wrote.
struct Invoke {
    code: i32,
    stdout: String,
    stderr: String,
}

fn invoke(args: &[&str]) -> Invoke {
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let code = run(args.iter().copied(), &mut stdout, &mut stderr);
    Invoke { code, stdout: String::from_utf8(stdout).unwrap(), stderr: String::from_utf8(stderr).unwrap() }
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

/// Exit 2, nothing on stdout, and a clap usage error on stderr: `error: ` with `message`, ending
/// with the `--help` hint.
#[track_caller]
fn assert_usage_error(got: &Invoke, message: &str) {
    assert_eq!(got.code, 2, "stderr: {}", got.stderr);
    assert_eq!(got.stdout, "");
    assert!(got.stderr.starts_with(&format!("error: {message}")), "{}", got.stderr);
    assert!(got.stderr.ends_with("\n\nFor more information, try '--help'.\n"), "{}", got.stderr);
}

/// Exit 0, help on stdout, nothing on stderr.
#[track_caller]
fn assert_help(got: &Invoke) -> &str {
    assert_eq!((got.code, got.stderr.as_str()), (0, ""), "{}", got.stdout);
    &got.stdout
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

#[test]
fn top_level_help_lists_every_subcommand_on_stdout() {
    let long = invoke(&["--help"]);
    let help = assert_help(&long);
    assert!(help.starts_with("soundcraft-cli: SoundCraft without a window\n\nUsage: soundcraft-cli <COMMAND>\n"), "{help}");
    for sub in ["info", "convert", "run", "script", "commands", "describe", "app", "mcp", "parity", "plugins", "help"] {
        assert!(help.contains(&format!("\n  {sub} ")), "{sub}: {help}");
    }
    assert!(help.contains("-V, --version") && !help.contains("\n  version "), "{help}");
    for spelling in ["-h", "help"] {
        let got = invoke(&[spelling]);
        assert_eq!((got.code, got.stdout, got.stderr), (0, long.stdout.clone(), String::new()), "{spelling}");
    }
}

#[test]
fn version_flags_print_the_long_version_to_stdout() {
    let expected = format!("soundcraft-cli {} ({} {})\n", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
    for spelling in ["--version", "-V"] {
        let got = invoke(&[spelling]);
        assert_eq!((got.code, got.stdout, got.stderr), (0, expected.clone(), String::new()), "{spelling}");
    }
}

#[test]
fn version_is_not_a_subcommand() {
    assert_usage_error(&invoke(&["version"]), "unrecognized subcommand 'version'");
}

#[test]
fn help_subcommand_shows_that_subcommands_help() {
    let got = invoke(&["help", "convert"]);
    let help = assert_help(&got);
    assert!(help.starts_with("Convert an audio file"), "{help}");
    assert_eq!(help, assert_help(&invoke(&["convert", "--help"])));
}

#[test]
fn help_names_every_format_the_command_reads_and_writes() {
    use soundcraft_audio_io::FileFormat;
    let read = format!("({})", FileFormat::readable_extensions().join(", "));
    let write = format!("its extension picks the format ({})", FileFormat::writable_extensions().join(", "));
    let convert = invoke(&["convert", "--help"]);
    let convert = assert_help(&convert);
    assert!(convert.contains(&read) && convert.contains(&write), "{convert}");
    let info = invoke(&["info", "--help"]);
    assert!(assert_help(&info).contains(&read), "{}", info.stdout);
}

#[test]
fn no_arguments_prints_the_help_to_stderr_and_exits_2() {
    let got = invoke(&[]);
    assert_eq!((got.code, got.stdout.as_str()), (2, ""));
    assert_eq!(got.stderr, invoke(&["--help"]).stdout);
}

#[test]
fn unknown_subcommand_is_a_usage_error() {
    assert_usage_error(&invoke(&["bogus"]), "unrecognized subcommand 'bogus'");
    assert_usage_error(&invoke(&["bogus", "--help"]), "unrecognized subcommand 'bogus'");
    // Subcommand names are case sensitive and not abbreviated.
    assert_usage_error(&invoke(&["INFO"]), "unrecognized subcommand 'INFO'");
    assert!(invoke(&["plug"]).stderr.contains("tip: a similar subcommand exists: 'plugins'"));
    assert_usage_error(&invoke(&["--bogus"]), "unexpected argument '--bogus' found");
}

/// Each subcommand's help: its usage line, then every argument and flag it takes.
#[test]
fn every_subcommand_has_help_listing_its_arguments() {
    let engine = [
        "--in <S.scraft>",
        "--demo",
        "--sample",
        "--bounce <OUT>",
        "--start <T>",
        "--end <T>",
        "--save <S.scraft>",
        "--inspect",
        "--compact",
        "--in-place",
    ];
    let cases: &[(&str, &str, &[&str])] = &[
        ("info", "info [OPTIONS] <FILE>", &["<FILE>", "--compact"]),
        (
            "convert",
            "convert [OPTIONS] <IN> <OUT>",
            &["<IN>", "<OUT>", "--bit-depth <16|24|32f>", "--rate <HZ>", "--normalize", "--dither", "--no-dither", "--in-place"],
        ),
        ("run", "run [OPTIONS]", &["--cmd <ID[=JSON]>"]),
        ("script", "script [OPTIONS] <FILE>", &["<FILE>", "--keep-going"]),
        ("commands", "commands [FILTER]", &["[FILTER]"]),
        ("describe", "describe [OPTIONS] <ID>", &["<ID>", "--compact"]),
        (
            "app",
            "app [OPTIONS] <METHOD> [JSON]",
            &["--port <P>", "--compact", "<METHOD>", "[JSON]", "[env: SOUNDCRAFT_CONTROL_PORT", "[default: 7979]"],
        ),
        (
            "mcp",
            "mcp [OPTIONS]",
            &["--connect <PORT>", "--demo", "--automation-read-root <DIR>", "--automation-write-root <DIR>", "conflict with --connect"],
        ),
        ("parity", "parity [OPTIONS]", &["--write <PATH>"]),
        ("plugins", "plugins", &[]),
    ];
    for &(sub, usage, args) in cases {
        let long = invoke(&[sub, "--help"]);
        let help = assert_help(&long);
        assert!(help.contains(&format!("\nUsage: soundcraft-cli {usage}\n")), "{sub}: {help}");
        let shared: &[&str] = if matches!(sub, "run" | "script") { &engine } else { &[] };
        for arg in args.iter().chain(shared).chain(&["-h, --help"]) {
            assert!(help.contains(arg), "{sub} lacks {arg}: {help}");
        }
        // `help SUB` is `SUB --help`; `-h` is the one-line-per-flag summary.
        assert_eq!(invoke(&["help", sub]).stdout, long.stdout, "help {sub}");
        assert!(assert_help(&invoke(&[sub, "-h"])).contains(&format!("\nUsage: soundcraft-cli {usage}\n")), "{sub} -h");
    }
}

#[test]
fn help_wins_over_a_mistake_before_it() {
    let help = invoke(&["run", "--help"]).stdout;
    assert_eq!(assert_help(&invoke(&["run", "--bounce", "--help"])), help);
    assert_eq!(assert_help(&invoke(&["run", "--bogus", "--help"])), help);
    assert!(assert_help(&invoke(&["convert", "--rate", "-h"])).contains("Usage: soundcraft-cli convert"));
    assert!(assert_help(&invoke(&["mcp", "--connect", "--help"])).contains("Usage: soundcraft-cli mcp"));
    // After `--`, `--help` is a value.
    assert_eq!(invoke(&["info", "--", "--help"]).code, 1);
}

#[test]
fn run_and_script_value_flags_without_a_value_are_usage_errors() {
    for (flag, name) in [("--cmd", "ID[=JSON]"), ("--in", "S.scraft"), ("--save", "S.scraft"), ("--bounce", "OUT"), ("--start", "T"), ("--end", "T")]
    {
        let message = format!("a value is required for '{flag} <{name}>' but none was supplied");
        assert_usage_error(&invoke(&["run", flag]), &message);
        if flag != "--cmd" {
            assert_usage_error(&invoke(&["script", "s.txt", flag]), &message);
        }
    }
    // A following flag is not the value.
    assert_usage_error(&invoke(&["run", "--bounce", "--inspect"]), "a value is required for '--bounce <OUT>'");
    assert_usage_error(&invoke(&["script", "s.txt", "--demo", "--bounce"]), "a value is required for '--bounce <OUT>'");
}

#[test]
fn boolean_flags_reject_an_inline_value() {
    assert_usage_error(&invoke(&["run", "--demo=1"]), "unexpected value '1' for '--demo' found; no more were expected");
    assert_usage_error(&invoke(&["run", "--inspect=1"]), "unexpected value '1' for '--inspect' found; no more were expected");
    assert_usage_error(&invoke(&["script", "s.txt", "--keep-going=1"]), "unexpected value '1' for '--keep-going' found; no more were expected");
    assert_usage_error(&invoke(&["mcp", "--demo=1"]), "unexpected value '1' for '--demo' found; no more were expected");
}

#[test]
fn missing_and_extra_positionals_are_usage_errors() {
    assert_usage_error(&invoke(&["info"]), "the following required arguments were not provided:\n  <FILE>");
    assert_usage_error(&invoke(&["convert", "in.wav"]), "the following required arguments were not provided:\n  <OUT>");
    assert_usage_error(&invoke(&["script"]), "the following required arguments were not provided:\n  <FILE>");
    assert_usage_error(&invoke(&["describe"]), "the following required arguments were not provided:\n  <ID>");
    assert_usage_error(&invoke(&["app", "--port", "1"]), "the following required arguments were not provided:\n  <METHOD>");
    for args in [
        &["info", "a", "b"][..],
        &["convert", "in.wav", "out.wav", "b"],
        &["script", "s.txt", "b"],
        &["run", "b"],
        &["commands", "bounce", "b"],
        &["describe", "file.bounce_mix", "b"],
        &["app", "--port", "1", "session.inspect", "{}", "b"],
        &["mcp", "b"],
        &["parity", "b"],
        &["plugins", "b"],
    ] {
        assert_usage_error(&invoke(args), "unexpected argument 'b' found");
    }
}

#[test]
fn unknown_flags_are_usage_errors_on_every_subcommand() {
    for args in [
        &["info", "in.wav", "--bogus"][..],
        &["convert", "in.wav", "out.wav", "--bogus"],
        &["run", "--bogus"],
        &["script", "s.txt", "--bogus"],
        &["commands", "--bogus"],
        &["describe", "--bogus"],
        &["app", "--bogus", "session.inspect"],
        &["mcp", "--bogus"],
        &["parity", "--bogus"],
        &["plugins", "--bogus"],
    ] {
        assert_usage_error(&invoke(args), "unexpected argument '--bogus' found");
    }
    // A single-dash word is a short flag, and there are none but -h.
    assert_usage_error(&invoke(&["run", "-x"]), "unexpected argument '-x' found");
    assert_usage_error(&invoke(&["mcp", "--automation-bogus=x"]), "unexpected argument '--automation-bogus' found");
}

/// `args_os`: text that isn't valid Unicode reaches clap instead of aborting the process.
#[cfg(unix)]
#[test]
fn a_non_unicode_argument_is_a_usage_error_not_a_panic() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let code = run([OsString::from("describe"), OsString::from_vec(b"\xe9.wav".to_vec())], &mut stdout, &mut stderr);
    assert_eq!((code, stdout.as_slice()), (2, &b""[..]));
    assert!(String::from_utf8_lossy(&stderr).starts_with("error: invalid UTF-8 was detected in one or more arguments"));
}

/// A bounce or save path goes to the engine as JSON text: a non-Unicode one fails before any command runs.
#[cfg(unix)]
#[test]
fn a_non_unicode_output_fails_before_the_commands_run() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    for flag in ["--bounce", "--save"] {
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let args = ["run", "--cmd", "track.new", flag].map(OsString::from);
        let code = run(args.into_iter().chain([OsString::from_vec(b"\xe9.out".to_vec())]), &mut stdout, &mut stderr);
        assert_eq!((code, stdout.as_slice()), (1, &b""[..]), "{flag}: {}", String::from_utf8_lossy(&stderr));
    }
}

#[test]
fn run_demo_and_sample_are_the_same_flag() {
    let demo = invoke(&["run", "--demo", "--inspect"]);
    let sample = invoke(&["run", "--sample", "--inspect"]);
    assert_eq!((demo.code, sample.code), (0, 0));
    assert_eq!(demo.stdout, sample.stdout);
    assert!(demo.stdout.contains("\"name\": \"Verb\""), "{}", demo.stdout);
}

/// `--compact` prints the same JSON on one line.
#[track_caller]
fn assert_compact_matches_pretty(pretty: &[&str], compact: &[&str]) {
    let (p, c) = (invoke(pretty), invoke(compact));
    assert_eq!((p.code, c.code), (0, 0), "{} {}", p.stderr, c.stderr);
    assert!(p.stdout.trim_end().contains('\n'), "pretty form spans lines: {}", p.stdout);
    assert_eq!(c.stdout.trim_end().lines().count(), 1, "{}", c.stdout);
    assert!(c.stdout.ends_with('\n'));
    let (p, c): (serde_json::Value, serde_json::Value) = (serde_json::from_str(&p.stdout).unwrap(), serde_json::from_str(&c.stdout).unwrap());
    assert_eq!(p, c);
}

#[test]
fn compact_prints_describe_and_run_inspect_on_one_line() {
    assert_compact_matches_pretty(&["describe", "file.bounce_mix"], &["describe", "file.bounce_mix", "--compact"]);
    assert_compact_matches_pretty(&["run", "--demo", "--inspect"], &["run", "--demo", "--inspect", "--compact"]);
}

#[test]
fn compact_without_inspect_is_a_usage_error() {
    assert_usage_error(&invoke(&["run", "--compact"]), "the following required arguments were not provided:\n  --inspect");
}

#[test]
fn run_command_failures() {
    assert_fails(&invoke(&["run", "--cmd", "nosuch.cmd"]), "nosuch.cmd: unknown command `nosuch.cmd`");
    assert_fails(&invoke(&["run", "--in", "nope.scraft"]), &format!("nope.scraft: {}", not_found()));
    assert_fails(&invoke(&["run", "--in=nope.scraft"]), &format!("nope.scraft: {}", not_found()));
}

/// Relations and typed values are checked by clap: exit 2, the message names the flag, and nothing runs.
#[test]
fn flag_relations_and_typed_values_are_usage_errors() {
    for (args, message) in [
        (&["convert", "in.wav", "o.wav", "--dither", "--no-dither"][..], "the argument '--dither' cannot be used with '--no-dither'"),
        (&["convert", "in.wav", "o.wav", "--rate", "0"], "invalid value '0' for '--rate <HZ>'"),
        (&["convert", "in.wav", "o.wav", "--rate=abc"], "invalid value 'abc' for '--rate <HZ>'"),
        (&["convert", "in.wav", "o.wav", "--bit-depth", "8"], "invalid value '8' for '--bit-depth <16|24|32f>'"),
        (&["run", "--in", "s.scraft", "--demo"], "the argument '--in <S.scraft>' cannot be used with '--demo'"),
        (&["script", "s.txt", "--in", "s.scraft", "--sample"], "the argument '--in <S.scraft>' cannot be used with '--demo'"),
        (&["run", "--start", "1"], "the following required arguments were not provided:\n  --bounce <OUT>"),
        (&["script", "s.txt", "--end", "1"], "the following required arguments were not provided:\n  --bounce <OUT>"),
        (&["run", "--bounce", "b.wav", "--start", ""], "invalid value '' for '--start <T>'"),
        (&["run", "--bounce", "b.wav", "--start", "nan"], "invalid value 'nan' for '--start <T>': expected a finite number"),
        (&["run", "--bounce", "b.wav", "--end", "inf"], "invalid value 'inf' for '--end <T>': expected a finite number"),
        (&["run", "--bounce", "b.wav", "--end", "1e999"], "invalid value '1e999' for '--end <T>': expected a finite number"),
        (&["run", "--cmd", "track.new={", "--bounce", "b.wav"], "invalid value 'track.new={' for '--cmd <ID[=JSON]>': bad JSON:"),
        (&["run", "--cmd", "=1"], "invalid value '=1' for '--cmd <ID[=JSON]>'"),
        (&["describe", ""], "a value is required for '<ID>'"),
        (&["commands", ""], "a value is required for '[FILTER]'"),
        (&["app", ""], "a value is required for '<METHOD>'"),
    ] {
        let got = invoke(args);
        assert_usage_error(&got, message);
    }
}

#[test]
fn a_bad_later_cmd_runs_none_of_them() {
    assert_usage_error(&invoke(&["run", "--cmd", "track.new", "--cmd", "track.new={"]), "invalid value 'track.new={'");
}

#[test]
fn commands_filters_by_id_or_label() {
    let all = invoke(&["commands"]);
    assert_eq!((all.code, all.stderr.as_str()), (0, ""));
    assert!(all.stdout.starts_with("session.new "), "{}", all.stdout);
    let bounce = invoke(&["commands", "bounce"]);
    assert!(bounce.stdout.contains("file.bounce_mix ") && !bounce.stdout.contains("session.new"), "{}", bounce.stdout);
    // Matching is case-insensitive on the label only: an upper-case filter finds the labels, not the ids.
    assert!(invoke(&["commands", "BOUNCE"]).stdout.contains("Bounce Mix..."));
    assert_silent_success(&invoke(&["commands", "zzzzqq"]));
}

#[test]
fn describe_prints_json_for_a_known_id_and_fails_for_an_unknown_one() {
    let got = invoke(&["describe", "file.bounce_mix"]);
    assert_eq!((got.code, got.stderr.as_str()), (0, ""));
    assert!(got.stdout.starts_with("{\n  \"id\": \"file.bounce_mix\",\n  \"label\": \"Bounce Mix...\""), "{}", got.stdout);
    assert_fails(&invoke(&["describe", "nosuch"]), "unknown command `nosuch`");
}

#[test]
fn plugins_lists_one_line_per_plugin() {
    let got = invoke(&["plugins"]);
    assert_eq!((got.code, got.stderr.as_str()), (0, ""));
    assert!(got.stdout.starts_with("eq_1band             EQ 1-Band              Eq\n"), "{}", got.stdout);
}

#[test]
fn app_reports_a_refused_connection_and_checks_arguments_first() {
    for args in [
        vec!["app", "--port", "1", "session.inspect"],
        vec!["app", "--port=1", "session.inspect"],
        vec!["app", "session.inspect", "--port", "1"],
        vec!["app", "--port", "1", "session.inspect", "{}"],
    ] {
        assert_refused(&invoke(&args), 1);
    }
    assert_usage_error(
        &invoke(&["app", "--port", "1", "session.inspect", "{bad"]),
        "invalid value '{bad' for '[JSON]': bad JSON: key must be a string at line 1 column 2",
    );
    for bad in ["abc", "", "70000", ":80", "host:", "host:abc"] {
        assert_usage_error(
            &invoke(&["app", "--port", bad, "session.inspect"]),
            &format!("invalid value '{bad}' for '--port <P>': expected a port, 0-65535, or host:port"),
        );
        assert_usage_error(&invoke(&["mcp", "--connect", bad]), &format!("invalid value '{bad}' for '--connect <PORT>': expected a port"));
    }
}

#[test]
fn mcp_argument_errors_fail_before_reading_stdin() {
    assert_usage_error(&invoke(&["mcp", "--connect"]), "a value is required for '--connect <PORT>' but none was supplied");
    assert_usage_error(&invoke(&["mcp", "--connect", "--demo"]), "a value is required for '--connect <PORT>' but none was supplied");
    for flag in ["--automation-read-root", "--automation-write-root"] {
        let message = format!("a value is required for '{flag} <DIR>' but none was supplied");
        assert_usage_error(&invoke(&["mcp", flag]), &message);
        assert_usage_error(&invoke(&["mcp", flag, "--demo"]), &message);
        assert_usage_error(&invoke(&["mcp", &format!("{flag}=")]), &message);
    }
    assert_fails(
        &invoke(&["mcp", "--automation-read-root", "missing-dir"]),
        &format!("cannot open the automation read root `missing-dir`: {}", not_found()),
    );
    assert_usage_error(
        &invoke(&["mcp", "--automation-read-root", ".", "--connect", "1"]),
        "the argument '--automation-read-root <DIR>' cannot be used with '--connect <PORT>'",
    );
}
