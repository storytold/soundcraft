//! SoundCraft's native caption rule, with an explicitly selected channel name.

/// The OS window title: the document (with a leading `*` when it has unsaved changes) and the
/// channel's name, so a Nightly window never looks like a stable one. Without a document, the
/// channel name alone.
pub fn os_title(app_name: &str, document: Option<(&str, bool)>, show_file_name: bool) -> String {
    let Some((name, dirty)) = document else { return app_name.into() };
    let prefix = if dirty { "*" } else { "" };
    if show_file_name { format!("{prefix}{name} \u{2014} {app_name}") } else { format!("{prefix}{app_name}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nightly_native_caption_keeps_channel_across_document_dirty_and_hidden_name_updates() {
        let channel = "SoundCraft Nightly";
        assert_eq!(os_title(channel, None, true), channel);
        for dirty in [false, true] {
            let shown = os_title(channel, Some(("my session.scraft", dirty)), true);
            assert!(shown.contains("my session.scraft"));
            assert!(shown.ends_with(channel));
            assert_eq!(shown.starts_with('*'), dirty);
            let hidden = os_title(channel, Some(("my session.scraft", dirty)), false);
            assert!(hidden.ends_with(channel));
            assert!(!hidden.contains("my session.scraft"));
            assert_eq!(hidden.starts_with('*'), dirty);
        }
    }

    #[test]
    fn stable_caption_is_unchanged() {
        assert_eq!(os_title("SoundCraft", None, true), "SoundCraft");
        assert_eq!(os_title("SoundCraft", Some(("Song", false)), true), "Song \u{2014} SoundCraft");
    }
}
