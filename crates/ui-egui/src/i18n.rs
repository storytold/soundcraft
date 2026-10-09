//! UI-only localization. Engine command IDs, parameters and session data remain language neutral.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    #[default]
    System,
    En,
    Uk,
}

impl Language {
    pub const ALL: [Self; 3] = [Self::System, Self::En, Self::Uk];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => tr("System language"),
            Self::En => "English",
            Self::Uk => "Українська",
        }
    }

    pub fn resolve(self, system_locale: Option<&str>) -> Self {
        match self {
            Self::System => match system_locale {
                Some(locale) if locale.split(['-', '_', '.', '@']).next().is_some_and(|tag| tag.eq_ignore_ascii_case("uk")) => Self::Uk,
                _ => Self::En,
            },
            explicit => explicit,
        }
    }
}

thread_local! {
    static CURRENT: Cell<Language> = const { Cell::new(Language::En) };
}

/// A frame-scoped language, restored on exit so separate apps/tests cannot affect each other.
pub(crate) struct LanguageScope(Language);

impl Drop for LanguageScope {
    fn drop(&mut self) {
        CURRENT.set(self.0);
    }
}

pub(crate) fn enter(language: Language) -> LanguageScope {
    LanguageScope(CURRENT.replace(language.resolve(None)))
}

fn decode(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('\\') => result.push('\\'),
                Some(other) => {
                    result.push('\\');
                    result.push(other);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(ch);
        }
    }
    result
}

fn catalog() -> &'static HashMap<String, String> {
    static CATALOG: OnceLock<HashMap<String, String>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        include_str!("i18n/uk.tsv").lines().filter_map(|line| line.split_once('\t')).map(|(key, value)| (decode(key), decode(value))).collect()
    })
}

pub fn translate(language: Language, source: &str) -> &str {
    if language == Language::Uk { catalog().get(source).map(String::as_str).unwrap_or(source) } else { source }
}

pub(crate) fn tr(source: &str) -> &str {
    translate(CURRENT.get(), source)
}

/// Substitute preformatted values in one pass. Values are never interpreted as another template.
pub(crate) fn render(source: &str, values: &[(&str, String)]) -> String {
    let template = tr(source);
    let mut result = String::with_capacity(template.len());
    let mut remaining = template;
    while let Some(start) = remaining.find('{') {
        result.push_str(&remaining[..start]);
        remaining = &remaining[start..];
        let Some(end) = remaining.find('}') else { break };
        let marker = &remaining[..=end];
        if let Some((_, value)) = values.iter().find(|(key, _)| *key == marker) {
            result.push_str(value);
        } else {
            result.push_str(marker);
        }
        remaining = &remaining[end + 1..];
    }
    result.push_str(remaining);
    result
}

/// Paths displayed by the palette/shortcut list; callers retain the original path for dispatch.
pub(crate) fn menu_path(parts: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    parts.into_iter().map(|part| tr(part.as_ref()).to_owned()).collect::<Vec<_>>().join(" › ")
}

pub(crate) fn automation_mode(mode: soundcraft_model::AutomationMode) -> &'static str {
    if mode == soundcraft_model::AutomationMode::Trim { tr("Automation Trim") } else { tr(mode.label()) }
}

pub(crate) fn automation_parameter(parameter: &soundcraft_model::AutoParam, track: &soundcraft_model::Track) -> String {
    use soundcraft_model::AutoParam;
    if CURRENT.get() != Language::Uk {
        return parameter.label();
    }
    let letter = |slot: u8| char::from(b'A'.saturating_add(slot));
    match parameter {
        AutoParam::Volume => tr("volume").to_owned(),
        AutoParam::Pan(0) => tr("pan").to_owned(),
        AutoParam::Pan(index) => render("Pan {index}", &[("{index}", (u16::from(*index) + 1).to_string())]),
        AutoParam::Mute => tr("mute").to_owned(),
        AutoParam::SendLevel(slot) => render("Send {slot} level", &[("{slot}", letter(*slot).to_string())]),
        AutoParam::SendPan(slot) => render("Send {slot} pan", &[("{slot}", letter(*slot).to_string())]),
        AutoParam::SendMute(slot) => render("Send {slot} mute", &[("{slot}", letter(*slot).to_string())]),
        AutoParam::Plugin { slot, param } => {
            let caption = track
                .mixer
                .inserts
                .get(usize::from(*slot))
                .and_then(Option::as_ref)
                .and_then(|insert| soundcraft_dsp::plugin_info(&insert.plugin))
                .and_then(|info| info.param(param))
                .map_or(param.as_str(), |info| tr(info.name));
            render("Insert {slot}: {param}", &[("{slot}", letter(*slot).to_string()), ("{param}", caption.to_owned())])
        }
    }
}

pub(crate) fn grid_label(grid: &soundcraft_time::GridValue) -> String {
    use soundcraft_time::GridValue;
    if CURRENT.get() != Language::Uk {
        return grid.label();
    }
    match grid {
        GridValue::Note { value, dotted, triplet } => {
            let mut label = tr(value.label()).to_owned();
            for (enabled, suffix) in [(*dotted, "dotted"), (*triplet, "triplet")] {
                if enabled {
                    label.push(' ');
                    label.push_str(tr(suffix));
                }
            }
            label
        }
        GridValue::Seconds(seconds) => render("{seconds:.3} sec", &[("{seconds:.3}", format!("{seconds:.3}"))]),
        GridValue::Frames(frames) => render("Frames: {frames}", &[("{frames}", frames.to_string())]),
        GridValue::Samples(samples) => render("Samples: {samples}", &[("{samples}", samples.to_string())]),
    }
}

pub(crate) fn parameter_value(parameter: &soundcraft_dsp::ParamInfo, value: f32) -> String {
    use soundcraft_dsp::Unit;
    if CURRENT.get() != Language::Uk {
        return parameter.format(value);
    }
    let value = parameter.clamp(value);
    match parameter.unit {
        Unit::Choice => parameter.choices.get(value as usize).map(|choice| tr(choice).to_owned()).unwrap_or_else(|| value.to_string()),
        Unit::Toggle => tr(if value >= 0.5 { "On" } else { "Off" }).to_owned(),
        Unit::Db if value <= -95.9 => tr("-inf dB").to_owned(),
        Unit::Db => format!("{value:.1} {}", tr("dB")),
        Unit::Hz if value >= 1000.0 => format!("{:.2} {}", value / 1000.0, tr("kHz")),
        Unit::Hz => format!("{value:.1} {}", tr("Hz")),
        Unit::Ms => format!("{value:.1} {}", tr("ms")),
        Unit::Semitones => format!("{value:.0} {}", tr("st")),
        Unit::Cents => format!("{value:.0} {}", tr("ct")),
        Unit::Seconds => format!("{value:.2} {}", tr("s")),
        _ => parameter.format(value),
    }
}

pub(crate) fn plugin_name(info: &soundcraft_dsp::PluginInfo) -> &str {
    if soundcraft_dsp::plugin_info(info.id).is_some() { tr(info.name) } else { info.name }
}

pub(crate) fn parameter_name<'a>(info: &soundcraft_dsp::PluginInfo, parameter: &'a soundcraft_dsp::ParamInfo) -> &'a str {
    if soundcraft_dsp::plugin_info(info.id).is_some() { tr(parameter.name) } else { parameter.name }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Services, SoundApp, UiState};
    use soundcraft_model::{AutomationMode, ChannelFormat, FadeShape, Tool, TrackHeight, TrackKind};
    use soundcraft_time::{GridValue, NoteValue, TimeFormat};
    use std::collections::{BTreeMap, BTreeSet};

    fn markers(text: &str) -> BTreeMap<String, usize> {
        let mut markers = BTreeMap::new();
        let mut remainder = text;
        while let Some(start) = remainder.find('{') {
            remainder = &remainder[start..];
            let Some(end) = remainder.find('}') else { break };
            *markers.entry(remainder[..=end].to_owned()).or_default() += 1;
            remainder = &remainder[end + 1..];
        }
        markers
    }

    #[test]
    fn catalog_has_unique_nonempty_keys_and_preserves_every_placeholder() {
        let mut keys = BTreeSet::new();
        for (index, line) in include_str!("i18n/uk.tsv").lines().enumerate() {
            let fields: Vec<_> = line.split('\t').collect();
            assert_eq!(fields.len(), 2, "catalog row {}", index + 1);
            let source = decode(fields[0]);
            let translation = decode(fields[1]);
            assert!(!source.trim().is_empty() && !translation.trim().is_empty(), "empty row {}", index + 1);
            assert!(keys.insert(source.clone()), "duplicate key: {source}");
            assert_eq!(markers(&source), markers(&translation), "placeholder mismatch: {source}");
        }
        assert_eq!(keys.len(), catalog().len());
    }

    #[test]
    fn catalog_covers_all_displayed_commands_menu_nodes_and_builtin_parameters() {
        let mut required = BTreeSet::new();
        for command in soundcraft_engine::command_specs() {
            required.insert(command.label);
            required.extend(command.menu.iter().copied());
        }
        for (_, label, path, _) in crate::menus::UI_COMMANDS {
            required.insert(*label);
            required.extend(path.split(" > ").filter(|part| !part.is_empty()));
        }
        for path in soundcraft_engine::catalog::catalog() {
            required.extend(path.split(" > "));
        }
        fn menu_labels(nodes: &[crate::menus::MenuNode]) {
            for node in nodes {
                assert!(catalog().contains_key(&node.label), "missing menu translation: {}", node.label);
                menu_labels(&node.children);
            }
        }
        menu_labels(&crate::menus::tree());
        required.extend(crate::theme::ThemeMode::ALL.into_iter().map(crate::theme::ThemeMode::label));
        for plugin in soundcraft_dsp::plugins() {
            required.extend([plugin.name, plugin.short_name]);
            for parameter in plugin.params {
                required.insert(parameter.name);
                required.extend(parameter.choices.iter().copied());
            }
        }
        required.extend(TrackKind::ALL.into_iter().map(TrackKind::label));
        required.extend(TrackHeight::ALL.into_iter().map(TrackHeight::label));
        required.extend(AutomationMode::ALL.into_iter().map(AutomationMode::label));
        required.extend(ChannelFormat::ALL.into_iter().map(ChannelFormat::label));
        required.extend(Tool::ALL.into_iter().map(Tool::label));
        required.extend(FadeShape::ALL.into_iter().map(FadeShape::label));
        required.extend(TimeFormat::ALL.into_iter().map(TimeFormat::label));
        required.extend(NoteValue::ALL.into_iter().map(NoteValue::label));
        let missing: Vec<_> = required.into_iter().filter(|key| !catalog().contains_key(*key)).collect();
        assert!(missing.is_empty(), "missing translations: {missing:?}");
    }

    #[test]
    fn language_selection_persists_and_system_detection_has_english_fallback() {
        for locale in ["uk", "uk-UA", "UK_ua.UTF-8", "uk_UA@euro"] {
            assert_eq!(Language::System.resolve(Some(locale)), Language::Uk);
        }
        for locale in [None, Some(""), Some("ru-UA"), Some("en-GB"), Some("ukrainian"), Some("pl_PL")] {
            assert_eq!(Language::System.resolve(locale), Language::En);
        }
        assert_eq!(Language::En.resolve(Some("uk-UA")), Language::En);
        assert_eq!(Language::Uk.resolve(Some("en-US")), Language::Uk);
        let mut preferences: UiState = serde_json::from_str(r#"{"window":"Mix"}"#).unwrap();
        assert_eq!(preferences.language, Language::System);
        preferences.language = Language::Uk;
        let saved = serde_json::to_vec(&preferences).unwrap();
        assert_eq!(serde_json::from_slice::<UiState>(&saved).unwrap().language, Language::Uk);
    }

    #[test]
    fn scopes_restore_language_and_templates_preserve_user_text_verbatim() {
        assert_eq!(tr("Save"), "Save");
        {
            let _language = enter(Language::Uk);
            assert_eq!(tr("Save"), "Зберегти");
            assert_eq!(tr("new key from a newer engine"), "new key from a newer engine");
            let user_value = "Save {other} \\ literal\nСеанс";
            assert_eq!(render("Session file: {p}", &[("{p}", user_value.into())]), format!("Файл сеансу: {user_value}"));
            {
                let _english = enter(Language::En);
                assert_eq!(tr("Save"), "Save");
            }
            assert_eq!(tr("Save"), "Зберегти");
        }
        assert_eq!(tr("Save"), "Save");
        assert_eq!(decode(r"line\nwith\ttab\\path"), "line\nwith\ttab\\path");
        assert_eq!(decode(r"unknown\x"), r"unknown\x");
    }

    #[test]
    fn count_and_grid_labels_do_not_use_english_plural_suffixes() {
        let _language = enter(Language::Uk);
        for count in [0, 1, 2, 5, 11, 21, 104] {
            assert_eq!(render("Tracks: {0}", &[("{0}", count.to_string())]), format!("Доріжок: {count}"));
            assert_eq!(grid_label(&GridValue::Frames(count)), format!("Кадрів: {count}"));
            assert_eq!(grid_label(&GridValue::Samples(count)), format!("Відліків: {count}"));
        }
        assert_eq!(grid_label(&GridValue::Note { value: NoteValue::Sixteenth, dotted: true, triplet: false }), "1/16 ноти із крапкою");
        assert_eq!(automation_mode(AutomationMode::Trim), "Коригування");
        assert_eq!(tr("Trim"), "Обрізати");
        assert_eq!(tr("Release"), "Відновлення");
        assert_eq!(tr("Amp Release"), "Згасання підсилювача після відпускання");
    }

    fn drawn_text(shape: &egui::Shape, text: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text_shape) => text.push(text_shape.galley.job.text.clone()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    drawn_text(shape, text);
                }
            }
            _ => {}
        }
    }

    fn frame(app: &mut SoundApp, context: &egui::Context) -> Vec<String> {
        let mut output = context.run_ui(
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))), ..Default::default() },
            |ui| {
                let context = ui.ctx().clone();
                app.logic(&context);
                app.ui(ui);
            },
        );
        output.textures_delta.clear();
        let mut text = Vec::new();
        for shape in output.shapes {
            drawn_text(&shape.shape, &mut text);
        }
        text
    }

    #[test]
    fn rendered_language_switch_preserves_session_user_names_and_engine_ids() {
        let context = egui::Context::default();
        let mut engine = soundcraft_engine::Engine::default();
        engine.execute("track.new", &serde_json::json!({"name":"Mix {slot} ҐґЄєІіЇї"})).unwrap();
        engine.session_mut().name = "Save {session}".into();
        let mut app = SoundApp::new(engine, None, Services::default());
        let before = serde_json::to_value(app.engine.session()).unwrap();
        for _ in 0..3 {
            frame(&mut app, &context);
        }
        let english = frame(&mut app, &context);
        assert!(english.iter().any(|text| text == "TRACKS"));
        app.run("ui.language", serde_json::json!({"language":"uk"})).unwrap();
        let ukrainian = frame(&mut app, &context);
        assert!(ukrainian.iter().any(|text| text == "ДОРІЖКИ"));
        assert!(ukrainian.iter().any(|text| text == "Такти|Долі"));
        assert!(ukrainian.iter().any(|text| text == "Mix {slot} ҐґЄєІіЇї"));
        assert!(ukrainian.iter().any(|text| text.contains("Save {session}")));
        assert_eq!(serde_json::to_value(app.engine.session()).unwrap(), before);
        app.run("window.ui_customization", serde_json::json!({"value": true})).unwrap();
        for (mode, caption) in [("light", "Світла"), ("dark", "Темна")] {
            app.run("ui.theme", serde_json::json!({"mode": mode})).unwrap();
            // Give a newly opened floating window a frame to settle its layout.
            frame(&mut app, &context);
            let appearance = frame(&mut app, &context);
            for label in ["Вигляд", "Мова", "Тема", caption] {
                assert!(appearance.iter().any(|text| text == label), "missing appearance caption: {label}");
            }
            assert_eq!(app.ui.language, Language::Uk);
            assert_eq!(serde_json::to_value(app.engine.session()).unwrap(), before);
        }
        assert!(app.run("ui.language", serde_json::json!({"language":"unsupported"})).is_err());
        assert_eq!(app.ui.language, Language::Uk);
        app.run("ui.language", serde_json::json!({"language":"en"})).unwrap();
        assert!(frame(&mut app, &context).iter().any(|text| text == "TRACKS"));
        assert_eq!(serde_json::to_value(app.engine.session()).unwrap(), before);
    }
}
