//! The command line, as clap types. A flag a subcommand doesn't take is a usage error (exit 2), so
//! a typo such as `--bounces` can't be silently ignored (#549), and so is a value flag without its
//! value: a following flag is never taken as the value.

use std::borrow::Cow;
use std::ffi::OsString;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use clap::builder::NonEmptyStringValueParser;
use clap::error::ErrorKind;
use clap::{ArgAction, Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use serde_json::{Value, json};
use soundcraft_audio_io::FileFormat;
use soundcraft_automation::ControlTarget;
use soundcraft_automation::args::{AutomationArgs, ControlClientArgs, blank_env_is_unset, control_target, long_version, name_env_vars};

/// Help text that names the extensions `list` gives, built once from
/// [`FileFormat::readable_extensions`] or [`FileFormat::writable_extensions`] so it can't drift
/// from what the code accepts.
fn with_formats(cell: &'static OnceLock<String>, before: &str, list: fn() -> Vec<&'static str>, after: &str) -> &'static str {
    cell.get_or_init(|| format!("{before}({}){after}", list().join(", ")))
}

fn info_file_help() -> &'static str {
    static C: OnceLock<String> = OnceLock::new();
    with_formats(&C, "An audio file ", FileFormat::readable_extensions, " or a .scraft session")
}

fn read_help() -> &'static str {
    static C: OnceLock<String> = OnceLock::new();
    with_formats(&C, "The audio file to read ", FileFormat::readable_extensions, "")
}

fn write_help() -> &'static str {
    static C: OnceLock<String> = OnceLock::new();
    with_formats(&C, "The audio file to write; its extension picks the format ", FileFormat::writable_extensions, "")
}

/// soundcraft-cli: SoundCraft without a window
#[derive(Parser)]
#[command(name = "soundcraft-cli", version = long_version(), arg_required_else_help = true)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    Info(InfoArgs),
    Convert(ConvertArgs),
    Run(RunArgs),
    Script(ScriptArgs),
    Commands(CommandsArgs),
    Describe(DescribeArgs),
    App(AppArgs),
    Mcp(McpArgs),
    Parity(ParityArgs),
    /// List the built-in plugins
    Plugins,
}

/// Print an audio file's format as JSON, or a .scraft session's summary
#[derive(Args)]
pub(crate) struct InfoArgs {
    #[arg(help = info_file_help())]
    pub(crate) file: PathBuf,
    /// One line of JSON instead of pretty-printed (a .scraft summary is text and ignores it)
    #[arg(long)]
    pub(crate) compact: bool,
}

/// Convert an audio file: resample, change the bit depth, normalize
///
/// Dithers only when the output loses resolution, unless --dither or --no-dither says otherwise.
#[derive(Args)]
pub(crate) struct ConvertArgs {
    #[arg(value_name = "IN", help = read_help())]
    pub(crate) input: PathBuf,
    #[arg(value_name = "OUT", help = write_help())]
    pub(crate) output: PathBuf,
    /// The output's bit depth
    #[arg(long, value_enum, value_name = "16|24|32f", default_value = "24")]
    pub(crate) bit_depth: BitDepthArg,
    /// Resample to this sample rate
    #[arg(long, value_name = "HZ")]
    pub(crate) rate: Option<NonZeroU32>,
    /// Normalize the peak to -0.1 dBFS
    #[arg(long)]
    pub(crate) normalize: bool,
    /// Always dither
    #[arg(long, conflicts_with = "no_dither")]
    pub(crate) dither: bool,
    /// Never dither
    #[arg(long)]
    pub(crate) no_dither: bool,
    /// Allow an <OUT> that is the <IN> file, replacing the original
    #[arg(long)]
    pub(crate) in_place: bool,
}

/// Where `run` and `script` start, and what they do with the result.
#[derive(Args)]
pub(crate) struct EngineArgs {
    /// Open this session first
    #[arg(long = "in", value_name = "S.scraft", conflicts_with = "demo")]
    pub(crate) input: Option<PathBuf>,
    /// Start from the demo session
    #[arg(long, visible_alias = "sample")]
    pub(crate) demo: bool,
    /// Bounce the mix to this file at the end; without --start and --end, the whole session
    #[arg(long, value_name = "OUT")]
    pub(crate) bounce: Option<PathBuf>,
    /// Where the bounce starts: seconds (12.5) or a time string
    #[arg(long, value_name = "T", allow_negative_numbers = true, requires = "bounce", value_parser = time_arg)]
    pub(crate) start: Option<TimeArg>,
    /// Where the bounce ends: seconds (12.5) or a time string
    #[arg(long, value_name = "T", allow_negative_numbers = true, requires = "bounce", value_parser = time_arg)]
    pub(crate) end: Option<TimeArg>,
    /// Save the session to this file at the end, replacing it
    #[arg(long, value_name = "S.scraft")]
    pub(crate) save: Option<PathBuf>,
    /// Print the session as JSON at the end
    #[arg(long)]
    pub(crate) inspect: bool,
    /// Print the --inspect JSON on one line instead of pretty-printed
    #[arg(long, requires = "inspect")]
    pub(crate) compact: bool,
    /// Allow a --save that is the --in session, replacing the original
    #[arg(long, requires = "input")]
    pub(crate) in_place: bool,
}

/// Run engine commands on a new, demo or saved session
///
/// Prints each command's JSON result on one line, then bounces, saves and inspects, in that order.
/// The --inspect JSON is pretty-printed unless --compact is given.
#[derive(Args)]
pub(crate) struct RunArgs {
    /// An engine command, with its JSON parameters after `=` (default {}); repeat for several, run
    /// in order
    #[arg(long, value_name = "ID[=JSON]", action = ArgAction::Append, value_parser = cmd_spec)]
    pub(crate) cmd: Vec<CmdSpec>,
    #[command(flatten)]
    pub(crate) engine: EngineArgs,
}

/// Run a file of engine commands, one `ID JSON` per line
///
/// Blank lines and lines starting with # are skipped. A failing line stops the script unless
/// --keep-going is given.
#[derive(Args)]
pub(crate) struct ScriptArgs {
    /// The script to run
    pub(crate) file: PathBuf,
    /// Continue after command errors and finish requested output, then exit nonzero if any command failed.
    /// Malformed JSON stops immediately
    #[arg(long)]
    pub(crate) keep_going: bool,
    #[command(flatten)]
    pub(crate) engine: EngineArgs,
}

/// List the engine commands
#[derive(Args)]
pub(crate) struct CommandsArgs {
    /// Only the commands whose id, or label in any case, contains this text
    #[arg(value_parser = NonEmptyStringValueParser::new())]
    pub(crate) filter: Option<String>,
}

/// Print one engine command's details as JSON
#[derive(Args)]
pub(crate) struct DescribeArgs {
    /// The command's id, such as file.bounce_mix
    #[arg(value_parser = NonEmptyStringValueParser::new())]
    pub(crate) id: String,
    /// One line of JSON instead of pretty-printed
    #[arg(long)]
    pub(crate) compact: bool,
}

/// Call a running app's control channel
///
/// A dotted engine command id is shorthand for engine.execute; the control methods
/// (session.inspect, ui.screenshot, …) stay methods. Prints the reply as JSON.
#[derive(Args)]
pub(crate) struct AppArgs {
    #[command(flatten)]
    pub(crate) control: ControlClientArgs,
    /// A control method or engine command id
    #[arg(value_parser = NonEmptyStringValueParser::new())]
    pub(crate) method: String,
    /// Its parameters as JSON [default: {}]
    #[arg(value_parser = json_value)]
    pub(crate) json: Option<Value>,
    /// One line of JSON instead of pretty-printed
    #[arg(long)]
    pub(crate) compact: bool,
}

/// Serve MCP on stdin and stdout
///
/// The automation roots confine every file the agent's commands read (open, import, presets) and
/// write (save, bounce, export) to those folders, links followed. Each root flag may be given more
/// than once, a path inside any of its folders being allowed, and a kind left out grants none of
/// its access. The roots confine the in-process server, so they, like --demo, conflict with --connect.
#[derive(Args)]
pub(crate) struct McpArgs {
    /// Serve a running app's control channel at this port or host:port instead of an in-process
    /// engine; conflicts with --demo, which starts only that engine
    #[arg(long, value_name = "PORT", value_parser = control_target, conflicts_with_all = ["demo", "automation_read_root", "automation_write_root"])]
    pub(crate) connect: Option<ControlTarget>,
    /// Start the in-process engine from the demo session
    #[arg(long)]
    pub(crate) demo: bool,
    #[command(flatten)]
    pub(crate) automation: AutomationArgs,
}

/// Print the parity report (markdown)
#[derive(Args)]
pub(crate) struct ParityArgs {
    /// Write the report to this file instead
    #[arg(long, value_name = "PATH")]
    pub(crate) write: Option<PathBuf>,
}

impl Command {
    /// The outputs clap can't check: one that is an input file, however it is spelt, or both of
    /// `--bounce` and `--save` naming one file. The paths are compared before any work starts and
    /// written later, so a link swapped in meanwhile goes unnoticed: this guards against mistakes,
    /// it is not a security boundary.
    fn check_outputs(&self) -> Result<(), OutputError> {
        match self {
            Self::Convert(a) if !a.in_place && same_file(&a.input, &a.output) => {
                Err(OutputError::ReplacesInput { output: "<OUT>", input: "<IN> file", path: a.output.clone() })
            }
            Self::Run(a) => a.engine.check_outputs(None),
            Self::Script(a) => a.engine.check_outputs(Some(&a.file)),
            Self::Info(_)
            | Self::Convert(_)
            | Self::Commands(_)
            | Self::Describe(_)
            | Self::App(_)
            | Self::Mcp(_)
            | Self::Parity(_)
            | Self::Plugins => Ok(()),
        }
    }
}

impl ConvertArgs {
    /// The file `convert` writes: <OUT>, or the file it links to when it replaces <IN> in place.
    pub(crate) fn write_path(&self) -> Cow<'_, Path> {
        in_place_target(self.in_place, Some(&self.input), &self.output)
    }
}

impl EngineArgs {
    /// The file --save writes: its path, or the file it links to when it replaces --in in place.
    pub(crate) fn save_path(&self) -> Option<Cow<'_, Path>> {
        self.save.as_deref().map(|save| in_place_target(self.in_place, self.input.as_deref(), save))
    }

    /// `script` is the script file `script` runs, which no flag lets an output replace. --in-place
    /// lets only --save replace the --in session: a bounce would turn it into audio.
    fn check_outputs(&self, script: Option<&Path>) -> Result<(), OutputError> {
        for (flag, path) in [("--bounce", &self.bounce), ("--save", &self.save)] {
            let Some(path) = path else { continue };
            if script.is_some_and(|script| same_file(script, path)) {
                return Err(OutputError::ReplacesScript { output: flag, path: path.clone() });
            }
        }
        let replaces_input = |path: &Path| self.input.as_ref().is_some_and(|input| same_file(input, path));
        if let Some(bounce) = &self.bounce
            && replaces_input(bounce)
        {
            return Err(OutputError::BounceReplacesInput(bounce.clone()));
        }
        if let Some(save) = &self.save
            && !self.in_place
            && replaces_input(save)
        {
            return Err(OutputError::ReplacesInput { output: "--save", input: "--in session", path: save.clone() });
        }
        match (&self.bounce, &self.save) {
            (Some(bounce), Some(save)) if same_file(bounce, save) => Err(OutputError::SameOutputs(save.clone())),
            _ => Ok(()),
        }
    }
}

/// Whether `a` and `b` name one file, however spelt (`a.wav`, `./a.wav`, a symbolic or hard link).
/// When either exists, only two regular files can be one, compared by identity (device and inode);
/// a FIFO or device is never opened, so the check can't block. When neither exists yet, they are
/// one if they would be created at the same place.
fn same_file(a: &Path, b: &Path) -> bool {
    if a.exists() || b.exists() {
        return a.is_file() && b.is_file() && same_file::is_same_file(a, b).unwrap_or(false);
    }
    matches!((destination(a), destination(b)), (Some(a), Some(b)) if a == b)
}

/// With `in_place`, an `output` that is a symbolic link to `input` resolved to the file it names.
/// A save writes a new file and renames it over its path, which would replace the link and leave
/// the input as it was: through the resolved path, `convert` and `--save` both replace the input.
fn in_place_target<'a>(in_place: bool, input: Option<&Path>, output: &'a Path) -> Cow<'a, Path> {
    let link = output.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink());
    if in_place
        && link
        && input.is_some_and(|input| same_file(input, output))
        && let Ok(target) = output.canonicalize()
    {
        return Cow::Owned(target);
    }
    Cow::Borrowed(output)
}

/// Where a file that doesn't exist yet would be created: its folder resolved, links followed.
fn destination(path: &Path) -> Option<PathBuf> {
    let folder = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    Some(folder.canonicalize().ok()?.join(path.file_name()?))
}

/// An output [`Command::check_outputs`] refuses: `parse` prints it as a usage error (exit 2).
#[derive(Debug, thiserror::Error)]
pub(crate) enum OutputError {
    #[error("{output} {} is the {input}, which it would replace: choose another file, or pass --in-place", .path.display())]
    ReplacesInput { output: &'static str, input: &'static str, path: PathBuf },
    #[error("--bounce {} is the --in session, which it would replace with audio: choose another file", .0.display())]
    BounceReplacesInput(PathBuf),
    #[error("{output} {} is the script file, which it would replace: choose another file", .path.display())]
    ReplacesScript { output: &'static str, path: PathBuf },
    #[error("--bounce and --save both name {}, so the save would replace the bounce: choose another file for one of them", .0.display())]
    SameOutputs(PathBuf),
}

impl OutputError {
    /// Two arguments that may not name one file: clap's conflict kind.
    fn into_clap(self, cmd: &mut clap::Command) -> clap::Error {
        cmd.error(ErrorKind::ArgumentConflict, self)
    }
}

/// The bit depths `convert` writes. `32` is the same as `32f`.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum BitDepthArg {
    #[value(name = "16")]
    Int16,
    #[value(name = "24")]
    Int24,
    #[value(name = "32f", alias = "32")]
    Float32,
}

impl From<BitDepthArg> for soundcraft_audio_io::BitDepth {
    fn from(depth: BitDepthArg) -> Self {
        match depth {
            BitDepthArg::Int16 => Self::Int16,
            BitDepthArg::Int24 => Self::Int24,
            BitDepthArg::Float32 => Self::Float32,
        }
    }
}

/// A bounce position: seconds, or a time string the engine reads.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TimeArg {
    Seconds(f64),
    Text(String),
}

impl TimeArg {
    /// The `start` / `end` parameter of `file.bounce_mix`.
    pub(crate) fn to_json(&self) -> Value {
        match self {
            Self::Seconds(s) => json!({"seconds": s}),
            Self::Text(t) => json!(t),
        }
    }
}

/// One `--cmd ID[=JSON]`: the command id and its parameters (`{}` when none are given).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CmdSpec {
    pub(crate) id: String,
    pub(crate) params: Value,
}

/// Why a value was refused: clap prints it after the flag and its value (exit 2).
#[derive(Debug, thiserror::Error)]
pub(crate) enum ValueError {
    #[error("expected seconds (12.5) or a time string, not an empty value")]
    EmptyTime,
    #[error("expected a finite number of seconds")]
    NotFinite,
    #[error("expected a command id, optionally followed by `=` and JSON parameters")]
    EmptyCommandId,
    #[error("bad JSON: {0}")]
    Json(#[from] serde_json::Error),
}

fn time_arg(s: &str) -> Result<TimeArg, ValueError> {
    if s.is_empty() {
        return Err(ValueError::EmptyTime);
    }
    match s.parse::<f64>() {
        // JSON has no NaN or infinity: serde_json would send the engine `null`.
        Ok(secs) if !secs.is_finite() => Err(ValueError::NotFinite),
        Ok(secs) => Ok(TimeArg::Seconds(secs)),
        Err(_) => Ok(TimeArg::Text(s.to_owned())),
    }
}

fn json_value(s: &str) -> Result<Value, ValueError> {
    Ok(serde_json::from_str(s)?)
}

/// `ID` or `ID=JSON`; malformed JSON is an error, not `{}`.
fn cmd_spec(s: &str) -> Result<CmdSpec, ValueError> {
    let (id, params) = match s.split_once('=') {
        Some((id, p)) => (id, json_value(p)?),
        None => (s, json!({})),
    };
    if id.is_empty() {
        return Err(ValueError::EmptyCommandId);
    }
    Ok(CmdSpec { id: id.to_owned(), params })
}

/// Parse the arguments (without the binary name). clap reads them as `OsString`s: a folder need
/// not be valid Unicode, text must. A flag wins over its environment variable, unless that is
/// empty, and a bad value from one is a usage error naming the variable. A bare `--help` or `-h`
/// is help even after a mistake on the same line (`run --bounce --help`): clap reports the mistake
/// first. An output that would replace an input is refused here too
/// ([`Command::check_outputs`]), so nothing has run when it is reported.
pub(crate) fn parse(args: &[OsString]) -> Result<Cli, clap::Error> {
    let mut cmd = Cli::command().mut_subcommands(|sub| sub.mut_args(blank_env_is_unset));
    let argv = std::iter::once(OsString::from("soundcraft-cli")).chain(args.iter().cloned());
    let matches = match cmd.try_get_matches_from_mut(argv) {
        Err(e) if e.kind() != ErrorKind::DisplayHelp => {
            let Some(flag) = args.iter().take_while(|a| *a != "--").find(|a| *a == "--help" || *a == "-h") else {
                return Err(name_env_vars(e, &cmd, args));
            };
            let help = [OsString::from("soundcraft-cli"), args.first().cloned().unwrap_or_default(), flag.clone()];
            return Err(cmd.try_get_matches_from_mut(help).err().unwrap_or(e));
        }
        matches => matches?,
    };
    let cli = Cli::from_arg_matches(&matches)?;
    if let Err(e) = cli.command.check_outputs() {
        return Err(match matches.subcommand_name().and_then(|name| cmd.find_subcommand_mut(name)) {
            Some(sub) => e.into_clap(sub),
            None => e.into_clap(&mut cmd),
        });
    }
    Ok(cli)
}
