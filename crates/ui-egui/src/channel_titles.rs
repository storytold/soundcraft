//! SoundCraft's native window caption, with an explicitly selected channel name.

/// The OS window title beside a native menu bar, which shows neither the document nor its unsaved
/// marker anywhere else: Stable's exact format, `"{document}{ *} — SoundCraft — {section}"`, with
/// only the channel's name substituted, so a Nightly window never looks like a stable one.
pub fn os_title_with_section(app_name: &str, document: Option<(&str, bool)>, section: &str) -> String {
    let Some((name, dirty)) = document else { return app_name.into() };
    format!("{name}{} \u{2014} {app_name} \u{2014} {section}", if dirty { " *" } else { "" })
}

/// The OS window title without a native menu bar. The egui menu bar already shows the unsaved
/// marker, so Stable keeps it out of the title: `"{document} — SoundCraft"`, with only the
/// channel's name substituted.
pub fn os_title(app_name: &str, document: Option<&str>) -> String {
    match document {
        Some(name) => format!("{name} \u{2014} {app_name}"),
        None => app_name.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stable's title is user-visible and must not change when the channel's name is substituted.
    #[test]
    fn stable_caption_is_unchanged_in_both_menu_bar_states() {
        assert_eq!(os_title("SoundCraft", None), "SoundCraft");
        assert_eq!(os_title("SoundCraft", Some("Song")), "Song \u{2014} SoundCraft");
        assert_eq!(os_title_with_section("SoundCraft", None, "Edit"), "SoundCraft");
        assert_eq!(os_title_with_section("SoundCraft", Some(("Song", false)), "Edit"), "Song \u{2014} SoundCraft \u{2014} Edit");
        // Stable marks unsaved changes between the name and the separator, never as a prefix.
        assert_eq!(os_title_with_section("SoundCraft", Some(("Song", true)), "Mix"), "Song * \u{2014} SoundCraft \u{2014} Mix");
    }

    #[test]
    fn nightly_native_caption_keeps_channel_across_document_dirty_and_menu_bar_states() {
        let channel = "SoundCraft Nightly";
        assert_eq!(os_title(channel, None), channel);
        assert_eq!(os_title(channel, Some("my session.scraft")), "my session.scraft \u{2014} SoundCraft Nightly");
        assert_eq!(
            os_title_with_section(channel, Some(("my session.scraft", false)), "Edit"),
            "my session.scraft \u{2014} SoundCraft Nightly \u{2014} Edit"
        );
        for section in ["Edit", "Mix"] {
            let shown = os_title_with_section(channel, Some(("my session.scraft", true)), section);
            assert_eq!(shown, format!("my session.scraft * \u{2014} SoundCraft Nightly \u{2014} {section}"));
        }
    }
}
