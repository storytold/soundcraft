//! Display-only terminology. Catalog paths, command ids, processor ids and saved values are stable.
//! Never use the returned label for command dispatch or persistence. Labels for features we do not
//! have describe a placeholder, which the menu draws disabled ("Not available yet.") until the
//! behaviour exists: `catalog::PLACEHOLDERS` lists them.

pub fn display_label(label: &str) -> &str {
    match label {
        "AudioSuite" => "Offline Processing",
        "AudioSuite Process" => "Process Clips Offline",
        "Beat Detective" => "Beat Analysis",
        "Elastic Audio" => "Real-Time Stretch",
        "Elastic Properties" => "Real-Time Stretch Properties",
        "Enable Elastic Audio on New Tracks" => "Enable Real-Time Stretch on New Tracks",
        "x-form" => "Rendered (High Quality)",
        "QuickPunch" => "Punch Recording",
        "TrackPunch" => "Track Punch Recording",
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
            ("AudioSuite", "Offline Processing"),
            ("Beat Detective", "Beat Analysis"),
            ("Elastic Audio", "Real-Time Stretch"),
            ("x-form", "Rendered (High Quality)"),
            ("QuickPunch", "Punch Recording"),
            ("TrackPunch", "Track Punch Recording"),
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
}
