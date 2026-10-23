//! Bounded AAF timeline import.
//!
//! This reads the standard, publicly documented AAF low-level object model directly from
//! the compound-file container: a per-file `MetaDictionary` of `ClassDefinition`/`PropertyDef`
//! objects establishes which local, per-file numeric property IDs ("PIDs") back each class's
//! properties, which this importer resolves dynamically rather than assuming any fixed PID for
//! a given field (vendor extensions are free to assign PIDs differently file to file). Lowering
//! walks `Header -> ContentStorage -> Mobs -> CompositionMob -> TimelineMobSlot -> Sequence ->
//! SourceClip`, resolving each `SourceClip` against its `SourceMob` (and that mob's `Locator`s,
//! when present) by `MobID`. Only audio (`DataDef_Sound`) timeline tracks are imported; this
//! never reads PTX data or decodes embedded AAF essence. The exact binary layout implemented
//! here (CFB object-storage-per-object, `"properties"` stream PID/format/length encoding, and
//! strong/weak-reference and reference-vector/-set encodings) and the well-known, standardized
//! AAF class and property AUIDs below are derived solely from publicly documented AAF behavior,
//! cross-checked against the MIT-licensed `pyaaf2` project purely as a behavioral reference (no
//! code from it is copied here).

use crate::io::ConfinedHandle;
use crate::{Engine, EngineError, Result};
use serde_json::{Value, json};
use soundcraft_audio_io::AudioBuffer;
use soundcraft_model::{ChannelFormat, Clip, Source, SourceId, TrackKind};
use soundcraft_time::{MAX_POSITION, SampleRate, Samples};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// AAF containers with embedded essence are intentionally outside this first
/// importer's scope. The structure itself is normally much smaller.
const MAX_AAF_FILE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_AAF_METADATA_STREAM_BYTES: u64 = 4 * 1024 * 1024;
/// AAF's compound-file metadata uses one directory entry per small property/dictionary stream,
/// not per track or clip: an ordinary single-composition Pro Tools export of under 2 MiB already
/// needs several thousand (observed: 3,796 for a ~1.1 MiB session). This leaves generous headroom
/// for larger sessions while still bounding the directory-entry table's size and walk time.
const MAX_AAF_DIRECTORY_ENTRIES: usize = 65_536;
const MAX_AAF_COMPOSITIONS: usize = 64;
const MAX_AAF_TRACKS: usize = 256;
const MAX_AAF_CLIPS: usize = 10_000;
const MAX_AAF_MEDIA_BYTES: u64 = 128 * 1024 * 1024;
const MAX_AAF_MEDIA_SAMPLES: usize = 1 << 24;
const MAX_WARNINGS: usize = 200;
/// A Pro Tools (or other DAW) session report is a plain-text export, not session state; this
/// still bounds it so a hostile or enormous file cannot be read into memory wholesale.
const MAX_SESSION_REPORT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SESSION_REPORT_LINES: usize = 50_000;
const MAX_SESSION_REPORT_NOTES: usize = 200;
/// Case-insensitive substrings that mark a session-report line as migration-relevant. Matching
/// is deliberately shallow (no field parsing, no structured extraction): a line is either kept
/// verbatim as a note or it isn't, so this can never misreport what a proprietary binary contains.
const SESSION_REPORT_KEYWORDS: &[&str] =
    &["session name", "sample rate", "bit depth", "timecode", "track name", "plug-in", "plugin", "mute", "# of audio", "number of audio"];
const CFB_SECTOR_SIZE: u64 = 512;
const CFB_END_OF_CHAIN: u32 = 0xFFFF_FFFE;
const CFB_FREE_SECTOR: u32 = 0xFFFF_FFFF;
const CFB_FAT_SECTOR: u32 = 0xFFFF_FFFD;
const CFB_DIFAT_SECTOR: u32 = 0xFFFF_FFFC;
/// CFB directory "no sibling/child" sentinel; numerically identical to `CFB_FREE_SECTOR` but
/// named separately here because it means something different in a directory-tree reference.
const CFB_NO_ENTRY: u32 = 0xFFFF_FFFF;

/// A single AAF `"properties"` stream maps one small, fixed-shape object (a Mob, a slot, a
/// component, a class/property definition...); real Pro Tools exports stay in the low hundreds
/// of properties per object even for the richest classes, so this is already generous headroom
/// against a hostile or corrupt stream claiming an enormous property count.
const MAX_AAF_PROPERTY_ENTRIES: usize = 4_096;
/// Bounds a single strong/weak-reference vector or set (e.g. `ContentStorage.Mobs`,
/// `Mob.Slots`, `Sequence.Components`): generous for any real session, but still well short of
/// a hostile file forcing an unbounded allocation.
const MAX_AAF_COLLECTION_ENTRIES: usize = 65_536;
/// `MetaDictionary.ClassDefinitions` describes the whole AAF class hierarchy; the full public
/// specification itself defines well under a thousand classes, so this leaves headroom for
/// vendor extensions while still bounding a hostile file's class count.
const MAX_AAF_CLASSES: usize = 4_096;
const MAX_AAF_PROPERTIES_PER_CLASS: usize = 512;
/// Bounds how far a class's `ParentClass` chain is walked while resolving a property's
/// per-file PID; the real AAF class hierarchy is only a few levels deep.
const MAX_AAF_CLASS_DEPTH: usize = 64;
/// AAF "stored form" (property format) tags, as fixed by the public AAF Object Specification.
const SF_DATA: u16 = 0x82;
const SF_STRONG_OBJECT_REFERENCE: u16 = 0x22;
const SF_STRONG_OBJECT_REFERENCE_VECTOR: u16 = 0x32;
const SF_STRONG_OBJECT_REFERENCE_SET: u16 = 0x3A;
const SF_WEAK_OBJECT_REFERENCE: u16 = 0x02;

const fn hex_val(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0,
    }
}

const fn hex_byte(s: &[u8; 36], i: usize) -> u8 {
    (hex_val(s[i]) << 4) | hex_val(s[i + 1])
}

/// Parses a standard `8-4-4-4-12` hex GUID/AUID string literal into its 16-byte, mixed-endian
/// on-disk form (the same layout used by CFB CLSIDs and AAF `AUID`/class-identifier values):
/// the first three fields are stored little-endian, the last two are stored byte-for-byte as
/// written. Used only to transcribe AUIDs exactly as published (by the AAF specification, and
/// cross-checked against `pyaaf2`'s model tables) without any by-hand endian arithmetic.
const fn parse_auid(s: &[u8; 36]) -> [u8; 16] {
    let b0 = hex_byte(s, 0);
    let b1 = hex_byte(s, 2);
    let b2 = hex_byte(s, 4);
    let b3 = hex_byte(s, 6);
    let b4 = hex_byte(s, 9);
    let b5 = hex_byte(s, 11);
    let b6 = hex_byte(s, 14);
    let b7 = hex_byte(s, 16);
    let b8 = hex_byte(s, 19);
    let b9 = hex_byte(s, 21);
    let b10 = hex_byte(s, 24);
    let b11 = hex_byte(s, 26);
    let b12 = hex_byte(s, 28);
    let b13 = hex_byte(s, 30);
    let b14 = hex_byte(s, 32);
    let b15 = hex_byte(s, 34);
    [b3, b2, b1, b0, b5, b4, b7, b6, b8, b9, b10, b11, b12, b13, b14, b15]
}

// Well-known, publicly documented AAF class and property identifiers (the AAF Object
// Specification's fixed AUIDs), used only as dictionary lookup *keys*: the per-file numeric
// PID that actually backs each property on disk is never assumed and is always resolved from
// the file's own `MetaDictionary` (see `resolve_pid`). Root's two properties and the bootstrap
// ClassDef/PropertyDef fields are the sole exception: every compliant AAF reader (including
// this one) must hardcode those to read the MetaDictionary at all, matching the spec.
const CLASS_ROOT: [u8; 16] = parse_auid(b"b3b398a5-1c90-11d4-8053-080036210804");
const CLASS_META_DICTIONARY: [u8; 16] = parse_auid(b"0d010101-0225-0000-060e-2b3402060101");
const CLASS_CLASS_DEF: [u8; 16] = parse_auid(b"0d010101-0201-0000-060e-2b3402060101");
const CLASS_PROPERTY_DEF: [u8; 16] = parse_auid(b"0d010101-0202-0000-060e-2b3402060101");
const CLASS_CONTENT_STORAGE: [u8; 16] = parse_auid(b"0d010101-0101-1800-060e-2b3402060101");
const CLASS_HEADER: [u8; 16] = parse_auid(b"0d010101-0101-2f00-060e-2b3402060101");
const CLASS_MOB: [u8; 16] = parse_auid(b"0d010101-0101-3400-060e-2b3402060101");
const CLASS_COMPOSITION_MOB: [u8; 16] = parse_auid(b"0d010101-0101-3500-060e-2b3402060101");
const CLASS_MASTER_MOB: [u8; 16] = parse_auid(b"0d010101-0101-3600-060e-2b3402060101");
const CLASS_SOURCE_MOB: [u8; 16] = parse_auid(b"0d010101-0101-3700-060e-2b3402060101");
const CLASS_TIMELINE_MOB_SLOT: [u8; 16] = parse_auid(b"0d010101-0101-3b00-060e-2b3402060101");
const CLASS_COMPONENT: [u8; 16] = parse_auid(b"0d010101-0101-0200-060e-2b3402060101");
const CLASS_SEQUENCE: [u8; 16] = parse_auid(b"0d010101-0101-0f00-060e-2b3402060101");
const CLASS_SOURCE_CLIP: [u8; 16] = parse_auid(b"0d010101-0101-1100-060e-2b3402060101");
const CLASS_FILLER: [u8; 16] = parse_auid(b"0d010101-0101-0900-060e-2b3402060101");
const CLASS_TRANSITION: [u8; 16] = parse_auid(b"0d010101-0101-1700-060e-2b3402060101");
const CLASS_OPERATION_GROUP: [u8; 16] = parse_auid(b"0d010101-0101-0a00-060e-2b3402060101");
const CLASS_NETWORK_LOCATOR: [u8; 16] = parse_auid(b"0d010101-0101-3200-060e-2b3402060101");
const CLASS_MOB_SLOT: [u8; 16] = parse_auid(b"0d010101-0101-3800-060e-2b3402060101");
const CLASS_ESSENCE_DESCRIPTOR: [u8; 16] = parse_auid(b"0d010101-0101-2400-060e-2b3402060101");

const PROP_HEADER_CONTENT: [u8; 16] = parse_auid(b"06010104-0201-0000-060e-2b3401010102");
const PROP_CONTENT_STORAGE_MOBS: [u8; 16] = parse_auid(b"06010104-0501-0000-060e-2b3401010102");
const PROP_MOB_NAME: [u8; 16] = parse_auid(b"01030302-0100-0000-060e-2b3401010101");
const PROP_MOB_SLOTS: [u8; 16] = parse_auid(b"06010104-0605-0000-060e-2b3401010102");
const PROP_MOBSLOT_SLOTNAME: [u8; 16] = parse_auid(b"01070102-0100-0000-060e-2b3401010102");
const PROP_MOBSLOT_SEGMENT: [u8; 16] = parse_auid(b"06010104-0204-0000-060e-2b3401010102");
const PROP_TIMELINE_EDIT_RATE: [u8; 16] = parse_auid(b"05300405-0000-0000-060e-2b3401010102");
const PROP_TIMELINE_ORIGIN: [u8; 16] = parse_auid(b"07020103-0103-0000-060e-2b3401010102");
const PROP_SEQUENCE_COMPONENTS: [u8; 16] = parse_auid(b"06010104-0609-0000-060e-2b3401010102");
const PROP_OPERATION_GROUP_INPUT_SEGMENTS: [u8; 16] = parse_auid(b"06010104-0602-0000-060e-2b3401010102");
const PROP_COMPONENT_LENGTH: [u8; 16] = parse_auid(b"07020201-0103-0000-060e-2b3401010102");
const PROP_COMPONENT_DATA_DEFINITION: [u8; 16] = parse_auid(b"04070100-0000-0000-060e-2b3401010102");
const PROP_SOURCE_REFERENCE_SOURCE_ID: [u8; 16] = parse_auid(b"06010103-0100-0000-060e-2b3401010102");
const PROP_SOURCE_REFERENCE_SOURCE_SLOT_ID: [u8; 16] = parse_auid(b"06010103-0200-0000-060e-2b3401010102");
const PROP_SOURCE_CLIP_START_TIME: [u8; 16] = parse_auid(b"07020103-0104-0000-060e-2b3401010102");
const PROP_SOURCE_MOB_ESSENCE_DESCRIPTION: [u8; 16] = parse_auid(b"06010104-0203-0000-060e-2b3401010102");
const PROP_ESSENCE_DESCRIPTOR_LOCATOR: [u8; 16] = parse_auid(b"06010104-0603-0000-060e-2b3401010102");
const PROP_NETWORK_LOCATOR_URL_STRING: [u8; 16] = parse_auid(b"01020101-0100-0000-060e-2b3401010101");
const PROP_MOBSLOT_SLOT_ID: [u8; 16] = parse_auid(b"01070101-0000-0000-060e-2b3401010102");

/// `DataDefinition` identifiers for audio ("sound") timeline slots. A `Sequence`'s (or other
/// `Segment`'s) `DataDefinition` is a weak reference whose on-disk key *is* the referenced
/// `DataDefinition` object's own AUID; these are two of the small set of built-in, standardized
/// data-definition AUIDs every AAF file (old or new) uses for this, so no dictionary lookup is
/// needed to recognize them.
const DATA_DEF_SOUND: [u8; 16] = parse_auid(b"01030202-0200-0000-060e-2b3404010101");
const DATA_DEF_LEGACY_SOUND: [u8; 16] = parse_auid(b"78e1ebe1-6cef-11d2-807d-006008143e6f");

/// Root's own two properties and the ClassDef/PropertyDef bootstrap fields are fixed, universal
/// PIDs from the AAF specification itself: `ClassDefinition`/`PropertyDef` objects are not part
/// of the normal, dictionary-driven data model (they *are* the dictionary), so every compliant
/// reader hardcodes these to bootstrap the MetaDictionary before any other PID can be resolved.
const PID_ROOT_META_DICTIONARY: u16 = 0x0001;
const PID_ROOT_HEADER: u16 = 0x0002;
const PID_META_DICTIONARY_CLASS_DEFS: u16 = 0x0003;
const PID_DEF_AUID: u16 = 0x0005;
const PID_DEF_PARENT: u16 = 0x0008;
const PID_DEF_PROPERTIES: u16 = 0x0009;
const PID_DEF_PID: u16 = 0x000D;

#[derive(Debug, Clone)]
pub struct AafImportReport {
    pub composition: String,
    pub sample_rate: u32,
    pub audio_tracks: usize,
    pub clips: usize,
    pub linked_wav_files: usize,
    pub warnings: Vec<String>,
}

impl AafImportReport {
    pub fn json(&self) -> Value {
        json!({
            "composition": self.composition,
            "sample_rate": self.sample_rate,
            "audio_tracks": self.audio_tracks,
            "clips": self.clips,
            "linked_wav_files": self.linked_wav_files,
            "warnings": self.warnings,
        })
    }
}

#[derive(Debug, Clone)]
struct TrackPlan {
    name: String,
    clips: Vec<ClipPlan>,
    stereo: bool,
}

#[derive(Debug, Clone)]
struct ClipPlan {
    source_key: String,
    source_name: String,
    name: String,
    start: Samples,
    offset: Samples,
    length: Samples,
    right_source_key: Option<String>,
    right_source_name: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct ImportedSource {
    id: SourceId,
    frames: Samples,
    channels: usize,
    loaded: bool,
}

/// One `CompositionMob`'s audio timeline, as lowered from its `TimelineMobSlot`s.
#[derive(Debug, Clone)]
struct Composition {
    name: String,
    tracks: Vec<TimelineTrack>,
}

/// One audio (`DataDef_Sound`) `TimelineMobSlot`.
#[derive(Debug, Clone)]
struct TimelineTrack {
    name: String,
    edit_rate: (i32, i32),
    origin: i64,
    components: Vec<ComponentEntry>,
}

#[derive(Debug, Clone)]
struct ComponentEntry {
    kind: ComponentKind,
    length: i64,
}

#[derive(Debug, Clone)]
enum ComponentKind {
    SourceClip { start_time: i64, source_mob_id: [u8; 32], source_slot_id: u32 },
    Filler,
    Transition,
    Effect,
}

/// A Mob found anywhere in `ContentStorage.Mobs`, kept for `SourceClip.SourceID` resolution
/// regardless of which (if any) composition references it.
#[derive(Debug, Clone, Default)]
struct MobInfo {
    name: String,
    locator_url: Option<String>,
    slots: HashMap<u32, Vec<ComponentEntry>>,
}

/// The whole AAF object graph this importer understands, lowered from the file's own
/// `MetaDictionary`-described structure.
#[derive(Debug, Clone, Default)]
struct AafModel {
    compositions: Vec<Composition>,
    master_mob_count: usize,
    source_mob_count: usize,
    mobs: HashMap<[u8; 32], MobInfo>,
}

#[derive(Debug, Clone)]
struct DirectoryEntry {
    name: String,
    kind: u8,
    left: u32,
    right: u32,
    child: u32,
    clsid: [u8; 16],
    start_sector: u32,
    size: u64,
}

/// The files directly in `dir` whose extension is one of `exts` (any case), sorted.
pub fn files_with_extension(dir: &Path, exts: &[&str]) -> Result<Vec<PathBuf>> {
    let mut found: Vec<PathBuf> = crate::io::list_dir(dir, MAX_AAF_COLLECTION_ENTRIES)?
        .into_iter()
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x))) && crate::io::is_readable_file(p)
        })
        .collect();
    found.sort();
    Ok(found)
}

fn find_aaf_in(dir: &Path) -> Result<PathBuf> {
    let mut found = files_with_extension(dir, &["aaf"])?.into_iter();
    match (found.next(), found.next()) {
        (Some(only), None) => Ok(only),
        (None, _) => Err(EngineError::Unsupported(format!("no .aaf file found in `{}`", dir.display()))),
        _ => Err(EngineError::Unsupported(format!("more than one .aaf file in `{}`; choose the file itself", dir.display()))),
    }
}

/// Replace the current document with an imported audio AAF composition.
///
/// The parser is run only after a conservative CFB preflight. The whole
/// document is assembled off-engine so malformed input cannot leave a partial
/// session behind.
pub fn import(engine: &mut Engine, input: &Path, media_dir: Option<&Path>) -> Result<AafImportReport> {
    // A folder (the usual Pro Tools export) stands for the single AAF inside it, with its WAVs beside it.
    let (input, media_dir) =
        if crate::io::is_readable_dir(input) { (find_aaf_in(input)?, media_dir.or(Some(input))) } else { (input.to_path_buf(), media_dir) };
    let input = input.as_path();
    // Pro Tools exports keep the WAV files next to the AAF: that folder is the default media location.
    let media_dir = media_dir.or_else(|| Some(input.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."))));
    let model = read_aaf(input)?;
    let mut compositions = model.compositions;
    if compositions.is_empty() {
        return Err(EngineError::Unsupported(format!(
            "{}: the compound-file container and its AAF MetaDictionary were read successfully, but no CompositionMob was found in ContentStorage.Mobs (also found {} master mobs, {} source mobs); no SoundCraft session was written. Re-export the session or report this AAF file if you believe this is an error",
            input.display(),
            model.master_mob_count,
            model.source_mob_count
        )));
    }
    if compositions.len() > MAX_AAF_COMPOSITIONS {
        return Err(EngineError::Unsupported(format!("AAF has more than {MAX_AAF_COMPOSITIONS} compositions")));
    }
    compositions.sort_by(|a, b| a.name.cmp(&b.name));
    let composition_count = compositions.len();
    // `compositions` was already checked non-empty above, so this always has an element.
    let composition = compositions.remove(0);

    let mut warnings = Vec::new();
    if composition_count > 1 {
        warning(
            &mut warnings,
            format!(
                "AAF contains {} compositions; imported `{}` (the first name in deterministic order)",
                composition_count,
                label(&composition.name)
            ),
        );
    }

    let tracks = composition.tracks;
    if tracks.is_empty() {
        return Err(EngineError::Unsupported(
            "the AAF composition has no audio (DataDef_Sound) timeline tracks; no SoundCraft session was written".into(),
        ));
    }
    if tracks.len() > MAX_AAF_TRACKS {
        return Err(EngineError::Unsupported(format!("AAF composition has more than {MAX_AAF_TRACKS} audio tracks")));
    }

    let sample_rate = match media_sample_rate(media_dir)? {
        Some(sample_rate) => sample_rate,
        None => session_rate(&tracks, &mut warnings)?,
    };
    let session_name = nonempty_label(&composition.name, "AAF import");
    let mut session = soundcraft_model::Session::new(session_name.clone(), sample_rate);
    let plans = pair_stereo_tracks(plan_tracks(&model.mobs, &tracks, sample_rate.hz(), &mut warnings)?, &mut warnings);
    if plans.iter().all(|track| track.clips.is_empty()) {
        return Err(EngineError::Unsupported("the AAF audio tracks contain no supported source clips; no SoundCraft session was written".into()));
    }

    let mut required_frames: HashMap<String, Samples> = HashMap::new();
    let mut source_names: HashMap<String, String> = HashMap::new();
    for track in &plans {
        for clip in &track.clips {
            let end = clip.offset.saturating_add(clip.length).max(1);
            required_frames.entry(clip.source_key.clone()).and_modify(|frames| *frames = (*frames).max(end)).or_insert(end);
            source_names.entry(clip.source_key.clone()).or_insert_with(|| clip.source_name.clone());
            if let (Some(right_key), Some(right_name)) = (&clip.right_source_key, &clip.right_source_name) {
                required_frames.entry(right_key.clone()).and_modify(|frames| *frames = (*frames).max(end)).or_insert(end);
                source_names.entry(right_key.clone()).or_insert_with(|| right_name.clone());
            }
        }
    }

    let mut sources = HashMap::new();
    let mut linked_wav_files = 0usize;
    for (source_key, required) in &required_frames {
        let source_name = source_names.get(source_key).map(String::as_str).unwrap_or("AAF source");
        let imported = match load_wav_source(&mut session, source_name, media_dir) {
            Ok(Some(imported)) => {
                linked_wav_files = linked_wav_files.saturating_add(1);
                imported
            }
            Ok(None) => {
                warning(&mut warnings, offline_source_warning(source_name, media_dir));
                add_offline_source(&mut session, source_name, offline_path(source_name, media_dir), *required)
            }
            Err(reason) => {
                warning(&mut warnings, format!("{}; leaving `{}` offline", reason, label(source_name)));
                add_offline_source(&mut session, source_name, offline_path(source_name, media_dir), *required)
            }
        };
        sources.insert(source_key.clone(), imported);
    }

    let mut clip_count = 0usize;
    let mut stereo_sources = HashMap::new();
    let mut used_source_ids = HashSet::new();
    for plan in plans {
        let track_format = if plan.stereo {
            ChannelFormat::Stereo
        } else {
            plan.clips
                .iter()
                .filter_map(|clip| sources.get(&clip.source_key))
                .find(|source| source.loaded)
                .map_or(ChannelFormat::Mono, |source| ChannelFormat::for_channels(source.channels))
        };
        let track_id = session.add_track(TrackKind::Audio, track_format, Some(&plan.name));
        for clip in plan.clips {
            let source = clip
                .right_source_key
                .as_deref()
                .and_then(|right_key| {
                    stereo_source(
                        &mut session,
                        &sources,
                        &mut stereo_sources,
                        &clip.source_key,
                        right_key,
                        &clip.source_name,
                        clip.right_source_name.as_deref().unwrap_or("AAF right channel"),
                    )
                })
                .or_else(|| sources.get(&clip.source_key).copied());
            let Some(source) = source else {
                warning(&mut warnings, format!("AAF clip `{}` has no usable source reference", label(&clip.name)));
                continue;
            };
            let mut length = clip.length;
            if source.loaded {
                if clip.offset >= source.frames {
                    warning(
                        &mut warnings,
                        format!("AAF clip `{}` starts beyond decoded WAV `{}` and was skipped", label(&clip.name), label(&clip.source_name)),
                    );
                    continue;
                }
                let available = source.frames.saturating_sub(clip.offset);
                if length > available {
                    warning(
                        &mut warnings,
                        format!("AAF clip `{}` exceeds decoded WAV `{}` and was shortened", label(&clip.name), label(&clip.source_name)),
                    );
                    length = available;
                }
            }
            if length <= 0 {
                continue;
            }
            let id = session.new_clip_id();
            crate::edit::place_clip(&mut session, track_id, Clip::audio(id, clip.name, source.id, clip.offset, clip.start, length));
            used_source_ids.insert(source.id);
            clip_count = clip_count.saturating_add(1);
        }
    }

    if clip_count == 0 {
        return Err(EngineError::Unsupported("AAF source clips could not be represented safely; no SoundCraft session was written".into()));
    }
    session.sources.retain(|source| used_source_ids.contains(&source.id));
    for source_id in session.pool.ids().collect::<Vec<_>>() {
        if !used_source_ids.contains(&source_id) {
            session.pool.remove(source_id);
        }
    }
    for message in &warnings {
        engine.message(message.clone());
    }
    engine.replace_session(session);
    // The imported session is a new document: saving must not overwrite the one it replaced.
    engine.path = None;
    Ok(AafImportReport {
        composition: session_name,
        sample_rate: sample_rate.hz(),
        audio_tracks: tracks.len(),
        clips: clip_count,
        linked_wav_files,
        warnings,
    })
}

/// Read a Pro Tools (or other DAW) plain-text session report and keep only the lines that
/// mention a migration-relevant field, as safe, verbatim, user-facing notes.
///
/// This never reconstructs session state from the report and never touches the engine's
/// document: it only highlights lines a person migrating a session may want to read (sample
/// rate, bit depth, timecode, track names, mute state, plug-in listings) so they can follow up
/// manually. The report is treated purely as text; nothing here parses or assumes a specific
/// proprietary layout.
pub fn session_report_notes(path: &Path) -> Result<Vec<String>> {
    let len = crate::io::file_len(path)?;
    if len > MAX_SESSION_REPORT_BYTES {
        return Err(EngineError::Unsupported(format!(
            "{}: session reports larger than {} MiB are not accepted",
            path.display(),
            MAX_SESSION_REPORT_BYTES / (1024 * 1024)
        )));
    }
    let bytes = crate::io::read_file(path)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut notes = Vec::new();
    for line in text.lines().take(MAX_SESSION_REPORT_LINES) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if SESSION_REPORT_KEYWORDS.iter().any(|keyword| lower.contains(keyword)) {
            if notes.len() < MAX_SESSION_REPORT_NOTES.saturating_sub(1) {
                notes.push(label(trimmed));
            } else if notes.len() == MAX_SESSION_REPORT_NOTES.saturating_sub(1) {
                notes.push("further session-report notes were suppressed".into());
            }
        }
    }
    Ok(notes)
}

fn read_aaf(input: &Path) -> Result<AafModel> {
    let (file, len) = ConfinedHandle::open(input)?;
    if len > MAX_AAF_FILE_BYTES {
        return Err(EngineError::Unsupported(format!(
            "{}: AAF files larger than {} MiB are not accepted by this initial importer",
            input.display(),
            MAX_AAF_FILE_BYTES / (1024 * 1024)
        )));
    }
    let mut cfb = preflight_cfb(file, len, input)?;
    build_aaf_model(&mut cfb, input)
}

fn session_rate(tracks: &[TimelineTrack], warnings: &mut Vec<String>) -> Result<SampleRate> {
    let mut rates = tracks.iter().filter_map(|track| rational_sample_rate(track.edit_rate.0, track.edit_rate.1)).collect::<Vec<_>>();
    rates.sort_unstable();
    rates.dedup();
    let rate = rates.first().copied().unwrap_or(48_000);
    if rates.is_empty() {
        warning(warnings, "AAF audio edit-rate metadata was absent or not a supported integer sample rate; using 48000 Hz".into());
    } else if rates.len() > 1 {
        warning(warnings, format!("AAF audio tracks declare multiple edit rates ({rates:?}); using {rate} Hz and resampling linked WAV files"));
    }
    SampleRate::new(rate).map_err(|err| EngineError::Unsupported(format!("AAF sample rate {rate}: {err}")))
}

fn rational_sample_rate(numerator: i32, denominator: i32) -> Option<u32> {
    let numerator = u32::try_from(numerator).ok()?;
    let denominator = u32::try_from(denominator).ok()?;
    if denominator == 0 || numerator % denominator != 0 {
        return None;
    }
    let rate = numerator / denominator;
    SampleRate::new(rate).ok().map(|rate| rate.hz())
}

/// AAF `TimelineMobSlot.EditRate` normally describes edit units (often video frames), not PCM
/// samples. When the user supplies external media, its WAV header is authoritative for the
/// SoundCraft session rate; this also prevents an otherwise unnecessary resample on import.
fn media_sample_rate(media_dir: Option<&Path>) -> Result<Option<SampleRate>> {
    let Some(media_dir) = media_dir else {
        return Ok(None);
    };
    let mut entries = crate::io::list_dir(media_dir, MAX_AAF_COLLECTION_ENTRIES)
        .map_err(|err| EngineError::Unsupported(format!("cannot list AAF media directory `{}`: {err}", media_dir.display())))?;
    // Directory order is unspecified: sort so the chosen rate does not vary between runs.
    entries.sort();
    for path in entries {
        let is_wav = path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| extension.eq_ignore_ascii_case("wav"));
        if !is_wav || crate::file_access::check_read(&path).is_err() {
            continue;
        }
        // An unreadable or unrelated WAV only leaves its own source offline (see `load_wav_source`).
        if crate::io::file_len(&path).map_or(true, |len| len > MAX_AAF_MEDIA_BYTES) {
            continue;
        }
        let Ok(bytes) = crate::io::read_file(&path) else { continue };
        let Ok((info, _)) = soundcraft_audio_io::decode(&bytes, Some("wav")) else { continue };
        if info.format == soundcraft_audio_io::FileFormat::Wav
            && let Ok(rate) = SampleRate::new(info.sample_rate)
        {
            return Ok(Some(rate));
        }
    }
    Ok(None)
}

fn plan_tracks(mobs: &HashMap<[u8; 32], MobInfo>, tracks: &[TimelineTrack], session_rate: u32, warnings: &mut Vec<String>) -> Result<Vec<TrackPlan>> {
    let mut all = Vec::new();
    let mut total_clips = 0usize;
    for track in tracks {
        let mut cursor = track.origin;
        let mut clips = Vec::new();
        if track.components.len() > MAX_AAF_CLIPS {
            return Err(EngineError::Unsupported(format!("AAF track `{}` has more than {MAX_AAF_CLIPS} components", label(&track.name))));
        }
        for component in &track.components {
            let length = component.length;
            if length <= 0 {
                warning(warnings, format!("AAF track `{}` has a component with no positive duration", label(&track.name)));
            } else if let ComponentKind::SourceClip { start_time, source_mob_id, source_slot_id } = &component.kind {
                let start = units_to_samples(cursor, track.edit_rate.0, track.edit_rate.1, session_rate)?;
                let (resolved_mob_id, resolved_start_time) =
                    resolve_source_reference(mobs, *source_mob_id, *source_slot_id, *start_time, &track.name, warnings)?;
                let offset = units_to_samples(resolved_start_time, track.edit_rate.0, track.edit_rate.1, session_rate)?;
                let duration = units_to_samples(length, track.edit_rate.0, track.edit_rate.1, session_rate)?;
                if offset < 0 || duration <= 0 {
                    warning(warnings, format!("AAF source clip on `{}` has an unsupported negative offset or empty duration", label(&track.name)));
                } else {
                    let source_key = mob_id_key(&resolved_mob_id);
                    let source_name = mobs
                        .get(&resolved_mob_id)
                        .map(|mob| mob.locator_url.as_deref().and_then(locator_file_name).unwrap_or_else(|| nonempty_label(&mob.name, "AAF source")))
                        .unwrap_or_else(|| format!("AAF source {source_key}"));
                    clips.push(ClipPlan {
                        source_key,
                        source_name: source_name.clone(),
                        name: source_name,
                        start,
                        offset,
                        length: duration,
                        right_source_key: None,
                        right_source_name: None,
                    });
                    total_clips = total_clips.saturating_add(1);
                    if total_clips > MAX_AAF_CLIPS {
                        return Err(EngineError::Unsupported(format!("AAF composition has more than {MAX_AAF_CLIPS} source clips")));
                    }
                }
            } else {
                warning(
                    warnings,
                    format!(
                        "AAF `{}` component on `{}` is not rendered by this initial importer; its declared duration is retained for following clips",
                        component_name(&component.kind),
                        label(&track.name)
                    ),
                );
            }
            cursor = cursor
                .checked_add(length.max(0))
                .ok_or_else(|| EngineError::Unsupported(format!("AAF track `{}` has a timeline position overflow", label(&track.name))))?;
        }
        all.push(TrackPlan { name: nonempty_label(&track.name, "AAF audio"), clips, stereo: false });
    }
    Ok(all)
}

/// Pro Tools exports a stereo audio track as adjacent, independently-addressable left/right
/// TimelineMobSlots with the same slot name. Pair only those duplicate-name neighbours whose
/// clip placement agrees, retaining the original mono tracks whenever the AAF does not prove
/// that relationship.
fn pair_stereo_tracks(mut plans: Vec<TrackPlan>, warnings: &mut Vec<String>) -> Vec<TrackPlan> {
    let mut paired = Vec::with_capacity(plans.len());
    while !plans.is_empty() {
        let mut left = plans.remove(0);
        let Some(right) = plans.first() else {
            paired.push(left);
            continue;
        };
        if right.name != left.name || left.clips.len() != right.clips.len() {
            paired.push(left);
            continue;
        }
        let aligned = left.clips.iter().zip(&right.clips).all(|(left_clip, right_clip)| {
            left_clip.start == right_clip.start && left_clip.offset == right_clip.offset && left_clip.length == right_clip.length
        });
        if !aligned {
            warning(
                warnings,
                format!("AAF stereo candidates `{}` and `{}` are not clip-aligned; they were retained as mono tracks", left.name, right.name),
            );
            paired.push(left);
            continue;
        }
        let right = plans.remove(0);
        for (left_clip, right_clip) in left.clips.iter_mut().zip(right.clips) {
            left_clip.right_source_key = Some(right_clip.source_key);
            left_clip.right_source_name = Some(right_clip.source_name);
        }
        left.stereo = true;
        paired.push(left);
    }
    paired
}

/// Creates one portable stereo source from a verified pair of decoded mono WAVs. It never
/// guesses channel ordering: only one-channel, equal-rate sources are combined; callers keep
/// their left channel when either side is unavailable or malformed.
fn stereo_source(
    session: &mut soundcraft_model::Session,
    sources: &HashMap<String, ImportedSource>,
    cache: &mut HashMap<(String, String), ImportedSource>,
    left_key: &str,
    right_key: &str,
    left_name: &str,
    right_name: &str,
) -> Option<ImportedSource> {
    let cache_key = (left_key.to_owned(), right_key.to_owned());
    if let Some(source) = cache.get(&cache_key).copied() {
        return Some(source);
    }
    let (left, right) = (sources.get(left_key).copied()?, sources.get(right_key).copied()?);
    if !left.loaded || !right.loaded || left.channels != 1 || right.channels != 1 {
        return None;
    }
    let (left_channel, right_channel) = {
        let left_audio = session.pool.get(left.id)?;
        let right_audio = session.pool.get(right.id)?;
        if left_audio.buffer.sample_rate != right_audio.buffer.sample_rate {
            return None;
        }
        let left_channel = left_audio.buffer.channels.first()?.clone();
        let right_channel = right_audio.buffer.channels.first()?.clone();
        (left_channel, right_channel)
    };
    let frames = left_channel.len().min(right_channel.len());
    if frames == 0 {
        return None;
    }
    let sample_rate = session.sample_rate.hz();
    let id = crate::io::add_source(
        session,
        &format!("{} + {}", left_name.trim_end_matches(".wav"), right_name.trim_end_matches(".wav")),
        AudioBuffer {
            sample_rate,
            channels: vec![left_channel.into_iter().take(frames).collect(), right_channel.into_iter().take(frames).collect()],
        },
        None,
        soundcraft_audio_io::FileFormat::Wav,
    );
    let source = ImportedSource { id, frames: i64::try_from(frames).ok()?, channels: 2, loaded: true };
    cache.insert(cache_key, source);
    Some(source)
}

/// Resolves the indirection normally emitted by Pro Tools: a composition SourceClip points to
/// a `MasterMob` slot, whose own SourceClip then points at the `SourceMob` holding the external
/// WAV locator. The outer clip owns the timeline duration; each hop contributes only its source
/// offset. Cycles and ambiguous slots are retained as offline references instead of guessing.
fn resolve_source_reference(
    mobs: &HashMap<[u8; 32], MobInfo>,
    mut mob_id: [u8; 32],
    mut slot_id: u32,
    mut start_time: i64,
    track_name: &str,
    warnings: &mut Vec<String>,
) -> Result<([u8; 32], i64)> {
    let mut seen = HashSet::new();
    for _ in 0..MAX_AAF_CLASS_DEPTH {
        if !seen.insert((mob_id, slot_id)) {
            return Err(EngineError::Unsupported(format!("AAF source references form a cycle on `{}`", label(track_name))));
        }
        let Some(mob) = mobs.get(&mob_id) else {
            warning(warnings, format!("AAF source mob for `{}` was not found; its clip was kept offline", label(track_name)));
            return Ok((mob_id, start_time));
        };
        if mob.locator_url.is_some() {
            return Ok((mob_id, start_time));
        }
        let Some(components) = mob.slots.get(&slot_id) else {
            warning(warnings, format!("AAF source slot {slot_id} on `{}` could not be resolved; its clip was kept offline", label(&mob.name)));
            return Ok((mob_id, start_time));
        };
        let mut source_clips = components.iter().filter_map(|component| match &component.kind {
            ComponentKind::SourceClip { start_time, source_mob_id, source_slot_id } => Some((*start_time, *source_mob_id, *source_slot_id)),
            _ => None,
        });
        let Some((next_start, next_mob_id, next_slot_id)) = source_clips.next() else {
            return Ok((mob_id, start_time));
        };
        if source_clips.next().is_some() {
            warning(
                warnings,
                format!("AAF source slot {slot_id} on `{}` contains multiple source clips; its clip was kept offline", label(&mob.name)),
            );
            return Ok((mob_id, start_time));
        }
        start_time = start_time.checked_add(next_start).ok_or_else(|| EngineError::Unsupported("AAF source offset overflows".into()))?;
        mob_id = next_mob_id;
        slot_id = next_slot_id;
    }
    Err(EngineError::Unsupported(format!("AAF source reference depth exceeds {MAX_AAF_CLASS_DEPTH} on `{}`", label(track_name))))
}

fn component_name(kind: &ComponentKind) -> &'static str {
    match kind {
        ComponentKind::SourceClip { .. } => "source clip",
        ComponentKind::Filler => "filler",
        ComponentKind::Transition => "transition",
        ComponentKind::Effect => "effect",
    }
}

/// A short, stable, printable key for a raw 32-byte MobID, used only to group clips by source
/// and to label a source whose Mob could not be found by name; not a parsed/validated MobID.
fn mob_id_key(mob_id: &[u8; 32]) -> String {
    mob_id.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Extracts and percent-decodes the final path segment of a `NetworkLocator.URLString` value
/// (e.g. `file:///C:/Media/Vocal_01.wav` -> `Vocal_01.wav`), used only as a candidate WAV
/// filename to look up under `--media-dir`; never used for path confinement itself (the actual
/// file read is confined separately by `wav_candidate`/`crate::file_access::check_read`).
fn locator_file_name(url: &str) -> Option<String> {
    let segment = url.rsplit(['/', '\\']).next()?;
    (!segment.is_empty()).then(|| percent_decode(segment))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index.saturating_add(2) < bytes.len()
            && bytes[index + 1].is_ascii_hexdigit()
            && bytes[index + 2].is_ascii_hexdigit()
        {
            out.push((hex_val(bytes[index + 1]) << 4) | hex_val(bytes[index + 2]));
            index = index.saturating_add(3);
        } else {
            out.push(bytes[index]);
            index = index.saturating_add(1);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn units_to_samples(units: i64, numerator: i32, denominator: i32, session_rate: u32) -> Result<Samples> {
    if numerator <= 0 || denominator <= 0 {
        return Err(EngineError::Unsupported("AAF has an invalid audio edit rate".into()));
    }
    let sign = if units < 0 { -1i128 } else { 1i128 };
    let magnitude = i128::from(units).unsigned_abs();
    let scaled = magnitude
        .checked_mul(u128::try_from(denominator).unwrap_or(0))
        .and_then(|value| value.checked_mul(u128::from(session_rate)))
        .ok_or_else(|| EngineError::Unsupported("AAF timeline value is too large".into()))?;
    let divisor = u128::try_from(numerator).unwrap_or(1);
    let rounded = scaled
        .checked_add(divisor / 2)
        .map(|value| value / divisor)
        .ok_or_else(|| EngineError::Unsupported("AAF timeline value is too large".into()))?;
    let rounded = i128::try_from(rounded).map_err(|_| EngineError::Unsupported("AAF timeline value is too large".into()))?;
    let samples = rounded.checked_mul(sign).ok_or_else(|| EngineError::Unsupported("AAF timeline value is too large".into()))?;
    let samples = i64::try_from(samples).map_err(|_| EngineError::Unsupported("AAF timeline value is too large".into()))?;
    if !(-MAX_POSITION..=MAX_POSITION).contains(&samples) {
        return Err(EngineError::Unsupported(format!("AAF timeline value exceeds SoundCraft's {}-sample safety limit", MAX_POSITION)));
    }
    Ok(samples)
}

fn load_wav_source(
    session: &mut soundcraft_model::Session,
    source_name: &str,
    media_dir: Option<&Path>,
) -> std::result::Result<Option<ImportedSource>, String> {
    let Some(media_dir) = media_dir else {
        return Ok(None);
    };
    let Some(path) = wav_candidate(media_dir, source_name) else {
        return Ok(None);
    };
    crate::file_access::check_read(&path)?;
    let len = crate::io::file_len(&path).map_err(|err| err.to_string())?;
    if len > MAX_AAF_MEDIA_BYTES {
        return Err(format!("{} is larger than the {} MiB WAV import limit", path.display(), MAX_AAF_MEDIA_BYTES / (1024 * 1024)));
    }
    let bytes = crate::io::read_file(&path).map_err(|err| err.to_string())?;
    let (info, buffer) = soundcraft_audio_io::decode(&bytes, Some("wav")).map_err(|err| format!("{}: {err}", path.display()))?;
    if info.format != soundcraft_audio_io::FileFormat::Wav {
        return Err(format!("{} is not WAV audio", path.display()));
    }
    let buffer = crate::io::at_rate(buffer, session.sample_rate.hz(), MAX_AAF_MEDIA_SAMPLES)
        .ok_or_else(|| format!("{} decodes to too many samples", path.display()))?;
    if buffer.frames() == 0 || buffer.num_channels() == 0 {
        return Err(format!("{} has no audio", path.display()));
    }
    let frames = i64::try_from(buffer.frames()).map_err(|_| format!("{} is too long", path.display()))?;
    let channels = buffer.num_channels();
    let id = crate::io::add_source(session, source_name, buffer, Some(&path.to_string_lossy()), soundcraft_audio_io::FileFormat::Wav);
    Ok(Some(ImportedSource { id, frames, channels, loaded: true }))
}

fn wav_candidate(media_dir: &Path, source_name: &str) -> Option<PathBuf> {
    let file_name = Path::new(source_name).file_name()?;
    let is_wav = file_name
        .to_str()
        .and_then(|name| Path::new(name).extension())
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"));
    is_wav.then(|| media_dir.join(file_name))
}

fn offline_path(source_name: &str, media_dir: Option<&Path>) -> String {
    let file_name = Path::new(source_name)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "AAF source.wav".into());
    media_dir
        .map(|dir| dir.join(&file_name).to_string_lossy().into_owned())
        .unwrap_or_else(|| Path::new("AAF Media").join(file_name).to_string_lossy().into_owned())
}

fn offline_source_warning(source_name: &str, media_dir: Option<&Path>) -> String {
    match media_dir {
        Some(dir) if wav_candidate(dir, source_name).is_none() => {
            format!("AAF source `{}` is not named as a WAV file, so it was kept offline", label(source_name))
        }
        Some(dir) => {
            format!(
                "AAF WAV `{}` was not found in `{}`; keep the WAV files in the same folder as the AAF. Its clip reference was kept offline",
                label(source_name),
                label(&dir.to_string_lossy())
            )
        }
        None => format!("AAF source `{}` was kept offline; keep its exact WAV filename in the same folder as the AAF to link it", label(source_name)),
    }
}

fn add_offline_source(session: &mut soundcraft_model::Session, name: &str, path: String, required_frames: Samples) -> ImportedSource {
    let id = SourceId(session.alloc());
    let frames = required_frames.max(1);
    session.sources.push(Source {
        id,
        name: nonempty_label(name, "AAF source"),
        path,
        channels: 1,
        frames: u64::try_from(frames).unwrap_or(u64::MAX),
        sample_rate: session.sample_rate.hz(),
        format: soundcraft_audio_io::FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
    });
    ImportedSource { id, frames, channels: 1, loaded: false }
}

fn warning(warnings: &mut Vec<String>, message: String) {
    if warnings.len() < MAX_WARNINGS.saturating_sub(1) {
        warnings.push(message);
    } else if warnings.len() == MAX_WARNINGS.saturating_sub(1) {
        warnings.push("further AAF import warnings were suppressed".into());
    }
}

fn label(text: &str) -> String {
    const LIMIT: usize = 120;
    let mut text = text.chars().take(LIMIT).collect::<String>();
    if text.is_empty() {
        text = "unnamed".into();
    }
    text
}

fn nonempty_label(text: &str, fallback: &str) -> String {
    let text = label(text);
    if text == "unnamed" { fallback.into() } else { text }
}

/// Every CFB storage/stream reachable from the root, plus the allocation tables needed to
/// read any stream's bytes on demand. Built once by `preflight_cfb` after validating the whole
/// directory tree and FAT/mini-FAT chains, then reused for every subsequent AAF object read.
struct CfbContainer {
    file: ConfinedHandle,
    entries: Vec<DirectoryEntry>,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    sector_count: usize,
    input: PathBuf,
    root_ministream: Option<Vec<u8>>,
}

impl CfbContainer {
    /// Bounded, cycle-safe listing of a storage's immediate children (its `child` red-black
    /// tree node plus that node's own left/right siblings, walked iteratively).
    fn children(&self, parent: usize) -> Result<Vec<usize>> {
        let Some(parent_entry) = self.entries.get(parent) else {
            return Err(EngineError::Unsupported("AAF CFB directory reference is out of range".into()));
        };
        let mut result = Vec::new();
        if parent_entry.child == CFB_NO_ENTRY {
            return Ok(result);
        }
        let mut stack = vec![parent_entry.child];
        let mut seen = HashSet::new();
        while let Some(index) = stack.pop() {
            if index == CFB_NO_ENTRY {
                continue;
            }
            let index = usize::try_from(index).map_err(|_| EngineError::Unsupported("AAF CFB directory index out of range".into()))?;
            if !seen.insert(index) {
                return Err(EngineError::Unsupported("AAF CFB directory tree has a cycle".into()));
            }
            if result.len() >= MAX_AAF_DIRECTORY_ENTRIES {
                return Err(EngineError::Unsupported(format!("AAF storage has more than {MAX_AAF_DIRECTORY_ENTRIES} entries")));
            }
            let Some(entry) = self.entries.get(index) else {
                return Err(EngineError::Unsupported("AAF CFB directory reference is out of range".into()));
            };
            result.push(index);
            stack.push(entry.left);
            stack.push(entry.right);
        }
        Ok(result)
    }

    /// Finds a direct child of `parent` by exact, case-insensitive name match, as AAF/CFB
    /// storage and stream names are compared case-insensitively per the specification.
    fn find_child(&mut self, parent: usize, name: &str) -> Result<Option<usize>> {
        for index in self.children(parent)? {
            let Some(entry) = self.entries.get(index) else { continue };
            if entry.name.eq_ignore_ascii_case(name) {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    /// Populates the cached root mini-stream (the root directory entry's own regular-stream
    /// contents, which every mini-stream entry is a slice of) if it has not been read yet.
    /// Split from `stream_bytes` so callers can borrow `self.mini_fat`/`self.input` immutably
    /// afterwards without holding a mutable borrow across the call.
    fn ensure_root_ministream(&mut self) -> Result<()> {
        if self.root_ministream.is_none() {
            let Some(root) = self.entries.first().cloned() else {
                return Err(EngineError::Unsupported("AAF CFB has no root directory entry".into()));
            };
            let bytes = read_regular_stream_bytes(&mut self.file, &root, &self.fat, self.sector_count, &self.input, u64::MAX)?;
            self.root_ministream = Some(bytes);
        }
        Ok(())
    }

    /// Reads a stream directory entry's full contents, routing through the mini-stream for
    /// small streams and the regular FAT chain otherwise, exactly as the CFB specification
    /// requires, bounded by `MAX_AAF_METADATA_STREAM_BYTES`.
    fn stream_bytes(&mut self, index: usize) -> Result<Vec<u8>> {
        let Some(entry) = self.entries.get(index).cloned() else {
            return Err(EngineError::Unsupported("AAF CFB directory reference is out of range".into()));
        };
        if entry.kind != 2 {
            return Err(EngineError::Unsupported(format!("{}: AAF CFB entry is not a stream", self.input.display())));
        }
        if entry.size > MAX_AAF_METADATA_STREAM_BYTES {
            return Err(EngineError::Unsupported(format!("{}: AAF metadata stream is implausibly large", self.input.display())));
        }
        if entry.size < 4096 {
            self.ensure_root_ministream()?;
            let ministream = self.root_ministream.as_deref().unwrap_or_default();
            let start = usize::try_from(entry.start_sector).unwrap_or(usize::MAX);
            let size = usize::try_from(entry.size).unwrap_or(0);
            return read_mini_stream_bytes(ministream, start, size, &self.mini_fat, &self.input);
        }
        read_regular_stream_bytes(&mut self.file, &entry, &self.fat, self.sector_count, &self.input, MAX_AAF_METADATA_STREAM_BYTES)
    }

    /// A strong-reference vector (`SF_STRONG_OBJECT_REFERENCE_VECTOR`): an ordered list of
    /// child storage indices, named `"{index_name}{hex(local_key)}"` (the hex digits themselves
    /// wrapped in a literal `{`/`}` pair, per the reference AAF toolkit's own naming convention)
    /// and indexed by a sibling `"{index_name} index"` stream.
    fn strong_ref_vector(&mut self, obj: &RawObject, pid: u16) -> Result<Vec<usize>> {
        let Some(index_name) = obj.collection_index_name(pid)? else {
            return Ok(Vec::new());
        };
        let Some(index_entry) = self.find_child(obj.storage, &format!("{index_name} index"))? else {
            return Ok(Vec::new());
        };
        let bytes = self.stream_bytes(index_entry)?;
        if bytes.len() < 12 {
            return Err(EngineError::Unsupported("AAF strong-reference vector index is too short".into()));
        }
        let count = usize::try_from(le_u32(&bytes, 0)?).map_err(|_| EngineError::Unsupported("AAF vector count out of range".into()))?;
        if count > MAX_AAF_COLLECTION_ENTRIES {
            return Err(EngineError::Unsupported(format!("AAF strong-reference vector has more than {MAX_AAF_COLLECTION_ENTRIES} entries")));
        }
        let mut result = Vec::with_capacity(count.min(MAX_AAF_COLLECTION_ENTRIES));
        for slot in 0..count {
            let offset = 12usize
                .checked_add(slot.saturating_mul(4))
                .ok_or_else(|| EngineError::Unsupported("AAF strong-reference vector index overflows".into()))?;
            let local_key = le_u32(&bytes, offset)?;
            let child_name = format!("{index_name}{{{local_key:x}}}");
            match self.find_child(obj.storage, &child_name)? {
                Some(child) => result.push(child),
                None => return Err(EngineError::Unsupported(format!("AAF strong reference `{child_name}` was not found"))),
            }
        }
        Ok(result)
    }

    /// A strong-reference set (`SF_STRONG_OBJECT_REFERENCE_SET`): like a vector, but each entry
    /// additionally carries the referenced object's own unique-key bytes (its `key`), letting
    /// callers look a member up by that key (e.g. a `SourceClip.SourceID` against
    /// `ContentStorage.Mobs`) without re-reading each member's own property.
    fn strong_ref_set(&mut self, obj: &RawObject, pid: u16) -> Result<Vec<(Vec<u8>, usize)>> {
        let Some(index_name) = obj.collection_index_name(pid)? else {
            return Ok(Vec::new());
        };
        let Some(index_entry) = self.find_child(obj.storage, &format!("{index_name} index"))? else {
            return Ok(Vec::new());
        };
        let bytes = self.stream_bytes(index_entry)?;
        if bytes.len() < 15 {
            return Err(EngineError::Unsupported("AAF strong-reference set index is too short".into()));
        }
        let count = usize::try_from(le_u32(&bytes, 0)?).map_err(|_| EngineError::Unsupported("AAF set count out of range".into()))?;
        if count > MAX_AAF_COLLECTION_ENTRIES {
            return Err(EngineError::Unsupported(format!("AAF strong-reference set has more than {MAX_AAF_COLLECTION_ENTRIES} entries")));
        }
        let key_size = usize::from(*bytes.get(14).ok_or_else(|| EngineError::Unsupported("AAF strong-reference set index is too short".into()))?);
        if key_size > 256 {
            return Err(EngineError::Unsupported("AAF strong-reference set key is implausibly large".into()));
        }
        let entry_size = 8usize.checked_add(key_size).ok_or_else(|| EngineError::Unsupported("AAF strong-reference set entry overflows".into()))?;
        let mut result = Vec::with_capacity(count.min(MAX_AAF_COLLECTION_ENTRIES));
        let mut offset = 15usize;
        for _ in 0..count {
            if bytes.len() < offset.checked_add(entry_size).ok_or_else(|| EngineError::Unsupported("AAF strong-reference set overflows".into()))? {
                return Err(EngineError::Unsupported("AAF strong-reference set index is truncated".into()));
            }
            let local_key = le_u32(&bytes, offset)?;
            let key = bytes.get(offset + 8..offset + entry_size).map(|slice| slice.to_vec()).unwrap_or_default();
            let child_name = format!("{index_name}{{{local_key:x}}}");
            match self.find_child(obj.storage, &child_name)? {
                Some(child) => result.push((key, child)),
                None => return Err(EngineError::Unsupported(format!("AAF strong reference `{child_name}` was not found"))),
            }
            offset = offset.checked_add(entry_size).ok_or_else(|| EngineError::Unsupported("AAF strong-reference set overflows".into()))?;
        }
        Ok(result)
    }

    /// A single strong object reference (`SF_STRONG_OBJECT_REFERENCE`): the property's payload
    /// is the referenced child storage's own name, used verbatim (not an index-stream lookup).
    fn strong_ref(&mut self, obj: &RawObject, pid: u16) -> Result<Option<usize>> {
        match obj.strong_ref_name(pid)? {
            Some(name) => self.find_child(obj.storage, &name),
            None => Ok(None),
        }
    }
}

/// Reads a regular (non-mini) FAT-chain stream's bytes in full, bounded by `limit`.
fn read_regular_stream_bytes(
    file: &mut ConfinedHandle,
    entry: &DirectoryEntry,
    fat: &[u32],
    sector_count: usize,
    input: &Path,
    limit: u64,
) -> Result<Vec<u8>> {
    if entry.size > limit {
        return Err(EngineError::Unsupported(format!("{}: AAF stream exceeds its size limit", input.display())));
    }
    let chain = walk_chain(entry.start_sector, fat, sector_count, &entry.name)?;
    let mut bytes = Vec::with_capacity(usize::try_from(entry.size).unwrap_or(0).min(MAX_AAF_METADATA_STREAM_BYTES as usize));
    let mut remaining = entry.size;
    for sector_id in chain {
        let sector = read_sector(file, sector_id, input)?;
        let take = remaining.min(CFB_SECTOR_SIZE);
        let take_usize = usize::try_from(take).unwrap_or(0);
        bytes.extend_from_slice(sector.get(..take_usize).unwrap_or(&sector));
        remaining = remaining.saturating_sub(take);
    }
    Ok(bytes)
}

/// Reads a mini-stream (small-stream) entry's bytes from the already-loaded root mini-stream
/// buffer, following the mini-FAT chain starting at `start_sector` (a 64-byte mini-sector
/// index into `ministream`).
fn read_mini_stream_bytes(ministream: &[u8], start_sector: usize, size: usize, mini_fat: &[u32], input: &Path) -> Result<Vec<u8>> {
    const MINI_SECTOR_SIZE: usize = 64;
    let mut bytes = Vec::with_capacity(size.min(MAX_AAF_METADATA_STREAM_BYTES as usize));
    if size == 0 {
        return Ok(bytes);
    }
    let needed = size.div_ceil(MINI_SECTOR_SIZE);
    let mut sector =
        u32::try_from(start_sector).map_err(|_| EngineError::Unsupported(format!("{}: AAF mini-stream sector overflows", input.display())))?;
    let mut seen = HashSet::new();
    let mut remaining = size;
    for _ in 0..needed {
        if is_chain_end(sector) {
            return Err(EngineError::Unsupported(format!("{}: AAF mini-stream is truncated", input.display())));
        }
        let index =
            usize::try_from(sector).map_err(|_| EngineError::Unsupported(format!("{}: AAF mini-stream sector overflows", input.display())))?;
        if !seen.insert(index) {
            return Err(EngineError::Unsupported(format!("{}: AAF mini-stream has a cycle", input.display())));
        }
        let start = index
            .checked_mul(MINI_SECTOR_SIZE)
            .ok_or_else(|| EngineError::Unsupported(format!("{}: AAF mini-stream offset overflows", input.display())))?;
        let end = start
            .checked_add(MINI_SECTOR_SIZE)
            .ok_or_else(|| EngineError::Unsupported(format!("{}: AAF mini-stream offset overflows", input.display())))?;
        let chunk = ministream
            .get(start..end)
            .ok_or_else(|| EngineError::Unsupported(format!("{}: AAF mini-stream is outside the root stream", input.display())))?;
        let take = remaining.min(MINI_SECTOR_SIZE);
        bytes.extend_from_slice(&chunk[..take]);
        remaining = remaining.saturating_sub(take);
        sector = mini_fat
            .get(index)
            .copied()
            .ok_or_else(|| EngineError::Unsupported(format!("{}: AAF mini-stream chain is outside the mini-FAT", input.display())))?;
    }
    Ok(bytes)
}

fn preflight_cfb(mut file: ConfinedHandle, file_len: u64, input: &Path) -> Result<CfbContainer> {
    if file_len < CFB_SECTOR_SIZE {
        return Err(EngineError::Unsupported(format!("{}: invalid CFB container length", input.display())));
    }
    // Several real-world AAF writers (observed from a Pro Tools export) stop writing a file
    // right after its last meaningful byte instead of padding the final sector out to
    // `CFB_SECTOR_SIZE`: the last addressable sector can be a few hundred bytes short. Counting
    // sectors by ceiling division (rather than requiring an exact multiple) makes that final,
    // partially-written sector addressable; `read_sector` separately zero-pads just that one
    // read instead of failing it. Every earlier sector is still required to be fully present:
    // the arithmetic below guarantees a short read is only ever possible for sector
    // `sector_count - 1`, so this never masks a genuinely truncated or corrupt container.
    let sector_count = usize::try_from((file_len - CFB_SECTOR_SIZE).div_ceil(CFB_SECTOR_SIZE))
        .map_err(|_| EngineError::Unsupported(format!("{}: CFB is too large", input.display())))?;
    if sector_count == 0 || sector_count > MAX_AAF_FILE_BYTES as usize / CFB_SECTOR_SIZE as usize {
        return Err(EngineError::Unsupported(format!("{}: invalid CFB sector count", input.display())));
    }

    let mut header = [0u8; CFB_SECTOR_SIZE as usize];
    file.seek(SeekFrom::Start(0)).and_then(|_| file.read_exact(&mut header)).map_err(|err| EngineError::Io(format!("{}: {err}", input.display())))?;
    if header.get(..8) != Some(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return Err(EngineError::Unsupported(format!("{}: not an AAF compound-file container", input.display())));
    }
    let major = le_u16(&header, 26)?;
    let byte_order = le_u16(&header, 28)?;
    let sector_shift = le_u16(&header, 30)?;
    let mini_sector_shift = le_u16(&header, 32)?;
    if !matches!(major, 3 | 4) || byte_order != 0xFFFE || sector_shift != 9 || mini_sector_shift != 6 {
        return Err(EngineError::Unsupported(format!("{}: unsupported CFB sector layout", input.display())));
    }
    if le_u32(&header, 56)? != 4096 {
        return Err(EngineError::Unsupported(format!("{}: unsupported CFB mini-stream cutoff", input.display())));
    }

    let fat_count =
        usize::try_from(le_u32(&header, 44)?).map_err(|_| EngineError::Unsupported(format!("{}: invalid CFB FAT count", input.display())))?;
    let directory_start = le_u32(&header, 48)?;
    let mini_fat_start = le_u32(&header, 60)?;
    let mini_fat_count =
        usize::try_from(le_u32(&header, 64)?).map_err(|_| EngineError::Unsupported(format!("{}: invalid CFB mini-FAT count", input.display())))?;
    let difat_start = le_u32(&header, 68)?;
    let difat_count =
        usize::try_from(le_u32(&header, 72)?).map_err(|_| EngineError::Unsupported(format!("{}: invalid CFB DIFAT count", input.display())))?;
    if fat_count > sector_count || mini_fat_count > sector_count || difat_count > sector_count {
        return Err(EngineError::Unsupported(format!("{}: impossible CFB allocation counts", input.display())));
    }

    let mut fat_sectors = Vec::new();
    for offset in (76..CFB_SECTOR_SIZE as usize).step_by(4) {
        let sector = le_u32(&header, offset)?;
        if sector != CFB_FREE_SECTOR {
            valid_sector(sector, sector_count, "FAT")?;
            fat_sectors.push(sector);
        }
    }
    let mut next_difat = difat_start;
    let mut seen_difat = HashSet::new();
    for index in 0..difat_count {
        valid_sector(next_difat, sector_count, "DIFAT")?;
        if !seen_difat.insert(next_difat) {
            return Err(EngineError::Unsupported("AAF CFB DIFAT has a cycle".into()));
        }
        let sector = read_sector(&mut file, next_difat, input)?;
        for offset in (0..508).step_by(4) {
            let fat_sector = le_u32(&sector, offset)?;
            if fat_sector != CFB_FREE_SECTOR {
                valid_sector(fat_sector, sector_count, "FAT")?;
                fat_sectors.push(fat_sector);
            }
        }
        let following = le_u32(&sector, 508)?;
        if index.saturating_add(1) == difat_count {
            if !is_chain_end(following) {
                return Err(EngineError::Unsupported("AAF CFB DIFAT count does not terminate its chain".into()));
            }
        } else {
            valid_sector(following, sector_count, "DIFAT")?;
        }
        next_difat = following;
    }
    if difat_count == 0 && !is_chain_end(difat_start) {
        return Err(EngineError::Unsupported("AAF CFB has an unexpected DIFAT chain".into()));
    }
    if fat_sectors.len() != fat_count {
        return Err(EngineError::Unsupported("AAF CFB FAT count does not match its DIFAT".into()));
    }
    let mut unique_fat = HashSet::new();
    if fat_sectors.iter().any(|sector| !unique_fat.insert(*sector)) {
        return Err(EngineError::Unsupported("AAF CFB repeats a FAT sector".into()));
    }

    let mut fat = Vec::with_capacity(fat_sectors.len().saturating_mul(128));
    for sector_id in fat_sectors {
        let sector = read_sector(&mut file, sector_id, input)?;
        for offset in (0..CFB_SECTOR_SIZE as usize).step_by(4) {
            fat.push(le_u32(&sector, offset)?);
        }
    }

    let directory_chain = walk_chain(directory_start, &fat, sector_count, "directory")?;
    if directory_chain.is_empty() {
        return Err(EngineError::Unsupported("AAF CFB has no directory stream".into()));
    }
    let mut entries = Vec::new();
    for sector_id in directory_chain {
        let sector = read_sector(&mut file, sector_id, input)?;
        for offset in (0..CFB_SECTOR_SIZE as usize).step_by(128) {
            if entries.len() >= MAX_AAF_DIRECTORY_ENTRIES {
                return Err(EngineError::Unsupported(format!("AAF has more than {MAX_AAF_DIRECTORY_ENTRIES} compound-file entries")));
            }
            entries.push(parse_directory_entry(&sector, offset)?);
        }
    }
    let Some(root) = entries.first() else {
        return Err(EngineError::Unsupported("AAF CFB has no root directory entry".into()));
    };
    if root.kind != 5 {
        return Err(EngineError::Unsupported("AAF CFB has an invalid root directory entry".into()));
    }
    validate_directory_tree(&entries)?;

    let mini_fat_chain = if mini_fat_count == 0 {
        if !is_chain_end(mini_fat_start) {
            return Err(EngineError::Unsupported("AAF CFB has an unexpected mini-FAT chain".into()));
        }
        Vec::new()
    } else {
        let chain = walk_chain(mini_fat_start, &fat, sector_count, "mini-FAT")?;
        if chain.len() != mini_fat_count {
            return Err(EngineError::Unsupported("AAF CFB mini-FAT count does not match its chain".into()));
        }
        chain
    };
    let mut mini_fat = Vec::with_capacity(mini_fat_chain.len().saturating_mul(128));
    for sector_id in mini_fat_chain {
        let sector = read_sector(&mut file, sector_id, input)?;
        for offset in (0..CFB_SECTOR_SIZE as usize).step_by(4) {
            mini_fat.push(le_u32(&sector, offset)?);
        }
    }

    validate_regular_stream(root, &fat, sector_count, "root mini-stream")?;
    // Every stream (kind == 2) anywhere in the directory tree is validated here, regardless of
    // its name: real AAF files keep most metadata objects as *storages* (kind == 1), each
    // holding its own `"properties"` stream and any index/essence streams it needs, so a
    // name-based allowlist (as opposed to this generic, structural pass) would silently skip
    // validating the overwhelming majority of a real file's streams.
    for entry in &entries {
        if entry.kind == 2 {
            validate_stream(entry, root.size, &fat, &mini_fat, sector_count)?;
        }
    }
    Ok(CfbContainer { file, entries, fat, mini_fat, sector_count, input: input.to_path_buf(), root_ministream: None })
}

fn validate_directory_tree(entries: &[DirectoryEntry]) -> Result<()> {
    let mut state = vec![0u8; entries.len()];
    let mut stack = vec![(0usize, false)];
    while let Some((entry_id, finishing)) = stack.pop() {
        let Some(state_at) = state.get_mut(entry_id) else {
            return Err(EngineError::Unsupported("AAF CFB directory reference is out of range".into()));
        };
        if finishing {
            *state_at = 2;
            continue;
        }
        if *state_at == 1 {
            return Err(EngineError::Unsupported("AAF CFB directory tree has a cycle".into()));
        }
        if *state_at == 2 {
            continue;
        }
        *state_at = 1;
        stack.push((entry_id, true));
        let Some(entry) = entries.get(entry_id) else {
            return Err(EngineError::Unsupported("AAF CFB directory reference is out of range".into()));
        };
        for reference in [entry.left, entry.right, entry.child] {
            if reference != CFB_FREE_SECTOR {
                let reference =
                    usize::try_from(reference).map_err(|_| EngineError::Unsupported("AAF CFB directory reference is out of range".into()))?;
                if reference >= entries.len() {
                    return Err(EngineError::Unsupported("AAF CFB directory reference is out of range".into()));
                }
                stack.push((reference, false));
            }
        }
    }
    Ok(())
}

fn validate_stream(entry: &DirectoryEntry, root_size: u64, fat: &[u32], mini_fat: &[u32], sector_count: usize) -> Result<()> {
    if entry.kind != 2 {
        return Err(EngineError::Unsupported(format!("AAF CFB `{}` is not a stream", label(&entry.name))));
    }
    if entry.size > MAX_AAF_METADATA_STREAM_BYTES {
        return Err(EngineError::Unsupported(format!(
            "AAF `{}` stream exceeds the {} MiB metadata limit",
            label(&entry.name),
            MAX_AAF_METADATA_STREAM_BYTES / (1024 * 1024)
        )));
    }
    if entry.size == 0 {
        return Ok(());
    }
    if entry.size < 4096 {
        let chain = walk_chain(entry.start_sector, mini_fat, mini_fat.len(), "mini-stream")?;
        let needed =
            usize::try_from(entry.size.saturating_add(63) / 64).map_err(|_| EngineError::Unsupported("AAF mini-stream is too large".into()))?;
        if chain.len() != needed {
            return Err(EngineError::Unsupported(format!("AAF `{}` has a truncated mini-stream", label(&entry.name))));
        }
        let final_sector = chain.last().copied().unwrap_or(0);
        let end = u64::from(final_sector)
            .checked_add(1)
            .and_then(|sector| sector.checked_mul(64))
            .ok_or_else(|| EngineError::Unsupported("AAF mini-stream offset overflows".into()))?;
        if end > root_size {
            return Err(EngineError::Unsupported(format!("AAF `{}` mini-stream is outside the root stream", label(&entry.name))));
        }
    } else {
        validate_regular_stream(entry, fat, sector_count, &entry.name)?;
    }
    Ok(())
}

fn validate_regular_stream(entry: &DirectoryEntry, fat: &[u32], sector_count: usize, name: &str) -> Result<()> {
    if entry.size > MAX_AAF_METADATA_STREAM_BYTES {
        return Err(EngineError::Unsupported(format!(
            "AAF `{}` stream exceeds the {} MiB metadata limit",
            label(name),
            MAX_AAF_METADATA_STREAM_BYTES / (1024 * 1024)
        )));
    }
    if entry.size == 0 {
        return Ok(());
    }
    let chain = walk_chain(entry.start_sector, fat, sector_count, name)?;
    let needed = usize::try_from(entry.size.saturating_add(CFB_SECTOR_SIZE - 1) / CFB_SECTOR_SIZE)
        .map_err(|_| EngineError::Unsupported("AAF stream is too large".into()))?;
    if chain.len() != needed {
        return Err(EngineError::Unsupported(format!("AAF `{}` has a truncated stream", label(name))));
    }
    Ok(())
}

fn walk_chain(start: u32, table: &[u32], sector_count: usize, name: &str) -> Result<Vec<u32>> {
    if is_chain_end(start) {
        return Ok(Vec::new());
    }
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut sector = start;
    loop {
        valid_sector(sector, sector_count, name)?;
        if !seen.insert(sector) {
            return Err(EngineError::Unsupported(format!("AAF CFB {name} chain has a cycle")));
        }
        chain.push(sector);
        let index = usize::try_from(sector).map_err(|_| EngineError::Unsupported(format!("AAF CFB {name} index overflows")))?;
        let next =
            table.get(index).copied().ok_or_else(|| EngineError::Unsupported(format!("AAF CFB {name} chain is outside its allocation table")))?;
        if is_chain_end(next) {
            break;
        }
        sector = next;
    }
    Ok(chain)
}

fn valid_sector(sector: u32, sector_count: usize, name: &str) -> Result<()> {
    if is_chain_end(sector) || matches!(sector, CFB_FAT_SECTOR | CFB_DIFAT_SECTOR) {
        return Err(EngineError::Unsupported(format!("AAF CFB has an invalid {name} sector reference")));
    }
    let sector = usize::try_from(sector).map_err(|_| EngineError::Unsupported(format!("AAF CFB {name} sector overflows")))?;
    if sector >= sector_count {
        return Err(EngineError::Unsupported(format!("AAF CFB {name} sector is outside the file")));
    }
    Ok(())
}

fn is_chain_end(sector: u32) -> bool {
    matches!(sector, CFB_END_OF_CHAIN | CFB_FREE_SECTOR)
}

fn read_sector(file: &mut ConfinedHandle, sector: u32, input: &Path) -> Result<[u8; CFB_SECTOR_SIZE as usize]> {
    let offset = u64::from(sector)
        .checked_mul(CFB_SECTOR_SIZE)
        .and_then(|offset| offset.checked_add(CFB_SECTOR_SIZE))
        .ok_or_else(|| EngineError::Unsupported("AAF CFB sector offset overflows".into()))?;
    let mut bytes = [0u8; CFB_SECTOR_SIZE as usize];
    file.seek(SeekFrom::Start(offset)).map_err(|err| EngineError::Io(format!("{}: {err}", input.display())))?;
    // `preflight_cfb` sizes `sector_count` so this call is only ever made for a sector that
    // starts before EOF; a short read here can only be the final, partially-written sector a
    // real AAF writer left behind (see `preflight_cfb`), so the unfilled tail is left zeroed
    // instead of treated as an error. Reading zero bytes at all means `sector` named something
    // past EOF despite that guarantee, which is reported rather than silently accepted.
    let mut filled = 0usize;
    while filled < bytes.len() {
        let read = file.read(&mut bytes[filled..]).map_err(|err| EngineError::Io(format!("{}: {err}", input.display())))?;
        if read == 0 {
            break;
        }
        filled = filled.saturating_add(read);
    }
    if filled == 0 {
        return Err(EngineError::Unsupported(format!("{}: AAF CFB sector {sector} is past the end of the file", input.display())));
    }
    Ok(bytes)
}

fn parse_directory_entry(data: &[u8], offset: usize) -> Result<DirectoryEntry> {
    let entry =
        data.get(offset..offset.saturating_add(128)).ok_or_else(|| EngineError::Unsupported("AAF CFB directory entry is truncated".into()))?;
    let name_len = usize::from(le_u16(entry, 64)?).min(64);
    let name_end = name_len.saturating_sub(2).min(64);
    let mut code_units = Vec::new();
    for index in (0..name_end).step_by(2) {
        code_units.push(le_u16(entry, index)?);
    }
    let mut clsid = [0u8; 16];
    clsid.copy_from_slice(entry.get(80..96).ok_or_else(|| EngineError::Unsupported("AAF CFB directory entry is truncated".into()))?);
    Ok(DirectoryEntry {
        name: String::from_utf16_lossy(&code_units),
        kind: *entry.get(66).ok_or_else(|| EngineError::Unsupported("AAF CFB directory entry is truncated".into()))?,
        left: le_u32(entry, 68)?,
        right: le_u32(entry, 72)?,
        child: le_u32(entry, 76)?,
        clsid,
        start_sector: le_u32(entry, 116)?,
        size: le_u64(entry, 120)?,
    })
}

fn le_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let bytes: [u8; 2] = bytes
        .get(offset..offset.saturating_add(2))
        .ok_or_else(|| EngineError::Unsupported("AAF CFB field is truncated".into()))?
        .try_into()
        .map_err(|_| EngineError::Unsupported("AAF CFB field is truncated".into()))?;
    Ok(u16::from_le_bytes(bytes))
}

fn le_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let bytes: [u8; 4] = bytes
        .get(offset..offset.saturating_add(4))
        .ok_or_else(|| EngineError::Unsupported("AAF CFB field is truncated".into()))?
        .try_into()
        .map_err(|_| EngineError::Unsupported("AAF CFB field is truncated".into()))?;
    Ok(u32::from_le_bytes(bytes))
}

fn le_u64(bytes: &[u8], offset: usize) -> Result<u64> {
    let bytes: [u8; 8] = bytes
        .get(offset..offset.saturating_add(8))
        .ok_or_else(|| EngineError::Unsupported("AAF CFB field is truncated".into()))?
        .try_into()
        .map_err(|_| EngineError::Unsupported("AAF CFB field is truncated".into()))?;
    Ok(u64::from_le_bytes(bytes))
}

/// One decoded entry from an object storage's `"properties"` stream: the stored-form ("format")
/// tag plus its raw payload bytes, exactly as written. Interpreting a payload as a particular
/// Rust type (integer, rational, string, reference) is left to the small accessor methods below,
/// each of which is told by its caller which property it expects and checks the stored form
/// actually matches before decoding anything.
#[derive(Debug, Clone)]
struct RawProperty {
    format: u16,
    data: Vec<u8>,
}

/// One AAF object's own decoded properties (its `"properties"` stream, if it has one) plus the
/// CFB directory index of its storage, so a caller can reach any named child streams/storages
/// it owns (e.g. a strong-reference collection's index stream and per-item storages).
#[derive(Debug, Clone)]
struct RawObject {
    storage: usize,
    clsid: [u8; 16],
    properties: HashMap<u16, RawProperty>,
}

impl RawObject {
    fn data(&self, pid: u16) -> Result<Option<&[u8]>> {
        match self.properties.get(&pid) {
            Some(property) if property.format == SF_DATA => Ok(Some(&property.data)),
            Some(_) => Err(EngineError::Unsupported("AAF property has an unexpected stored form".into())),
            None => Ok(None),
        }
    }

    fn i64(&self, pid: u16) -> Result<Option<i64>> {
        match self.data(pid)? {
            Some(bytes) if bytes.len() == 8 => le_u64(bytes, 0).map(|value| Some(value as i64)),
            Some(_) => Err(EngineError::Unsupported("AAF integer property has an unexpected size".into())),
            None => Ok(None),
        }
    }

    fn u32(&self, pid: u16) -> Result<Option<u32>> {
        match self.data(pid)? {
            Some(bytes) if bytes.len() == 4 => le_u32(bytes, 0).map(Some),
            Some(bytes) if bytes.len() == 2 => le_u16(bytes, 0).map(u32::from).map(Some),
            Some(_) => Err(EngineError::Unsupported("AAF unsigned integer property has an unexpected size".into())),
            None => Ok(None),
        }
    }

    fn rational(&self, pid: u16) -> Result<Option<(i32, i32)>> {
        match self.data(pid)? {
            Some(bytes) if bytes.len() == 8 => {
                let numerator = le_u32(bytes, 0)? as i32;
                let denominator = le_u32(bytes, 4)? as i32;
                Ok(Some((numerator, denominator)))
            }
            Some(_) => Err(EngineError::Unsupported("AAF rational property has an unexpected size".into())),
            None => Ok(None),
        }
    }

    fn utf16_string(&self, pid: u16) -> Result<Option<String>> {
        Ok(self.data(pid)?.map(decode_utf16le))
    }

    fn mob_id(&self, pid: u16) -> Result<Option<[u8; 32]>> {
        match self.data(pid)? {
            Some(bytes) => {
                let array: [u8; 32] = bytes.try_into().map_err(|_| EngineError::Unsupported("AAF MobID property has an unexpected size".into()))?;
                Ok(Some(array))
            }
            None => Ok(None),
        }
    }

    fn strong_ref_name(&self, pid: u16) -> Result<Option<String>> {
        match self.properties.get(&pid) {
            Some(property) if property.format == SF_STRONG_OBJECT_REFERENCE => Ok(Some(decode_utf16le(&property.data))),
            Some(_) => Err(EngineError::Unsupported("AAF property has an unexpected stored form".into())),
            None => Ok(None),
        }
    }

    /// The index-stream name prefix for a strong-reference vector or set property (its own
    /// payload, not a fixed/guessed name): used by `CfbContainer::strong_ref_vector`/`strong_ref_set`.
    fn collection_index_name(&self, pid: u16) -> Result<Option<String>> {
        match self.properties.get(&pid) {
            Some(property) if matches!(property.format, SF_STRONG_OBJECT_REFERENCE_VECTOR | SF_STRONG_OBJECT_REFERENCE_SET) => {
                Ok(Some(decode_utf16le(&property.data)))
            }
            Some(_) => Err(EngineError::Unsupported("AAF property has an unexpected stored form".into())),
            None => Ok(None),
        }
    }

    /// A weak object reference's target key bytes: the on-disk encoding is
    /// `weakref_index:u16, key_pid:u16, key_size:u8` followed by `key_size` raw bytes that equal
    /// the referenced object's own unique-key property value (e.g. a class AUID for a
    /// `ParentClass` weak reference) — resolved by direct byte comparison, never by storage
    /// lookup, matching the public AAF weak-reference encoding.
    fn weak_ref_key(&self, pid: u16) -> Result<Option<Vec<u8>>> {
        match self.properties.get(&pid) {
            Some(property) if property.format == SF_WEAK_OBJECT_REFERENCE => {
                let key_size = usize::from(*property.data.get(4).ok_or_else(|| EngineError::Unsupported("AAF weak reference is too short".into()))?);
                let key = property
                    .data
                    .get(5..5usize.checked_add(key_size).ok_or_else(|| EngineError::Unsupported("AAF weak reference key overflows".into()))?)
                    .ok_or_else(|| EngineError::Unsupported("AAF weak reference key is truncated".into()))?
                    .to_vec();
                Ok(Some(key))
            }
            Some(_) => Err(EngineError::Unsupported("AAF property has an unexpected stored form".into())),
            None => Ok(None),
        }
    }
}

/// Decodes a UTF-16LE byte buffer, truncating at the first embedded NUL *code unit* (not
/// necessarily an aligned double-zero byte pair), matching how AAF string properties are
/// written and read in practice. An odd trailing byte (malformed input) is simply ignored
/// rather than treated as an error, since this never affects anything but display text.
fn decode_utf16le(bytes: &[u8]) -> String {
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.as_chunks::<2>().0 {
        let unit = u16::from_le_bytes(*chunk);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    String::from_utf16_lossy(&units)
}

/// Reads one AAF object's properties stream (if it has one — an object with no properties of
/// its own, such as a bare `Locator`, is not an error).
fn read_object(cfb: &mut CfbContainer, storage: usize) -> Result<RawObject> {
    let clsid = cfb
        .entries
        .get(storage)
        .map(|entry| entry.clsid)
        .ok_or_else(|| EngineError::Unsupported("AAF CFB directory reference is out of range".into()))?;
    let properties = match cfb.find_child(storage, "properties")? {
        Some(index) => parse_properties_stream(&cfb.stream_bytes(index)?)?,
        None => HashMap::new(),
    };
    Ok(RawObject { storage, clsid, properties })
}

/// Parses an object storage's `"properties"` stream: `byte_order:u8, version:u8,
/// entry_count:u16-LE`, then `entry_count` fixed `(pid:u16, format:u16, byte_size:u16)` headers,
/// then the concatenated payload bytes (each `byte_size` long, in header order).
fn parse_properties_stream(bytes: &[u8]) -> Result<HashMap<u16, RawProperty>> {
    if bytes.is_empty() {
        return Ok(HashMap::new());
    }
    if bytes.len() < 4 {
        return Err(EngineError::Unsupported("AAF properties stream is too short".into()));
    }
    if bytes[0] != 0x4C {
        return Err(EngineError::Unsupported("AAF properties stream has an unsupported byte order".into()));
    }
    let entry_count = usize::from(le_u16(bytes, 2)?);
    if entry_count > MAX_AAF_PROPERTY_ENTRIES {
        return Err(EngineError::Unsupported(format!("AAF object has more than {MAX_AAF_PROPERTY_ENTRIES} properties")));
    }
    const HEADER_SIZE: usize = 6;
    let headers_end = 4usize
        .checked_add(
            entry_count.checked_mul(HEADER_SIZE).ok_or_else(|| EngineError::Unsupported("AAF properties stream header count overflows".into()))?,
        )
        .ok_or_else(|| EngineError::Unsupported("AAF properties stream header count overflows".into()))?;
    if bytes.len() < headers_end {
        return Err(EngineError::Unsupported("AAF properties stream headers are truncated".into()));
    }
    let mut headers = Vec::with_capacity(entry_count);
    for slot in 0..entry_count {
        let offset = 4 + slot * HEADER_SIZE;
        let pid = le_u16(bytes, offset)?;
        let format = le_u16(bytes, offset + 2)?;
        let size = usize::from(le_u16(bytes, offset + 4)?);
        headers.push((pid, format, size));
    }
    let mut properties = HashMap::with_capacity(entry_count);
    let mut cursor = headers_end;
    for (pid, format, size) in headers {
        let end = cursor.checked_add(size).ok_or_else(|| EngineError::Unsupported("AAF properties stream payload overflows".into()))?;
        let data = bytes.get(cursor..end).ok_or_else(|| EngineError::Unsupported("AAF properties stream payload is truncated".into()))?.to_vec();
        properties.insert(pid, RawProperty { format, data });
        cursor = end;
    }
    Ok(properties)
}

/// One `ClassDefinition` from the file's own `MetaDictionary`: its `ParentClass` (`None` once
/// the self-referencing "no parent" terminator is reached) and the per-file numeric PIDs this
/// class itself declares, keyed by each property's well-known, standardized AUID.
#[derive(Debug, Clone, Default)]
struct ClassDef {
    parent: Option<[u8; 16]>,
    properties: HashMap<[u8; 16], u16>,
}

/// Builds the file's class dictionary by walking `MetaDictionary.ClassDefinitions` (a
/// strong-reference set) and, for each `ClassDefinition`, its own nested `Properties`
/// (`PropertyDefinition`) strong-reference set. Every PID this importer ultimately uses is
/// resolved against this dictionary (see `resolve_pid`) rather than assumed fixed, since a
/// vendor's AAF writer is free to assign its own per-file PID numbering.
fn read_class_dictionary(cfb: &mut CfbContainer, metadict_storage: usize) -> Result<HashMap<[u8; 16], ClassDef>> {
    let metadict = read_object(cfb, metadict_storage)?;
    if metadict.clsid != CLASS_META_DICTIONARY {
        return Err(EngineError::Unsupported("AAF root's MetaDictionary has an unexpected class; this is not a valid AAF file".into()));
    }
    let class_storages = cfb.strong_ref_set(&metadict, PID_META_DICTIONARY_CLASS_DEFS)?;
    if class_storages.len() > MAX_AAF_CLASSES {
        return Err(EngineError::Unsupported(format!("AAF MetaDictionary declares more than {MAX_AAF_CLASSES} classes")));
    }
    let mut classes = HashMap::with_capacity(class_storages.len());
    for (_, class_storage) in class_storages {
        let class_obj = read_object(cfb, class_storage)?;
        if class_obj.clsid != CLASS_CLASS_DEF {
            continue;
        }
        let Some(auid) = class_obj.data(PID_DEF_AUID)?.and_then(|bytes| <[u8; 16]>::try_from(bytes).ok()) else {
            continue;
        };
        let parent_key = class_obj.weak_ref_key(PID_DEF_PARENT)?;
        let parent = parent_key.and_then(|key| <[u8; 16]>::try_from(key.as_slice()).ok()).filter(|parent_auid| *parent_auid != auid);

        let mut properties = HashMap::new();
        let property_storages = cfb.strong_ref_set(&class_obj, PID_DEF_PROPERTIES)?;
        if property_storages.len() > MAX_AAF_PROPERTIES_PER_CLASS {
            return Err(EngineError::Unsupported(format!("AAF class declares more than {MAX_AAF_PROPERTIES_PER_CLASS} properties")));
        }
        for (_, property_storage) in property_storages {
            let property_obj = read_object(cfb, property_storage)?;
            if property_obj.clsid != CLASS_PROPERTY_DEF {
                continue;
            }
            let Some(property_auid) = property_obj.data(PID_DEF_AUID)?.and_then(|bytes| <[u8; 16]>::try_from(bytes).ok()) else {
                continue;
            };
            let Some(pid) = property_obj.data(PID_DEF_PID)?.and_then(|bytes| <[u8; 2]>::try_from(bytes).ok()).map(u16::from_le_bytes) else {
                continue;
            };
            properties.insert(property_auid, pid);
        }
        classes.insert(auid, ClassDef { parent, properties });
    }
    Ok(classes)
}

/// Resolves a well-known property AUID to its per-file numeric PID for a given class, walking
/// the class's `ParentClass` chain (bounded, since the real AAF class hierarchy is only a few
/// levels deep) so inherited properties (e.g. `SourceClip` not redeclaring
/// `SourceReference.SourceID`) are found without assuming any fixed PID for them.
fn resolve_pid(classes: &HashMap<[u8; 16], ClassDef>, class_auid: [u8; 16], property_auid: &[u8; 16]) -> Option<u16> {
    let mut current = class_auid;
    for _ in 0..MAX_AAF_CLASS_DEPTH {
        let class = classes.get(&current)?;
        if let Some(pid) = class.properties.get(property_auid) {
            return Some(*pid);
        }
        current = class.parent?;
    }
    None
}

/// Builds the whole AAF object graph this importer understands from an already-preflighted CFB
/// container: bootstraps `Root -> MetaDictionary`/`Root -> Header`, builds the class dictionary,
/// then lowers `Header -> ContentStorage -> Mobs -> CompositionMob -> TimelineMobSlot ->
/// Sequence -> SourceClip`, resolving each `SourceClip` against its `SourceMob` by `MobID`.
fn build_aaf_model(cfb: &mut CfbContainer, input: &Path) -> Result<AafModel> {
    let root = read_object(cfb, 0)?;
    if root.clsid != CLASS_ROOT {
        return Err(EngineError::Unsupported(format!("{}: AAF root object has an unexpected class; this is not a valid AAF file", input.display())));
    }
    let Some(metadict_storage) = cfb.strong_ref(&root, PID_ROOT_META_DICTIONARY)? else {
        return Err(EngineError::Unsupported(format!("{}: AAF root object has no MetaDictionary; this is not a valid AAF file", input.display())));
    };
    let Some(header_storage) = cfb.strong_ref(&root, PID_ROOT_HEADER)? else {
        return Err(EngineError::Unsupported(format!("{}: AAF root object has no Header; this is not a valid AAF file", input.display())));
    };
    let classes = read_class_dictionary(cfb, metadict_storage)?;
    let resolve = |class_auid: [u8; 16], property_auid: &[u8; 16], label: &str| {
        resolve_pid(&classes, class_auid, property_auid)
            .ok_or_else(|| EngineError::Unsupported(format!("{}: AAF MetaDictionary has no property mapping for `{label}`", input.display())))
    };

    let header = read_object(cfb, header_storage)?;
    let pid_header_content = resolve(CLASS_HEADER, &PROP_HEADER_CONTENT, "Header.Content")?;
    let Some(content_storage) = cfb.strong_ref(&header, pid_header_content)? else {
        return Err(EngineError::Unsupported(format!("{}: AAF Header has no ContentStorage; no SoundCraft session was written", input.display())));
    };

    let content = read_object(cfb, content_storage)?;
    let pid_mobs = resolve(CLASS_CONTENT_STORAGE, &PROP_CONTENT_STORAGE_MOBS, "ContentStorage.Mobs")?;
    let mob_storages = cfb.strong_ref_set(&content, pid_mobs)?;

    let pid_mob_name = resolve(CLASS_MOB, &PROP_MOB_NAME, "Mob.Name")?;
    let pid_mob_slots = resolve(CLASS_MOB, &PROP_MOB_SLOTS, "Mob.Slots")?;
    let pid_slot_name = resolve(CLASS_MOB_SLOT, &PROP_MOBSLOT_SLOTNAME, "MobSlot.SlotName")?;
    let pid_slot_segment = resolve(CLASS_MOB_SLOT, &PROP_MOBSLOT_SEGMENT, "MobSlot.Segment")?;
    let pid_edit_rate = resolve(CLASS_TIMELINE_MOB_SLOT, &PROP_TIMELINE_EDIT_RATE, "TimelineMobSlot.EditRate")?;
    let pid_origin = resolve(CLASS_TIMELINE_MOB_SLOT, &PROP_TIMELINE_ORIGIN, "TimelineMobSlot.Origin")?;
    let pid_components = resolve(CLASS_SEQUENCE, &PROP_SEQUENCE_COMPONENTS, "Sequence.Components")?;
    let pid_operation_inputs = resolve(CLASS_OPERATION_GROUP, &PROP_OPERATION_GROUP_INPUT_SEGMENTS, "OperationGroup.InputSegments").ok();
    let pid_length = resolve(CLASS_COMPONENT, &PROP_COMPONENT_LENGTH, "Component.Length").ok();
    let pid_data_definition = resolve(CLASS_COMPONENT, &PROP_COMPONENT_DATA_DEFINITION, "Component.DataDefinition").ok();
    let pid_source_id = resolve(CLASS_SOURCE_CLIP, &PROP_SOURCE_REFERENCE_SOURCE_ID, "SourceReference.SourceID")?;
    let pid_source_slot_id = resolve(CLASS_SOURCE_CLIP, &PROP_SOURCE_REFERENCE_SOURCE_SLOT_ID, "SourceReference.SourceMobSlotID")?;
    let pid_start_time = resolve(CLASS_SOURCE_CLIP, &PROP_SOURCE_CLIP_START_TIME, "SourceClip.StartTime")?;
    let pid_slot_id = resolve(CLASS_MOB_SLOT, &PROP_MOBSLOT_SLOT_ID, "MobSlot.SlotID")?;
    let pid_essence_description = resolve(CLASS_SOURCE_MOB, &PROP_SOURCE_MOB_ESSENCE_DESCRIPTION, "SourceMob.EssenceDescription").ok();
    let pid_locator = resolve(CLASS_ESSENCE_DESCRIPTOR, &PROP_ESSENCE_DESCRIPTOR_LOCATOR, "EssenceDescriptor.Locator").ok();
    let pid_url_string = resolve(CLASS_NETWORK_LOCATOR, &PROP_NETWORK_LOCATOR_URL_STRING, "NetworkLocator.URLString").ok();

    if mob_storages.len() > MAX_AAF_COMPOSITIONS.saturating_add(MAX_AAF_COLLECTION_ENTRIES) {
        return Err(EngineError::Unsupported("AAF ContentStorage declares an implausible number of mobs".into()));
    }

    let mut model = AafModel::default();
    for (mob_id_key, mob_storage) in &mob_storages {
        let Ok(mob_id): std::result::Result<[u8; 32], _> = mob_id_key.as_slice().try_into() else { continue };
        let mob = read_object(cfb, *mob_storage)?;
        let name = mob.utf16_string(pid_mob_name)?.unwrap_or_default();
        let locator_url = pid_essence_description
            .zip(pid_locator)
            .zip(pid_url_string)
            .and_then(|((essence_pid, locator_pid), url_pid)| resolve_locator_url(cfb, &mob, essence_pid, locator_pid, url_pid).ok().flatten());
        let mut slots = HashMap::new();
        for slot_storage in cfb.strong_ref_vector(&mob, pid_mob_slots)? {
            let slot = read_object(cfb, slot_storage)?;
            let Some(slot_id) = slot.u32(pid_slot_id)? else {
                continue;
            };
            let Some(segment_storage) = cfb.strong_ref(&slot, pid_slot_segment)? else {
                continue;
            };
            let segment = read_object(cfb, segment_storage)?;
            let components = read_sequence_components(
                cfb,
                &segment,
                pid_components,
                pid_operation_inputs,
                pid_length,
                pid_source_id,
                pid_source_slot_id,
                pid_start_time,
                &name,
            )?;
            slots.insert(slot_id, components);
        }
        model.mobs.insert(mob_id, MobInfo { name: name.clone(), locator_url, slots });

        match mob.clsid {
            clsid if clsid == CLASS_COMPOSITION_MOB => {
                if model.compositions.len() >= MAX_AAF_COMPOSITIONS {
                    return Err(EngineError::Unsupported(format!("AAF has more than {MAX_AAF_COMPOSITIONS} compositions")));
                }
                let slot_storages = cfb.strong_ref_vector(&mob, pid_mob_slots)?;
                if slot_storages.len() > MAX_AAF_TRACKS.saturating_mul(4) {
                    return Err(EngineError::Unsupported(format!("AAF composition `{}` has an implausible number of slots", label(&name))));
                }
                let mut tracks = Vec::new();
                for slot_storage in slot_storages {
                    let slot = read_object(cfb, slot_storage)?;
                    if slot.clsid != CLASS_TIMELINE_MOB_SLOT {
                        continue;
                    }
                    let Some(segment_storage) = cfb.strong_ref(&slot, pid_slot_segment)? else { continue };
                    let segment = read_object(cfb, segment_storage)?;
                    if let Some(data_definition_pid) = pid_data_definition
                        && let Some(data_def_key) = segment.weak_ref_key(data_definition_pid)?
                    {
                        let is_sound = <[u8; 16]>::try_from(data_def_key.as_slice())
                            .map(|key| key == DATA_DEF_SOUND || key == DATA_DEF_LEGACY_SOUND)
                            .unwrap_or(false);
                        if !is_sound {
                            continue;
                        }
                    }
                    let slot_name = slot.utf16_string(pid_slot_name)?.unwrap_or_default();
                    let edit_rate = slot.rational(pid_edit_rate)?.unwrap_or((48_000, 1));
                    let origin = slot.i64(pid_origin)?.unwrap_or(0);
                    let components = read_sequence_components(
                        cfb,
                        &segment,
                        pid_components,
                        pid_operation_inputs,
                        pid_length,
                        pid_source_id,
                        pid_source_slot_id,
                        pid_start_time,
                        &name,
                    )?;
                    tracks.push(TimelineTrack { name: slot_name, edit_rate, origin, components });
                    if tracks.len() > MAX_AAF_TRACKS {
                        return Err(EngineError::Unsupported(format!("AAF composition has more than {MAX_AAF_TRACKS} audio tracks")));
                    }
                }
                model.compositions.push(Composition { name: nonempty_label(&name, "AAF composition"), tracks });
            }
            clsid if clsid == CLASS_MASTER_MOB => model.master_mob_count = model.master_mob_count.saturating_add(1),
            clsid if clsid == CLASS_SOURCE_MOB => model.source_mob_count = model.source_mob_count.saturating_add(1),
            _ => {}
        }
    }
    Ok(model)
}

/// Lowers one `Segment` into a flat, bounded list of timeline components: a `Sequence`'s own
/// `Components` vector if the segment is a `Sequence`, or the segment itself as a single
/// one-component list otherwise (a `TimelineMobSlot` may hold a bare `SourceClip`/`Filler`
/// directly, without a wrapping `Sequence`).
fn read_sequence_components(
    cfb: &mut CfbContainer,
    segment: &RawObject,
    pid_components: u16,
    pid_operation_inputs: Option<u16>,
    pid_length: Option<u16>,
    pid_source_id: u16,
    pid_source_slot_id: u16,
    pid_start_time: u16,
    track_name: &str,
) -> Result<Vec<ComponentEntry>> {
    read_segment_components(
        cfb,
        segment,
        pid_components,
        pid_operation_inputs,
        pid_length,
        pid_source_id,
        pid_source_slot_id,
        pid_start_time,
        track_name,
        0,
    )
}

/// Lowers a regular segment or the primary input of an OperationGroup. Effects are not rendered
/// by this interchange importer, but their primary input is the audible source timeline and can
/// therefore be preserved. Multiple inputs remain a single opaque effect because choosing a
/// transition's A/B input would silently change the edit.
fn read_segment_components(
    cfb: &mut CfbContainer,
    segment: &RawObject,
    pid_components: u16,
    pid_operation_inputs: Option<u16>,
    pid_length: Option<u16>,
    pid_source_id: u16,
    pid_source_slot_id: u16,
    pid_start_time: u16,
    track_name: &str,
    depth: usize,
) -> Result<Vec<ComponentEntry>> {
    if depth >= MAX_AAF_CLASS_DEPTH {
        return Err(EngineError::Unsupported(format!("AAF effect nesting exceeds {MAX_AAF_CLASS_DEPTH} levels on `{}`", label(track_name))));
    }
    let component_storages = if segment.clsid == CLASS_SEQUENCE { cfb.strong_ref_vector(segment, pid_components)? } else { vec![segment.storage] };
    if component_storages.len() > MAX_AAF_CLIPS {
        return Err(EngineError::Unsupported(format!("AAF track `{}` has more than {MAX_AAF_CLIPS} components", label(track_name))));
    }
    let mut components = Vec::with_capacity(component_storages.len());
    for component_storage in component_storages {
        let component = if component_storage == segment.storage { segment.clone() } else { read_object(cfb, component_storage)? };
        let length = pid_length.map(|pid| component.i64(pid)).transpose()?.flatten().unwrap_or(0);
        if component.clsid == CLASS_OPERATION_GROUP {
            let Some(pid_operation_inputs) = pid_operation_inputs else {
                components.push(ComponentEntry { kind: ComponentKind::Effect, length });
                continue;
            };
            let inputs = cfb.strong_ref_vector(&component, pid_operation_inputs)?;
            if inputs.len() == 1 {
                let input = read_object(cfb, inputs[0])?;
                let mut lowered = read_segment_components(
                    cfb,
                    &input,
                    pid_components,
                    Some(pid_operation_inputs),
                    pid_length,
                    pid_source_id,
                    pid_source_slot_id,
                    pid_start_time,
                    track_name,
                    depth.saturating_add(1),
                )?;
                let lowered_length = lowered.iter().try_fold(0i64, |total, component| total.checked_add(component.length.max(0)));
                if lowered_length == Some(length.max(0)) {
                    components.append(&mut lowered);
                    continue;
                }
            }
            components.push(ComponentEntry { kind: ComponentKind::Effect, length });
            continue;
        }
        let kind = if component.clsid == CLASS_SOURCE_CLIP {
            let source_mob_id = component.mob_id(pid_source_id)?.unwrap_or([0u8; 32]);
            let source_slot_id = component.u32(pid_source_slot_id)?.unwrap_or(0);
            let start_time = component.i64(pid_start_time)?.unwrap_or(0);
            ComponentKind::SourceClip { start_time, source_mob_id, source_slot_id }
        } else if component.clsid == CLASS_FILLER {
            ComponentKind::Filler
        } else if component.clsid == CLASS_TRANSITION {
            ComponentKind::Transition
        } else {
            ComponentKind::Effect
        };
        components.push(ComponentEntry { kind, length });
    }
    Ok(components)
}

/// Best-effort `SourceMob -> EssenceDescription -> EssenceDescriptor.Locator -> NetworkLocator`
/// resolution: returns the first `NetworkLocator.URLString` found, if any. Any missing
/// intermediate link (no `EssenceDescription`, no locators, a non-network locator) simply
/// yields `None` rather than an error, since a `Mob`'s name is still usable as a fallback.
fn resolve_locator_url(cfb: &mut CfbContainer, mob: &RawObject, essence_pid: u16, locator_pid: u16, url_pid: u16) -> Result<Option<String>> {
    let Some(essence_storage) = cfb.strong_ref(mob, essence_pid)? else { return Ok(None) };
    let essence = read_object(cfb, essence_storage)?;
    let locator_storages = cfb.strong_ref_vector(&essence, locator_pid)?;
    for locator_storage in locator_storages {
        let locator = read_object(cfb, locator_storage)?;
        if locator.clsid != CLASS_NETWORK_LOCATOR {
            continue;
        }
        if let Some(url) = locator.utf16_string(url_pid)?
            && !url.is_empty()
        {
            return Ok(Some(url));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    // --- Minimal, hand-built CFB/AAF fixture writer -------------------------------------------
    //
    // `oximedia-aaf`'s own writer produces an unrelated, invented on-disk format incompatible
    // with a real AAF reader, so these tests build a synthetic compound-file container directly,
    // encoding only the standard AAF object/property layout this importer actually implements
    // (see the module doc comment at the top of this file). Every stream here is padded to at
    // least the 4096-byte mini-stream cutoff so it is always read through the regular FAT path;
    // this keeps the fixture writer from needing to also implement mini-stream/mini-FAT support,
    // at the cost of a larger-than-strictly-necessary (but still tiny) fixture file.

    const NOSTREAM: u32 = CFB_NO_ENTRY;

    #[derive(Clone)]
    struct FxNode {
        name: String,
        kind: u8,
        clsid: [u8; 16],
        data: Vec<u8>,
        children: Vec<FxNode>,
    }

    fn fx_storage(name: &str, clsid: [u8; 16], children: Vec<FxNode>) -> FxNode {
        FxNode { name: name.into(), kind: 1, clsid, data: Vec::new(), children }
    }

    fn fx_stream(name: &str, data: Vec<u8>) -> FxNode {
        FxNode { name: name.into(), kind: 2, clsid: [0; 16], data, children: Vec::new() }
    }

    fn properties_stream(entries: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
        let mut out = vec![0x4C, 0x00];
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (pid, format, data) in entries {
            out.extend_from_slice(&pid.to_le_bytes());
            out.extend_from_slice(&format.to_le_bytes());
            out.extend_from_slice(&(data.len() as u16).to_le_bytes());
        }
        for (_, _, data) in entries {
            out.extend_from_slice(data);
        }
        out
    }

    fn utf16le(text: &str) -> Vec<u8> {
        let mut out = Vec::new();
        for unit in text.encode_utf16() {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn weak_ref(key: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; 4];
        out.push(key.len() as u8);
        out.extend_from_slice(key);
        out
    }

    fn vector_index(count: usize) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(count as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        for key in 0..count as u32 {
            out.extend_from_slice(&key.to_le_bytes());
        }
        out
    }

    fn set_index(keys: &[Vec<u8>], key_size: u8) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(keys.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.push(key_size);
        for (local_key, key) in keys.iter().enumerate() {
            out.extend_from_slice(&(local_key as u32).to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(key);
        }
        out
    }

    /// A strong-reference vector property: names each child `"{index_name}{hex}"` (hex digits
    /// wrapped in a literal `{`/`}` pair) and builds the matching `"{index_name} index"` stream,
    /// exactly as a real AAF writer would (confirmed against the reference AAF toolkit's own
    /// `index_ref_name` convention: `"%s{%x}" % (index_name, local_key)`).
    fn fx_vector(index_name: &str, items: Vec<FxNode>) -> (FxNode, Vec<FxNode>) {
        let count = items.len();
        let named = items.into_iter().enumerate().map(|(i, mut node)| {
            node.name = format!("{index_name}{{{i:x}}}");
            node
        });
        (fx_stream(&format!("{index_name} index"), vector_index(count)), named.collect())
    }

    /// A strong-reference set property: like `fx_vector`, but each item also carries its own
    /// unique-key bytes (e.g. a Mob's `MobID`, a class's own AUID).
    fn fx_set(index_name: &str, items: Vec<(Vec<u8>, FxNode)>, key_size: u8) -> (FxNode, Vec<FxNode>) {
        let keys: Vec<Vec<u8>> = items.iter().map(|(key, _)| key.clone()).collect();
        let named = items.into_iter().enumerate().map(|(i, (_, mut node))| {
            node.name = format!("{index_name}{{{i:x}}}");
            node
        });
        (fx_stream(&format!("{index_name} index"), set_index(&keys, key_size)), named.collect())
    }

    fn fx_propdef(auid: [u8; 16], pid: u16) -> FxNode {
        let props = properties_stream(&[(PID_DEF_AUID, SF_DATA, auid.to_vec()), (PID_DEF_PID, SF_DATA, pid.to_le_bytes().to_vec())]);
        fx_storage("PropertyDef", CLASS_PROPERTY_DEF, vec![fx_stream("properties", props)])
    }

    fn fx_classdef(class_auid: [u8; 16], properties: Vec<([u8; 16], u16)>) -> FxNode {
        let propdefs = properties.into_iter().map(|(auid, pid)| (auid.to_vec(), fx_propdef(auid, pid))).collect();
        let (index_stream, named) = fx_set("Properties", propdefs, 16);
        let props = properties_stream(&[
            (PID_DEF_AUID, SF_DATA, class_auid.to_vec()),
            (PID_DEF_PROPERTIES, SF_STRONG_OBJECT_REFERENCE_SET, utf16le("Properties")),
        ]);
        let mut children = vec![fx_stream("properties", props), index_stream];
        children.extend(named);
        fx_storage("ClassDef", CLASS_CLASS_DEF, children)
    }

    fn fx_metadictionary(classdefs: Vec<FxNode>) -> FxNode {
        let items = classdefs.into_iter().map(|node| (node.clsid_auid_key(), node)).collect();
        let (index_stream, named) = fx_set("ClassDefinitions", items, 16);
        let props = properties_stream(&[(PID_META_DICTIONARY_CLASS_DEFS, SF_STRONG_OBJECT_REFERENCE_SET, utf16le("ClassDefinitions"))]);
        let mut children = vec![fx_stream("properties", props), index_stream];
        children.extend(named);
        fx_storage("MetaDictionary", CLASS_META_DICTIONARY, children)
    }

    impl FxNode {
        /// Extracts the class AUID a `fx_classdef` node was built with, from its own
        /// `"properties"` stream (so `fx_metadictionary` does not need it passed separately).
        fn clsid_auid_key(&self) -> Vec<u8> {
            self.children
                .iter()
                .find(|child| child.name == "properties")
                .and_then(|stream| parse_properties_stream(&stream.data).ok())
                .and_then(|props| props.get(&PID_DEF_AUID).map(|p| p.data.clone()))
                .unwrap_or_default()
        }
    }

    /// Builds a complete, standard-layout AAF compound-file container from a `Root`'s children
    /// and writes it to `path`. See the module comment above for the design this mirrors.
    fn write_aaf_fixture(path: &Path, root_children: Vec<FxNode>) {
        struct RawEntry {
            name: String,
            kind: u8,
            clsid: [u8; 16],
            left: u32,
            right: u32,
            child: u32,
            start_sector: u32,
            size: u64,
            stream_data: Option<Vec<u8>>,
        }

        fn add_children(entries: &mut Vec<RawEntry>, children: Vec<FxNode>) -> u32 {
            if children.is_empty() {
                return NOSTREAM;
            }
            let mut indices = Vec::new();
            for node in children {
                let idx = entries.len() as u32;
                entries.push(RawEntry {
                    name: node.name.clone(),
                    kind: node.kind,
                    clsid: node.clsid,
                    left: NOSTREAM,
                    right: NOSTREAM,
                    child: NOSTREAM,
                    start_sector: 0,
                    size: 0,
                    stream_data: (node.kind == 2).then(|| node.data.clone()),
                });
                indices.push(idx);
                if node.kind == 1 {
                    let child_head = add_children(entries, node.children);
                    entries[idx as usize].child = child_head;
                }
            }
            for window in 0..indices.len().saturating_sub(1) {
                let (current, next) = (indices[window], indices[window + 1]);
                entries[current as usize].right = next;
            }
            indices[0]
        }

        let mut entries = vec![RawEntry {
            name: "Root Entry".into(),
            kind: 5,
            clsid: CLASS_ROOT,
            left: NOSTREAM,
            right: NOSTREAM,
            child: NOSTREAM,
            start_sector: 0,
            size: 0,
            stream_data: None,
        }];
        let root_child = add_children(&mut entries, root_children);
        entries[0].child = root_child;

        // Pad every stream past the mini-stream cutoff so this fixture never needs to also
        // implement mini-stream/mini-FAT support (see the module comment above).
        for entry in &mut entries {
            if let Some(data) = &mut entry.stream_data {
                let target = data.len().max(4096).div_ceil(512) * 512;
                data.resize(target, 0);
            }
        }

        let dir_sector_count = entries.len().div_ceil(4);
        let stream_sector_counts: Vec<usize> = entries.iter().map(|entry| entry.stream_data.as_ref().map_or(0, |d| d.len() / 512)).collect();
        let stream_sectors_total: usize = stream_sector_counts.iter().sum();
        let non_fat_sectors = dir_sector_count + stream_sectors_total;
        let mut fat_count = 1usize;
        loop {
            let needed = (non_fat_sectors + fat_count).div_ceil(128);
            if needed == fat_count {
                break;
            }
            fat_count = needed;
        }
        assert!(fat_count <= 109, "fixture needs DIFAT sectors, which this test-only writer does not implement");

        let dir_sector_start = fat_count;
        let mut cursor = dir_sector_start + dir_sector_count;
        for (entry, sector_count) in entries.iter_mut().zip(stream_sector_counts.iter()) {
            if *sector_count == 0 {
                entry.start_sector = CFB_END_OF_CHAIN;
                entry.size = 0;
            } else {
                entry.start_sector = cursor as u32;
                entry.size = entry.stream_data.as_ref().map_or(0, |d| d.len() as u64);
                cursor += sector_count;
            }
        }
        let total_sectors = cursor;

        let mut fat = vec![CFB_FREE_SECTOR; total_sectors];
        for sector in fat.iter_mut().take(fat_count) {
            *sector = CFB_FAT_SECTOR;
        }
        let dir_chain_end = dir_sector_start + dir_sector_count;
        for (sector_index, slot) in fat.iter_mut().enumerate().take(dir_chain_end).skip(dir_sector_start) {
            *slot = if sector_index + 1 == dir_chain_end { CFB_END_OF_CHAIN } else { (sector_index + 1) as u32 };
        }
        for entry in &entries {
            if let Some(data) = &entry.stream_data {
                let start = entry.start_sector as usize;
                let count = data.len() / 512;
                for offset in 0..count {
                    fat[start + offset] = if offset + 1 == count { CFB_END_OF_CHAIN } else { (start + offset + 1) as u32 };
                }
            }
        }

        let mut out = vec![0u8; 512];
        out[0..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
        out[26..28].copy_from_slice(&3u16.to_le_bytes());
        out[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes());
        out[30..32].copy_from_slice(&9u16.to_le_bytes());
        out[32..34].copy_from_slice(&6u16.to_le_bytes());
        out[44..48].copy_from_slice(&(fat_count as u32).to_le_bytes());
        out[48..52].copy_from_slice(&(dir_sector_start as u32).to_le_bytes());
        out[56..60].copy_from_slice(&4096u32.to_le_bytes());
        out[60..64].copy_from_slice(&CFB_END_OF_CHAIN.to_le_bytes());
        out[68..72].copy_from_slice(&CFB_END_OF_CHAIN.to_le_bytes());
        for (slot, offset) in (76..512).step_by(4).enumerate() {
            let value = if slot < fat_count { slot as u32 } else { CFB_FREE_SECTOR };
            out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }

        for sector_index in 0..fat_count {
            let mut sector = vec![0u8; 512];
            for slot in 0..128 {
                let value = fat.get(sector_index * 128 + slot).copied().unwrap_or(CFB_FREE_SECTOR);
                sector[slot * 4..slot * 4 + 4].copy_from_slice(&value.to_le_bytes());
            }
            out.extend_from_slice(&sector);
        }

        for sector_index in 0..dir_sector_count {
            let mut sector = vec![0u8; 512];
            for slot in 0..4 {
                let Some(entry) = entries.get(sector_index * 4 + slot) else { break };
                let base = slot * 128;
                let units: Vec<u16> = entry.name.encode_utf16().collect();
                for (i, unit) in units.iter().enumerate() {
                    sector[base + i * 2..base + i * 2 + 2].copy_from_slice(&unit.to_le_bytes());
                }
                sector[base + 64..base + 66].copy_from_slice(&(((units.len() + 1) * 2) as u16).to_le_bytes());
                sector[base + 66] = entry.kind;
                sector[base + 68..base + 72].copy_from_slice(&entry.left.to_le_bytes());
                sector[base + 72..base + 76].copy_from_slice(&entry.right.to_le_bytes());
                sector[base + 76..base + 80].copy_from_slice(&entry.child.to_le_bytes());
                sector[base + 80..base + 96].copy_from_slice(&entry.clsid);
                sector[base + 116..base + 120].copy_from_slice(&entry.start_sector.to_le_bytes());
                sector[base + 120..base + 128].copy_from_slice(&entry.size.to_le_bytes());
            }
            out.extend_from_slice(&sector);
        }

        for entry in &entries {
            if let Some(data) = &entry.stream_data {
                out.extend_from_slice(data);
            }
        }

        std::fs::write(path, out).expect("writing the AAF test fixture");
    }

    /// Builds a minimal, standard AAF file with one `CompositionMob` (name `"AAF timing"`)
    /// holding one audio `TimelineMobSlot` (`"DX"`, edit rate 48000/1, origin 9600) whose
    /// `Sequence` is `[Filler(4800), SourceClip(length 48000, start_time 2400)]`, where the
    /// `SourceClip` resolves to a `SourceMob` named `"take.wav"`.
    fn synthetic_aaf(path: &Path) {
        let source_mob_id: [u8; 32] = {
            let mut id = [0u8; 32];
            id[0] = 0x51;
            id
        };

        let classdefs = vec![
            fx_classdef(CLASS_HEADER, vec![(PROP_HEADER_CONTENT, 0x3b03)]),
            fx_classdef(CLASS_CONTENT_STORAGE, vec![(PROP_CONTENT_STORAGE_MOBS, 0x1901)]),
            fx_classdef(CLASS_MOB, vec![(PROP_MOB_NAME, 0x4402), (PROP_MOB_SLOTS, 0x4403)]),
            fx_classdef(CLASS_MOB_SLOT, vec![(PROP_MOBSLOT_SLOT_ID, 0x4801), (PROP_MOBSLOT_SLOTNAME, 0x4802), (PROP_MOBSLOT_SEGMENT, 0x4803)]),
            fx_classdef(CLASS_TIMELINE_MOB_SLOT, vec![(PROP_TIMELINE_EDIT_RATE, 0x4b01), (PROP_TIMELINE_ORIGIN, 0x4b02)]),
            fx_classdef(CLASS_SEQUENCE, vec![(PROP_SEQUENCE_COMPONENTS, 0x1001)]),
            fx_classdef(CLASS_COMPONENT, vec![(PROP_COMPONENT_LENGTH, 0x0202), (PROP_COMPONENT_DATA_DEFINITION, 0x0201)]),
            fx_classdef(
                CLASS_SOURCE_CLIP,
                vec![
                    (PROP_SOURCE_REFERENCE_SOURCE_ID, 0x1101),
                    (PROP_SOURCE_REFERENCE_SOURCE_SLOT_ID, 0x1102),
                    (PROP_SOURCE_CLIP_START_TIME, 0x1201),
                ],
            ),
        ];
        let metadictionary = fx_metadictionary(classdefs);

        let filler = fx_storage(
            "Filler",
            CLASS_FILLER,
            vec![fx_stream("properties", properties_stream(&[(0x0202, SF_DATA, 4_800i64.to_le_bytes().to_vec())]))],
        );
        let source_clip = fx_storage(
            "SourceClip",
            CLASS_SOURCE_CLIP,
            vec![fx_stream(
                "properties",
                properties_stream(&[
                    (0x0202, SF_DATA, 48_000i64.to_le_bytes().to_vec()),
                    (0x1201, SF_DATA, 2_400i64.to_le_bytes().to_vec()),
                    (0x1101, SF_DATA, source_mob_id.to_vec()),
                    (0x1102, SF_DATA, 1u32.to_le_bytes().to_vec()),
                ]),
            )],
        );
        let (components_index, components) = fx_vector("Components", vec![filler, source_clip]);
        let mut segment_children = vec![
            fx_stream(
                "properties",
                properties_stream(&[
                    (0x1001, SF_STRONG_OBJECT_REFERENCE_VECTOR, utf16le("Components")),
                    (0x0201, SF_WEAK_OBJECT_REFERENCE, weak_ref(&DATA_DEF_SOUND)),
                ]),
            ),
            components_index,
        ];
        segment_children.extend(components);
        let segment = fx_storage("Segment", CLASS_SEQUENCE, segment_children);

        let mut numerator_denominator = 48_000i32.to_le_bytes().to_vec();
        numerator_denominator.extend_from_slice(&1i32.to_le_bytes());
        let slot = fx_storage(
            "TimelineMobSlot",
            CLASS_TIMELINE_MOB_SLOT,
            vec![
                fx_stream(
                    "properties",
                    properties_stream(&[
                        (0x4801, SF_DATA, 1u32.to_le_bytes().to_vec()),
                        (0x4802, SF_DATA, utf16le("DX")),
                        (0x4803, SF_STRONG_OBJECT_REFERENCE, utf16le("Segment")),
                        (0x4b01, SF_DATA, numerator_denominator),
                        (0x4b02, SF_DATA, 9_600i64.to_le_bytes().to_vec()),
                    ]),
                ),
                segment,
            ],
        );
        let (slots_index, slots) = fx_vector("Slots", vec![slot]);
        let mut composition_children = vec![
            fx_stream(
                "properties",
                properties_stream(&[(0x4402, SF_DATA, utf16le("AAF timing")), (0x4403, SF_STRONG_OBJECT_REFERENCE_VECTOR, utf16le("Slots"))]),
            ),
            slots_index,
        ];
        composition_children.extend(slots);
        let composition_mob = fx_storage("CompositionMob", CLASS_COMPOSITION_MOB, composition_children);

        let source_mob =
            fx_storage("SourceMob", CLASS_SOURCE_MOB, vec![fx_stream("properties", properties_stream(&[(0x4402, SF_DATA, utf16le("take.wav"))]))]);

        let (mobs_index, mobs) = fx_set("Mobs", vec![(source_mob_id.to_vec(), source_mob), (vec![0x52; 32], composition_mob)], 32);
        let mut content_storage_children =
            vec![fx_stream("properties", properties_stream(&[(0x1901, SF_STRONG_OBJECT_REFERENCE_SET, utf16le("Mobs"))])), mobs_index];
        content_storage_children.extend(mobs);
        let content_storage = fx_storage("ContentStorage", CLASS_CONTENT_STORAGE, content_storage_children);

        let header = fx_storage(
            "Header",
            CLASS_HEADER,
            vec![fx_stream("properties", properties_stream(&[(0x3b03, SF_STRONG_OBJECT_REFERENCE, utf16le("ContentStorage"))])), content_storage],
        );

        let root_props = properties_stream(&[
            (PID_ROOT_META_DICTIONARY, SF_STRONG_OBJECT_REFERENCE, utf16le("MetaDictionary")),
            (PID_ROOT_HEADER, SF_STRONG_OBJECT_REFERENCE, utf16le("Header")),
        ]);
        write_aaf_fixture(path, vec![fx_stream("properties", root_props), metadictionary, header]);
    }

    #[test]
    fn the_import_command_saves_the_session_in_the_chosen_folder() {
        let dir = crate::test_scratch::root().join(format!("soundcraft-aaf-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        synthetic_aaf(&dir.join("session.aaf"));
        let mut engine = Engine::default();
        let res = engine.execute("file.import_aaf", &json!({"path": dir.to_string_lossy(), "save_in_folder": true})).unwrap();
        let saved = std::path::PathBuf::from(res["saved_to"].as_str().unwrap());
        assert_eq!(saved.parent(), Some(dir.as_path()));
        assert!(saved.is_file());
        assert_eq!(engine.path.as_deref(), saved.to_str());
        let again = engine.execute("file.import_aaf", &json!({"path": dir.to_string_lossy(), "save_in_folder": true})).unwrap();
        let plain = engine.execute("file.import_aaf", &json!({"path": dir.to_string_lossy()})).unwrap();
        assert!(plain.get("saved_to").is_none());
        assert_ne!(again["saved_to"], res["saved_to"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_import_command_also_imports_the_midi_and_report_in_the_folder() {
        let dir = crate::test_scratch::root().join(format!("soundcraft-aaf-companions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        synthetic_aaf(&dir.join("session.aaf"));
        let mut smf = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk".to_vec();
        let track = [0, 0x90, 60, 100, 0x83, 0x60, 0x80, 60, 0, 0, 0xff, 0x2f, 0];
        smf.extend((track.len() as u32).to_be_bytes());
        smf.extend(track);
        std::fs::write(dir.join("song.mid"), smf).unwrap();
        std::fs::write(dir.join("report.txt"), "SESSION NAME: The Gift\n").unwrap();
        let mut engine = Engine::default();
        let res = engine.execute("file.import_aaf", &json!({"path": dir.to_string_lossy()})).unwrap();
        assert_eq!(res["midi_tracks"], 1, "{res}");
        assert!(res["notes"].as_array().is_some_and(|n| !n.is_empty()), "{res}");

        let none = engine.execute("file.import_aaf", &json!({"path": dir.to_string_lossy(), "companions": false})).unwrap();
        assert!(none.get("midi_tracks").is_none() && none.get("notes").is_none(), "{none}");

        std::fs::write(dir.join("other.mid"), b"").unwrap();
        let many = engine.execute("file.import_aaf", &json!({"path": dir.to_string_lossy()})).unwrap();
        assert!(many.get("midi_tracks").is_none(), "{many}");
        assert!(many["warnings"].as_array().is_some_and(|w| w.iter().any(|w| w.as_str().is_some_and(|w| w.contains("more than one")))), "{many}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn listing_a_directory_over_the_limit_is_an_error() {
        let dir = crate::test_scratch::root().join(format!("soundcraft-list-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for n in 0..3 {
            std::fs::write(dir.join(format!("{n}.wav")), b"").unwrap();
        }
        assert_eq!(crate::io::list_dir(&dir, 3).unwrap().len(), 3);
        assert!(crate::io::list_dir(&dir, 2).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_corrupt_wav_does_not_abort_the_sample_rate_probe() {
        let dir = crate::test_scratch::root().join(format!("soundcraft-aaf-badwav-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.wav"), b"not a wav").unwrap();
        assert_eq!(media_sample_rate(Some(&dir)).unwrap(), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn importing_a_folder_uses_the_aaf_inside_it() {
        let dir = crate::test_scratch::root().join(format!("soundcraft-aaf-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(import(&mut Engine::default(), &dir, None).is_err());
        synthetic_aaf(&dir.join("session.aaf"));
        let mut engine = Engine::default();
        assert_eq!(import(&mut engine, &dir, None).unwrap().clips, 1);
        assert!(engine.path.is_none());
        engine.path = Some("old.scraft".into());
        import(&mut engine, &dir, None).unwrap();
        assert!(engine.path.is_none());
        synthetic_aaf(&dir.join("other.AAF"));
        assert!(import(&mut engine, &dir, None).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn imports_timing_and_offline_wav_reference_from_a_synthetic_aaf() {
        let dir = crate::test_scratch::root().join(format!("soundcraft-aaf-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("timing.aaf");
        synthetic_aaf(&path);

        let mut engine = Engine::default();
        let report = import(&mut engine, &path, None).unwrap();

        assert_eq!(report.sample_rate, 48_000);
        assert_eq!(report.audio_tracks, 1);
        assert_eq!(report.clips, 1);
        assert_eq!(report.linked_wav_files, 0);
        let track = engine.session().tracks.first().unwrap();
        let clip = track.clips().first().unwrap();
        assert_eq!(track.name, "DX");
        assert_eq!(clip.start, 14_400);
        assert_eq!(clip.source_offset(), 2_400);
        assert_eq!(clip.length, 48_000);
        assert_eq!(engine.session().sources.first().map(|source| source.unsaved), Some(false));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn links_wav_files_found_next_to_the_aaf_by_default() {
        let dir = crate::test_scratch::root().join(format!("soundcraft-aaf-same-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("timing.aaf");
        synthetic_aaf(&path);
        let audio = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0; 48_000]] };
        let wav = soundcraft_audio_io::encode(&audio, &soundcraft_audio_io::EncodeOptions::default()).unwrap();
        std::fs::write(dir.join("take.wav"), wav).unwrap();

        let mut engine = Engine::default();
        let report = import(&mut engine, &path, None).unwrap();

        assert_eq!(report.linked_wav_files, 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn imports_a_cfb_container_with_trailing_slack_bytes_after_its_last_sector() {
        // Real Pro Tools AAF exports are commonly a few bytes short of the next sector boundary
        // (observed: a `The Gift.aaf` export landed 179 bytes into what would be its next
        // 512-byte sector). Those bytes sit past every sector `preflight_cfb` can address, so
        // they are inert slack rather than part of the container; such a file must still import.
        let dir = crate::test_scratch::root().join(format!("soundcraft-aaf-slack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("slack.aaf");
        synthetic_aaf(&path);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend_from_slice(&[0u8; 179]);
        std::fs::write(&path, &bytes).unwrap();

        let mut engine = Engine::default();
        let report = import(&mut engine, &path, None).unwrap();

        assert_eq!(report.audio_tracks, 1);
        assert_eq!(report.clips, 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_a_non_compound_file_without_changing_the_document() {
        let dir = crate::test_scratch::root().join(format!("soundcraft-aaf-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.aaf");
        // Header sector plus one data sector of non-CFB bytes: long enough to pass the length
        // and sector-count preflight so the test exercises the magic-signature check (what a
        // `.ptx`/`.ptf` or other non-AAF file hits), not the "too short to be a container" check.
        std::fs::write(&path, [b'x'; 2 * CFB_SECTOR_SIZE as usize]).unwrap();

        let mut engine = Engine::default();
        engine.session_mut().name = "unchanged".into();
        let error = import(&mut engine, &path, None).unwrap_err();

        assert!(error.to_string().contains("compound-file"), "{error}");
        assert_eq!(engine.session().name, "unchanged");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_a_file_too_short_to_be_a_container_without_changing_the_document() {
        let dir = crate::test_scratch::root().join(format!("soundcraft-aaf-short-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("short.aaf");
        std::fs::write(&path, b"not an AAF").unwrap();

        let mut engine = Engine::default();
        engine.session_mut().name = "unchanged".into();
        let error = import(&mut engine, &path, None).unwrap_err();

        assert!(error.to_string().contains("CFB"), "{error}");
        assert_eq!(engine.session().name, "unchanged");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_an_aaf_larger_than_the_file_size_limit() {
        let dir = crate::test_scratch::root().join(format!("soundcraft-aaf-huge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("huge.aaf");
        // Sparse file: the limit is checked from metadata length before any read.
        let file = File::create(&path).unwrap();
        file.set_len(MAX_AAF_FILE_BYTES + 1).unwrap();
        drop(file);

        let mut engine = Engine::default();
        let error = import(&mut engine, &path, None).unwrap_err();

        assert!(error.to_string().contains("MiB"), "{error}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn session_report_keeps_only_migration_relevant_lines() {
        let dir = crate::test_scratch::root().join(format!("soundcraft-aaf-report-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("report.txt");
        std::fs::write(
            &path,
            "SESSION NAME: The Gift\n\
             SAMPLE RATE: 48000\n\
             BIT DEPTH: 24-bit\n\
             # OF AUDIO TRACKS: 12\n\
             an unrelated line that should be dropped\n\
             TRACK NAME: Lead Vocal\n\
             PLUG-INS:  1 - Some AAX Plug-in\n\
             CHANNEL  EVENT  CLIP NAME  START TIME\n",
        )
        .unwrap();

        let notes = session_report_notes(&path).unwrap();

        assert!(notes.iter().any(|n| n.contains("SESSION NAME")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("SAMPLE RATE")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("BIT DEPTH")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("# OF AUDIO TRACKS")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("TRACK NAME")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("PLUG-INS")), "{notes:?}");
        assert!(!notes.iter().any(|n| n.contains("unrelated line")), "{notes:?}");
        assert!(!notes.iter().any(|n| n.contains("CHANNEL  EVENT")), "{notes:?}");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn session_report_notes_are_bounded_and_say_so() {
        let dir = crate::test_scratch::root().join(format!("soundcraft-aaf-report-bound-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("report.txt");
        let text = "PLUG-INS: filler\n".repeat(MAX_SESSION_REPORT_NOTES + 10);
        std::fs::write(&path, text).unwrap();

        let notes = session_report_notes(&path).unwrap();

        assert_eq!(notes.len(), MAX_SESSION_REPORT_NOTES);
        assert_eq!(notes.last().map(String::as_str), Some("further session-report notes were suppressed"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn session_report_notes_reports_a_missing_file_clearly() {
        let dir = crate::test_scratch::root().join(format!("soundcraft-aaf-report-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("missing.txt");

        let error = session_report_notes(&path).unwrap_err();

        assert!(error.to_string().contains("missing.txt"), "{error}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
