use soundcraft_audio_io::{FileFormat, encode_format_for};

#[test]
fn only_formats_the_encoder_writes_are_named() {
    assert_eq!(encode_format_for("wav"), Some(FileFormat::Wav));
    assert_eq!(encode_format_for("WAV"), Some(FileFormat::Wav));
    assert_eq!(encode_format_for(".aif"), Some(FileFormat::Aiff));
    assert_eq!(encode_format_for("aiff"), Some(FileFormat::Aiff));
    assert_eq!(encode_format_for("flac"), Some(FileFormat::Flac));
    for other in ["mp3", "ogg", "m4a", "xyz", ""] {
        assert_eq!(encode_format_for(other), None, "{other}");
    }
}
