//! Strict, read-only AAF reader that emits an interchange [`Plan`]. Pure Rust over `cfb`.
//!
//! Reads the AAF structured-storage object model directly (properties streams, strong
//! reference vectors/sets and dictionary weak references) for a deliberately small subset:
//! one CompositionMob whose timeline slots hold Sequences of SourceClip and Filler, resolved
//! through MasterMobs to SourceMobs with a WAVEDescriptor and one external NetworkLocator.
//! Anything else in an audio slot aborts instead of producing a misleading montage.
//!
//! No filesystem access happens here: the caller passes the opened AAF and a [`MediaResolver`]
//! that turns a decoded locator path into checked media properties.
use crate::{ImportClip, ImportError, ImportTrack, Media, Plan, Rational, TimecodeRate};
use std::{
    collections::HashMap,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, ImportError>;
fn fail(s: impl Into<String>) -> ImportError {
    ImportError(s.into())
}

/// Upper bound on mobs, slots, components and collection entries.
pub const MAX_ITEMS: usize = 100_000;
const MAX_STREAM_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SOURCE_DEPTH: usize = 32;

const CLASS_COMPOSITION_MOB: &str = "0d010101-0101-3500-060e-2b3402060101";
const CLASS_MASTER_MOB: &str = "0d010101-0101-3600-060e-2b3402060101";
const CLASS_SOURCE_MOB: &str = "0d010101-0101-3700-060e-2b3402060101";
const CLASS_TIMELINE_SLOT: &str = "0d010101-0101-3b00-060e-2b3402060101";
const CLASS_SEQUENCE: &str = "0d010101-0101-0f00-060e-2b3402060101";
const CLASS_SOURCE_CLIP: &str = "0d010101-0101-1100-060e-2b3402060101";
const CLASS_FILLER: &str = "0d010101-0101-0900-060e-2b3402060101";
const CLASS_TIMECODE: &str = "0d010101-0101-1400-060e-2b3402060101";
const CLASS_TRANSITION: &str = "0d010101-0101-1700-060e-2b3402060101";
const CLASS_WAVE_DESCRIPTOR: &str = "0d010101-0101-2c00-060e-2b3402060101";

const PID_HEADER: u16 = 0x0002;
const PID_COMPONENT_DATADEF: u16 = 0x0201;
const PID_COMPONENT_LENGTH: u16 = 0x0202;
const PID_SEQUENCE_COMPONENTS: u16 = 0x1001;
const PID_SOURCE_ID: u16 = 0x1101;
const PID_SOURCE_SLOT_ID: u16 = 0x1102;
const PID_START_TIME: u16 = 0x1201;
const PID_TIMECODE_START: u16 = 0x1501;
const PID_TIMECODE_FPS: u16 = 0x1502;
const PID_TIMECODE_DROP: u16 = 0x1503;
const PID_CONTENT_MOBS: u16 = 0x1901;
const PID_CONTENT_ESSENCE: u16 = 0x1902;
const PID_DEF_NAME: u16 = 0x1B02;
const PID_ESSENCE_MOB_ID: u16 = 0x2701;
const PID_DESCRIPTOR_LOCATORS: u16 = 0x2F01;
const PID_FILE_SAMPLE_RATE: u16 = 0x3001;
const PID_FILE_LENGTH: u16 = 0x3002;
const PID_HEADER_CONTENT: u16 = 0x3B03;
const PID_URL_STRING: u16 = 0x4001;
const PID_MOB_ID: u16 = 0x4401;
const PID_MOB_NAME: u16 = 0x4402;
const PID_MOB_SLOTS: u16 = 0x4403;
const PID_SOURCE_MOB_DESCRIPTOR: u16 = 0x4701;
const PID_SLOT_ID: u16 = 0x4801;
const PID_SLOT_NAME: u16 = 0x4802;
const PID_SLOT_SEGMENT: u16 = 0x4803;
const PID_SLOT_PHYSICAL: u16 = 0x4804;
const PID_SLOT_EDIT_RATE: u16 = 0x4B01;
const PID_SLOT_ORIGIN: u16 = 0x4B02;

const SF_DATA: u16 = 0x82;
const SF_STRONG_REF: u16 = 0x22;
const SF_STRONG_VECTOR: u16 = 0x32;
const SF_STRONG_SET: u16 = 0x3A;
const SF_WEAK_REF: u16 = 0x02;

/// Media properties of one external source, checked by the caller.
#[derive(Debug, Clone)]
pub struct MediaInfo {
    /// Absolute, resolved path of the file that will be imported.
    pub path: String,
    /// The resolved locator path when the caller substituted another file (explicit relink).
    pub relinked_from: Option<String>,
    pub channels: u16,
    pub frames: u64,
    pub sample_rate: u32,
}

#[derive(Debug)]
pub enum MediaError {
    /// The (possibly relinked) file does not exist.
    Missing(String),
    Invalid(String),
}

/// Supplies checked properties for a locator. `locator` is the percent-decoded local path,
/// already joined to the AAF's folder when the URL was relative. Implementations decide how
/// to resolve, confine and relink it.
pub trait MediaResolver {
    fn resolve(&mut self, locator: &Path) -> std::result::Result<MediaInfo, MediaError>;
}

pub struct AafOptions<'a> {
    /// Explicit CompositionMob MobID (`urn:smpte:umid:...`) when the file holds several.
    pub composition: Option<&'a str>,
    /// Folder of the AAF file, used for relative locators.
    pub base_dir: &'a Path,
    /// Session name when the composition has none (usually the AAF file stem).
    pub default_name: &'a str,
}

/// Readable names for classes that commonly appear in rejected structures.
const CLASS_NAMES: &[(&str, &str)] = &[
    (CLASS_COMPOSITION_MOB, "CompositionMob"),
    (CLASS_MASTER_MOB, "MasterMob"),
    (CLASS_SOURCE_MOB, "SourceMob"),
    (CLASS_TIMELINE_SLOT, "TimelineMobSlot"),
    (CLASS_SEQUENCE, "Sequence"),
    (CLASS_SOURCE_CLIP, "SourceClip"),
    (CLASS_FILLER, "Filler"),
    (CLASS_TIMECODE, "Timecode"),
    (CLASS_TRANSITION, "Transition"),
    (CLASS_WAVE_DESCRIPTOR, "WAVEDescriptor"),
    ("0d010101-0101-0a00-060e-2b3402060101", "OperationGroup"),
    ("0d010101-0101-0e00-060e-2b3402060101", "Selector"),
    ("0d010101-0101-0b00-060e-2b3402060101", "NestedScope"),
    ("0d010101-0101-0d00-060e-2b3402060101", "ScopeReference"),
    ("0d010101-0101-0500-060e-2b3402060101", "EssenceGroup"),
    ("0d010101-0101-0c00-060e-2b3402060101", "Pulldown"),
    ("0d010101-0101-0400-060e-2b3402060101", "EdgeCode"),
    ("0d010101-0101-0600-060e-2b3402060101", "Event"),
    ("0d010101-0101-0800-060e-2b3402060101", "CommentMarker"),
    ("0d010101-0101-2e00-060e-2b3402060101", "TapeDescriptor"),
    ("0d010101-0101-4800-060e-2b3402060101", "PCMDescriptor"),
    ("0d010101-0101-2600-060e-2b3402060101", "AIFCDescriptor"),
    ("0d010101-0101-4400-060e-2b3402060101", "MultipleDescriptor"),
    ("0d010101-0101-4a00-060e-2b3402060101", "ImportDescriptor"),
    ("0d010101-0101-3900-060e-2b3402060101", "EventMobSlot"),
    ("0d010101-0101-3a00-060e-2b3402060101", "StaticMobSlot"),
];

// ---------------------------------------------------------------------------------------------
// Exact rational arithmetic (Python Fraction semantics within i128).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Q {
    n: i128,
    d: i128,
}
fn gcd(mut a: i128, mut b: i128) -> i128 {
    a = a.abs();
    b = b.abs();
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}
impl Q {
    fn new(n: i128, d: i128) -> Result<Q> {
        if d == 0 {
            return Err(fail("zero denominator"));
        }
        let (n, d) = if d < 0 { (n.checked_neg(), d.checked_neg()) } else { (Some(n), Some(d)) };
        let (n, d) = n.zip(d).ok_or_else(|| fail("rational overflow"))?;
        let g = gcd(n, d).max(1);
        Ok(Q { n: n / g, d: d / g })
    }
    fn int(n: i64) -> Q {
        Q { n: i128::from(n), d: 1 }
    }
    fn add(self, o: Q) -> Result<Q> {
        let n = self.n.checked_mul(o.d).zip(o.n.checked_mul(self.d)).and_then(|(a, b)| a.checked_add(b));
        let d = self.d.checked_mul(o.d);
        Q::new(n.ok_or_else(|| fail("rational overflow"))?, d.ok_or_else(|| fail("rational overflow"))?)
    }
    fn div(self, o: Q) -> Result<Q> {
        let n = self.n.checked_mul(o.d);
        let d = self.d.checked_mul(o.n);
        Q::new(n.ok_or_else(|| fail("rational overflow"))?, d.ok_or_else(|| fail("rational overflow"))?)
    }
    fn lt(self, o: Q) -> bool {
        // Denominators are positive after normalisation; overflow compares as "not less".
        match (self.n.checked_mul(o.d), o.n.checked_mul(self.d)) {
            (Some(a), Some(b)) => a < b,
            _ => false,
        }
    }
    /// Python `round()`: nearest integer, ties to even.
    fn round_half_even(self) -> i128 {
        let floor = self.n.div_euclid(self.d);
        let twice = (self.n - floor * self.d) * 2;
        if twice < self.d || (twice == self.d && floor % 2 == 0) { floor } else { floor + 1 }
    }
    fn plan(self) -> Result<Rational> {
        Ok(Rational(i64::try_from(self.n).map_err(|_| fail("time overflow"))?, i64::try_from(self.d).map_err(|_| fail("time overflow"))?))
    }
}

// ---------------------------------------------------------------------------------------------
// Structured storage object model.

#[derive(Debug, Clone)]
struct Obj {
    path: String,
    class: String,
    props: HashMap<u16, (u16, Vec<u8>)>,
}

impl Obj {
    fn data(&self, pid: u16) -> Option<&[u8]> {
        self.props.get(&pid).filter(|(form, _)| *form == SF_DATA).map(|(_, d)| d.as_slice())
    }
    fn has(&self, pid: u16) -> bool {
        self.props.contains_key(&pid)
    }
    fn bytes<const N: usize>(&self, pid: u16, what: &str) -> Result<[u8; N]> {
        self.data(pid).and_then(|d| d.get(..N)).and_then(|d| d.try_into().ok()).ok_or_else(|| fail(format!("missing or short {what}")))
    }
    fn u32(&self, pid: u16, what: &str) -> Result<u32> {
        self.bytes::<4>(pid, what).map(u32::from_le_bytes)
    }
    fn i64(&self, pid: u16, what: &str) -> Result<i64> {
        self.bytes::<8>(pid, what).map(i64::from_le_bytes)
    }
    fn opt_u32(&self, pid: u16, what: &str) -> Result<Option<u32>> {
        if self.has(pid) { self.u32(pid, what).map(Some) } else { Ok(None) }
    }
    fn rational(&self, pid: u16, what: &str) -> Result<Q> {
        let b = self.bytes::<8>(pid, what)?;
        let n = i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let d = i32::from_le_bytes([b[4], b[5], b[6], b[7]]);
        Q::new(i128::from(n), i128::from(d))
    }
    /// Optional string; absent or empty reads as `None` like a falsy Python name.
    fn string(&self, pid: u16) -> Option<String> {
        self.data(pid).map(utf16).filter(|s| !s.is_empty())
    }
    fn length(&self) -> Result<i64> {
        self.i64(PID_COMPONENT_LENGTH, "component Length")
    }
    fn is(&self, class: &str) -> bool {
        self.class == class
    }
    /// Class name for messages; unknown (extension) classes print their AUID.
    fn kind(&self) -> &str {
        CLASS_NAMES.iter().find(|(id, _)| *id == self.class).map_or(self.class.as_str(), |(_, n)| n)
    }
}

fn utf16(d: &[u8]) -> String {
    let units: Vec<u16> = d.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|u| *u != 0).collect();
    String::from_utf16_lossy(&units)
}

fn read_u16(d: &[u8], at: usize) -> Result<u16> {
    d.get(at..at + 2).and_then(|b| b.try_into().ok()).map(u16::from_le_bytes).ok_or_else(|| fail("truncated AAF structure"))
}
fn read_u32(d: &[u8], at: usize) -> Result<u32> {
    d.get(at..at + 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes).ok_or_else(|| fail("truncated AAF structure"))
}

/// `urn:smpte:umid:` text identical to pyaaf2 and Pro Tools' MobID notation.
fn mob_urn(b: &[u8; 32]) -> String {
    let label: String = b[..12].chunks(4).map(|c| c.iter().map(|x| format!("{x:02x}")).collect::<String>()).collect::<Vec<_>>().join(".");
    let head = format!("urn:smpte:umid:{label}.{:02x}{:02x}{:02x}{:02x}", b[12], b[13], b[14], b[15]);
    let data1 = u32::from_le_bytes([b[16], b[17], b[18], b[19]]);
    let data2 = u16::from_le_bytes([b[20], b[21]]);
    let data3 = u16::from_le_bytes([b[22], b[23]]);
    let d4 = &b[24..32];
    let d4a = format!("{:02x}{:02x}{:02x}{:02x}", d4[0], d4[1], d4[2], d4[3]);
    let d4b = format!("{:02x}{:02x}{:02x}{:02x}", d4[4], d4[5], d4[6], d4[7]);
    // Material numbers stored half swapped (an SMPTE label in Data4) print label-first.
    if b[11] == 0 && d4[..6] == [0x06, 0x0e, 0x2b, 0x34, 0x7f, 0x7f] {
        format!("{head}.{d4a}.{d4b}.{data1:08x}.{data2:04x}{data3:04x}")
    } else {
        format!("{head}.{data1:08x}.{data2:04x}{data3:04x}.{d4a}.{d4b}")
    }
}

struct Store<F> {
    cfb: cfb::CompoundFile<F>,
    objects_read: usize,
    weakrefs: Vec<Vec<u16>>,
    datadefs: HashMap<Vec<u8>, Option<String>>,
}

impl<F: Read + Seek> Store<F> {
    fn open(file: F) -> Result<Self> {
        let cfb = cfb::CompoundFile::open(file).map_err(|e| fail(format!("not an AAF structured-storage file: {e}")))?;
        let mut s = Store { cfb, objects_read: 0, weakrefs: Vec::new(), datadefs: HashMap::new() };
        s.weakrefs = s.read_weakref_table()?;
        Ok(s)
    }

    fn stream(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        if !self.cfb.is_stream(path) {
            return Ok(None);
        }
        let s = self.cfb.open_stream(path).map_err(|e| fail(format!("{path}: {e}")))?;
        let mut out = Vec::new();
        s.take(MAX_STREAM_BYTES + 1).read_to_end(&mut out).map_err(|e| fail(format!("{path}: {e}")))?;
        if out.len() as u64 > MAX_STREAM_BYTES {
            return Err(fail(format!("{path}: AAF stream exceeds 32 MiB")));
        }
        Ok(Some(out))
    }

    fn read_weakref_table(&mut self) -> Result<Vec<Vec<u16>>> {
        let Some(d) = self.stream("/referenced properties")? else { return Ok(Vec::new()) };
        if d.first() != Some(&0x4c) {
            return Err(fail("unsupported AAF byte order"));
        }
        let paths = usize::from(read_u16(&d, 1)?);
        let pids = read_u32(&d, 3)? as usize;
        if pids > MAX_ITEMS {
            return Err(fail("referenced property table too large"));
        }
        let mut table = Vec::new();
        let mut path = Vec::new();
        for i in 0..pids {
            match read_u16(&d, 7 + 2 * i)? {
                0 => table.push(std::mem::take(&mut path)),
                pid => path.push(pid),
            }
        }
        if table.len() != paths {
            return Err(fail("inconsistent referenced property table"));
        }
        Ok(table)
    }

    fn object(&mut self, path: &str) -> Result<Obj> {
        self.objects_read += 1;
        if self.objects_read > 4 * MAX_ITEMS {
            return Err(fail("too many AAF objects"));
        }
        let entry = self.cfb.entry(path).map_err(|e| fail(format!("{path}: {e}")))?;
        if !entry.is_storage() {
            return Err(fail(format!("{path}: not an AAF object")));
        }
        let class = entry.clsid().to_string();
        let props_path = if path == "/" { "/properties".to_string() } else { format!("{path}/properties") };
        let d = self.stream(&props_path)?.ok_or_else(|| fail(format!("{path}: missing properties")))?;
        if d.first() != Some(&0x4c) {
            return Err(fail("unsupported AAF byte order"));
        }
        let count = usize::from(read_u16(&d, 2)?);
        let mut props = HashMap::new();
        let mut at = 4 + 6 * count;
        for i in 0..count {
            let pid = read_u16(&d, 4 + 6 * i)?;
            let form = read_u16(&d, 6 + 6 * i)?;
            let size = usize::from(read_u16(&d, 8 + 6 * i)?);
            let value = d.get(at..at + size).ok_or_else(|| fail(format!("{path}: truncated properties")))?.to_vec();
            at += size;
            props.insert(pid, (form, value));
        }
        Ok(Obj { path: path.to_string(), class, props })
    }

    fn child_path(parent: &Obj, name: &str) -> String {
        if parent.path == "/" { format!("/{name}") } else { format!("{}/{name}", parent.path) }
    }

    fn reference(parent: &Obj, pid: u16, form: u16) -> Option<String> {
        parent.props.get(&pid).filter(|(f, _)| *f == form).map(|(_, d)| utf16(d))
    }

    fn strong(&mut self, parent: &Obj, pid: u16, what: &str) -> Result<Obj> {
        let name = Self::reference(parent, pid, SF_STRONG_REF).ok_or_else(|| fail(format!("missing {what}")))?;
        self.object(&Self::child_path(parent, &name))
    }

    fn opt_strong(&mut self, parent: &Obj, pid: u16, what: &str) -> Result<Option<Obj>> {
        if parent.has(pid) { self.strong(parent, pid, what).map(Some) } else { Ok(None) }
    }

    /// Strong reference vector, in stored order. Absent optional vectors read as empty.
    fn vector(&mut self, parent: &Obj, pid: u16, what: &str, required: bool) -> Result<Vec<Obj>> {
        let Some(name) = Self::reference(parent, pid, SF_STRONG_VECTOR) else {
            return if required { Err(fail(format!("missing {what}"))) } else { Ok(Vec::new()) };
        };
        let index = self.stream(&Self::child_path(parent, &format!("{name} index")))?.ok_or_else(|| fail(format!("missing {what} index")))?;
        let count = read_u32(&index, 0)? as usize;
        if count > MAX_ITEMS {
            return Err(fail(format!("too many {what}")));
        }
        (0..count)
            .map(|i| {
                let key = read_u32(&index, 12 + 4 * i)?;
                self.object(&Self::child_path(parent, &format!("{name}{{{key:x}}}")))
            })
            .collect()
    }

    /// Strong reference set: (key bytes, child storage path) in stored order.
    fn set_entries(&mut self, parent: &Obj, pid: u16, what: &str) -> Result<Vec<(Vec<u8>, String)>> {
        let Some(name) = Self::reference(parent, pid, SF_STRONG_SET) else { return Ok(Vec::new()) };
        let index = self.stream(&Self::child_path(parent, &format!("{name} index")))?.ok_or_else(|| fail(format!("missing {what} index")))?;
        let count = read_u32(&index, 0)? as usize;
        let key_size = usize::from(*index.get(14).ok_or_else(|| fail(format!("truncated {what} index")))?);
        if count > MAX_ITEMS || !matches!(key_size, 16 | 32) {
            return Err(fail(format!("unsupported {what} index")));
        }
        let entry = 8 + key_size;
        (0..count)
            .map(|i| {
                let at = 15 + entry * i;
                let local = read_u32(&index, at)?;
                let key = index.get(at + 8..at + 8 + key_size).ok_or_else(|| fail(format!("truncated {what} index")))?.to_vec();
                Ok((key, Self::child_path(parent, &format!("{name}{{{local:x}}}"))))
            })
            .collect()
    }

    fn set(&mut self, parent: &Obj, pid: u16, what: &str) -> Result<Vec<Obj>> {
        let entries = self.set_entries(parent, pid, what)?;
        entries.into_iter().map(|(_, path)| self.object(&path)).collect()
    }

    /// Short name of a component's data definition (`DataDef_` prefix removed), as pyaaf2's
    /// `media_kind`. Resolved through the file's referenced-property table, not built-ins.
    fn media_kind(&mut self, root: &Obj, component: &Obj) -> Result<Option<String>> {
        let Some((form, d)) = component.props.get(&PID_COMPONENT_DATADEF) else { return Ok(None) };
        if *form != SF_WEAK_REF {
            return Err(fail("unsupported DataDefinition reference"));
        }
        let index = usize::from(read_u16(d, 0)?);
        let size = usize::from(*d.get(4).ok_or_else(|| fail("truncated DataDefinition reference"))?);
        let key = d.get(5..5 + size).ok_or_else(|| fail("truncated DataDefinition reference"))?.to_vec();
        if let Some(name) = self.datadefs.get(&key) {
            return Ok(name.clone());
        }
        let path = self.weakrefs.get(index).cloned().ok_or_else(|| fail("unknown DataDefinition reference"))?;
        let (last, walk) = path.split_last().ok_or_else(|| fail("empty weak reference path"))?;
        let mut owner = root.clone();
        for pid in walk {
            owner = self.strong(&owner, *pid, "weak reference owner")?;
        }
        let target = self.set_entries(&owner, *last, "data definitions")?.into_iter().find(|(k, _)| *k == key).map(|(_, p)| p);
        let path = target.ok_or_else(|| fail("DataDefinition not found in dictionary"))?;
        let name = self.object(&path)?.string(PID_DEF_NAME).map(|n| n.replace("DataDef_", "").replace("ContainerDef_", ""));
        self.datadefs.insert(key, name.clone());
        Ok(name)
    }
}

// ---------------------------------------------------------------------------------------------
// Plan construction (same rules and messages as the original pyaaf2 adapter).

fn rate(slot: &Obj) -> Result<Q> {
    if !slot.is(CLASS_TIMELINE_SLOT) || slot.i64(PID_SLOT_ORIGIN, "slot Origin")? != 0 {
        let id = slot.u32(PID_SLOT_ID, "SlotID").map(|i| i.to_string()).unwrap_or_default();
        return Err(fail(format!("Unsupported slot/origin: {} {id}", slot.kind())));
    }
    let r = slot.rational(PID_SLOT_EDIT_RATE, "slot EditRate")?;
    if r.n <= 0 {
        return Err(fail("Edit rate must be positive"));
    }
    Ok(r)
}

/// Local path from a NetworkLocator URL (`file:` or scheme-less), percent-decoded.
fn locator_path(url: &str, base: &Path) -> Result<PathBuf> {
    let mut rest = url;
    let mut scheme = String::new();
    if let Some(i) = url.find(':') {
        let s = &url[..i];
        if s.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) {
            scheme = s.to_ascii_lowercase();
            rest = &url[i + 1..];
        }
    }
    let mut netloc = "";
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        netloc = &after[..end];
        rest = &after[end..];
    }
    let (rest, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    if !matches!(scheme.as_str(), "" | "file") || !matches!(netloc, "" | "localhost") {
        return Err(fail(format!("Unsupported non-local locator: {url}")));
    }
    if !query.is_empty() || !fragment.is_empty() {
        return Err(fail(format!("Locator must escape special characters: {url}")));
    }
    let p = PathBuf::from(percent_decode(path));
    Ok(if p.is_absolute() { p } else { base.join(p) })
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b[i], hex) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

struct Builder<'a, F> {
    store: Store<F>,
    root: Obj,
    mobs: HashMap<String, Obj>,
    embedded: Vec<String>,
    options: &'a AafOptions<'a>,
    media: &'a mut dyn MediaResolver,
    sources: Vec<Media>,
    warnings: Vec<String>,
}

impl<F: Read + Seek> Builder<'_, F> {
    fn slot_of(&mut self, mob: &Obj, slot_id: u32) -> Result<Option<Obj>> {
        for slot in self.store.vector(mob, PID_MOB_SLOTS, "mob slots", true)? {
            if slot.u32(PID_SLOT_ID, "SlotID")? == slot_id {
                return Ok(Some(slot));
            }
        }
        Ok(None)
    }

    fn slot_length(&mut self, slot: &Obj) -> Result<i64> {
        let seg = self.store.strong(slot, PID_SLOT_SEGMENT, "slot Segment")?;
        if !seg.is(CLASS_SEQUENCE) {
            return seg.length();
        }
        let mut total = 0i64;
        for c in self.store.vector(&seg, PID_SEQUENCE_COMPONENTS, "sequence components", true)? {
            let l = c.length()?;
            total = if c.is(CLASS_TRANSITION) { total.checked_sub(l) } else { total.checked_add(l) }.ok_or_else(|| fail("length overflow"))?;
        }
        Ok(total)
    }

    /// Follows a SourceClip to its file SourceMob. Returns (source key, mob, offset seconds).
    fn resolve(&mut self, clip: &Obj, incoming: Q, mut offset: Q, seen: &mut Vec<(String, u32)>) -> Result<(String, Q)> {
        if !clip.is(CLASS_SOURCE_CLIP) {
            return Err(fail(format!("Unsupported source chain component: {}", clip.kind())));
        }
        let id = clip.bytes::<32>(PID_SOURCE_ID, "SourceID").ok();
        let slot_id = clip.u32(PID_SOURCE_SLOT_ID, "SourceMobSlotID")?;
        let urn = id.filter(|b| b.iter().any(|x| *x != 0)).map(|b| mob_urn(&b));
        let mob = urn.as_ref().and_then(|u| self.mobs.get(u)).cloned();
        let Some(mob) = mob else {
            return Err(fail(format!("Unresolved SourceClip MobID/slot: {}/{slot_id}", urn.unwrap_or_default())));
        };
        let key_urn = urn.unwrap_or_default();
        let slot = self.slot_of(&mob, slot_id)?.ok_or_else(|| fail(format!("Unresolved SourceClip MobID/slot: {key_urn}/{slot_id}")))?;
        if seen.contains(&(key_urn.clone(), slot_id)) || seen.len() >= MAX_SOURCE_DEPTH {
            return Err(fail("Cyclic or too deep source chain"));
        }
        let rr = rate(&slot)?;
        let start = clip.i64(PID_START_TIME, "SourceClip StartTime")?;
        offset = offset.add(Q::int(start).div(rr)?)?;
        let name = mob.string(PID_MOB_NAME);
        if mob.is(CLASS_SOURCE_MOB) && self.embedded.contains(&key_urn) {
            return Err(fail(format!("Embedded essence unsupported: {}", name.unwrap_or_default())));
        }
        if mob.is(CLASS_SOURCE_MOB) {
            let desc = self.store.opt_strong(&mob, PID_SOURCE_MOB_DESCRIPTOR, "EssenceDescription")?;
            if let Some(desc) = desc.filter(|d| d.is(CLASS_WAVE_DESCRIPTOR)) {
                return self.file_source(&mob, &slot, &desc, key_urn, offset);
            }
        }
        if !mob.is(CLASS_MASTER_MOB) {
            return Err(fail(format!("Unsupported referenced mob/descriptor: {}", name.unwrap_or_default())));
        }
        let window_end = offset.add(Q::int(clip.length()?).div(incoming)?)?;
        let slot_end = Q::int(self.slot_length(&slot)?).div(rr)?;
        if offset.lt(Q::int(0)) || slot_end.lt(window_end) {
            return Err(fail(format!("SourceClip exceeds master slot: {}", name.unwrap_or_default())));
        }
        let segment = self.store.strong(&slot, PID_SLOT_SEGMENT, "slot Segment")?;
        if !segment.is(CLASS_SOURCE_CLIP) {
            return Err(fail(format!("Unsupported master segment: {}", segment.kind())));
        }
        seen.push((key_urn, slot_id));
        self.resolve(&segment, rr, offset, seen)
    }

    fn file_source(&mut self, mob: &Obj, slot: &Obj, desc: &Obj, key: String, offset: Q) -> Result<(String, Q)> {
        let name = mob.string(PID_MOB_NAME);
        let locators = self.store.vector(desc, PID_DESCRIPTOR_LOCATORS, "locators", false)?;
        let urls: Vec<String> = locators.iter().map(|l| l.data(PID_URL_STRING).map(utf16).unwrap_or_default()).collect();
        let [url] = urls.as_slice() else {
            return Err(fail(format!("Expected one unambiguous WAV locator: {}", name.unwrap_or_default())));
        };
        let locator = locator_path(url, self.options.base_dir)?;
        let info = self.media.resolve(&locator).map_err(|e| match e {
            MediaError::Missing(p) => fail(format!("Missing media: {p}; locator={url}")),
            MediaError::Invalid(m) => fail(m),
        })?;
        if let Some(from) = &info.relinked_from {
            self.warnings.push(format!("Relinked {from} -> {}", info.path));
        }
        if !matches!(info.channels, 1 | 2) {
            return Err(fail("Only mono/stereo PCM WAV supported"));
        }
        if desc.rational(PID_FILE_SAMPLE_RATE, "descriptor SampleRate")? != Q::int(i64::from(info.sample_rate)) {
            return Err(fail(format!("Descriptor/file sample rates differ: {}", name.unwrap_or_default())));
        }
        if u64::try_from(desc.i64(PID_FILE_LENGTH, "descriptor Length")?).ok() != Some(info.frames) {
            return Err(fail(format!("Descriptor/file lengths differ: {}", name.unwrap_or_default())));
        }
        // Full interleaved WAV only: selecting one physical channel needs a channel map.
        if !matches!(slot.opt_u32(PID_SLOT_PHYSICAL, "PhysicalTrackNumber")?, None | Some(0)) {
            return Err(fail("Physical channel selection / split stereo unsupported"));
        }
        if !self.sources.iter().any(|s| s.key == key) {
            let stem = Path::new(&info.path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            self.sources.push(Media {
                key: key.clone(),
                name: name.unwrap_or(stem),
                path: info.path,
                channels: info.channels,
                frames: info.frames,
                sample_rate: info.sample_rate,
            });
        }
        Ok((key, offset))
    }
}

/// Reads one composition of an AAF as an interchange plan. Fails on anything outside the
/// supported subset; no partial plan is returned.
pub fn read_aaf<F: Read + Seek>(file: F, options: &AafOptions, media: &mut dyn MediaResolver) -> Result<Plan> {
    let mut store = Store::open(file)?;
    let root = store.object("/")?;
    let header = store.strong(&root, PID_HEADER, "Header")?;
    let content = store.strong(&header, PID_HEADER_CONTENT, "ContentStorage")?;
    let all = store.set(&content, PID_CONTENT_MOBS, "mobs")?;
    if all.len() > MAX_ITEMS {
        return Err(fail("Too many mobs"));
    }
    let mut mobs = HashMap::new();
    let mut compositions = Vec::new();
    for mob in all {
        let urn = mob_urn(&mob.bytes::<32>(PID_MOB_ID, "MobID")?);
        if mob.is(CLASS_COMPOSITION_MOB) {
            compositions.push((urn.clone(), mob.clone()));
        }
        mobs.insert(urn, mob);
    }
    let mut embedded = Vec::new();
    for e in store.set(&content, PID_CONTENT_ESSENCE, "essence data")? {
        embedded.push(mob_urn(&e.bytes::<32>(PID_ESSENCE_MOB_ID, "EssenceData MobID")?));
    }
    if let Some(id) = options.composition {
        compositions.retain(|(urn, _)| urn == id);
    }
    let [(_, comp)] = compositions.as_slice() else {
        let listed: Vec<String> = compositions.iter().map(|(u, m)| format!("({:?}, {u:?})", m.string(PID_MOB_NAME).unwrap_or_default())).collect();
        return Err(fail(format!("Choose exactly one composition by MobID: [{}]", listed.join(", "))));
    };
    let comp = comp.clone();
    let mut b = Builder {
        store,
        root,
        mobs,
        embedded,
        options,
        media,
        sources: Vec::new(),
        warnings: vec![
            "AAF subset: plugins, routing, automation and metadata are not transferred.".into(),
            "Clip gain, fades, transitions, selectors, nested sequences and embedded media are unsupported; encountered editorial structures fail."
                .into(),
        ],
    };
    let mut plan_tracks = Vec::new();
    let mut frame_rate = None;
    let mut timecode_start = Rational(0, 1);
    let mut count = 0usize;
    let slots = b.store.vector(&comp, PID_MOB_SLOTS, "composition slots", true)?;
    for slot in slots {
        if plan_tracks.len() >= MAX_ITEMS {
            return Err(fail("Too many tracks"));
        }
        let rr = rate(&slot)?;
        let seg = b.store.strong(&slot, PID_SLOT_SEGMENT, "slot Segment")?;
        let slot_id = slot.u32(PID_SLOT_ID, "SlotID")?;
        if seg.is(CLASS_TIMECODE) {
            if frame_rate.is_some() {
                return Err(fail("Multiple timecode slots unsupported"));
            }
            let fps = u16::from_le_bytes(seg.bytes::<2>(PID_TIMECODE_FPS, "Timecode FPS")?);
            let drop = seg.bytes::<1>(PID_TIMECODE_DROP, "Timecode Drop")?[0] != 0;
            if rr.round_half_even() != i128::from(fps) {
                return Err(fail("Timecode FPS disagrees with edit rate"));
            }
            frame_rate = Some(TimecodeRate { rate: rr.plan()?, drop });
            timecode_start = Q::int(seg.i64(PID_TIMECODE_START, "Timecode Start")?).div(rr)?.plan()?;
            continue;
        }
        let kind = b.store.media_kind(&b.root, &seg)?;
        if kind.as_deref() != Some("Sound") {
            b.warnings.push(format!("Skipped non-audio slot {slot_id}: {}", kind.as_deref().unwrap_or("None")));
            continue;
        }
        let physical = slot.opt_u32(PID_SLOT_PHYSICAL, "PhysicalTrackNumber")?;
        let fallback = format!("Audio {}", physical.unwrap_or(slot_id));
        let name = match slot.string(PID_SLOT_NAME) {
            Some(n) => n,
            None => {
                b.warnings.push(format!("Unnamed AAF slot {slot_id}: assigned {fallback}"));
                fallback
            }
        };
        let items =
            if seg.is(CLASS_SEQUENCE) { b.store.vector(&seg, PID_SEQUENCE_COMPONENTS, "sequence components", true)? } else { vec![seg.clone()] };
        let mut cursor = 0i64;
        let mut formats = Vec::new();
        let mut clips = Vec::new();
        for item in items {
            count += 1;
            if count > MAX_ITEMS {
                return Err(fail("Too many components"));
            }
            let len = item.length()?;
            if len < 0 {
                return Err(fail("Negative component length"));
            }
            if item.is(CLASS_FILLER) {
                cursor = cursor.checked_add(len).ok_or_else(|| fail("length overflow"))?;
                continue;
            }
            if !item.is(CLASS_SOURCE_CLIP) {
                return Err(fail(format!("Unsupported component in {name}: {}", item.kind())));
            }
            let (sid, offset) = b.resolve(&item, rr, Q::int(0), &mut Vec::new())?;
            let source = b.sources.iter().find(|s| s.key == sid).ok_or_else(|| fail("source missing"))?;
            if !formats.contains(&source.channels) {
                formats.push(source.channels);
            }
            let source_name = source.name.clone();
            let clip_name = item
                .bytes::<32>(PID_SOURCE_ID, "SourceID")
                .ok()
                .and_then(|id| b.mobs.get(&mob_urn(&id)))
                .and_then(|m| m.string(PID_MOB_NAME))
                .unwrap_or(source_name);
            let end = cursor.checked_add(len).ok_or_else(|| fail("length overflow"))?;
            clips.push(ImportClip {
                name: clip_name,
                source: sid,
                start: Q::int(cursor).div(rr)?.plan()?,
                end: Q::int(end).div(rr)?.plan()?,
                offset: offset.plan()?,
            });
            cursor = end;
        }
        if seg.is(CLASS_SEQUENCE) && seg.length()? != cursor {
            return Err(fail("Sequence declared length differs from components"));
        }
        if formats.len() > 1 {
            return Err(fail(format!("Mixed channel layouts in track: {name}")));
        }
        let channels = match formats.first() {
            Some(c) => *c,
            None => {
                b.warnings.push(format!("Empty track {name}: mono layout assumed"));
                1
            }
        };
        plan_tracks.push(ImportTrack { name, channels, clips });
    }
    let mut rates: Vec<u32> = b.sources.iter().map(|s| s.sample_rate).collect();
    rates.sort_unstable();
    rates.dedup();
    let [sample_rate] = rates.as_slice() else {
        return Err(fail("Require at least one source and a single source sample rate; no resampling yet"));
    };
    if frame_rate.is_none() {
        b.warnings.push("No composition timecode: start=0, frame rate defaults to 30".into());
    }
    Ok(Plan {
        schema_version: 1,
        name: comp.string(PID_MOB_NAME).unwrap_or_else(|| options.default_name.to_string()),
        sample_rate: *sample_rate,
        timecode_start,
        frame_rate,
        sources: b.sources,
        tracks: plan_tracks,
        warnings: b.warnings,
    })
}

/// PCM WAV header properties. Reads chunk headers only; sample data is not loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavInfo {
    pub channels: u16,
    pub sample_rate: u32,
    pub frames: u64,
}

const KSDATAFORMAT_SUBTYPE_PCM: [u8; 16] = [1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71];

/// Integer PCM RIFF/WAVE only (format 1, or WAVE_FORMAT_EXTENSIBLE with the PCM subtype).
pub fn wav_info<R: Read + Seek>(mut r: R) -> std::result::Result<WavInfo, String> {
    let mut head = [0u8; 12];
    r.read_exact(&mut head).map_err(|_| "not a RIFF/WAVE file".to_string())?;
    if &head[..4] != b"RIFF" || &head[8..] != b"WAVE" {
        return Err("Only mono/stereo PCM WAV supported (not RIFF/WAVE)".into());
    }
    let mut fmt: Option<(u16, u32, u16, u16)> = None;
    for _ in 0..4096 {
        let mut ch = [0u8; 8];
        if r.read_exact(&mut ch).is_err() {
            break;
        }
        let size = u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]);
        let padded = u64::from(size) + u64::from(size & 1);
        match &ch[..4] {
            b"fmt " => {
                let mut f = vec![0u8; usize::try_from(size.min(64)).unwrap_or(64)];
                r.read_exact(&mut f).map_err(|_| "truncated WAV fmt chunk".to_string())?;
                let u16_at = |i: usize| f.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
                let tag = u16_at(0).ok_or("truncated WAV fmt chunk")?;
                let channels = u16_at(2).ok_or("truncated WAV fmt chunk")?;
                let rate = f.get(4..8).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or("truncated WAV fmt chunk")?;
                let bits = u16_at(14).ok_or("truncated WAV fmt chunk")?;
                let pcm = tag == 1 || (tag == 0xfffe && f.get(24..40) == Some(&KSDATAFORMAT_SUBTYPE_PCM[..]));
                if !pcm {
                    return Err("Only mono/stereo PCM WAV supported".into());
                }
                fmt = Some((tag, rate, channels, bits));
                r.seek(SeekFrom::Current(i64::try_from(padded).unwrap_or(i64::MAX) - f.len() as i64)).map_err(|e| e.to_string())?;
            }
            b"data" => {
                let (_, sample_rate, channels, bits) = fmt.ok_or("WAV data before fmt chunk")?;
                let frame = u64::from(channels) * u64::from(bits.div_ceil(8));
                if frame == 0 || sample_rate == 0 {
                    return Err("invalid WAV format".into());
                }
                return Ok(WavInfo { channels, sample_rate, frames: u64::from(size) / frame });
            }
            _ => {
                r.seek(SeekFrom::Current(i64::try_from(padded).map_err(|e| e.to_string())?)).map_err(|e| e.to_string())?;
            }
        }
    }
    Err("WAV data chunk not found".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rationals_reduce_and_round_like_python() {
        let r = Q::new(24000, 1001).unwrap();
        assert_eq!(r.round_half_even(), 24);
        assert_eq!(Q::new(5, 2).unwrap().round_half_even(), 2);
        assert_eq!(Q::new(7, 2).unwrap().round_half_even(), 4);
        assert_eq!(Q::int(48).div(Q::int(96)).unwrap(), Q { n: 1, d: 2 });
        assert!(Q::new(1, 0).is_err());
    }

    #[test]
    fn locators_accept_only_local_files() {
        let base = Path::new("/base");
        assert_eq!(locator_path("file:///Macintosh%20HD/a%23b.wav", base).unwrap(), PathBuf::from("/Macintosh HD/a#b.wav"));
        assert_eq!(locator_path("file://localhost/x.wav", base).unwrap(), PathBuf::from("/x.wav"));
        assert_eq!(locator_path("rel%C3%A1.wav", base).unwrap(), PathBuf::from("/base/relá.wav"));
        assert!(locator_path("http://host/x.wav", base).is_err());
        assert!(locator_path("file://server/x.wav", base).is_err());
        assert!(locator_path("file:///x.wav?y", base).is_err());
        assert!(locator_path("file:///x.wav#y", base).is_err());
    }

    #[test]
    fn mob_ids_print_as_smpte_urns() {
        let mut b = [0u8; 32];
        b[..12].copy_from_slice(&[0x06, 0x0a, 0x2b, 0x34, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x0f, 0x00]);
        b[12] = 0x13;
        b[16..20].copy_from_slice(&0x4fa5c20fu32.to_le_bytes());
        b[20..22].copy_from_slice(&0x4e30u16.to_le_bytes());
        b[22..24].copy_from_slice(&0x1e50u16.to_le_bytes());
        b[24..32].copy_from_slice(&[0x06, 0x0e, 0x2b, 0x34, 0x7f, 0x7f, 0x2a, 0x80]);
        assert_eq!(mob_urn(&b), "urn:smpte:umid:060a2b34.01010101.01010f00.13000000.060e2b34.7f7f2a80.4fa5c20f.4e301e50");
    }

    fn wav(tag: u16, channels: u16, bits: u16, data: u32, junk: u32) -> Vec<u8> {
        let mut v = b"RIFF\0\0\0\0WAVE".to_vec();
        v.extend(b"JUNK");
        v.extend(junk.to_le_bytes());
        v.extend(vec![0; (junk + (junk & 1)) as usize]);
        v.extend(b"fmt ");
        v.extend(16u32.to_le_bytes());
        v.extend(tag.to_le_bytes());
        v.extend(channels.to_le_bytes());
        v.extend(48000u32.to_le_bytes());
        v.extend((48000 * u32::from(channels) * u32::from(bits / 8)).to_le_bytes());
        v.extend((channels * bits / 8).to_le_bytes());
        v.extend(bits.to_le_bytes());
        v.extend(b"data");
        v.extend(data.to_le_bytes());
        v
    }

    #[test]
    fn wav_headers_are_read_without_sample_data() {
        let info = wav_info(std::io::Cursor::new(wav(1, 2, 24, 6 * 1000, 3))).unwrap();
        assert_eq!(info, WavInfo { channels: 2, sample_rate: 48000, frames: 1000 });
        assert!(wav_info(std::io::Cursor::new(wav(3, 1, 32, 400, 0))).is_err());
        assert!(wav_info(std::io::Cursor::new(b"RIFX\0\0\0\0WAVE".to_vec())).is_err());
    }
}
