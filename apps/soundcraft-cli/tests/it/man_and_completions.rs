//! The clap definition is consistent, and every shell's completions and every man page
//! (`examples/man-and-completions.rs`) can be generated from it.

use clap::ValueEnum;
use clap_complete::Shell;

// The example itself, so the test writes the files exactly as the packages get them.
#[allow(dead_code)]
#[path = "../../examples/man-and-completions.rs"]
mod example;

#[test]
fn command_is_consistent() {
    soundcraft_cli::command().debug_assert();
}

#[test]
fn completions_generate_for_every_shell() {
    for &shell in Shell::value_variants() {
        let mut out = Vec::new();
        clap_complete::generate(shell, &mut soundcraft_cli::command(), "soundcraft-cli", &mut out);
        assert!(!out.is_empty(), "{shell}");
    }
}

#[test]
fn man_pages_render_for_the_command_and_every_subcommand() {
    let cmd = soundcraft_cli::command();
    for page in std::iter::once(&cmd).chain(cmd.get_subcommands()) {
        let mut out = Vec::new();
        clap_mangen::Man::new(page.clone()).render(&mut out).unwrap();
        assert!(!out.is_empty(), "{}", page.get_name());
    }
}

/// The example writes the five completion files under the names `packaging/linux/package.sh` and
/// `packaging/freebsd/package.sh` install, `soundcraft-cli.1` and a page per subcommand, and no
/// pages for the `help` subcommand.
#[test]
fn the_example_writes_the_files_the_packages_install() {
    let dir = std::env::temp_dir().join(format!("sc-cli-{}-man-and-completions", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    example::generate(&dir).unwrap();

    let mut written: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    written.sort();
    let completions = ["soundcraft-cli.bash", "_soundcraft-cli", "soundcraft-cli.fish", "soundcraft-cli.elv", "_soundcraft-cli.ps1"];
    let subcommands = ["info", "convert", "run", "script", "commands", "describe", "app", "mcp", "parity", "plugins"];
    let mut expected: Vec<String> = completions.map(String::from).into();
    expected.push("soundcraft-cli.1".into());
    expected.extend(subcommands.iter().map(|s| format!("soundcraft-cli-{s}.1")));
    expected.sort();
    assert_eq!(written, expected);
    std::fs::remove_dir_all(&dir).unwrap();
}
