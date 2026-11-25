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
/// Japanese fallback (kana and Japanese Han forms), appended after every family so Japanese
/// session, track, clip and marker names draw whatever the interface language. CJK is
/// Japanese-first: the Chinese face below covers what this one lacks.
const JAPANESE: &[(&str, u32)] = &[
    ("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc", 0),
    ("/System/Library/Fonts/AquaKana.ttc", 0),
    ("C:\\Windows\\Fonts\\YuGothM.ttc", 0),
    ("C:\\Windows\\Fonts\\meiryo.ttc", 0),
    ("C:\\Windows\\Fonts\\msgothic.ttc", 0),
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
    ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 0),
    ("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", 0),
    ("/usr/share/fonts/opentype/noto/NotoSansJP-Regular.otf", 0),
    ("/usr/share/fonts/truetype/noto/NotoSansJP-Regular.otf", 0),
];
/// Chinese fallback for the Han characters the Japanese face lacks (页, 选, 删…) and for Chinese
/// names in general. It follows the Japanese face, so shared ideographs keep Japanese glyph
/// forms; a Traditional-Chinese-specific face is not selected yet (GB18030 coverage handles most
/// of its characters). Face 2 of the Noto Sans CJK collection and face 2 of PingFang are their
/// Simplified Chinese faces; PingFang's face 0 is the Hong Kong one.
const CHINESE: &[(&str, u32)] = &[
    ("/System/Library/Fonts/PingFang.ttc", 2),
    ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
    ("/System/Library/Fonts/STHeiti Light.ttc", 0),
    ("C:\\Windows\\Fonts\\msyh.ttc", 0),
    ("C:\\Windows\\Fonts\\simsun.ttc", 0),
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 2),
    ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 2),
    ("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", 2),
    ("/usr/share/fonts/opentype/noto/NotoSansSC-Regular.otf", 0),
    ("/usr/share/fonts/truetype/noto/NotoSansSC-Regular.otf", 0),
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
/// Font files larger than this are skipped: egui keeps the whole file in memory.
#[cfg(not(target_arch = "wasm32"))]
const MAX_FONT_BYTES: u64 = 64 << 20;

/// Asks fontconfig for the desktop's font matching `pattern`. Only TrueType/OpenType results are
/// used: egui cannot read Type 1 or bitmap fonts, which `fc-match` may return on minimal systems.
/// With `lang`, the match must support that language (`fc-match` always returns its best match,
/// which is a Latin face when no Korean font is installed).
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn load_from_fontconfig(pattern: &str, lang: Option<&str>) -> Option<FontData> {
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
    if std::fs::metadata(path).ok()?.len() > MAX_FONT_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let sfnt = matches!(bytes.get(..4), Some(b"\0\x01\0\0" | b"OTTO" | b"true" | b"ttcf"));
    if !sfnt {
        return None;
    }
    let mut font = FontData::from_owned(bytes);
    font.index = index;
    Some(font)
}

#[cfg(not(target_arch = "wasm32"))]
fn load(cands: &[(&str, u32)], pattern: &str, lang: Option<&str>, system_fonts: bool) -> Option<FontData> {
    if !system_fonts {
        return None;
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    if let Some(font) = load_from_fontconfig(pattern, lang) {
        return Some(font);
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    let _ = (pattern, lang);
    for (p, idx) in cands {
        if std::fs::metadata(p).is_ok_and(|m| m.len() > MAX_FONT_BYTES) {
            continue;
        }
        if let Ok(bytes) = std::fs::read(p) {
            let mut fd = FontData::from_owned(bytes);
            fd.index = *idx;
            return Some(fd);
        }
    }
    None
}

#[cfg(target_arch = "wasm32")]
fn load(_: &[(&str, u32)], _: &str, _: Option<&str>, _: bool) -> Option<FontData> {
    None
}

/// The font set: the bundled faces, plus the system ones when `system_fonts` is true.
pub fn definitions(system_fonts: bool) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let base: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut bold_family = base.clone();
    if let Some(reg) = load(REGULAR, "sans-serif", None, system_fonts) {
        fonts.font_data.insert("system-regular".into(), Arc::new(reg));
        if let Some(f) = fonts.families.get_mut(&FontFamily::Proportional) {
            f.insert(0, "system-regular".into());
        }
    }
    if let Some(b) = load(BOLD, "sans-serif:style=Bold", None, system_fonts) {
        fonts.font_data.insert("system-bold".into(), Arc::new(b));
        bold_family.insert(0, "system-bold".into());
    } else if fonts.font_data.contains_key("system-regular") {
        bold_family.insert(0, "system-regular".into());
    }
    fonts.families.insert(FontFamily::Name("bold".into()), bold_family);
    // CJK first: Japanese, then Chinese for the characters the Japanese face lacks, then Korean.
    // Every family gets them in that order, so a name draws in the script's own face and shared
    // Han ideographs keep Japanese glyph forms.
    for (name, candidates, pattern, lang) in [
        ("system-japanese", JAPANESE, "sans-serif:lang=ja", Some("ja")),
        ("system-chinese", CHINESE, "sans-serif:lang=zh-cn", Some("zh-cn")),
        ("system-hangul", HANGUL, "sans-serif:lang=ko", Some("ko")),
    ] {
        if let Some(f) = load(candidates, pattern, lang, system_fonts) {
            fonts.font_data.insert(name.into(), Arc::new(f));
            for family in fonts.families.values_mut() {
                family.push(name.into());
            }
        }
    }
    fonts
}

pub fn install(ctx: &egui::Context, system_fonts: bool) {
    ctx.set_fonts(definitions(system_fonts));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hangul_candidates_cover_every_desktop_platform() {
        for prefix in ["/System/Library/Fonts/", "C:\\Windows\\Fonts\\", "/usr/share/fonts/"] {
            assert!(HANGUL.iter().any(|(p, _)| p.starts_with(prefix)), "no Hangul candidate under {prefix}");
        }
        // Face 0 of the Noto Sans CJK collection is Japanese; the Korean face is 1.
        assert!(HANGUL.iter().filter(|(p, _)| p.ends_with("NotoSansCJK-Regular.ttc")).all(|(_, i)| *i == 1));
    }

    #[test]
    fn cjk_candidates_cover_every_desktop_platform() {
        for prefix in ["/System/Library/Fonts/", "C:\\Windows\\Fonts\\", "/usr/share/fonts/"] {
            assert!(JAPANESE.iter().any(|(p, _)| p.starts_with(prefix)), "no Japanese candidate under {prefix}");
            assert!(CHINESE.iter().any(|(p, _)| p.starts_with(prefix)), "no Chinese candidate under {prefix}");
        }
        // Face 0 of the Noto Sans CJK collection is Japanese, face 2 Simplified Chinese. PingFang's
        // collection is region-major: face 0 is Hong Kong Regular and face 2 Simplified Chinese.
        assert!(JAPANESE.iter().filter(|(p, _)| p.ends_with("NotoSansCJK-Regular.ttc")).all(|(_, i)| *i == 0));
        assert!(CHINESE.iter().filter(|(p, _)| p.ends_with("NotoSansCJK-Regular.ttc")).all(|(_, i)| *i == 2));
        assert!(CHINESE.iter().filter(|(p, _)| p.ends_with("PingFang.ttc")).all(|(_, i)| *i == 2));
    }

    /// Japanese and Simplified Chinese names draw in every family, not as boxes: the fallbacks are
    /// appended after every family, Japanese first so shared Han keeps Japanese glyph forms and the
    /// Chinese face answers the characters the Japanese one lacks (页, 选). Skipped where the
    /// system has no such font; the Hangul test below covers Korean unchanged.
    #[test]
    fn every_family_draws_japanese_and_chinese_with_system_fonts() {
        let defs = definitions(true);
        let (has_japanese, has_chinese) = (defs.font_data.contains_key("system-japanese"), defs.font_data.contains_key("system-chinese"));
        if !has_japanese && !has_chinese {
            eprintln!("skipping: no Japanese or Chinese system font");
            return;
        }
        // Japanese before Chinese before Korean, and all after the base UI fonts.
        let stack = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
        let at = |name: &str| stack.iter().position(|n| n == name);
        if let (Some(base), Some(ja)) = (at("system-regular"), at("system-japanese")) {
            assert!(base < ja, "the base UI font stays first: {stack:?}");
        }
        if has_japanese && has_chinese {
            assert!(at("system-japanese") < at("system-chinese"), "Japanese first: {stack:?}");
        }
        if has_chinese && defs.font_data.contains_key("system-hangul") {
            assert!(at("system-chinese") < at("system-hangul"), "Chinese before Korean: {stack:?}");
        }
        let families: Vec<FontFamily> = defs.families.keys().cloned().collect();
        let ctx = egui::Context::default();
        ctx.set_fonts(defs);
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
        for family in families {
            let font = egui::FontId::new(13.0, family.clone());
            if has_japanese {
                assert!(ctx.fonts_mut(|f| f.has_glyphs(&font, "あア漢字日本語セッション")), "{family:?} has no Japanese");
            }
            if has_chinese {
                assert!(ctx.fonts_mut(|f| f.has_glyphs(&font, "页选汉字测试")), "{family:?} has no Chinese");
            }
        }
    }

    /// Every literal glyph the UI fixtures paint is covered by the bundled faces in every family,
    /// without relying on an installed system font. This is the 0.37 font-feature regression test.
    #[test]
    fn bundled_faces_cover_the_ui_glyphs_in_every_family() {
        let mut defs = FontDefinitions::default();
        let proportional = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
        defs.families.insert(FontFamily::Name("bold".into()), proportional);
        let ctx = egui::Context::default();
        ctx.set_fonts(defs);
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
        // Status/menu text, transport/meter symbols, file/audio/search icons, music symbols and
        // platform key hints used by the UI. Close/delete controls paint a cross in `icons.rs`
        // instead of typesetting U+2715, which no bundled face covers in every family.
        let glyphs = ["›", "v", "♩", "𝄞", "⌘", "📁", "🔊", "🔍", "⬆", "−", "∞"];
        for family in [FontFamily::Proportional, FontFamily::Monospace, FontFamily::Name("bold".into())] {
            let font = egui::FontId::new(13.0, family.clone());
            for glyph in glyphs {
                assert!(ctx.fonts_mut(|f| f.has_glyphs(&font, glyph)), "{family:?} has no glyph for {glyph:?}");
            }
        }
    }

    /// Korean names draw in every family, not as boxes; skipped when the system has no Korean font.
    #[test]
    fn every_family_draws_hangul_with_a_system_korean_font() {
        let defs = definitions(true);
        if !defs.font_data.contains_key("system-hangul") {
            eprintln!("skipping: no Korean system font");
            return;
        }
        let families: Vec<FontFamily> = defs.families.keys().cloned().collect();
        let ctx = egui::Context::default();
        ctx.set_fonts(defs);
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
        for family in families {
            let font = egui::FontId::new(13.0, family.clone());
            assert!(ctx.fonts_mut(|f| f.has_glyphs(&font, "가한국어세션")), "{family:?} has no Hangul");
        }
    }
}
