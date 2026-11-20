use super::args::McpArgs;
use super::error::CliError;
use soundcraft_automation::{ControlTarget, Headless, Remote, mcp::Server};
use soundcraft_engine::Engine;
use soundcraft_engine::file_access::{self, AutomationRoots};
use std::io::Write;

/// What `mcp` serves.
struct Setup {
    connect: Option<ControlTarget>,
    demo: bool,
    roots: Option<std::sync::Arc<AutomationRoots>>,
}

/// Open the roots before the server starts: a folder that can't be opened must not leave the
/// server running unconfined.
fn setup(args: &McpArgs) -> Result<Setup, CliError> {
    let read: Vec<&std::path::Path> = args.automation.automation_read_root.iter().map(std::path::PathBuf::as_path).collect();
    let write: Vec<&std::path::Path> = args.automation.automation_write_root.iter().map(std::path::PathBuf::as_path).collect();
    let roots = AutomationRoots::new(&read, &write)?;
    Ok(Setup { connect: args.connect.clone(), demo: args.demo, roots })
}

/// Serves on the process's real stdin and stdout; only the notes go to `err`.
pub(crate) fn mcp(args: &McpArgs, err: &mut dyn Write) -> Result<(), CliError> {
    let Setup { connect, demo, roots } = setup(args)?;
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let served = if let Some(target) = connect {
        let mut s = Server::new(Remote::new(&target));
        s.serve(stdin.lock(), stdout.lock())
    } else {
        // `--demo` is the operator's own choice, made before the roots come into force.
        let e = if demo { soundcraft_engine::demo::demo_engine() } else { Engine::default() };
        let mut h = Headless::new(e);
        if let Some(roots) = roots {
            // Every thread of this process, not only the server's: nothing reaches past the roots.
            file_access::confine_process(roots.clone());
            outln!(err, "soundcraft-cli: files confined: {}", roots.describe());
            h = h.with_automation_roots(Some(roots));
        }
        let mut s = Server::new(h);
        s.serve(stdin.lock(), stdout.lock())
    };
    served.map_err(CliError::Serve)
}
