//! The desktop app's command line, as clap types: `soundcraft --help` comes from these doc
//! comments, so the help and the parser can't disagree. Anything they don't declare is a usage
//! error naming it (exit status 2), as is a bad `--control` or `SOUNDCRAFT_CONTROL_PORT`. A file
//! whose name starts with `-` goes after `--`. Another host's `host:port` is a usage error from
//! `--control` and ignored from the variable ([`control_port`]).
//!
//! The control flag is `soundcraft-automation`'s, shared with `soundcraft-cli`.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::builder::StyledStr;
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::parser::ValueSource;
use clap::{Command, CommandFactory, FromArgMatches, Parser};
use soundcraft_automation::ControlTarget;
use soundcraft_automation::args::{ControlServerArgs, blank_env_is_unset, name_env_vars};

/// Opens SoundCraft with the given files
#[derive(Parser)]
#[command(name = "soundcraft", version = soundcraft_automation::args::long_version(), after_help = crate::env::help())]
pub(crate) struct LaunchArgs {
    /// Sessions, audio and MIDI files to open. Put `--` before a name that starts with `-`
    pub(crate) files: Vec<PathBuf>,
    /// Start with the demo session
    #[arg(long, visible_alias = "sample")]
    pub(crate) demo: bool,
    /// Don't open an audio device
    #[arg(long)]
    pub(crate) no_audio: bool,
    #[command(flatten)]
    control: ControlServerArgs,
    /// The port the control server starts on, from `control` ([`control_port`]).
    #[arg(skip)]
    pub(crate) control_port: Option<u16>,
}

/// The command line, where an empty or blank variable counts as unset.
pub(crate) fn command() -> Command {
    LaunchArgs::command().mut_args(blank_env_is_unset)
}

/// Parses the launch arguments (without the program name) with `cmd`, from [`command`]. macOS
/// passes a process serial number (`-psn_…`) when an app is started from Finder on old versions:
/// it is dropped first, unless it comes after `--`. A bad value from a variable is a usage error
/// naming the variable.
pub(crate) fn parse(mut cmd: Command, args: impl IntoIterator<Item = OsString>) -> Result<LaunchArgs, clap::Error> {
    let mut options = true;
    let args: Vec<OsString> = args
        .into_iter()
        .filter(|a| {
            options &= a != "--";
            !(options && a.to_str().is_some_and(|s| s.starts_with("-psn_")))
        })
        .collect();
    let argv = std::iter::once(OsString::from("soundcraft")).chain(args.iter().cloned());
    let matches = cmd.try_get_matches_from_mut(argv).map_err(|e| name_env_vars(e, &cmd, &args))?;
    let mut launch = LaunchArgs::from_arg_matches(&matches)?;
    launch.control_port = control_port(launch.control.control.take(), matches.value_source("control")).map_err(|e| e.into_clap(&mut cmd))?;
    Ok(launch)
}

/// The port the control server starts on, from `--control` or `SOUNDCRAFT_CONTROL_PORT`
/// (`source`). The server listens on 127.0.0.1 only, so it can't serve another host's
/// `host:port`. From the flag that is a usage error. The variable is `soundcraft-cli app`'s too and
/// may point it at another machine, so from there it is ignored with a warning and the app opens
/// without a control server.
fn control_port(value: Option<ControlTarget>, source: Option<ValueSource>) -> Result<Option<u16>, RemoteHost> {
    match value {
        None => Ok(None),
        Some(target) if target.is_loopback() => Ok(Some(target.port)),
        Some(target) if source == Some(ValueSource::EnvVariable) => {
            log::warn!("SOUNDCRAFT_CONTROL_PORT={target} names another host, so no control server starts: it listens on 127.0.0.1 only");
            Ok(None)
        }
        Some(target) => Err(RemoteHost(target)),
    }
}

/// A `--control` `host:port` naming another host.
#[derive(Debug, PartialEq)]
struct RemoteHost(ControlTarget);

impl RemoteHost {
    /// clap's invalid-value error for `--control`, with the reason as a tip.
    fn into_clap(self, cmd: &mut Command) -> clap::Error {
        let flag = cmd.get_arguments().find(|a| a.get_id() == "control").map_or_else(|| "--control".to_owned(), ToString::to_string);
        let tip =
            StyledStr::from("the control server listens on 127.0.0.1 only: give a port, or host:port with a loopback host (127.0.0.1, localhost)");
        let mut error = clap::Error::new(ErrorKind::ValueValidation).with_cmd(cmd);
        error.insert(ContextKind::InvalidArg, ContextValue::String(flag));
        error.insert(ContextKind::InvalidValue, ContextValue::String(self.0.to_string()));
        error.insert(ContextKind::Suggested, ContextValue::StyledStrs(vec![tip]));
        error.insert(ContextKind::Usage, ContextValue::StyledStr(cmd.render_usage()));
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    /// Parse without the variable, so one exported where the tests run can't change them (the
    /// process tests in `tests/launch_flags.rs` cover it).
    fn parse_args(args: &[&str]) -> Result<LaunchArgs, clap::Error> {
        parse(command().mut_args(|a| a.env(None::<&str>)), args.iter().map(OsString::from))
    }

    fn usage_error(args: &[&str]) -> String {
        let Err(e) = parse_args(args) else { panic!("{args:?} parsed") };
        assert_eq!(e.exit_code(), 2, "{args:?}: {e}");
        e.to_string()
    }

    #[test]
    fn command_is_consistent() {
        command().debug_assert();
    }

    #[test]
    fn flags_and_files_are_parsed() {
        let a = parse_args(&["--demo", "--no-audio", "--control", "50494", "a.scraft", "b.wav"]).unwrap();
        assert!(a.demo && a.no_audio);
        assert_eq!(a.control_port, Some(50494));
        assert_eq!(a.files, [PathBuf::from("a.scraft"), PathBuf::from("b.wav")]);
        assert!(parse_args(&["--sample"]).unwrap().demo);
        let none = parse_args(&[]).unwrap();
        assert!(!none.demo && !none.no_audio && none.control_port.is_none() && none.files.is_empty());
    }

    #[test]
    fn version_and_help_stop_parsing() {
        for flag in ["--version", "-V"] {
            let e = parse_args(&[flag, "--control"]).err().unwrap();
            assert_eq!((e.kind(), e.exit_code()), (ErrorKind::DisplayVersion, 0));
            assert_eq!(e.to_string(), format!("soundcraft {}\n", soundcraft_automation::args::long_version()));
        }
        let e = parse_args(&["-h", "--bogus"]).err().unwrap();
        assert_eq!((e.kind(), e.exit_code()), (ErrorKind::DisplayHelp, 0));
        let help = e.to_string();
        for text in ["--demo", "--sample", "--no-audio", "--control <PORT>"] {
            assert!(help.contains(text), "{text}: {help}");
        }
        let help = command().render_help().to_string();
        assert!(help.contains("[env: SOUNDCRAFT_CONTROL_PORT"), "{help}");
        for var in ["SOUNDCRAFT_CONFIG_DIR", "SOUNDCRAFT_NO_PREFS", "SOUNDCRAFT_NO_SYSTEM_FONTS", "SOUNDCRAFT_WAYLAND", "RUST_LOG"] {
            assert!(help.contains(&format!("\n  {var} ")), "{var}: {help}");
        }
    }

    #[test]
    fn a_bad_control_port_is_a_usage_error_naming_it() {
        assert!(usage_error(&["--control", "78787"]).contains("invalid value '78787' for '--control <PORT>'"));
        assert!(usage_error(&["--control", "nope"]).contains("invalid value 'nope'"));
        assert!(usage_error(&["--control"]).contains("a value is required for '--control <PORT>'"));
        assert!(usage_error(&["--control", "--demo"]).contains("a value is required for '--control <PORT>'"));
        assert!(usage_error(&["--control", "-1"]).contains("invalid value '-1' for '--control <PORT>'"));
        let remote = usage_error(&["--control", "10.0.0.5:7979"]);
        assert!(
            remote.starts_with("error: invalid value '10.0.0.5:7979' for '--control <PORT>'\n\n  tip: the control server listens on 127.0.0.1 only"),
            "{remote}"
        );
        assert!(remote.contains("\nUsage: soundcraft "), "{remote}");
        assert!(usage_error(&["--control", "::1:7979"]).contains("invalid value '::1:7979' for '--control <PORT>'"));
    }

    /// The variable may point `soundcraft-cli app` at another machine: the app then opens without
    /// a control server.
    #[test]
    fn another_host_is_ignored_only_from_the_variable() {
        let remote = || ControlTarget { host: Some("10.0.0.5".to_owned()), port: 7979 };
        assert_eq!(control_port(Some(remote()), Some(ValueSource::EnvVariable)), Ok(None));
        assert_eq!(control_port(Some(remote()), Some(ValueSource::CommandLine)), Err(RemoteHost(remote())));
        let local = ControlTarget { host: None, port: 7979 };
        assert_eq!(control_port(Some(local), Some(ValueSource::EnvVariable)), Ok(Some(7979)));
        assert_eq!(control_port(None, None), Ok(None));
    }

    /// The variable `soundcraft-cli app` reads may hold `host:port`: a loopback one gives its port.
    #[test]
    fn a_loopback_host_and_port_gives_its_port() {
        for value in ["127.0.0.1:50494", "localhost:50494"] {
            assert_eq!(parse_args(&["--control", value]).unwrap().control_port, Some(50494), "{value}");
        }
    }

    /// The control server binds 127.0.0.1, so `[::1]` is another host.
    #[test]
    fn an_ipv6_loopback_host_is_not_loopback() {
        assert!(usage_error(&["--control", "[::1]:50494"]).contains("invalid value '[::1]:50494' for '--control <PORT>'"));
    }

    #[test]
    fn unknown_flags_and_repeats_are_usage_errors() {
        assert!(usage_error(&["--bogus"]).contains("unexpected argument '--bogus'"));
        assert!(usage_error(&["-x"]).contains("unexpected argument '-x'"));
        assert!(usage_error(&["--demo", "--demo"]).contains("cannot be used multiple times"));
    }

    #[test]
    fn double_dash_ends_options() {
        let a = parse_args(&["a.wav", "--", "--demo", "-x"]).unwrap();
        assert!(!a.demo);
        assert_eq!(a.files, [PathBuf::from("a.wav"), PathBuf::from("--demo"), PathBuf::from("-x")]);
    }

    #[test]
    fn a_finder_process_serial_number_is_dropped() {
        assert_eq!(parse_args(&["-psn_0_12345", "a.wav"]).unwrap().files, [PathBuf::from("a.wav")]);
        assert_eq!(parse_args(&["--", "-psn_x.wav"]).unwrap().files, [PathBuf::from("-psn_x.wav")]);
    }

    /// A file name that isn't valid Unicode reaches `files` as it is.
    #[cfg(unix)]
    #[test]
    fn non_unicode_file_names_are_kept() {
        use std::os::unix::ffi::OsStringExt as _;
        let latin1 = OsString::from_vec(b"caf\xe9.wav".to_vec());
        let a = parse(command().mut_args(|a| a.env(None::<&str>)), [latin1.clone()]).unwrap();
        assert_eq!(a.files, [PathBuf::from(latin1)]);
    }
}
