use super::args::{CmdSpec, EngineArgs, RunArgs, ScriptArgs};
use super::error::{CliError, unicode};
use super::output::print_json;
use serde_json::{Value, json};
use soundcraft_engine::Engine;
use std::io::Write;

/// One stderr line per media file a session refers to but could not load (the exit code is unchanged).
pub(crate) fn warn_missing(err: &mut dyn Write, missing: &[String]) {
    for m in missing {
        outln!(err, "warning: missing media: {m}");
    }
}

/// The outputs go to the engine as JSON text, so a non-Unicode one is refused before any command runs.
fn load_engine(args: &EngineArgs, err: &mut dyn Write) -> Result<Engine, CliError> {
    if let Some(bounce) = &args.bounce {
        unicode(bounce)?;
    }
    if let Some(save) = args.save_path() {
        unicode(&save)?;
    }
    if let Some(p) = &args.input {
        let mut e = Engine::default();
        let missing = soundcraft_engine::io::open_session(&mut e, p)?;
        warn_missing(err, &missing);
        Ok(e)
    } else if args.demo {
        Ok(soundcraft_engine::demo::demo_engine())
    } else {
        Ok(Engine::default())
    }
}

fn finish(e: &mut Engine, args: &EngineArgs, out: &mut dyn Write) -> Result<(), CliError> {
    if let Some(path) = &args.bounce {
        let mut p = json!({"path": unicode(path)?});
        if let Some(s) = &args.start {
            p["start"] = s.to_json();
        }
        if let Some(s) = &args.end {
            p["end"] = s.to_json();
        }
        // No range given: bounce the whole session, not whatever edit selection the commands left.
        if p.get("start").is_none() && p.get("end").is_none() {
            p["start"] = json!(0);
            p["end"] = json!(e.session().content_end().max(1));
        }
        let v = e.execute("file.bounce_mix", &p)?;
        outln!(out, "{v}");
    }
    if let Some(path) = args.save_path() {
        let v = e.execute("session.save", &json!({"path": unicode(&path)?, "replace": true}))?;
        outln!(out, "{v}");
    }
    if args.inspect {
        print_json(out, &soundcraft_engine::inspect::session(e, false), args.compact);
    }
    Ok(())
}

pub(crate) fn run_cmds(args: &RunArgs, out: &mut dyn Write, err: &mut dyn Write) -> Result<(), CliError> {
    let mut e = load_engine(&args.engine, err)?;
    for CmdSpec { id, params } in &args.cmd {
        match e.execute(id, params) {
            Ok(v) => outln!(out, "{id}: {v}"),
            Err(source) => return Err(CliError::Command { id: id.clone(), source }),
        }
    }
    finish(&mut e, &args.engine, out)
}

pub(crate) fn script(args: &ScriptArgs, out: &mut dyn Write, err: &mut dyn Write) -> Result<(), CliError> {
    let path = &args.file;
    let text = std::fs::read_to_string(path).map_err(|source| CliError::File { path: path.clone(), source })?;
    let mut e = load_engine(&args.engine, err)?;
    let keep = args.keep_going;
    let mut failed = false;
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (id, p) = line.split_once(char::is_whitespace).unwrap_or((line, "{}"));
        let params = serde_json::from_str::<Value>(p.trim()).map_err(|source| CliError::ScriptJson { path: path.clone(), line: n + 1, source })?;
        match e.execute(id, &params) {
            Ok(v) => outln!(out, "{id}: {v}"),
            Err(source) => {
                let failure = CliError::ScriptLine { path: path.clone(), line: n + 1, id: id.to_owned(), source };
                if !keep {
                    return Err(failure);
                }
                outln!(err, "{failure}");
                failed = true;
            }
        }
    }
    finish(&mut e, &args.engine, out)?;
    if failed { Err(CliError::ScriptFailed) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_media_gets_one_warning_each() {
        let mut err = Vec::new();
        warn_missing(&mut err, &[]);
        assert!(err.is_empty());
        warn_missing(&mut err, &["a.wav".to_string(), "b.wav: decode failed".to_string()]);
        assert_eq!(String::from_utf8(err).unwrap(), "warning: missing media: a.wav\nwarning: missing media: b.wav: decode failed\n");
    }
}
