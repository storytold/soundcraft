//! Audio sources (files in the session's Audio Files folder) and their decoded data, and the
//! movies video clips reference.

use crate::SourceId;
use soundcraft_audio_io::{AudioBuffer, FileFormat, peaks::Peaks};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::SyncSender;
use std::time::SystemTime;

/// Versioned receipt for the exact encoded bytes supplied to a native audio import.
/// The decoded [`Source`] remains the working audio used by edits and playback.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OriginalProvenance {
    pub version: u32,
    /// Generated relative name under `Original Files/`, never a referenced import path.
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

/// A control-side verification receipt. Metadata is a cache check, not proof against a
/// concurrently modified file: open and copy boundaries verify the complete checksum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginalAssetStamp {
    pub receipt_id: u64,
    pub len: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
}

/// Shared lifetime of retained encoded audio. It contains no file handles or encoded bytes.
/// Releasing a snapshot only wakes the collector; it never touches disk on that thread.
#[derive(Debug)]
pub struct OriginalAssetLease {
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
    pub stamp: OriginalAssetStamp,
    cleanup: Option<SyncSender<()>>,
}

impl OriginalAssetLease {
    pub fn new(path: PathBuf, sha256: String, bytes: u64, stamp: OriginalAssetStamp, cleanup: Option<SyncSender<()>>) -> Self {
        Self { path, sha256, bytes, stamp, cleanup }
    }
}

impl Drop for OriginalAssetLease {
    fn drop(&mut self) {
        if let Some(cleanup) = &self.cleanup {
            let _ = cleanup.try_send(());
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
    originals: BTreeMap<SourceId, Arc<OriginalAssetLease>>,
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
    pub fn insert_original(&mut self, id: SourceId, original: Arc<OriginalAssetLease>) {
        self.originals.insert(id, original);
    }
    pub fn original_asset(&self, id: SourceId) -> Option<&Arc<OriginalAssetLease>> {
        self.originals.get(&id)
    }
    pub fn remove_original(&mut self, id: SourceId) -> Option<Arc<OriginalAssetLease>> {
        self.originals.remove(&id)
    }
    pub fn retain_originals(&mut self, ids: &std::collections::BTreeSet<SourceId>) {
        self.originals.retain(|id, _| ids.contains(id));
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
        self.map.len() == other.map.len()
            && self.map.iter().zip(other.map.iter()).all(|((a, x), (b, y))| a == b && Arc::ptr_eq(x, y))
            && self.originals.len() == other.originals.len()
            && self.originals.iter().zip(other.originals.iter()).all(|((a, x), (b, y))| a == b && Arc::ptr_eq(x, y))
    }
}
