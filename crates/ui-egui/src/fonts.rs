//! Fonts: egui's bundled defaults, plus the operating system's own UI fonts loaded at runtime when
//! present (never redistributed). The web build, which has no operating-system fonts, instead
//! bundles a small Open Font License subset of Noto Sans SC (see `assets/fonts/` and
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
/// whatever the interface language. It follows the [`HAN`] face for Chinese and English interfaces
/// and precedes it for Korean; see [`han_before_hangul`]. The Noto Sans CJK collection's face 1 is
/// its Korean face.
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
/// A resolved font file: path and face index inside it.
#[cfg(not(target_arch = "wasm32"))]
type Resolved = Option<(String, u32)>;

/// One cache per family. `fc-match` is a subprocess per call, and on Linux a child of this
/// process keeps copies of its open files until it execs — which, in tests, can make a
/// just-released autosave lock look taken. Resolution therefore happens once per process.
#[cfg(not(target_arch = "wasm32"))]
static REGULAR_CACHE: std::sync::OnceLock<Resolved> = std::sync::OnceLock::new();
#[cfg(not(target_arch = "wasm32"))]
static BOLD_CACHE: std::sync::OnceLock<Resolved> = std::sync::OnceLock::new();
#[cfg(not(target_arch = "wasm32"))]
static HANGUL_CACHE: std::sync::OnceLock<Resolved> = std::sync::OnceLock::new();
#[cfg(not(target_arch = "wasm32"))]
static HAN_CACHE: std::sync::OnceLock<Resolved> = std::sync::OnceLock::new();

/// The Han-capable TrueType font bundled for the web build only: egui's default fonts carry no Han
/// glyphs and a browser cannot reach the operating system's fonts, so Simplified Chinese labels
/// would render as boxes. It is a Noto Sans SC (OFL-1.1) subset at weight 400 covering the UI
/// glyph set; see `assets/fonts/OFL.txt` and `ATTRIBUTION.md`. Native builds never embed it: the
/// include is compiled out of them (`test` keeps it available to the glyph-coverage tests).
#[cfg(any(target_arch = "wasm32", test))]
const BUNDLED_HAN: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/NotoSansSC-SoundCraftSubset.ttf"));

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

/// The web build has no operating-system fonts: the bundled subset is the CJK fallback.
#[cfg(target_arch = "wasm32")]
fn load() -> FontData {
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
/// Korean face. A future `zh-hant` (Traditional Chinese) or `ja` (Japanese) is expected to keep
/// Han first as well — only a language-suitable Han face would change (the [`HAN`] list is
/// Simplified-first today).
#[cfg(not(target_arch = "wasm32"))]
fn han_before_hangul(language: &str) -> bool {
    language.split(['-', '_']).next() != Some("ko")
}

/// The font definitions the UI runs with: egui's defaults, the operating system's faces where
/// present, and (web only) the bundled Han face as the last fallback of every family. The interface
/// language decides the order of the two CJK fallbacks.
pub fn definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let base: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    #[cfg(not(target_arch = "wasm32"))]
    let mut bold_family = base.clone();
    #[cfg(target_arch = "wasm32")]
    let bold_family = base.clone();
    #[cfg(not(target_arch = "wasm32"))]
    {
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
    }
    fonts.families.insert(FontFamily::Name("bold".into()), bold_family);
    #[cfg(not(target_arch = "wasm32"))]
    {
        // The first face with a glyph wins, so the interface language decides whether a Han or a
        // Korean face claims the Han glyphs a Chinese interface draws (see `han_before_hangul`).
        let language = crate::i18n::current().code();
        let hangul = load(&HANGUL_CACHE, HANGUL, "sans-serif:lang=ko", Some("ko"));
        let han = load(&HAN_CACHE, HAN, "sans-serif:lang=zh-cn", Some("zh-cn"));
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
        // No system fonts in the browser: the bundled Han face keeps Chinese labels (and Chinese
        // session, track and clip names) readable instead of drawing boxes.
        add_fallback(&mut fonts, "web-han", load());
    }
    fonts
}

pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions());
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

    /// The interface language orders the two CJK fallbacks: Chinese and English draw Han glyphs
    /// from the Han face first, Korean draws them from the Korean face.
    #[test]
    fn cjk_fallback_order_follows_the_interface_language() {
        assert!(han_before_hangul("zh-hans"));
        assert!(han_before_hangul("en"));
        // Expectations for a future Traditional Chinese / Japanese catalog: Han stays first, only
        // the Han face itself would become Traditional Chinese / Japanese.
        assert!(han_before_hangul("zh-hant"));
        assert!(han_before_hangul("ja"));
        assert!(!han_before_hangul("ko"));
        assert!(!han_before_hangul("ko-kr"));
    }

    /// Under a Chinese interface every family puts `system-han` before `system-hangul`; skipped
    /// when the system has no CJK font at all (CI containers), as the font tests above are.
    #[test]
    fn every_family_puts_han_before_hangul_for_chinese() {
        let Some(chinese) = crate::i18n::Lang::from_code("zh-hans") else {
            eprintln!("skipping: zh-hans is not registered");
            return;
        };
        let _language = crate::i18n::language_scope(chinese);
        let defs = definitions();
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

    /// The bundled web font must keep covering every character of the zh-Hans catalog: read the
    /// source TSV (not the deflated shipped copy) so a catalog edit fails this test until the
    /// subset is regenerated. Emoji outside the BMP come from egui's bundled emoji fonts.
    #[test]
    fn bundled_web_font_covers_the_zh_hans_catalog() {
        let Some(catalog) = zh_hans_catalog() else { return };
        let parsed = ttf_parser::Face::parse(BUNDLED_HAN, 0);
        assert!(parsed.is_ok(), "the bundled web font does not parse");
        let Ok(face) = parsed else { return };
        let mut wanted: Vec<char> = catalog.chars().filter(|c| !['\t', '\r', '\n'].contains(c)).collect();
        wanted.extend((0x20u32..0x7f).filter_map(char::from_u32));
        wanted.sort_unstable();
        wanted.dedup();
        let missing: Vec<char> = wanted.iter().copied().filter(|c| face.glyph_index(*c).is_none()).collect();
        let missing_non_emoji: Vec<char> = missing.iter().copied().filter(|c| !matches!(u32::from(*c), 0x1F000..=0x1FAFF)).collect();
        assert!(
            missing_non_emoji.is_empty(),
            "the bundled web font has no glyph for {missing_non_emoji:?} — regenerate assets/fonts/NotoSansSC-SoundCraftSubset.ttf"
        );
    }

    /// The web fallback chain (egui defaults + the bundled font) draws the entire zh-Hans catalog
    /// in every family — the property that keeps the web build free of tofu boxes.
    #[test]
    fn bundled_web_font_draws_the_zh_hans_catalog_in_every_family() {
        let Some(catalog) = zh_hans_catalog() else { return };
        let mut defs = definitions();
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
            // The family's charmap union, not `has_glyphs`: epaint's `has_glyph` reports a false
            // negative for characters owned by the face that also draws the replacement glyph (all
            // characters of `Hack` in Monospace, all emoji of `NotoEmoji-Regular` in Proportional).
            let missing: Vec<char> = ctx.fonts_mut(|fonts| {
                let mut font = fonts.fonts.font(&family);
                let characters = font.characters();
                sample.chars().filter(|c| !characters.contains_key(c)).collect()
            });
            assert!(missing.is_empty(), "{family:?} cannot draw {missing:?} of the zh-Hans catalog even with the bundled web font");
        }
    }

    /// The asset must not silently grow into a full CJK font (which would bloat the web download).
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
