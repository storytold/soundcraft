use std::path::{Path, PathBuf};

use soundcraft_audio_io::AudioError;
use soundcraft_automation::RemoteError;
use soundcraft_engine::EngineError;
use soundcraft_engine::file_access::RootError;

/// Why a subcommand failed: printed as `error: {e}`, exit 1.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CliError {
    #[error(transparent)]
    Convert(#[from] ConvertError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("{id}: {source}")]
    Command { id: String, source: EngineError },
    #[error("{}:{line}: bad JSON: {source}", path.display())]
    ScriptJson { path: PathBuf, line: usize, source: serde_json::Error },
    #[error("{}:{line}: {id}: {source}", path.display())]
    ScriptLine { path: PathBuf, line: usize, id: String, source: EngineError },
    #[error("script finished with command errors")]
    ScriptFailed,
    #[error("{}: {source}", path.display())]
    File { path: PathBuf, source: std::io::Error },
    #[error("{}: {source}", path.display())]
    Audio { path: PathBuf, source: AudioError },
    /// A `--bounce` or `--save` path: the engine's commands take them in JSON, which holds only
    /// text.
    #[error("{}: the path is not valid Unicode", .0.display())]
    NotUnicode(PathBuf),
    #[error(transparent)]
    Remote(#[from] RemoteError),
    #[error(transparent)]
    Roots(#[from] RootError),
    #[error(transparent)]
    Serve(std::io::Error),
}

/// A path as the text the engine's commands take.
pub(crate) fn unicode(path: &Path) -> Result<&str, CliError> {
    path.to_str().ok_or_else(|| CliError::NotUnicode(path.to_owned()))
}

/// A `convert` option or output the tool refuses.
#[derive(Debug, PartialEq, thiserror::Error)]
pub(crate) enum ConvertError {
    #[error("{}: cannot write `.{ext}` files (supported: {})", output.display(), soundcraft_audio_io::FileFormat::writable_extensions().join(", "))]
    OutputFormat { output: PathBuf, ext: String },
}
