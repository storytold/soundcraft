//! The command line's logger. Without one, every `log::warn!` and `log::error!` in the workspace
//! was dropped: a plugin that failed to load just vanished from `engine.clap_plugins`, and nothing
//! said why. Warnings and errors now go to standard error as `warning: <message>` and
//! `error: <message>`, like the CLI's own errors; standard output keeps only the results.
//! `RUST_LOG` stays a desktop app variable (docs/cli.md), so the level is fixed.

use std::io::Write;

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            // A closed standard error is not worth failing a command over.
            let _ = std::io::stderr().write_all(line(record.level(), &record.args().to_string()).as_bytes());
        }
    }

    fn flush(&self) {}
}

/// One record as the CLI prints it.
fn line(level: log::Level, message: &str) -> String {
    let label = if level == log::Level::Error { "error" } else { "warning" };
    format!("{label}: {}\n", message.strip_suffix('\n').unwrap_or(message))
}

/// Installs the logger. Once per process: `main` calls it, not `run`, which tests call many times.
pub fn install() {
    static LOGGER: StderrLogger = StderrLogger;
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_print_like_the_cli_errors() {
        assert_eq!(line(log::Level::Warn, "cannot load CLAP plugin x.clap: bad file\n"), "warning: cannot load CLAP plugin x.clap: bad file\n");
        assert_eq!(line(log::Level::Error, "panic: boom"), "error: panic: boom\n");
    }
}
