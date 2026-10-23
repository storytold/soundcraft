//! Fonts: egui's bundled defaults, plus the operating system's own UI fonts loaded at runtime when
//! present (never redistributed). A "bold" family is always defined so the UI can use it.

use egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

/// (path, ttc index) candidates for regular and bold UI text, by platform.
const REGULAR: &[(&str, u32)] = &[
    ("/System/Library/Fonts/HelveticaNeue.ttc", 0),
    ("/System/Library/Fonts/Helvetica.ttc", 0),
    ("C:\\Windows\\Fonts\\segoeui.ttf", 0),
    ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 0),
    ("/usr/share/fonts/TTF/DejaVuSans.ttf", 0),
    ("/usr/share/fonts/dejavu/DejaVuSans.ttf", 0),
    ("/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf", 0),
    ("/usr/local/share/fonts/dejavu/DejaVuSans.ttf", 0),
    ("/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf", 0),
    ("/usr/share/fonts/liberation-sans-fonts/LiberationSans-Regular.ttf", 0),
];
const BOLD: &[(&str, u32)] = &[
    ("/System/Library/Fonts/HelveticaNeue.ttc", 1),
    ("/System/Library/Fonts/Helvetica.ttc", 1),
    ("C:\\Windows\\Fonts\\segoeuib.ttf", 0),
    ("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", 0),
    ("/usr/share/fonts/TTF/DejaVuSans-Bold.ttf", 0),
    ("/usr/share/fonts/dejavu/DejaVuSans-Bold.ttf", 0),
    ("/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf", 0),
    ("/usr/local/share/fonts/dejavu/DejaVuSans-Bold.ttf", 0),
    ("/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf", 0),
    ("/usr/share/fonts/liberation-sans-fonts/LiberationSans-Bold.ttf", 0),
];
/// Hangul fallback, appended after every family so Korean session, track and clip names draw
/// whatever the interface language. The Noto Sans CJK collection's face 1 is its Korean face.
const HANGUL: &[(&str, u32)] = &[
    ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 0),
    ("/System/Library/Fonts/Supplemental/AppleGothic.ttf", 0),
    ("C:\\Windows\\Fonts\\malgun.ttf", 0),
    ("C:\\Windows\\Fonts\\gulim.ttc", 0),
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 1),
    ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 1),
    ("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", 1),
    ("/usr/share/fonts/truetype/nanum/NanumGothic.ttf", 0),
    ("/usr/share/fonts/naver-nanum/NanumGothic.ttf", 0),
];
/// Han fallback (Simplified Chinese first), appended after every family so a Chinese interface and
/// Chinese session, track and clip names draw instead of boxes. Face 2 of the Noto Sans CJK
/// collection is its Simplified Chinese face (face 0 is Japanese, 1 Korean), and `Source Han Sans
/// SC` / `Noto Sans CJK SC` ship as single-face files.
const HAN: &[(&str, u32)] = &[
    ("/System/Library/Fonts/PingFang.ttc", 0),
    ("/System/Library/Fonts/Supplemental/Songti.ttc", 0),
    ("/System/Library/Fonts/STHeiti Light.ttc", 0),
    ("C:\\Windows\\Fonts\\msyh.ttc", 0),
    ("C:\\Windows\\Fonts\\msyh.ttf", 0),
    ("C:\\Windows\\Fonts\\simhei.ttf", 0),
    ("/usr/share/fonts/opentype/source-han-cjk/SourceHanSansSC-Regular.otf", 0),
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 2),
    ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 2),
    ("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", 2),
    ("/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf", 0),
    ("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc", 0),
    ("/usr/share/fonts/wqy-microhei/wqy-microhei.ttc", 0),
    ("/usr/share/fonts/truetype/arphic/uming.ttc", 0),
];
/// Font files larger than this are skipped: egui keeps the whole file in memory.
#[cfg(not(target_arch = "wasm32"))]
const MAX_FONT_BYTES: u64 = 64 << 20;
/// A resolved font file: path and face index inside it.
type Resolved = Option<(String, u32)>;

/// One cache per family. `fc-match` is a subprocess per call, and on Linux a child of this
/// process keeps copies of its open files until it execs — which, in tests, can make a
/// just-released autosave lock look taken. Resolution therefore happens once per process.
static REGULAR_CACHE: std::sync::OnceLock<Resolved> = std::sync::OnceLock::new();
static BOLD_CACHE: std::sync::OnceLock<Resolved> = std::sync::OnceLock::new();
static HANGUL_CACHE: std::sync::OnceLock<Resolved> = std::sync::OnceLock::new();
static HAN_CACHE: std::sync::OnceLock<Resolved> = std::sync::OnceLock::new();

/// Is this a TrueType/OpenType file egui can read, and small enough to hold in memory? Type 1
/// and bitmap fonts, which `fc-match` may return on minimal systems, are rejected here.
#[cfg(not(target_arch = "wasm32"))]
fn acceptable(path: &str) -> bool {
    use std::io::Read;
    if !std::fs::metadata(path).is_ok_and(|m| m.len() <= MAX_FONT_BYTES) {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(path) else { return false };
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).is_ok() && matches!(&magic, b"\0\x01\0\0" | b"OTTO" | b"true" | b"ttcf")
}

/// Asks fontconfig for the desktop's font matching `pattern`. With `lang`, the match must support
/// that language (`fc-match` always returns its best match, which is a Latin face when no Korean
/// or Han font is installed).
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn fontconfig_match(pattern: &str, lang: Option<&str>) -> Option<(String, u32)> {
    let output = std::process::Command::new("fc-match").args(["--format=%{file}\\n%{index}\\n%{lang}\\n", pattern]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let mut lines = stdout.lines();
    let path = lines.next()?.trim();
    // The upper 16 bits carry a variable font's named instance; the face index is the lower 16.
    let index = lines.next()?.trim().parse::<u32>().ok()? & 0xFFFF;
    if let Some(lang) = lang
        && !lines.next().unwrap_or_default().split('|').any(|l| l.trim() == lang)
    {
        return None;
    }
    acceptable(path).then(|| (path.to_string(), index))
}

/// The file for one family, resolved once: fontconfig's answer first, then the built-in list.
#[cfg(not(target_arch = "wasm32"))]
fn resolve(cache: &'static std::sync::OnceLock<Resolved>, cands: &[(&str, u32)], pattern: &str, lang: Option<&str>) -> Resolved {
    cache
        .get_or_init(|| {
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            if let Some(found) = fontconfig_match(pattern, lang) {
                return Some(found);
            }
            #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
            let _ = (pattern, lang);
            cands.iter().find_map(|(p, idx)| acceptable(p).then(|| ((*p).to_string(), *idx)))
        })
        .clone()
}

#[cfg(not(target_arch = "wasm32"))]
fn load(cache: &'static std::sync::OnceLock<Resolved>, cands: &[(&str, u32)], pattern: &str, lang: Option<&str>) -> Option<FontData> {
    if std::env::var_os("SOUNDCRAFT_NO_SYSTEM_FONTS").is_some() {
        return None;
    }
    let (path, index) = resolve(cache, cands, pattern, lang)?;
    let mut font = FontData::from_owned(std::fs::read(path).ok()?);
    font.index = index;
    Some(font)
}

#[cfg(target_arch = "wasm32")]
fn load(_: &'static std::sync::OnceLock<Resolved>, _: &[(&str, u32)], _: &str, _: Option<&str>) -> Option<FontData> {
    None
}

pub fn definitions() -> FontDefinitions {
    definitions_for(false)
}

/// The fonts for an interface language: Han ahead of Hangul only for Simplified Chinese.
pub fn definitions_for(han_first: bool) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let base: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut bold_family = base.clone();
    if let Some(reg) = load(&REGULAR_CACHE, REGULAR, "sans-serif", None) {
        fonts.font_data.insert("system-regular".into(), Arc::new(reg));
        if let Some(f) = fonts.families.get_mut(&FontFamily::Proportional) {
            f.insert(0, "system-regular".into());
        }
    }
    if let Some(b) = load(&BOLD_CACHE, BOLD, "sans-serif:style=Bold", None) {
        fonts.font_data.insert("system-bold".into(), Arc::new(b));
        bold_family.insert(0, "system-bold".into());
    } else if fonts.font_data.contains_key("system-regular") {
        bold_family.insert(0, "system-regular".into());
    }
    fonts.families.insert(FontFamily::Name("bold".into()), bold_family);
    let hangul = load(&HANGUL_CACHE, HANGUL, "sans-serif:lang=ko", Some("ko")).map(|h| ("system-hangul", h));
    let han = load(&HAN_CACHE, HAN, "sans-serif:lang=zh-cn", Some("zh-cn")).map(|h| ("system-han", h));
    // Last in every family: Latin text keeps the system face and CJK glyphs fall through. Noto Sans
    // CJK's Korean face also covers Han, so Hangul goes first unless the interface is Chinese.
    let ordered = if han_first { [han, hangul] } else { [hangul, han] };
    for (key, data) in ordered.into_iter().flatten() {
        fonts.font_data.insert(key.into(), Arc::new(data));
        for family in fonts.families.values_mut() {
            family.push(key.into());
        }
    }
    fonts
}

pub fn install(ctx: &egui::Context, lang_code: &str) {
    ctx.set_fonts(definitions_for(lang_code == "zh-Hans"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hangul_candidates_cover_every_desktop_platform() {
        for prefix in ["/System/Library/Fonts/", "C:\\Windows\\Fonts\\", "/usr/share/fonts/"] {
            assert!(HANGUL.iter().any(|(p, _)| p.starts_with(prefix)), "no Hangul candidate under {prefix}");
            assert!(HAN.iter().any(|(p, _)| p.starts_with(prefix)), "no Han candidate under {prefix}");
        }
        // Face 0 of the Noto Sans CJK collection is Japanese, the Korean face is 1, Simplified
        // Chinese is 2.
        assert!(HANGUL.iter().filter(|(p, _)| p.ends_with("NotoSansCJK-Regular.ttc")).all(|(_, i)| *i == 1));
        assert!(HAN.iter().filter(|(p, _)| p.ends_with("NotoSansCJK-Regular.ttc")).all(|(_, i)| *i == 2));
    }

    /// Korean and Han names draw in every family, not as boxes; skipped when the system has no
    /// such font.
    #[test]
    fn every_family_draws_hangul_and_han_with_system_fonts() {
        let defs = definitions();
        for (key, sample) in [("system-hangul", "가한국어세션"), ("system-han", "轨道片段混音")] {
            if !defs.font_data.contains_key(key) {
                eprintln!("skipping: no system font for {key}");
                continue;
            }
            let families: Vec<FontFamily> = defs.families.keys().cloned().collect();
            let ctx = egui::Context::default();
            ctx.set_fonts(defs.clone());
            let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
            output.textures_delta.clear();
            for family in families {
                let font = egui::FontId::new(13.0, family.clone());
                assert!(ctx.fonts_mut(|f| f.has_glyphs(&font, sample)), "{family:?} has no {key} glyphs");
            }
        }
    }
}
