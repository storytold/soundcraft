//! Splits a JSFX file into its header (`desc:`, `sliderN:`, pins, options) and code sections.

use crate::JsfxError;
use crate::parse::MAX_SLIDERS;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SliderDef {
    /// 1-based slider number.
    pub index: usize,
    /// `slider1:amp=1<...>` gives the slider a second name.
    pub var: Option<String>,
    pub default: f64,
    pub min: f64,
    pub max: f64,
    pub label: String,
    pub choices: Vec<String>,
    pub log: bool,
    pub hidden: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Section {
    pub code: String,
    /// File line of the first code line.
    pub line: u32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct JsfxSource {
    pub desc: String,
    pub sliders: Vec<SliderDef>,
    pub in_pins: usize,
    pub out_pins: usize,
    /// `in_pin:none`: a generator.
    pub no_input: bool,
    pub maxmem: Option<usize>,
    pub init: Section,
    pub slider: Section,
    pub block: Section,
    pub sample: Section,
    /// Sections present in the file that this host skips (`serialize`, `gfx`, ...).
    pub ignored: Vec<String>,
    /// `import <file>` lines, in order.
    pub imports: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Cur {
    Header,
    Init,
    Slider,
    Block,
    Sample,
    Skip,
}

fn is_ident(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|f| f.is_ascii_alphabetic() || f == '_') && c.all(|x| x.is_ascii_alphanumeric() || x == '_' || x == '.')
}

fn nums(s: &str) -> Vec<f64> {
    s.split(',').filter_map(|p| p.trim().parse::<f64>().ok()).collect()
}

fn parse_slider(index: usize, rest: &str) -> Option<SliderDef> {
    let rest = rest.trim();
    if rest.starts_with('/') {
        return None; // file-picker sliders have no numeric value
    }
    let (var, rest) = match rest.split_once('=') {
        Some((v, r)) if is_ident(v.trim()) => (Some(v.trim().to_ascii_lowercase()), r),
        _ => (None, rest),
    };
    let rest = rest.trim_start();
    let end = rest.find(['<', ',']).unwrap_or(rest.len());
    // An empty default (`name=<0,1>`) is 0, as in REAPER.
    let text = rest.get(..end)?.trim();
    let default = if text.is_empty() { 0.0 } else { text.parse::<f64>().ok()? };
    let tail = rest.get(end..).unwrap_or("");
    let (inner, label) = if let Some(t) = tail.strip_prefix('<') {
        let (inner, label) = t.split_once('>').unwrap_or((t, ""));
        (inner, label)
    } else {
        ("", tail.strip_prefix(',').unwrap_or(tail))
    };
    let (spec, choices) = match inner.split_once('{') {
        Some((a, b)) => (a, b.split('}').next().unwrap_or("").split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()),
        None => (inner, Vec::new()),
    };
    let mut parts = spec.split(':');
    let range = nums(parts.next().unwrap_or(""));
    let log = parts.any(|p| p.trim().to_ascii_lowercase().starts_with("log"));
    let (mut min, mut max) = (range.first().copied().unwrap_or(0.0), range.get(1).copied().unwrap_or(1.0));
    if !choices.is_empty() {
        min = range.first().copied().unwrap_or(0.0);
        max = if range.len() > 1 { max } else { min + choices.len().saturating_sub(1) as f64 };
    }
    if max < min {
        std::mem::swap(&mut min, &mut max);
    }
    let label = label.trim();
    let (hidden, label) = match label.strip_prefix('-') {
        Some(l) => (true, l.trim()),
        None => (false, label),
    };
    let default = default.clamp(min, max);
    Some(SliderDef { index, var, default, min, max, label: label.to_string(), choices, log, hidden })
}

pub(crate) fn parse_source(text: &str) -> Result<JsfxSource, JsfxError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.contains('\0') {
        return Err(JsfxError::Syntax("not a text file".into()));
    }
    let mut src = JsfxSource { in_pins: 2, out_pins: 2, ..Default::default() };
    let mut cur = Cur::Header;
    let (mut in_set, mut out_set) = (false, false);
    // Inside a `/* */` comment in the header (libraries show usage examples there, with `import`
    // and `slider` lines that must not count).
    let mut in_comment = false;
    for (n, raw) in text.lines().enumerate() {
        let line_no = n as u32 + 1;
        if let Some(name) = raw.strip_prefix('@').filter(|_| !(in_comment && matches!(cur, Cur::Header))) {
            let name = name.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
            cur = match name.as_str() {
                "init" => Cur::Init,
                "slider" => Cur::Slider,
                "block" => Cur::Block,
                "sample" => Cur::Sample,
                other => {
                    src.ignored.push(other.to_string());
                    Cur::Skip
                }
            };
            let sec = match cur {
                Cur::Init => Some(&mut src.init),
                Cur::Slider => Some(&mut src.slider),
                Cur::Block => Some(&mut src.block),
                Cur::Sample => Some(&mut src.sample),
                _ => None,
            };
            if let Some(s) = sec
                && s.code.is_empty()
            {
                s.line = line_no + 1;
            }
            continue;
        }
        let sec = match cur {
            Cur::Header => {
                let visible = outside_comments(raw, &mut in_comment);
                if !visible.trim().is_empty() {
                    header_line(&mut src, &visible, &mut in_set, &mut out_set);
                }
                continue;
            }
            Cur::Init => &mut src.init,
            Cur::Slider => &mut src.slider,
            Cur::Block => &mut src.block,
            Cur::Sample => &mut src.sample,
            Cur::Skip => continue,
        };
        sec.code.push_str(raw);
        sec.code.push('\n');
    }
    Ok(src)
}

/// The parts of a header line outside `/* */` comments, which may span lines.
fn outside_comments(line: &str, in_comment: &mut bool) -> String {
    let mut out = String::new();
    let mut rest = line;
    loop {
        if *in_comment {
            match rest.find("*/") {
                Some(i) => {
                    *in_comment = false;
                    rest = &rest[i + 2..];
                }
                None => return out,
            }
        } else {
            // A `//` comment hides any `/*` after it (`//****` lines are common).
            let line_comment = rest.find("//");
            match rest.find("/*").filter(|i| line_comment.is_none_or(|l| l > *i)) {
                Some(i) => {
                    out.push_str(&rest[..i]);
                    *in_comment = true;
                    rest = &rest[i + 2..];
                }
                None => {
                    out.push_str(rest);
                    return out;
                }
            }
        }
    }
}

fn header_line(src: &mut JsfxSource, raw: &str, in_set: &mut bool, out_set: &mut bool) {
    let line = raw.trim();
    if let Some(name) = line.strip_prefix("import ") {
        let name = name.trim();
        if !name.is_empty() {
            src.imports.push(name.to_string());
        }
        return;
    }
    let Some((key, value)) = line.split_once(':') else { return };
    let key = key.trim().to_ascii_lowercase();
    let value = value.trim();
    if let Some(n) = key.strip_prefix("slider").and_then(|n| n.parse::<usize>().ok()) {
        if (1..=MAX_SLIDERS).contains(&n)
            && let Some(def) = parse_slider(n, value)
        {
            src.sliders.retain(|s| s.index != n);
            src.sliders.push(def);
        }
        return;
    }
    match key.as_str() {
        "desc" => src.desc = value.to_string(),
        "in_pin" => {
            if value.eq_ignore_ascii_case("none") {
                src.no_input = true;
                src.in_pins = 0;
            } else {
                src.in_pins = if *in_set { src.in_pins + 1 } else { 1 };
                *in_set = true;
            }
        }
        "out_pin" => {
            if value.eq_ignore_ascii_case("none") {
                src.out_pins = 0;
            } else {
                src.out_pins = if *out_set { src.out_pins + 1 } else { 1 };
                *out_set = true;
            }
        }
        "options" => {
            for opt in value.split_whitespace() {
                if let Some(v) = opt.strip_prefix("maxmem=") {
                    src.maxmem = v.parse().ok();
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_comments_hide_their_lines() {
        let s = parse_source("desc:lib\n//****\n/* Example\n   import other.jsfx-inc\nslider1:1<0,1>No\n*/ import real.jsfx-inc\n@init\nx = 1;\n")
            .unwrap();
        assert_eq!(s.imports, vec!["real.jsfx-inc".to_string()]);
        assert!(s.sliders.is_empty());
    }

    #[test]
    fn slider_forms() {
        let s = parse_slider(1, "0<-60,12,0.1>Gain (dB)").unwrap();
        assert_eq!((s.default, s.min, s.max, s.label.as_str()), (0.0, -60.0, 12.0, "Gain (dB)"));
        let s = parse_slider(2, "amp=1<0,2,0.01>Amp").unwrap();
        assert_eq!((s.var.as_deref(), s.default), (Some("amp"), 1.0));
        let s = parse_slider(3, "1<0,2,1{Off,On,Both}>Mode").unwrap();
        assert_eq!(s.choices, ["Off", "On", "Both"]);
        let s = parse_slider(4, "1000<20,20000,1:log>Freq").unwrap();
        assert!(s.log);
        let s = parse_slider(5, "0<0,1>-secret").unwrap();
        assert!(s.hidden && s.label == "secret");
        assert!(parse_slider(6, "/presets:default.wav:Sample").is_none());
        let s = parse_slider(7, "mode=<0,2,1{Off,On,Auto}>Mode").unwrap();
        assert_eq!((s.var.as_deref(), s.default), (Some("mode"), 0.0));
    }

    #[test]
    fn splits_header_and_sections() {
        let src = parse_source("desc:Test\nslider1:0<0,1>A\nin_pin:L\nin_pin:R\nout_pin:L\n@init\nx=1;\n@sample\nspl0*=x;\n@gfx 100 100\ngfx_x=0;\n")
            .unwrap();
        assert_eq!((src.desc.as_str(), src.in_pins, src.out_pins, src.sliders.len()), ("Test", 2, 1, 1));
        assert_eq!(src.init.code.trim(), "x=1;");
        assert_eq!(src.sample.code.trim(), "spl0*=x;");
        assert_eq!(src.sample.line, 9);
        assert_eq!(src.ignored, ["gfx"]);
    }

    #[test]
    fn generator_and_binary_files() {
        assert!(parse_source("desc:Gen\nin_pin:none\nout_pin:L\n").unwrap().no_input);
        assert!(parse_source("desc:x\0\0").is_err());
    }
}
