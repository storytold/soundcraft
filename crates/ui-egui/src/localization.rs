//! Language selection and source-language fallback for UI strings.

#[path = "../locales/pt-BR/mod.rs"]
mod pt_br;

use std::cell::Cell;

thread_local! {
    static CURRENT_LANGUAGE: Cell<Language> = const { Cell::new(Language::English) };
}

/// A language supported by the UI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Language {
    #[default]
    #[serde(rename = "en")]
    English,
    #[serde(rename = "pt-BR")]
    PortugueseBrazil,
}

impl Language {
    pub const ALL: [Self; 2] = [Self::English, Self::PortugueseBrazil];

    /// Stable language tag used in preferences and control commands.
    pub const fn id(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::PortugueseBrazil => "pt-BR",
        }
    }

    /// Language name displayed in the language's own form.
    pub const fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::PortugueseBrazil => "Português (Brasil)",
        }
    }

    pub fn parse(tag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|language| language.id().eq_ignore_ascii_case(tag.trim()))
    }

    /// Translates a UI string, falling back to its English source when no translation exists.
    pub fn text<'a>(self, source: &'a str) -> &'a str {
        match self {
            Self::English => source,
            Self::PortugueseBrazil => pt_br::translate(source).unwrap_or(source),
        }
    }
}

/// Sets the language used by text widgets on this thread for the duration of a UI frame.
pub fn enter(language: Language) -> LanguageScope {
    let previous = CURRENT_LANGUAGE.with(|current| current.replace(language));
    LanguageScope(previous)
}

/// Translates a source string using the language selected for the current UI frame.
pub fn text(source: &str) -> &str {
    CURRENT_LANGUAGE.with(|language| language.get().text(source))
}

pub fn current_language() -> Language {
    CURRENT_LANGUAGE.with(Cell::get)
}

pub struct LanguageScope(Language);

impl Drop for LanguageScope {
    fn drop(&mut self) {
        CURRENT_LANGUAGE.with(|language| language.set(self.0));
    }
}

#[macro_export]
macro_rules! localized_format {
    ($source:literal) => {
        $crate::localization::text($source).to_owned()
    };
    ($source:literal, $($args:tt)*) => {{
        match $crate::localization::current_language() {
            $crate::localization::Language::English => format!($source, $($args)*),
            $crate::localization::Language::PortugueseBrazil => match $source {
                "Active audio ends at {seconds:.2} seconds" => format!("O áudio ativo termina em {seconds:.2} segundos", $($args)*),
                "Active plugin instances: {plugins}" => format!("Instâncias de plugins ativas: {plugins}", $($args)*),
                "Active audio end time is unknown (invalid session sample rate)" => format!("O fim do áudio ativo é desconhecido (taxa de amostragem da sessão inválida)", $($args)*),
                "Audio: {}" => format!("Áudio: {}", $($args)*),
                "AudioSuite · {title}" => format!("AudioSuite · {title}", $($args)*),
                "Bouncing {name}" => format!("Renderizando {name}", $($args)*),
                "Bouncing {} ({:.0}%)" => format!("Renderizando {} ({:.0}%)", $($args)*),
                "Configuration {n}" => format!("Configuração {n}", $($args)*),
                "Default ({default})" => format!("Padrão ({default})", $($args)*),
                "Device default" => format!("Padrão do dispositivo", $($args)*),
                "Importing {name}{more}" => format!("Importando {name}{more}", $($args)*),
                "Main output: {}" => format!("Saída principal: {}", $($args)*),
                "No engine" => format!("Sem mecanismo de áudio", $($args)*),
                "Recording at 24-bit uses {per_min:.1} MB per mono track-minute" => {
                    format!("A gravação em 24 bits usa {per_min:.1} MB por minuto de faixa mono", $($args)*)
                }
                "Save changes to “{name}” before closing?" => format!("Salvar as alterações em “{name}” antes de fechar?", $($args)*),
                "Session file: {p}" => format!("Arquivo da sessão: {p}", $($args)*),
                "Session media: {} files, {:.1} MB in memory (32-bit float)" => {
                    format!("Mídia da sessão: {} arquivos, {:.1} MB na memória (ponto flutuante de 32 bits)", $($args)*)
                }
                "Tracks: {}" => format!("Faixas: {}", $($args)*),
                "UI frame time: {:.1} ms" => format!("Tempo do quadro da interface: {:.1} ms", $($args)*),
                "Undo steps: {}" => format!("Etapas para desfazer: {}", $($args)*),
                "Value in {suffix}" => format!("Valor em {suffix}", $($args)*),
                "Version {}" => format!("Versão {}", $($args)*),
                "{bars} bars" => format!("{bars} compassos", $($args)*),
                "{} contributors · {} commits" => format!("{} colaboradores · {} commits", $($args)*),
                "@{}: {} PRs, {} commits, +{} / −{} lines (Δ {}), +{} / −{} binary assets, {} – {}" => {
                    format!("@{}: {} PRs, {} commits, +{} / −{} linhas (Δ {}), +{} / −{} recursos binários, {} – {}", $($args)*)
                }
                "Instrument for {}" => format!("Instrumento de {}", $($args)*),
                "Insert {} for {}" => format!("Insert {} para {}", $($args)*),
                "Input for {}" => format!("Entrada da faixa {}", $($args)*),
                "Output for {}" => format!("Saída da faixa {}", $($args)*),
                "Group for {}" => format!("Grupo da faixa {}", $($args)*),
                "In {}" => format!("Entrada {}", $($args)*),
                "In 1-{}" => format!("Entradas 1-{}", $($args)*),
                "Send {} for {}" => format!("Envio {} de {}", $($args)*),
                "Send {} · {}" => format!("Envio {} · {}", $($args)*),
                "Send {which} level for {track}" => format!("Nível do envio {which} da faixa {track}", $($args)*),
                "(redo) {r}" => format!("(refazer) {r}", $($args)*),
                "{} / {} / {}{}" => format!("Faixa: {} / Playlist: {} / Clipe: {}{}", $($args)*),
                "{}  (missing)" => format!("{}  (ausente)", $($args)*),
                "Track view for {}" => format!("Visualização da faixa {}", $($args)*),
                "Automation mode for {}" => format!("Modo de automação da faixa {}", $($args)*),
                "Voice for {}" => format!("Voz da faixa {}", $($args)*),
                "{} for {}" => format!("{} para {}", $($args)*),
                "auto {}" => format!("Automação: {}", $($args)*),
                "{} @ {} Hz" => format!("{} a {} Hz", $($args)*),
                "{} samples" => format!("{} amostras", $($args)*),
                "{} · {} Hz · {} ch · {buf}" => format!("{} · {} Hz · {} canais · {buf}", $($args)*),
                "{} · {} Hz · {} ch" => format!("{} · {} Hz · {} canais", $($args)*),
                "{} audio clips checked · {} issues" => format!("{} clipes de áudio verificados · {} problemas", $($args)*),
                "{} ch" => format!("{} canais", $($args)*),
                "{} object track(s)" => format!("{} faixa(s) de objeto", $($args)*),
                "{} — {} from channel {}" => format!("{} — {} do canal {}", $($args)*),
                "“{}” already exists. Do you want to replace it?" => format!("“{}” já existe. Deseja substituí-lo?", $($args)*),
                "“{}” has {what} from an earlier run, autosaved {age}." => {
                    format!("“{}” tem {what} de uma execução anterior, salvo automaticamente {age}.", $($args)*)
                }
                "{question} Your changes will be lost." => format!("{question} Suas alterações serão perdidas.", $($args)*),
                "{path}\nnot found" => format!("{path}\nnão encontrado", $($args)*),
                "media offline: {why}" => format!("mídia indisponível: {why}", $($args)*),
                "media offline: {e}" => format!("mídia indisponível: {e}", $($args)*),
                "Window configuration could not be restored: {error}" => {
                    format!("Não foi possível restaurar a configuração da janela: {error}", $($args)*)
                }
                _ => format!($source, $($args)*),
            },
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::Language;

    #[test]
    fn language_tags_are_stable_and_round_trip() {
        for language in Language::ALL {
            assert_eq!(Language::parse(language.id()), Some(language));
            let encoded = serde_json::to_value(language).unwrap_or_default();
            let decoded: Language = serde_json::from_value(encoded).unwrap_or_default();
            assert_eq!(decoded, language);
        }
        assert_eq!(Language::parse(" PT-br "), Some(Language::PortugueseBrazil));
        assert_eq!(Language::parse("fr"), None);
    }

    #[test]
    fn language_translations_fall_back_to_the_english_source() {
        assert_eq!(Language::PortugueseBrazil.text("Appearance"), "Aparência");
        assert_eq!(Language::PortugueseBrazil.text("Unknown label"), "Unknown label");
        assert_eq!(Language::English.text("Appearance"), "Appearance");
    }

    #[test]
    fn language_scope_restores_the_previous_language() {
        assert_eq!(text("Appearance"), "Appearance");
        {
            let _scope = enter(Language::PortugueseBrazil);
            assert_eq!(text("Appearance"), "Aparência");
        }
        assert_eq!(text("Appearance"), "Appearance");
    }

    #[test]
    fn portuguese_catalog_covers_every_menu_label() {
        for path in include_str!("../../engine/catalog/menus.txt").lines() {
            for label in path.split(" > ").map(str::trim).filter(|label| !label.is_empty()) {
                assert!(super::pt_br::translate(label).is_some(), "missing pt-BR menu label: {label}");
            }
        }
    }
}
