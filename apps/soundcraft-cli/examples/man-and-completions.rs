//! Writes soundcraft-cli's shell completions ([`clap_complete`]: bash, zsh, fish, elvish,
//! PowerShell) and man pages ([`clap_mangen`]: `soundcraft-cli.1` and one per subcommand, such as
//! `soundcraft-cli-convert.1`) into a directory. The Linux and FreeBSD packages ship them
//! (`packaging/linux/package.sh`, `packaging/freebsd/package.sh`):
//!
//! ```sh
//! cargo run -p soundcraft-cli --example man-and-completions -- <out-dir>
//! ```

use std::path::Path;
use std::process::ExitCode;

use clap::ValueEnum;
use clap_complete::Shell;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let (Some(dir), None) = (args.next(), args.next()) else {
        eprintln!("usage: man-and-completions <out-dir>");
        return ExitCode::from(2);
    };
    let dir = Path::new(&dir);
    match generate(dir) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {}: {e}", dir.display());
            ExitCode::FAILURE
        }
    }
}

/// Also run by `tests/it/man_and_completions.rs`.
pub(crate) fn generate(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut cmd = soundcraft_cli::command();
    for &shell in Shell::value_variants() {
        clap_complete::generate_to(shell, &mut cmd, "soundcraft-cli", dir)?;
    }
    // A fresh command: `clap_complete::generate_to` built `cmd`, adding the `help` subcommand,
    // which `clap_mangen` would otherwise give pages of its own (`soundcraft-cli-help-convert.1`).
    clap_mangen::generate_to(soundcraft_cli::command(), dir)
}
