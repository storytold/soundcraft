//! The `plugin.*` commands: list, install (a file, base64 data, a GitHub repository or a
//! catalog's package), update, remove and run WebAssembly command plug-ins (RFC-0046), and the
//! catalogs they come from. These are not audio plug-ins (CLAP, VST3, AU: `mix.insert`,
//! `engine.plugins`).
//!
//! Only `plugin.run` is journaled: it is what edits the session (as one undo step it records
//! itself). The others manage the user's plug-in folder and catalogs, outside the session, and a
//! bundle can be large, so they are not journaled, as `mix.insert_state` isn't.

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, bool_or, str_param};
use crate::command_plugin::{self, InstalledPlugin, MAX_CATALOGS};
use crate::{Engine, Result, cmd};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "plugin.list",
            "List Command Plug-ins",
            [],
            None,
            "{} → {plugins: [{id, name, version, kind, permissions, commands, source, sha256, stored}], problems, catalogs, stored (kept in the Plug-ins folder; else this session only), network, last} — WebAssembly command plug-ins, not audio plug-ins (see engine.plugins)",
            always,
            list
        ),
        cmd!(
            query "plugin.install",
            "Install Command Plug-in",
            [],
            None,
            r#"{"path":str,"review":bool?,"sha256":str?} | {"data":base64 .craftplugin,"name":str?,"review":bool?,"sha256":str?} | {"repo":"owner/repo"|"owner/repo@tag" (its release's .craftplugin, checked against SHA256SUMS),"review":bool?,"sha256":str?} | {"catalog":index.json URL,"id":str,"version":str? (default: the newest for SoundCraft),"review":bool?,"sha256":str?} — review: check and describe it without installing (the confirmation); sha256: install only that bundle → {id, name, version, kind, permissions, commands, source, sha256, inCatalog, replaces, permissionsChanged, stored, installed}"#,
            always,
            install
        ),
        cmd!(
            query "plugin.update",
            "Update Command Plug-ins",
            [],
            None,
            r#"{"id":str?,"apply":bool? (install the updates that ask for no other permissions)} — checks each plug-in's catalog or repository → {updates: [{id, installed, available, install (plugin.install params), permissionsChanged?}], updated, errors}"#,
            always,
            update
        ),
        cmd!(query "plugin.remove", "Remove Command Plug-in", [], None, r#"{"id":str} — removes it and its settings → {removed}"#, always, remove),
        cmd!(
            noundo "plugin.run",
            "Run Command Plug-in",
            [],
            None,
            r#"{"id":str,"command":str? (default: the first),"params":{}? (the command's parameters),"fuel":u64? (a lower instruction budget)} — runs the commands it calls in their JSON form on the session document, as one undo step named after the plug-in, rolled back when it fails; never the transport, recording, devices, files or the clipboard → {plugin, command, output, notes, logs, calls, fuel, panel}"#,
            always,
            run
        ),
        cmd!(
            query "plugin.catalog.list",
            "Command Plug-in Catalogs",
            [],
            None,
            r#"{"url":str?} — the packages SoundCraft can run in that catalog, or in every catalog added → {catalogs, packages: [{catalog, id, name, version, author, description, license, repo, kind, sha256, installed}], warnings}"#,
            always,
            catalog_list
        ),
        cmd!(
            query "plugin.catalog.add",
            "Add Command Plug-in Catalog",
            [],
            None,
            r#"{"url":https:// on GitHub or file:// index.json,"remove":bool?} → {catalogs}"#,
            always,
            catalog_add
        ),
    ]
}

/// A non-empty text parameter.
fn text<'a>(p: &'a Value, k: &str) -> Option<&'a str> {
    str_param(p, k).map(str::trim).filter(|t| !t.is_empty())
}

fn list(e: &mut Engine, _p: &Value) -> Result<Value> {
    let rt = &e.command_plugins;
    let plugins: Vec<Value> = rt.list().iter().map(InstalledPlugin::describe).collect();
    let last = rt.last.as_ref().map(|(id, command, _)| json!({"id": id, "command": command}));
    Ok(json!({
        "plugins": plugins,
        "problems": rt.problems(),
        "catalogs": rt.prefs.catalogs,
        "stored": rt.has_store(),
        "network": rt.fetcher().has_network(),
        "last": last,
    }))
}

fn install(e: &mut Engine, p: &Value) -> Result<Value> {
    let cmd = "plugin.install";
    let request = command_plugin::request_key(p);
    let wanted = text(p, "sha256").map(str::to_string);
    let c = match wanted.as_deref().and_then(|h| command_plugin::take_held(e, h, &request)) {
        Some(c) => c,
        None => command_plugin::obtain(e, cmd, p)?,
    };
    let (_, mut info) = command_plugin::review(e, cmd, &c)?;
    let sha256 = info["sha256"].as_str().unwrap_or_default().to_string();
    if let Some(w) = &wanted
        && !w.eq_ignore_ascii_case(&sha256)
    {
        return Err(bad(cmd, format!("the bundle's SHA-256 is {sha256}, not the {w} that was reviewed: review it again")));
    }
    if bool_or(p, "review", false) {
        command_plugin::hold(e, &sha256, request, c);
        info["installed"] = json!(false);
        return Ok(info);
    }
    command_plugin::install(e, cmd, c)?;
    info["installed"] = json!(true);
    Ok(info)
}

fn update(e: &mut Engine, p: &Value) -> Result<Value> {
    let cmd = "plugin.update";
    let only = text(p, "id");
    if let Some(id) = only
        && e.command_plugins.get(id).is_none()
    {
        return Err(bad(cmd, format!("no command plug-in `{id}` is installed")));
    }
    let apply = bool_or(p, "apply", false);
    let targets: Vec<InstalledPlugin> =
        e.command_plugins.list().iter().filter(|x| only.is_none_or(|id| x.manifest.id.eq_ignore_ascii_case(id))).cloned().collect();
    let (mut updates, mut updated, mut errors) = (vec![], vec![], vec![]);
    let fetcher = e.command_plugins.fetcher();
    for old in targets {
        let id = old.manifest.id.clone();
        let (params, available) = match fetcher.with(|f| command_plugin::update_for_with(f, &old)) {
            Ok(Some(found)) => found,
            Ok(None) => continue,
            Err(err) => {
                errors.push(json!({"id": id, "error": err}));
                continue;
            }
        };
        let mut entry = json!({"id": id, "installed": old.manifest.version, "available": available, "install": params});
        if apply {
            match apply_update(e, cmd, &params) {
                Ok(true) => updated.push(json!(id)),
                Ok(false) => entry["permissionsChanged"] = json!(true),
                Err(err) => errors.push(json!({"id": id, "error": err.to_string()})),
            }
        }
        updates.push(entry);
    }
    Ok(json!({"updates": updates, "updated": updated, "errors": errors}))
}

/// Install an update unless it asks for other permissions (that one waits for its confirmation,
/// `plugin.install`): whether it was installed.
fn apply_update(e: &mut Engine, cmd: &str, params: &Value) -> Result<bool> {
    let c = command_plugin::obtain(e, cmd, params)?;
    let (_, info) = command_plugin::review(e, cmd, &c)?;
    if info["permissionsChanged"].as_bool().unwrap_or(true) {
        return Ok(false);
    }
    command_plugin::install(e, cmd, c).map(|_| true)
}

fn remove(e: &mut Engine, p: &Value) -> Result<Value> {
    let cmd = "plugin.remove";
    let id = text(p, "id").ok_or_else(|| bad(cmd, "`id` required (see plugin.list)"))?.to_string();
    if !command_plugin::remove(e, cmd, &id)? {
        return Err(bad(cmd, format!("no command plug-in `{id}` is installed")));
    }
    Ok(json!({"removed": id}))
}

fn run(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = text(p, "id").ok_or_else(|| bad("plugin.run", "`id` required (see plugin.list)"))?.to_string();
    let command = text(p, "command").map(str::to_string);
    let fuel = p.get("fuel").and_then(Value::as_u64);
    command_plugin::run(e, &id, command.as_deref(), p.get("params").unwrap_or(&Value::Null), fuel)
}

fn catalog_list(e: &mut Engine, p: &Value) -> Result<Value> {
    let urls = match text(p, "url") {
        Some(url) => vec![url.to_string()],
        None => e.command_plugins.prefs.catalogs.clone(),
    };
    let (mut packages, mut warnings) = (vec![], vec![]);
    let installed = e.command_plugins.list().to_vec();
    let fetcher = e.command_plugins.fetcher();
    for url in &urls {
        match fetcher.with(|f| command_plugin::catalog_packages_with(f, url, &installed)) {
            Ok((found, w)) => {
                packages.extend(found);
                warnings.extend(w.into_iter().map(|w| format!("{url}: {w}")));
            }
            Err(err) => warnings.push(format!("{url}: {err}")),
        }
    }
    Ok(json!({"catalogs": urls, "packages": packages, "warnings": warnings}))
}

fn catalog_add(e: &mut Engine, p: &Value) -> Result<Value> {
    let cmd = "plugin.catalog.add";
    let url = text(p, "url").ok_or_else(|| bad(cmd, "`url` required"))?.to_string();
    let rt = &mut e.command_plugins;
    if bool_or(p, "remove", false) {
        let before = rt.prefs.catalogs.len();
        rt.prefs.catalogs.retain(|u| *u != url);
        if rt.prefs.catalogs.len() != before {
            rt.prefs_touched();
        }
    } else if !rt.prefs.catalogs.contains(&url) {
        command_plugin::check_catalog_url(&url).map_err(|err| bad(cmd, err))?;
        if rt.prefs.catalogs.len() >= MAX_CATALOGS {
            return Err(bad(cmd, format!("at most {MAX_CATALOGS} catalogs")));
        }
        rt.prefs.catalogs.push(url);
        rt.prefs_touched();
    }
    Ok(json!({"catalogs": rt.prefs.catalogs}))
}
