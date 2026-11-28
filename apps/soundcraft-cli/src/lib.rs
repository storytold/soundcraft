//! `soundcraft-cli`: SoundCraft without a window. `run` is the whole program, so it can be tested
//! in-process as well as through the binary. `soundcraft-cli --help` lists the subcommands.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[macro_use]
mod output;

mod app;
mod args;
mod catalog;
mod convert;
mod error;
mod info;
pub mod logger;
mod mcp;
mod session;

use std::ffi::OsString;
use std::io::Write;

use clap::CommandFactory;

use app::app;
use args::{Cli, Command, parse};
use catalog::{commands, describe, parity};
use convert::convert;
use error::CliError;
use info::info;
use mcp::mcp;
use session::{run_cmds, script};

/// A Launcher-only protocol argument, in either the separate or the `=` form. Everything after
/// `--` is a file name, so it is not inspected.
fn launcher_protocol_flag(args: &[OsString]) -> Option<&'static str> {
    for arg in args {
        if arg == "--" {
            break;
        }
        let text = arg.to_string_lossy();
        if text == "--launch-profile" || text.starts_with("--launch-profile=") {
            return Some("--launch-profile");
        }
        if text == "--profile-root" || text.starts_with("--profile-root=") {
            return Some("--profile-root");
        }
    }
    None
}

/// The command line's clap definition.
pub fn command() -> clap::Command {
    Cli::command()
}

/// Run the CLI on its arguments (without the program name), writing results to `out` and
/// diagnostics to `err`. Returns the process exit code: 0 on success, 1 on failure, 2 for a usage
/// error. Help and the version go to `out`, exit 0. Plugin hosts are shut down by the caller once
/// the process is done with every engine (see `main`).
pub fn run<I, T>(args: I, out: &mut dyn Write, err: &mut dyn Write) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    // The Launcher's Nightly protocol belongs to the GUI alone. Refuse it here, with a usage error,
    // before the subcommand is parsed: `soundcraft-cli` never opens a profile root or a window.
    if let Some(flag) = launcher_protocol_flag(&args) {
        let _ = writeln!(err, "soundcraft-cli: {flag} belongs to SoundCraft Nightly; this is the command-line tool");
        return 2;
    }
    // Help and the version (`help`, `-h`, `<subcommand> --help`, `-V`) come back as errors too.
    let cli = match parse(&args) {
        Ok(cli) => cli,
        Err(e) => {
            let to: &mut dyn Write = if e.use_stderr() { err } else { out };
            let _ = write!(to, "{}", e.render());
            return e.exit_code();
        }
    };
    match dispatch(cli.command, out, err) {
        Ok(()) => 0,
        Err(e) => {
            outln!(err, "error: {e}");
            1
        }
    }
}

fn dispatch(command: Command, out: &mut dyn Write, err: &mut dyn Write) -> Result<(), CliError> {
    match command {
        Command::Info(a) => info(&a, out, err)?,
        Command::Convert(a) => convert(&a, out)?,
        Command::Run(a) => run_cmds(&a, out, err)?,
        Command::Script(a) => script(&a, out, err)?,
        Command::Commands(a) => commands(&a, out),
        Command::Describe(a) => describe(&a, out)?,
        Command::App(a) => app(&a, out)?,
        Command::Mcp(a) => mcp(&a, err)?,
        Command::Parity(a) => parity(&a, out)?,
        Command::Plugins => {
            for p in soundcraft_dsp::plugins() {
                outln!(out, "{:<20} {:<22} {:?}{}", p.id, p.name, p.category, if p.is_instrument { " (instrument)" } else { "" });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod launcher_protocol_tests {
    use super::*;

    #[test]
    fn the_launcher_only_protocol_is_a_usage_error_not_a_file() {
        for args in [
            vec!["--launch-profile", "nightly-v1", "--profile-root", "/tmp/profile", "--", "song.scraft"],
            vec!["--launch-profile=nightly-v1"],
            vec!["--profile-root=/tmp/profile"],
            vec!["convert", "in.wav", "--launch-profile", "nightly-v1"],
        ] {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = run(args.iter().map(OsString::from), &mut out, &mut err);
            assert_eq!(code, 2, "{args:?}");
            let message = String::from_utf8_lossy(&err);
            assert!(message.contains("SoundCraft Nightly"), "{args:?}: {message}");
        }
    }

    #[test]
    fn everything_after_the_delimiter_is_a_file_name() {
        let args: Vec<OsString> = ["convert", "--", "--launch-profile", "--profile-root=./x"].map(OsString::from).to_vec();
        assert_eq!(launcher_protocol_flag(&args), None);
        assert_eq!(launcher_protocol_flag(&[OsString::from("--profile-root"), OsString::from("/p")]), Some("--profile-root"));
    }
}
