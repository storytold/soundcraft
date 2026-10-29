use soundcraft_audio_io::FileFormat;

#[test]
fn every_read_and_write_format_round_trips_through_its_extensions() {
    for &format in FileFormat::READ {
        assert!(!format.extensions().is_empty(), "{format:?}");
        for ext in format.extensions() {
            assert_eq!(FileFormat::from_extension(ext), Some(format), "{ext}");
            assert_eq!(FileFormat::from_extension(&format!(".{}", ext.to_ascii_uppercase())), Some(format), "{ext}");
            assert_eq!(FileFormat::from_path(format!("dir.x/Take 1.{ext}")), Some(format), "{ext}");
        }
    }
    for &format in FileFormat::WRITE {
        assert!(!format.write_extensions().is_empty(), "{format:?}");
        for ext in format.write_extensions() {
            assert_eq!(FileFormat::from_write_extension(ext), Some(format), "{ext}");
            assert_eq!(FileFormat::from_write_extension(&ext.to_ascii_uppercase()), Some(format), "{ext}");
            assert_eq!(FileFormat::from_extension(ext), Some(format), "{ext}: what is written reads back");
        }
        assert!(format.write_extensions().contains(&format.default_extension()), "{format:?}");
    }
}

#[test]
fn only_names_whose_readers_expect_the_encoder_output_are_writable() {
    assert_eq!(FileFormat::writable_extensions(), ["wav", "wave", "bwf", "aif", "aiff", "flac"]);
    for read_only in ["rf64", "aifc", "mp3", "ogg", "m4a", "mkv"] {
        assert!(FileFormat::from_extension(read_only).is_some(), "{read_only}");
        assert_eq!(FileFormat::from_write_extension(read_only), None, "{read_only}");
    }
    for unknown in ["xyz", "", "mid"] {
        assert_eq!(FileFormat::from_extension(unknown), None, "{unknown}");
    }
    assert_eq!(FileFormat::from_path("no-extension"), None);
    assert_eq!(FileFormat::Aiff.default_extension(), "aif");
}
