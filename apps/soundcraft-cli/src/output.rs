/// Print to a writer without panicking on a closed pipe. A write error must not skip later work
/// such as `--bounce` or `--save`, and must not by itself report success.
macro_rules! outln {
    ($w:expr, $($t:tt)*) => {{
        let _ = writeln!($w, $($t)*);
    }};
}

/// Print `value` as JSON: pretty-printed, or on one line when `compact`. Every subcommand that
/// prints a JSON document goes through here, so `--compact` means the same thing everywhere.
pub(crate) fn print_json(out: &mut dyn std::io::Write, value: &impl serde::Serialize, compact: bool) {
    let text = if compact { serde_json::to_string(value) } else { serde_json::to_string_pretty(value) };
    outln!(out, "{}", text.unwrap_or_default());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compact_is_one_line_and_pretty_is_the_same_value_over_several() {
        let v = json!({"a": [1, 2], "b": {"c": "d"}});
        let (mut pretty, mut compact) = (Vec::new(), Vec::new());
        print_json(&mut pretty, &v, false);
        print_json(&mut compact, &v, true);
        let (pretty, compact) = (String::from_utf8(pretty).unwrap_or_default(), String::from_utf8(compact).unwrap_or_default());
        assert!(pretty.trim_end().lines().count() > 1);
        assert_eq!(compact, "{\"a\":[1,2],\"b\":{\"c\":\"d\"}}\n");
        assert_eq!(serde_json::from_str::<serde_json::Value>(&pretty).ok(), Some(v));
    }
}
