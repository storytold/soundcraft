//! Audio sources (files in the session's Audio Files folder) and their decoded data, and the
//! movies video clips reference.

use crate::SourceId;
use craft_audio::{AudioBuffer, AudioInfo, CodecId, Container, peaks::Peaks};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The kind of file a source came from, as session files store it in [`Source::format`]. The
/// variant names are part of the session format: craft-audio's `Container` and `CodecId` name
/// more formats under other names, so this stays here and maps from them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FileFormat {
    Wav,
    Aiff,
    Flac,
    Mp3,
    Ogg,
    Aac,
    Alac,
    Caf,
    /// Anything else that decodes: Matroska, WebM, raw AC-3.
    Other,
}

impl FileFormat {
    /// The kind of the file `info` describes.
    pub fn of(info: &AudioInfo) -> FileFormat {
        match info.container {
            Container::Wav => FileFormat::Wav,
            Container::Aiff => FileFormat::Aiff,
            Container::Flac => FileFormat::Flac,
            Container::MpegAudio => FileFormat::Mp3,
            Container::Ogg => FileFormat::Ogg,
            Container::Caf => FileFormat::Caf,
            Container::Mp4 | Container::QuickTime if info.codec == CodecId::Alac => FileFormat::Alac,
            Container::Mp4 | Container::QuickTime | Container::Adts => FileFormat::Aac,
            _ => FileFormat::Other,
        }
    }
}

/// Metadata for an audio file used by the session (serialised).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Source {
    pub id: SourceId,
    pub name: String,
    /// Path relative to the session folder (or absolute for referenced media).
    pub path: String,
    pub channels: u16,
    pub frames: u64,
    pub sample_rate: u32,
    pub format: FileFormat,
    /// Original BWF time stamp (samples), if any.
    #[serde(default)]
    pub time_reference: u64,
    /// True when the file has not been written to disk yet (recorded/rendered this session).
    #[serde(default)]
    pub unsaved: bool,
}

/// A movie used by Video track clips (serialised). The pictures themselves are never stored in
/// the session: the UI opens the movie from `path` and decodes frames on demand.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VideoSource {
    pub id: SourceId,
    pub name: String,
    /// Absolute path of the movie (referenced, never copied into the session folder).
    pub path: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    /// Frames per second (0 = unknown).
    #[serde(default)]
    pub frame_rate: f64,
    /// Picture length in seconds.
    #[serde(default)]
    pub duration: f64,
    /// Codec id (`h264`, `prores`, `mjpeg`, `hevc`, …).
    #[serde(default)]
    pub codec: String,
    /// Why the movie is not opened, when its stored path was refused on opening the session.
    #[serde(skip)]
    pub offline: Option<String>,
}

impl VideoSource {
    /// Display aspect ratio (width / height), 16:9 when unknown.
    pub fn aspect(&self) -> f64 {
        if self.width == 0 || self.height == 0 { 16.0 / 9.0 } else { f64::from(self.width) / f64::from(self.height) }
    }
}

/// Decoded audio plus waveform overviews (not serialised).
#[derive(Debug)]
pub struct SourceAudio {
    pub buffer: AudioBuffer,
    pub peaks: Vec<Peaks>,
}

impl SourceAudio {
    pub fn new(buffer: AudioBuffer) -> Self {
        let peaks = buffer.channels.iter().map(|c| Peaks::build(c)).collect();
        SourceAudio { buffer, peaks }
    }
    pub fn frames(&self) -> usize {
        self.buffer.frames()
    }
    pub fn channel(&self, ch: usize) -> Option<&[f32]> {
        self.buffer.channels.get(ch).map(Vec::as_slice)
    }
}

/// Shared decoded audio, keyed by source id.
#[derive(Debug, Clone, Default)]
pub struct SourcePool {
    map: BTreeMap<SourceId, Arc<SourceAudio>>,
}

impl SourcePool {
    pub fn insert(&mut self, id: SourceId, audio: Arc<SourceAudio>) {
        self.map.insert(id, audio);
    }
    pub fn get(&self, id: SourceId) -> Option<&Arc<SourceAudio>> {
        self.map.get(&id)
    }
    pub fn remove(&mut self, id: SourceId) -> Option<Arc<SourceAudio>> {
        self.map.remove(&id)
    }
    pub fn contains(&self, id: SourceId) -> bool {
        self.map.contains_key(&id)
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    pub fn ids(&self) -> impl Iterator<Item = SourceId> + '_ {
        self.map.keys().copied()
    }
}

impl PartialEq for SourcePool {
    fn eq(&self, other: &Self) -> bool {
        self.map.len() == other.map.len() && self.map.iter().zip(other.map.iter()).all(|((a, x), (b, y))| a == b && Arc::ptr_eq(x, y))
    }
}

#[cfg(test)]
mod tests {
    use super::FileFormat;
    use craft_audio::{AudioBuffer, EncodeFormat, EncodeOptions, encode, probe};

    /// Sessions written before the format code moved to craft-audio name a source's format with
    /// these words; they must keep reading, and new sessions must keep writing them.
    #[test]
    fn the_format_names_in_session_files_do_not_change() {
        use FileFormat::{Aac, Aiff, Alac, Caf, Flac, Mp3, Ogg, Other, Wav};
        let names = ["Wav", "Aiff", "Flac", "Mp3", "Ogg", "Aac", "Alac", "Caf", "Other"];
        for (format, name) in [Wav, Aiff, Flac, Mp3, Ogg, Aac, Alac, Caf, Other].into_iter().zip(names) {
            assert_eq!(serde_json::to_string(&format).unwrap(), format!("\"{name}\""));
            assert_eq!(serde_json::from_str::<FileFormat>(&format!("\"{name}\"")).unwrap(), format);
        }
    }

    #[test]
    fn a_probed_file_maps_to_its_format() {
        let buf = AudioBuffer::new(48_000, 1, 480);
        for (written, format) in
            [(EncodeFormat::Wav, FileFormat::Wav), (EncodeFormat::Aiff, FileFormat::Aiff), (EncodeFormat::Flac, FileFormat::Flac)]
        {
            let info = probe(&encode(&buf, &EncodeOptions::new(written)).unwrap(), None).unwrap();
            assert_eq!(FileFormat::of(&info), format);
        }
    }
}
