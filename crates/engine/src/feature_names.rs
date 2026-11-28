//! Display-only terminology. Catalog paths, command ids, processor ids and saved values are stable.
//! Never use the returned label for command dispatch or persistence. Labels for features we do not
//! have describe a placeholder, which the menu draws disabled ("Not available yet.") until the
//! behaviour exists: `catalog::PLACEHOLDERS` lists them.
//!
//! Names still being settled (RFC-0029) are not mapped here: the incumbent's label stays until the
//! RFC decides, so AudioSuite, Elastic Audio, Elastic Properties and TrackPunch pass through.

pub fn display_label(label: &str) -> &str {
    match label {
        "Beat Detective" => "Beat Analysis",
        "x-form" => "Rendered (High Quality)",
        "QuickPunch" => "Punch Recording",
        "Ableton Link" => "Tempo Sync",
        "Pro Tools Search" => "Search",
        "Pro Tools Projects" => "Shared Projects",
        "Melodyne" => "No ARA plug-ins",
        "HEAT" => "Analog Saturation",
        "Activate HEAT" => "Activate Analog Saturation",
        "Sibelius" => "MusicXML",
        "Sibelius..." => "MusicXML...",
        _ => label,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_labels_are_descriptive_without_changing_unknown_text() {
        for (old, new) in [
            ("Beat Detective", "Beat Analysis"),
            ("x-form", "Rendered (High Quality)"),
            ("QuickPunch", "Punch Recording"),
            ("Ableton Link", "Tempo Sync"),
            ("Pro Tools Search", "Search"),
            ("Pro Tools Projects", "Shared Projects"),
            ("Melodyne", "No ARA plug-ins"),
            ("HEAT", "Analog Saturation"),
            ("Activate HEAT", "Activate Analog Saturation"),
            ("Sibelius...", "MusicXML..."),
        ] {
            assert_eq!(display_label(old), new);
        }
        assert_eq!(display_label("My session"), "My session");
    }

    /// Names deferred to RFC-0029 keep the incumbent's label until the RFC settles them.
    #[test]
    fn deferred_names_pass_through() {
        for label in ["AudioSuite", "AudioSuite Process", "Elastic Audio", "Elastic Properties", "Enable Elastic Audio on New Tracks", "TrackPunch"] {
            assert_eq!(display_label(label), label);
        }
    }
}
