use super::args::AppArgs;
use super::error::CliError;
use super::output::print_json;
use serde_json::{Value, json};
use soundcraft_automation::Remote;
use std::io::Write;

/// Control-channel methods. Every other dotted id is an engine command.
fn is_control_method(method: &str) -> bool {
    matches!(
        method,
        "engine.execute"
            | "command"
            | "engine.commands"
            | "engine.parity"
            | "session.inspect"
            | "document.inspect"
            | "ui.inspect"
            | "ui.menu.list"
            | "ui.menu.invoke"
            | "ui.set"
            | "ui.move"
            | "ui.click"
            | "ui.drag"
            | "ui.key"
            | "ui.text"
            | "ui.screenshot"
            | "app.quit"
    )
}

/// A command id is shorthand for `engine.execute`. Documented control methods stay methods.
fn app_request(method: &str, params: Value) -> (String, Value) {
    if is_control_method(method) || !method.contains('.') {
        (method.to_string(), params)
    } else {
        ("engine.execute".to_string(), json!({"command": method, "params": params}))
    }
}

pub(crate) fn app(args: &AppArgs, out: &mut dyn Write) -> Result<(), CliError> {
    let method = &args.method;
    let params = args.json.clone().unwrap_or_else(|| json!({}));
    let mut r = Remote::new(&args.control.port);
    // A bare command id is shorthand for engine.execute. Control-channel methods stay methods.
    let (m, p) = app_request(method, params);
    let v = r.request(&m, p)?;
    print_json(out, &v, args.compact);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_ids_are_execute_shorthand_and_control_methods_stay_methods() {
        let (m, p) = app_request("engine.history", json!({}));
        assert_eq!(m, "engine.execute");
        assert_eq!(p["command"], "engine.history");
        let (m, p) = app_request("ui.theme", json!({"mode": "light"}));
        assert_eq!(m, "engine.execute");
        assert_eq!(p["params"]["mode"], "light");
        let (m, _) = app_request("session.new", json!({"name": "Session A"}));
        assert_eq!(m, "engine.execute");
        let (m, _) = app_request("track.new", json!({}));
        assert_eq!(m, "engine.execute");
        for method in ["session.inspect", "engine.commands", "engine.parity", "ui.screenshot", "app.quit"] {
            assert_eq!(app_request(method, json!({})).0, method);
        }
    }
}
