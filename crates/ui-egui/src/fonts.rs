//! Fonts: egui's bundled defaults, plus the operating system's own UI fonts loaded at runtime when
//! present (never redistributed). The web build, which reaches no operating-system fonts, bundles a
//! small Open Font License subset of Noto Sans SC instead (see `assets/fonts/` and
//! `ATTRIBUTION.md`). A "bold" family is always defined so the UI can use it.

use egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

/// (path, ttc index) candidates for regular and bold UI text, by platform.
#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
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
/// Han fallback (Simplified Chinese first), placed before [`HANGUL`] for every interface language
/// except Korean, so a Chinese interface and Chinese session, track and clip names draw in Chinese
/// shapes instead of boxes. On Linux the first resolvable CJK face is often Noto Sans CJK's Korean
/// face, whose Han glyphs are Korean-styled; the first face with a glyph wins, so Han must come
/// first. Face 2 of the Noto Sans CJK collection is its Simplified Chinese face (face 0 is
/// Japanese, 1 Korean), and `Source Han Sans SC` / `Noto Sans CJK SC` ship as single-face files.
#[cfg(not(target_arch = "wasm32"))]
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
/// The Han-capable TrueType font bundled for the web build only: egui's default fonts carry no Han
/// glyphs and a browser cannot reach the operating system's fonts, so a Chinese interface would
/// draw boxes. It is a Noto Sans SC (OFL-1.1) subset at weight 400 covering a superset of the UI
/// glyph set; regenerate it with `docs/i18n/tools/subset_web_font.py`. See `assets/fonts/OFL.txt`
/// and `ATTRIBUTION.md`. Native builds never embed it: the include is compiled out of them
/// (`test` keeps it available to the glyph-coverage tests).
#[cfg(any(target_arch = "wasm32", test))]
const BUNDLED_HAN: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/NotoSansSC-SoundCraftSubset.ttf"));

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

/// The web build has no operating-system fonts: the bundled subset is the CJK fallback.
#[cfg(target_arch = "wasm32")]
fn bundled_han() -> FontData {
    FontData::from_static(BUNDLED_HAN)
}

/// Appends `name` as the lowest-priority fallback of every family. Only appends, so the order the
/// faces are added in is the order glyphs are looked up in.
fn add_fallback(fonts: &mut FontDefinitions, name: &'static str, font: FontData) {
    fonts.font_data.insert(name.into(), Arc::new(font));
    for family in fonts.families.values_mut() {
        family.push(name.into());
    }
}

/// Should a Chinese-shaped Han face claim the Han glyphs before the Korean face? The first face
/// with a glyph wins, so a Chinese interface must come first (on Linux fontconfig often resolves
/// Noto Sans CJK's Korean face, whose Han glyphs are Korean-styled). Every interface language
/// except Korean orders Han first; Korean reverses the two, so Hangul and Hanja draw from the
/// Korean face. A future `zh-Hant` (Traditional Chinese) or `ja` (Japanese) is expected to keep
/// Han first as well; only a language-suitable Han face would change (the [`HAN`] list is
/// Simplified-first today).
#[cfg(not(target_arch = "wasm32"))]
fn han_before_hangul(language: &str) -> bool {
    language.split(['-', '_']).next() != Some("ko")
}

/// The font set: egui's bundled defaults, the operating system's faces where present and when
/// `system_fonts` is true, and (web only) the bundled Han face as the last fallback of every
/// family. The interface language decides the order of the two CJK fallbacks.
pub fn definitions(system_fonts: bool) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let base: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    #[cfg(not(target_arch = "wasm32"))]
    let mut bold_family = base.clone();
    #[cfg(not(target_arch = "wasm32"))]
    {
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
        // The first face with a glyph wins, so the interface language decides whether a Han or a
        // Korean face claims the Han glyphs a Chinese interface draws (see `han_before_hangul`).
        let language = crate::localization::current_language().id();
        let hangul = load(HANGUL, "sans-serif:lang=ko", Some("ko"), system_fonts);
        let han = load(HAN, "sans-serif:lang=zh-cn", Some("zh-cn"), system_fonts);
        let ordered = if han_before_hangul(language) {
            [("system-han", han), ("system-hangul", hangul)]
        } else {
            [("system-hangul", hangul), ("system-han", han)]
        };
        for (name, font) in ordered {
            if let Some(font) = font {
                add_fallback(&mut fonts, name, font);
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        // No system fonts in the browser: the bundled Han face keeps a Chinese interface, and
        // Chinese session, track and clip names, readable instead of drawing boxes.
        let _ = system_fonts;
        fonts.families.insert(FontFamily::Name("bold".into()), base);
        add_fallback(&mut fonts, "web-han", bundled_han());
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

    /// The interface language orders the two CJK fallbacks: a Chinese interface draws Han glyphs
    /// from the Han face first, Korean draws them from the Korean face.
    #[test]
    fn cjk_fallback_order_follows_the_interface_language() {
        assert!(han_before_hangul("zh-Hans"));
        assert!(han_before_hangul("en"));
        // A future Traditional Chinese or Japanese catalog keeps Han first; only the Han face
        // itself would become Traditional Chinese or Japanese.
        assert!(han_before_hangul("zh-Hant"));
        assert!(han_before_hangul("ja"));
        assert!(!han_before_hangul("ko"));
        assert!(!han_before_hangul("ko-kr"));
    }

    /// Chinese names draw in every family, not as boxes; skipped when the system has no Han font.
    #[test]
    fn every_family_draws_han_with_a_system_han_font() {
        let defs = definitions(true);
        if !defs.font_data.contains_key("system-han") {
            eprintln!("skipping: no Han system font");
            return;
        }
        let families: Vec<FontFamily> = defs.families.keys().cloned().collect();
        let ctx = egui::Context::default();
        ctx.set_fonts(defs);
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
        for family in families {
            let font = egui::FontId::new(13.0, family.clone());
            assert!(ctx.fonts_mut(|f| f.has_glyphs(&font, "轨道片段混音")), "{family:?} has no Han");
        }
    }

    /// Under a Chinese interface every family that resolved both faces puts Han before Hangul, so
    /// Chinese text never draws in Korean-styled shapes.
    #[test]
    fn chinese_interface_puts_han_before_hangul_in_every_family() {
        let _language = crate::localization::enter(crate::localization::Language::SimplifiedChinese);
        let defs = definitions(true);
        let mut compared = 0;
        for (family, family_fonts) in &defs.families {
            let han = family_fonts.iter().position(|font| font == "system-han");
            let hangul = family_fonts.iter().position(|font| font == "system-hangul");
            if let (Some(han), Some(hangul)) = (han, hangul) {
                assert!(han < hangul, "{family:?} puts Hangul before Han: {family_fonts:?}");
                compared += 1;
            }
        }
        if compared == 0 {
            eprintln!("skipping: no system Han and Hangul fonts resolved");
        }
    }

    /// The web fallback chain (egui defaults + the bundled face) draws the entire zh-Hans catalog in
    /// every family: the property that keeps the browser build free of boxes.
    #[test]
    fn bundled_web_font_draws_the_zh_hans_catalog_in_every_family() {
        let Some(catalog) = zh_hans_catalog() else { return };
        let mut defs = definitions(false);
        add_fallback(&mut defs, "web-han", FontData::from_static(BUNDLED_HAN));
        let mut sample: Vec<char> = catalog.chars().filter(|c| !['\t', '\r', '\n'].contains(c)).collect();
        sample.sort_unstable();
        sample.dedup();
        let sample: String = sample.into_iter().collect();
        let families: Vec<FontFamily> = defs.families.keys().cloned().collect();
        let ctx = egui::Context::default();
        ctx.set_fonts(defs);
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
        for family in families {
            let font = egui::FontId::new(13.0, family.clone());
            let missing: Vec<char> = sample.chars().filter(|c| !ctx.fonts_mut(|fonts| fonts.has_glyphs(&font, &c.to_string()))).collect();
            assert!(missing.is_empty(), "{family:?} cannot draw {missing:?} of the zh-Hans catalog even with the bundled web font");
        }
    }

    /// The asset must not silently grow into a full CJK font, which would bloat the web download.
    #[test]
    fn bundled_web_font_stays_within_its_size_budget() {
        const BUDGET: usize = 2 << 20; // 2 MiB
        assert!(BUNDLED_HAN.len() <= BUDGET, "the bundled web font is {} bytes; its budget is {BUDGET}", BUNDLED_HAN.len());
    }

    /// The source of the zh-Hans catalog, or `None` (skip) when it is not checked out.
    fn zh_hans_catalog() -> Option<String> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/i18n/zh-hans.tsv");
        match std::fs::read_to_string(path) {
            Ok(catalog) => Some(catalog),
            Err(err) => {
                eprintln!("skipping: cannot read {path}: {err}");
                None
            }
        }
    }
}
