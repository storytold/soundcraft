//! The control and automation launch options, as clap types, so each flag and its environment
//! variable are declared once for `soundcraft-cli` and the desktop app. A flag wins over its
//! variable.
//!
//! Parse with [`blank_env_is_unset`] applied to the arguments, so an empty variable counts as unset,
//! and pass a parse error through [`name_env_vars`], so a bad value names the variable it came from.
//! [`conflict`] reports two arguments that can't go together, each named as it was given, for a
//! conflict that depends on the mode, which clap's `conflicts_with` can't express.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock;

use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::parser::ValueSource;
use clap::{Arg, ArgAction, ArgMatches, Args, Command};

use crate::ControlTarget;

/// The running app a client calls (`soundcraft-cli app`).
#[derive(Args)]
pub struct ControlClientArgs {
    /// The app's control port or host:port
    #[arg(long, value_name = "P", env = "SOUNDCRAFT_CONTROL_PORT", default_value = "7979", value_parser = control_target)]
    pub port: ControlTarget,
}

/// The control server the desktop app starts. The same variable as [`ControlClientArgs`], so one
/// export connects the two.
#[derive(Args)]
pub struct ControlServerArgs {
    /// Start the localhost JSON-lines control server on this port (0 picks a free port and prints
    /// it)
    // allow_negative_numbers makes `--control -1` a bad port rather than an unexpected argument.
    #[arg(long, value_name = "PORT", env = "SOUNDCRAFT_CONTROL_PORT", value_parser = control_target, allow_negative_numbers = true)]
    pub control: Option<ControlTarget>,
}

/// The folders automation may touch. Once any root is given, files are confined to the roots, and
/// a kind left out grants none of its access. No variable stands in for them: one left exported
/// must not change what an agent may read or write.
#[derive(Args)]
pub struct AutomationArgs {
    /// A folder the agent's commands may read from; repeat for several
    #[arg(long, value_name = "DIR", action = ArgAction::Append)]
    pub automation_read_root: Vec<PathBuf>,
    /// A folder the agent's commands may write to; repeat for several
    #[arg(long, value_name = "DIR", action = ArgAction::Append)]
    pub automation_write_root: Vec<PathBuf>,
}

/// A [`ControlTarget`], for clap's `value_parser`: `soundcraft-cli app --port`, `mcp --connect`,
/// `soundcraft --control`, and SOUNDCRAFT_CONTROL_PORT, which may carry spaces.
pub fn control_target(s: &str) -> Result<ControlTarget, NotATarget> {
    let (host, port) = host_and_port(s.trim())?;
    Ok(ControlTarget { host: host.map(str::to_owned), port })
}

/// `PORT` or `HOST:PORT`, split. An IPv6 host goes in brackets (`[::1]:7979`), as a socket address
/// is written: in `::1:7979` the host and port can't be told apart.
fn host_and_port(s: &str) -> Result<(Option<&str>, u16), NotATarget> {
    let (host, port) = match s.rsplit_once(':') {
        Some(("", _)) => return Err(NotATarget),
        Some((host, _)) if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) => return Err(NotATarget),
        Some((host, port)) => (Some(host), port),
        None => (None, s),
    };
    Ok((host, port.parse().map_err(|_| NotATarget)?))
}

/// A [`control_target`] value that is neither a port nor `host:port`.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("expected a port, 0-65535, or host:port")]
pub struct NotATarget;

/// `0.1.0 (linux x86_64)`: the workspace version and the platform the binary was built for. Every
/// crate takes its version from the workspace, so the value is the same in both binaries. A
/// `const` can't build it: `concat!` accepts only literals, not `std::env::consts`.
pub fn long_version() -> &'static str {
    static LONG: OnceLock<String> = OnceLock::new();
    LONG.get_or_init(|| format!("{} ({} {})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH))
}

/// For `Command::mut_args`: an empty or blank variable counts as unset, so `SOUNDCRAFT_CONTROL_PORT=`
/// turns a fallback off without `unset`. clap would pass the empty value to the parser.
pub fn blank_env_is_unset(arg: Arg) -> Arg {
    let blank = arg.get_env().and_then(std::env::var_os).is_some_and(|v| v.to_str().is_some_and(|v| v.trim().is_empty()));
    if blank { arg.env(None::<&str>) } else { arg }
}

/// clap names the flag when a value from its variable is bad or conflicts with another, as if the
/// flag had been given: name the variable instead when the flag isn't among `args` (without the
/// binary name; a first one naming a subcommand selects whose arguments to look in).
pub fn name_env_vars(mut error: clap::Error, cmd: &Command, args: &[OsString]) -> clap::Error {
    // `--flag value` or `--flag=value`.
    let given =
        |long: &str| args.iter().filter_map(|a| a.to_str()?.strip_prefix("--")?.strip_prefix(long)).any(|r| r.is_empty() || r.starts_with('='));
    let sub = args.first().and_then(|name| cmd.find_subcommand(name)).unwrap_or(cmd);
    for kind in [ContextKind::InvalidArg, ContextKind::PriorArg] {
        let Some(ContextValue::String(shown)) = error.get(kind) else { continue };
        let Some(arg) = sub.get_arguments().find(|a| a.to_string() == *shown) else { continue };
        let (Some(long), Some(var)) = (arg.get_long(), arg.get_env()) else { continue };
        if !given(long) {
            error.insert(kind, ContextValue::String(var.to_string_lossy().into_owned()));
        }
    }
    error
}

/// clap's own conflict error when `m`, parsed by `cmd`, has both `a` and `b`, each named as it was
/// given: the flag, or its variable.
pub fn conflict(cmd: &mut Command, m: &ArgMatches, a: &str, b: &str) -> Result<(), clap::Error> {
    if m.value_source(a).is_none() || m.value_source(b).is_none() {
        return Ok(());
    }
    let shown = |id: &str| {
        let arg = cmd.get_arguments().find(|arg| arg.get_id() == id);
        match (m.value_source(id), arg.and_then(Arg::get_env)) {
            (Some(ValueSource::EnvVariable), Some(var)) => var.to_string_lossy().into_owned(),
            _ => arg.map_or_else(|| id.to_owned(), |arg| arg.to_string()),
        }
    };
    let (shown_a, shown_b) = (shown(a), shown(b));
    let mut error = clap::Error::new(ErrorKind::ArgumentConflict).with_cmd(cmd);
    error.insert(ContextKind::InvalidArg, ContextValue::String(shown_a));
    error.insert(ContextKind::PriorArg, ContextValue::String(shown_b));
    error.insert(ContextKind::Usage, ContextValue::StyledStr(cmd.render_usage()));
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_target_is_a_port_or_host_and_port() {
        let target = |host: Option<&str>, port| Ok(ControlTarget { host: host.map(str::to_owned), port });
        assert_eq!(control_target("7979"), target(None, 7979));
        assert_eq!(control_target(" 0 "), target(None, 0));
        assert_eq!(control_target("localhost:7979 "), target(Some("localhost"), 7979));
        assert_eq!(control_target("10.0.0.5:1"), target(Some("10.0.0.5"), 1));
        assert_eq!(control_target("[::1]:7979"), target(Some("[::1]"), 7979));
        for bad in ["", "x", "78787", ":7979", "host:", "host:x", "-1", "::1:7979", "[::1:7979"] {
            assert_eq!(control_target(bad), Err(NotATarget), "{bad:?}");
        }
    }

    /// The control server listens on 127.0.0.1 only: `[::1]` is another address.
    #[test]
    fn a_loopback_target_is_a_port_127_0_0_1_or_localhost() {
        for loopback in ["50494", "127.0.0.1:7979", "localhost:7979"] {
            assert!(control_target(loopback).is_ok_and(|t| t.is_loopback()), "{loopback:?}");
        }
        for remote in ["10.0.0.5:7979", "example.com:7979", "[::1]:7979"] {
            assert!(control_target(remote).is_ok_and(|t| !t.is_loopback()), "{remote:?}");
        }
    }

    #[test]
    fn a_control_target_displays_as_a_socket_address() {
        for (given, shown) in [("7979", "127.0.0.1:7979"), ("localhost:1", "localhost:1"), ("[::1]:7979", "[::1]:7979")] {
            assert_eq!(control_target(given).map(|t| t.to_string()), Ok(shown.to_owned()), "{given:?}");
        }
    }

    /// A command with one port flag backed by a variable no test sets.
    fn port_command() -> Command {
        Command::new("t").arg(Arg::new("port").long("port").env("SOUNDCRAFT_TEST_UNSET_PORT").value_parser(control_target))
    }

    #[test]
    fn name_env_vars_names_the_variable_only_when_the_flag_was_not_given() {
        let mut cmd = port_command();
        let e = cmd.try_get_matches_from_mut(["t", "--port", "x"]).unwrap_err();
        let flag = name_env_vars(e, &cmd, &["--port".into(), "x".into()]);
        assert_eq!(flag.get(ContextKind::InvalidArg), Some(&ContextValue::String("--port <port>".into())));
        let e = cmd.try_get_matches_from_mut(["t", "--port=x"]).unwrap_err();
        // As the error from the variable would read: the flag is not among the arguments.
        let var = name_env_vars(e, &cmd, &[]);
        assert_eq!(var.get(ContextKind::InvalidArg), Some(&ContextValue::String("SOUNDCRAFT_TEST_UNSET_PORT".into())));
    }

    #[test]
    fn blank_env_is_unset_keeps_an_unset_variable() {
        let arg = blank_env_is_unset(Arg::new("port").env("SOUNDCRAFT_TEST_UNSET_PORT"));
        assert_eq!(arg.get_env(), Some(std::ffi::OsStr::new("SOUNDCRAFT_TEST_UNSET_PORT")));
    }

    #[test]
    fn conflict_is_a_clap_usage_error_naming_both_flags() {
        let mut cmd = port_command().arg(Arg::new("connect").long("connect"));
        let m = cmd.try_get_matches_from_mut(["t", "--port", "1", "--connect", "2"]).unwrap();
        let e = conflict(&mut cmd, &m, "port", "connect").unwrap_err();
        assert_eq!((e.kind(), e.exit_code()), (ErrorKind::ArgumentConflict, 2));
        assert!(e.to_string().starts_with("error: the argument '--port <port>' cannot be used with '--connect <connect>'\n\nUsage: t"), "{e}");
        let m = cmd.try_get_matches_from_mut(["t", "--port", "1"]).unwrap();
        assert!(conflict(&mut cmd, &m, "port", "connect").is_ok());
    }
}
