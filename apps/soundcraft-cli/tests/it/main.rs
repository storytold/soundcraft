//! Integration tests, built as one test binary: each file is a module.

#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod behaviour;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod cli;
mod closed_stdout;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod man_and_completions;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod run;
mod script_status;
