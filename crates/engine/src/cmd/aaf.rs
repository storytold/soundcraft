use super::*;
use crate::cmd;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "file.preview_aaf", "Analyze AAF", [], None, "{path, media_dir, composition?} — read-only plan and source properties", always, |_,p| {
            let path = str_param(p,"path").ok_or_else(|| bad("file.preview_aaf","path required"))?;
            let media = str_param(p,"media_dir").ok_or_else(|| bad("file.preview_aaf","media_dir required"))?;
            crate::aaf::preview(path,media,str_param(p,"composition"))
        }),
        cmd!(
            "file.import_aaf",
            "Session Data (AAF)...",
            ["File", "Import"],
            None,
            "{plan, selected:[track indices], media?: copy, mapping?: absolute|relative, offset_samples?: 0} — new tracks, full source handles, undoable",
            stopped,
            |e, p| {
                stopped(e).map_err(|why| bad("file.import_aaf", why))?;
                let (session, report) = crate::aaf::import_into(e.session(), p)?;
                *e.session_mut() = session;
                Ok(report)
            }
        ),
    ]
}
fn stopped(e: &Engine) -> std::result::Result<(), String> {
    if e.transport.playing || e.transport.recording { Err("Stop playback and recording before importing session data".into()) } else { Ok(()) }
}
