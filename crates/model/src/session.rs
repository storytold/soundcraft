//! The session: the whole document.

use crate::clip::{bound, bound_sequence};
use crate::{
    Bus, BusId, ChannelFormat, Clip, ClipId, Group, GroupId, MarkerId, MarkerKind, MemoryLocation, OutputPath, Source, SourceId, SourcePool,
    TRACK_COLORS, Track, TrackId, TrackKind, VideoSource,
};
use soundcraft_time::{FrameRate, GridValue, Range, SampleRate, Samples, TempoMap, TimeFormat};

/// Native session file extension.
pub const SESSION_EXTENSION: &str = "scraft";

/// `EditState::values` key holding the feet+frames reading of sample 0, in frames.
pub const FEET_FRAMES_START: &str = "feet_frames.start";

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SessionError {
    #[error("not a SoundCraft session: {0}")]
    Format(String),
    #[error("session file version {0} is newer than this SoundCraft supports")]
    Version(u32),
    #[error("{0}")]
    Invalid(String),
}

/// Session bit depth for recording/rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum BitDepthSetting {
    Int16,
    #[default]
    Int24,
    Float32,
}

/// Edit modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum EditMode {
    Shuffle,
    #[default]
    Slip,
    Spot,
    Grid,
    /// Relative grid: moves keep their offset from the grid.
    GridRelative,
}

impl EditMode {
    pub fn label(self) -> &'static str {
        match self {
            EditMode::Shuffle => "SHUFFLE",
            EditMode::Slip => "SLIP",
            EditMode::Spot => "SPOT",
            EditMode::Grid => "GRID",
            EditMode::GridRelative => "REL GRID",
        }
    }
    pub fn from_id(s: &str) -> Option<EditMode> {
        match s.to_ascii_lowercase().as_str() {
            "shuffle" => Some(EditMode::Shuffle),
            "slip" => Some(EditMode::Slip),
            "spot" => Some(EditMode::Spot),
            "grid" | "absolute_grid" => Some(EditMode::Grid),
            "relative_grid" | "rel_grid" | "grid_relative" => Some(EditMode::GridRelative),
            _ => None,
        }
    }
    pub fn is_grid(self) -> bool {
        matches!(self, EditMode::Grid | EditMode::GridRelative)
    }
}

/// Edit tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum Tool {
    Zoom,
    Trim,
    #[default]
    Selector,
    Grabber,
    Scrubber,
    Pencil,
    Smart,
}

impl Tool {
    pub const ALL: [Tool; 7] = [Tool::Zoom, Tool::Trim, Tool::Selector, Tool::Grabber, Tool::Scrubber, Tool::Pencil, Tool::Smart];
    pub fn id(self) -> &'static str {
        match self {
            Tool::Zoom => "zoom",
            Tool::Trim => "trim",
            Tool::Selector => "selector",
            Tool::Grabber => "grabber",
            Tool::Scrubber => "scrubber",
            Tool::Pencil => "pencil",
            Tool::Smart => "smart",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Tool::Zoom => "Zoomer",
            Tool::Trim => "Trim",
            Tool::Selector => "Selector",
            Tool::Grabber => "Grabber",
            Tool::Scrubber => "Scrubber",
            Tool::Pencil => "Pencil",
            Tool::Smart => "Smart Tool",
        }
    }
    pub fn from_id(s: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.id() == s.to_ascii_lowercase())
    }
    /// F-key shortcut (F5..F10) like the incumbent.
    pub fn fkey(self) -> Option<&'static str> {
        match self {
            Tool::Zoom => Some("F5"),
            Tool::Trim => Some("F6"),
            Tool::Selector => Some("F7"),
            Tool::Grabber => Some("F8"),
            Tool::Scrubber => Some("F9"),
            Tool::Pencil => Some("F10"),
            Tool::Smart => None,
        }
    }
}

/// Track view lanes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TrackView {
    Waveform,
    Blocks,
    Volume,
    Pan,
    Mute,
}

/// Horizontal zoom + scroll.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ZoomState {
    /// Samples per pixel (point).
    pub samples_per_px: f64,
    /// Left edge of the visible timeline.
    pub scroll: Samples,
    /// Waveform vertical zoom factor.
    pub waveform_zoom: f32,
    /// Zoom preset memories 1..5.
    pub presets: [f64; 5],
}

impl Default for ZoomState {
    fn default() -> Self {
        ZoomState { samples_per_px: 1024.0, scroll: 0, waveform_zoom: 1.0, presets: [64.0, 256.0, 1024.0, 4096.0, 16384.0] }
    }
}

/// The view Zoom Toggle goes back to.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ZoomToggle {
    pub samples_per_px: f64,
    pub scroll: Samples,
    pub heights: Vec<(TrackId, crate::TrackHeight)>,
}

/// Edit-window state saved with the session (selection, tools, modes, view settings).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EditState {
    pub tool: Tool,
    pub edit_mode: EditMode,
    /// The edit selection (a point = insertion position).
    pub selection: Range,
    /// Tracks in the edit selection.
    pub selected_tracks: Vec<TrackId>,
    /// Clips explicitly selected (Grabber selection).
    #[serde(default)]
    pub selected_clips: Vec<ClipId>,
    /// Audio files selected in the Clip List.
    #[serde(default)]
    pub selected_sources: Vec<SourceId>,
    /// Playhead position when stopped.
    pub playhead: Samples,
    pub grid: GridValue,
    pub nudge: GridValue,
    #[serde(default = "yes")]
    pub grid_lines: bool,
    pub main_counter: TimeFormat,
    pub sub_counter: Option<TimeFormat>,
    pub zoom: ZoomState,
    /// Set while Zoom Toggle is on.
    pub zoom_toggle: Option<ZoomToggle>,
    /// Visible rulers.
    pub rulers: Vec<String>,
    pub link_timeline_edit: bool,
    pub link_track_edit: bool,
    pub insertion_follows_playback: bool,
    pub loop_playback: bool,
    pub pre_post_roll: bool,
    pub pre_roll: Samples,
    pub post_roll: Samples,
    pub tab_to_transient: bool,
    pub automation_follows_edit: bool,
    pub markers_follow_edit: bool,
    pub layered_editing: bool,
    pub mirrored_midi: bool,
    pub click: bool,
    pub countoff: bool,
    pub countoff_bars: u32,
    pub midi_merge: bool,
    pub record_mode: String,
    pub loop_record: bool,
    pub quickpunch: bool,
    pub scrolling: String,
    pub solo_mode: String,
    pub pre_fader_metering: bool,
    pub delay_compensation: bool,
    pub keyboard_focus: String,
    /// Timeline selection when it is unlinked from the edit selection.
    pub timeline_selection: Range,
    /// Generic on/off view and option flags keyed by id (e.g. `view.clip.name`, `waveform.rectified`,
    /// `options.midi_thru`). Saved with the session so every view setting persists.
    #[serde(default)]
    pub flags: std::collections::BTreeSet<String>,
    /// Numeric settings keyed by id (e.g. `click.volume_db`, `engine.buffer_size`).
    #[serde(default)]
    pub values: std::collections::BTreeMap<String, f64>,
}

impl EditState {
    pub fn flag(&self, id: &str) -> bool {
        self.flags.contains(id)
    }
    pub fn set_flag(&mut self, id: &str, on: bool) {
        if on {
            self.flags.insert(id.to_string());
        } else {
            self.flags.remove(id);
        }
    }
    pub fn value(&self, id: &str, default: f64) -> f64 {
        self.values.get(id).copied().filter(|v| v.is_finite()).unwrap_or(default)
    }
}

fn yes() -> bool {
    true
}

impl Default for EditState {
    fn default() -> Self {
        EditState {
            tool: Tool::Smart,
            edit_mode: EditMode::Slip,
            selection: Range::default(),
            selected_tracks: Vec::new(),
            selected_clips: Vec::new(),
            selected_sources: Vec::new(),
            playhead: 0,
            grid: GridValue::default(),
            nudge: GridValue::Seconds(1.0),
            grid_lines: true,
            main_counter: TimeFormat::MinSecs,
            sub_counter: Some(TimeFormat::Samples),
            zoom: ZoomState::default(),
            zoom_toggle: None,
            rulers: vec!["bars_beats".into(), "min_secs".into(), "tempo".into(), "meter".into(), "markers".into()],
            link_timeline_edit: true,
            link_track_edit: false,
            insertion_follows_playback: false,
            loop_playback: false,
            pre_post_roll: false,
            pre_roll: 96_000,
            post_roll: 96_000,
            tab_to_transient: false,
            automation_follows_edit: true,
            markers_follow_edit: false,
            layered_editing: false,
            mirrored_midi: false,
            click: false,
            countoff: false,
            countoff_bars: 2,
            midi_merge: false,
            record_mode: "normal".into(),
            loop_record: false,
            quickpunch: false,
            scrolling: "page".into(),
            solo_mode: "sip".into(),
            pre_fader_metering: false,
            delay_compensation: true,
            keyboard_focus: "commands".into(),
            timeline_selection: Range::default(),
            flags: ["view.clip.name", "view.clip.gain_line", "waveform.peak", "view.clip.overlap_shadows", "view.marker.ruler_lines"]
                .iter()
                .map(ToString::to_string)
                .collect(),
            values: std::collections::BTreeMap::new(),
        }
    }
}

/// Clip List › Auto Rename: the names that editing gives the clips it separates from a clip.
/// Each new clip is named `name`, then `zeros` zeros, then `number`, then `suffix`, and `number`
/// counts up by one for the next clip.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct AutoName {
    pub name: String,
    /// The number the next new clip gets.
    pub number: u64,
    pub zeros: usize,
    pub suffix: String,
    /// Tells this scheme from another one under the same key after an undo or in another session,
    /// for pastes from the clipboard. 0 matches no scheme.
    #[serde(default)]
    pub id: u64,
}

impl AutoName {
    /// The largest starting number.
    pub const MAX_NUMBER: u64 = 999_999_999;
    /// The most zeros before the number.
    pub const MAX_ZEROS: usize = 9;

    /// Room for `name` and `suffix` together: 29 characters stay free for 9 zeros and the 20
    /// digits of a u64, so a clip name never loses its number to [`MAX_NAME_CHARS`].
    pub const MAX_TEXT_CHARS: usize = MAX_NAME_CHARS - 29;

    /// Repair a scheme read from a file: names that can be drawn and at most [`Self::MAX_ZEROS`] zeros.
    pub fn sanitize(&mut self) {
        truncate_chars(&mut self.name, Self::MAX_TEXT_CHARS);
        truncate_chars(&mut self.suffix, Self::MAX_TEXT_CHARS - self.name.chars().count());
        self.zeros = self.zeros.min(Self::MAX_ZEROS);
    }
}

/// The document.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Session {
    pub format_version: u32,
    pub name: String,
    pub sample_rate: SampleRate,
    pub bit_depth: BitDepthSetting,
    pub frame_rate: FrameRate,
    /// Timecode of sample 0 (session start), as a sample count.
    pub timecode_start: Samples,
    pub tempo: TempoMap,
    #[serde(default)]
    pub key_signatures: Vec<(Samples, String)>,
    pub tracks: Vec<Track>,
    pub sources: Vec<Source>,
    /// Movies referenced by Video track clips.
    #[serde(default)]
    pub videos: Vec<VideoSource>,
    pub busses: Vec<Bus>,
    pub outputs: Vec<OutputPath>,
    pub markers: Vec<MemoryLocation>,
    pub groups: Vec<Group>,
    /// Clip List › Auto Rename schemes, keyed by the ids in [`Clip::auto_name`]. The clips separated
    /// from one clip share its scheme, so they count up one number sequence.
    #[serde(default)]
    pub auto_names: std::collections::BTreeMap<u64, crate::AutoName>,
    pub edit: EditState,
    pub next_id: u64,
    #[serde(default)]
    pub comments: String,
    /// Original MIDI performances keyed by clip id (Event › MIDI Operations › Restore/Flatten
    /// Performance). Independent of undo so a restore survives later edits.
    #[serde(default)]
    pub midi_originals: std::collections::BTreeMap<u64, soundcraft_midi::Sequence>,
    #[serde(skip)]
    pub pool: SourcePool,
    /// The folder relative source paths point into: where the session was opened or last saved.
    #[serde(skip)]
    pub folder: Option<std::path::PathBuf>,
}

impl Default for Session {
    fn default() -> Self {
        Session::new("Untitled", SampleRate::HZ_48000)
    }
}

impl Session {
    pub const FORMAT_VERSION: u32 = 1;

    pub fn new(name: impl Into<String>, sample_rate: SampleRate) -> Self {
        let mut s = Session {
            format_version: Self::FORMAT_VERSION,
            name: name.into(),
            sample_rate,
            bit_depth: BitDepthSetting::Int24,
            frame_rate: FrameRate::Fps30,
            timecode_start: 0,
            tempo: TempoMap::default(),
            key_signatures: Vec::new(),
            tracks: Vec::new(),
            sources: Vec::new(),
            videos: Vec::new(),
            busses: Vec::new(),
            outputs: vec![OutputPath { name: "Out 1-2".into(), first_channel: 0, format: ChannelFormat::Stereo }],
            markers: Vec::new(),
            groups: Vec::new(),
            auto_names: std::collections::BTreeMap::new(),
            edit: EditState::default(),
            next_id: 1,
            comments: String::new(),
            midi_originals: std::collections::BTreeMap::new(),
            pool: SourcePool::default(),
            folder: None,
        };
        s.edit.pre_roll = sample_rate.samples(2.0);
        s.edit.post_roll = sample_rate.samples(2.0);
        s
    }

    /// Allocate a fresh id.
    pub fn alloc(&mut self) -> u64 {
        let id = self.next_id.max(1);
        self.next_id = id.saturating_add(1);
        id
    }

    // ---- lookups -------------------------------------------------------------------------

    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }
    pub fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }
    pub fn track_index(&self, id: TrackId) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }
    pub fn track_by_name(&self, name: &str) -> Option<&Track> {
        self.tracks.iter().find(|t| t.name == name).or_else(|| self.tracks.iter().find(|t| t.name.eq_ignore_ascii_case(name)))
    }
    pub fn master(&self) -> Option<&Track> {
        self.tracks.iter().find(|t| t.kind == TrackKind::Master)
    }
    pub fn source(&self, id: SourceId) -> Option<&Source> {
        self.sources.iter().find(|s| s.id == id)
    }
    pub fn video(&self, id: SourceId) -> Option<&VideoSource> {
        self.videos.iter().find(|v| v.id == id)
    }
    pub fn bus(&self, id: BusId) -> Option<&Bus> {
        self.busses.iter().find(|b| b.id == id)
    }
    pub fn bus_by_name(&self, name: &str) -> Option<&Bus> {
        self.busses.iter().find(|b| b.name.eq_ignore_ascii_case(name))
    }
    pub fn group(&self, id: GroupId) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }
    pub fn marker(&self, id: MarkerId) -> Option<&MemoryLocation> {
        self.markers.iter().find(|m| m.id == id)
    }

    /// Find a clip anywhere (active playlists).
    pub fn find_clip(&self, id: ClipId) -> Option<(TrackId, &Clip)> {
        self.tracks.iter().find_map(|t| t.clips().iter().find(|c| c.id == id).map(|c| (t.id, c)))
    }

    pub fn find_clip_mut(&mut self, id: ClipId) -> Option<&mut Clip> {
        self.tracks.iter_mut().find_map(|t| t.playlist_mut().and_then(|p| p.clip_mut(id)))
    }

    /// Clips on every playlist, alternates included.
    pub fn all_clips(&self) -> impl Iterator<Item = &Clip> {
        self.tracks.iter().flat_map(|t| t.playlists.iter().flat_map(|p| &p.clips))
    }

    pub fn clips_using(&self, src: SourceId) -> impl Iterator<Item = &Clip> {
        self.all_clips().filter(move |c| c.source() == Some(src))
    }

    /// End of the last clip on any track (session length).
    pub fn content_end(&self) -> Samples {
        self.tracks.iter().filter_map(|t| t.playlist().map(|p| p.end())).max().unwrap_or(0)
    }

    /// Feet+frames reading of sample 0, in frames (Setup › Current Feet+Frames Position).
    pub fn feet_frames_start(&self) -> i64 {
        self.edit.value(FEET_FRAMES_START, 0.0) as i64
    }

    /// Format a position against this session's timecode and feet+frames references.
    pub fn format_position(&self, at: Samples, fmt: TimeFormat) -> String {
        soundcraft_time::format_position(at, fmt, self.sample_rate, &self.tempo, self.frame_rate, self.timecode_start, self.feet_frames_start())
    }

    /// Parse a position typed in `fmt` against this session's references.
    pub fn parse_position(&self, text: &str, fmt: TimeFormat) -> Result<Samples, soundcraft_time::TimeError> {
        soundcraft_time::parse_position(text, fmt, self.sample_rate, &self.tempo, self.frame_rate, self.timecode_start, self.feet_frames_start())
    }

    pub fn next_track_color(&self) -> [u8; 3] {
        let n = self.tracks.iter().filter(|t| t.kind != TrackKind::Master).count();
        TRACK_COLORS.get(n % TRACK_COLORS.len()).copied().unwrap_or([128, 128, 128])
    }

    /// A unique track name: `base`, or `base 1`, `base 2` … like the incumbent ("Audio 1").
    pub fn unique_track_name(&self, base: &str) -> String {
        let taken = |n: &str| self.tracks.iter().any(|t| t.name == n);
        if !base.is_empty() && !taken(base) && !matches!(base, "Audio" | "Aux" | "MIDI" | "Inst" | "VCA" | "Folder" | "Master") {
            return base.to_string();
        }
        let prefix = format!("{base} ");
        let used: std::collections::HashSet<u64> = self
            .tracks
            .iter()
            .filter_map(|t| t.name.strip_prefix(&prefix))
            .filter_map(|n| n.parse().ok().filter(|i: &u64| i.to_string() == n))
            .collect();
        let i = (1..).find(|i| !used.contains(i)).unwrap_or(1);
        format!("{base} {i}")
    }

    /// Add a track with defaults; returns its id.
    pub fn add_track(&mut self, kind: TrackKind, format: ChannelFormat, name: Option<&str>) -> TrackId {
        let id = TrackId(self.alloc());
        let name = self.unique_track_name(name.unwrap_or(kind.default_name()));
        let color = if kind == TrackKind::Master { [140, 140, 140] } else { self.next_track_color() };
        let mut t = Track::new(id, name, kind, format, color);
        if kind == TrackKind::Master {
            t.mixer.output = crate::Route::Main;
            t.mixer.input = crate::Route::Main;
        }
        // Master faders sit at the bottom like the incumbent; others go before any master.
        let pos = if kind == TrackKind::Master {
            self.tracks.len()
        } else {
            self.tracks.iter().position(|t| t.kind == TrackKind::Master).unwrap_or(self.tracks.len())
        };
        self.tracks.insert(pos, t);
        id
    }

    pub fn add_bus(&mut self, name: &str, format: ChannelFormat) -> BusId {
        if let Some(b) = self.bus_by_name(name) {
            return b.id;
        }
        let id = BusId(self.alloc());
        self.busses.push(Bus { id, name: name.to_string(), format });
        id
    }

    /// Add a memory location; returns its id.
    pub fn add_marker(&mut self, name: &str, kind: MarkerKind, start: Samples, end: Samples) -> MarkerId {
        let id = MarkerId(self.alloc());
        let number = self.markers.iter().map(|m| m.number).max().unwrap_or(0).saturating_add(1);
        self.markers.push(MemoryLocation {
            id,
            number,
            name: name.to_string(),
            kind,
            start,
            end: end.max(start),
            comments: String::new(),
            color: None,
            ruler: 1,
            track: None,
        });
        self.markers.sort_by_key(|m| m.start);
        id
    }

    pub fn new_clip_id(&mut self) -> ClipId {
        ClipId(self.alloc())
    }

    /// Drop the Auto Rename schemes that no clip on any playlist uses.
    pub fn prune_auto_names(&mut self) {
        let used: std::collections::BTreeSet<u64> = self.all_clips().filter_map(|c| c.auto_name).collect();
        self.auto_names.retain(|k, _| used.contains(k));
    }

    // ---- persistence ---------------------------------------------------------------------

    pub fn to_json(&self) -> Result<String, SessionError> {
        serde_json::to_string_pretty(self).map_err(|e| SessionError::Format(e.to_string()))
    }

    /// Parse a session; decoded audio must be attached separately (`pool`).
    pub fn from_json(text: &str) -> Result<Session, SessionError> {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| SessionError::Format(e.to_string()))?;
        let ver = v.get("format_version").and_then(serde_json::Value::as_u64).ok_or_else(|| SessionError::Format("missing format_version".into()))?;
        if ver > u64::from(Self::FORMAT_VERSION) {
            return Err(SessionError::Version(u32::try_from(ver).unwrap_or(u32::MAX)));
        }
        let mut s: Session = serde_json::from_value(v).map_err(|e| SessionError::Format(e.to_string()))?;
        s.sanitize();
        s.validate()?;
        Ok(s)
    }

    /// Repair invariants after loading untrusted data.
    pub fn sanitize(&mut self) {
        let max_id = self
            .tracks
            .iter()
            .map(|t| t.id.0)
            .chain(self.tracks.iter().flat_map(|t| t.playlists.iter().flat_map(|p| p.clips.iter().map(|c| c.id.0))))
            .chain(self.sources.iter().map(|s| s.id.0))
            .chain(self.auto_names.keys().copied())
            .chain(self.videos.iter().map(|v| v.id.0))
            .chain(self.busses.iter().map(|b| b.id.0))
            .chain(self.markers.iter().map(|m| m.id.0))
            .chain(self.groups.iter().map(|g| g.id.0))
            .max()
            .unwrap_or(0);
        self.next_id = self.next_id.max(max_id.saturating_add(1));
        truncate_chars(&mut self.name, MAX_NAME_CHARS);
        truncate_chars(&mut self.comments, MAX_COMMENT_CHARS);
        for t in &mut self.tracks {
            t.mixer.sanitize();
            truncate_chars(&mut t.name, MAX_NAME_CHARS);
            truncate_chars(&mut t.comments, MAX_COMMENT_CHARS);
            if t.playlists.is_empty() {
                t.playlists.push(crate::Playlist::new(t.name.clone()));
            }
            if t.active_playlist >= t.playlists.len() {
                t.active_playlist = 0;
            }
            for p in &mut t.playlists {
                truncate_chars(&mut p.name, MAX_NAME_CHARS);
                p.clips.retain(|c| c.length > 0);
                p.clips.iter_mut().for_each(Clip::sanitize);
                p.sort();
            }
            for l in &mut t.automation {
                l.points.retain(|p| p.value.is_finite());
                let (lo, hi, _) = l.param.range();
                l.points.iter_mut().for_each(|p| p.value = p.value.clamp(lo, hi));
                l.points.iter_mut().for_each(|p| bound(&mut p.at));
                l.points.sort_by_key(|p| p.at);
            }
        }
        for m in &mut self.markers {
            bound(&mut m.start);
            bound(&mut m.end);
            truncate_chars(&mut m.name, MAX_NAME_CHARS);
            truncate_chars(&mut m.comments, MAX_COMMENT_CHARS);
        }
        for (at, key) in &mut self.key_signatures {
            bound(at);
            truncate_chars(key, MAX_NAME_CHARS);
        }
        self.midi_originals.values_mut().for_each(bound_sequence);
        let e = &mut self.edit;
        for at in [
            &mut e.selection.start,
            &mut e.selection.end,
            &mut e.timeline_selection.start,
            &mut e.timeline_selection.end,
            &mut e.playhead,
            &mut e.pre_roll,
            &mut e.post_roll,
            &mut e.zoom.scroll,
            &mut self.timecode_start,
        ] {
            bound(at);
        }
        for v in &mut self.videos {
            if !v.frame_rate.is_finite() || v.frame_rate < 0.0 {
                v.frame_rate = 0.0;
            }
            if !v.duration.is_finite() || v.duration < 0.0 {
                v.duration = 0.0;
            }
            truncate_chars(&mut v.name, MAX_NAME_CHARS);
        }
        self.busses.iter_mut().for_each(|b| truncate_chars(&mut b.name, MAX_NAME_CHARS));
        self.outputs.iter_mut().for_each(|o| truncate_chars(&mut o.name, MAX_NAME_CHARS));
        self.groups.iter_mut().for_each(|g| truncate_chars(&mut g.name, MAX_NAME_CHARS));
        self.sources.iter_mut().for_each(|s| truncate_chars(&mut s.name, MAX_NAME_CHARS));
        // Drop performances whose clip no longer exists anywhere (any playlist).
        let clip_ids: std::collections::BTreeSet<u64> =
            self.tracks.iter().flat_map(|t| t.playlists.iter().flat_map(|p| p.clips.iter().map(|c| c.id.0))).collect();
        self.midi_originals.retain(|id, _| clip_ids.contains(id));
        self.prune_auto_names();
        self.auto_names.values_mut().for_each(crate::AutoName::sanitize);
        if !self.edit.zoom.samples_per_px.is_finite() || self.edit.zoom.samples_per_px <= 0.0 {
            self.edit.zoom.samples_per_px = 1024.0;
        }
        if SampleRate::new(self.sample_rate.hz()).is_err() {
            self.sample_rate = SampleRate::default();
        }
        self.tempo.sanitize();
    }

    /// Reject a document whose ids are ambiguous or exhausted, run after [`Session::sanitize`] on
    /// untrusted data. Duplicate ids in a namespace make commands, selections and source
    /// associations act on an arbitrary one of the objects; a maximal `next_id` -- or a maximal
    /// existing id, which `sanitize` folds into `next_id` -- makes every later allocation return
    /// `u64::MAX` and repeat itself.
    fn validate(&self) -> Result<(), SessionError> {
        if self.next_id == u64::MAX {
            return Err(SessionError::Invalid("the session id space is exhausted".into()));
        }
        distinct("track", self.tracks.iter().map(|t| t.id.0))?;
        distinct("clip", self.tracks.iter().flat_map(|t| t.playlists.iter().flat_map(|p| p.clips.iter().map(|c| c.id.0))))?;
        distinct("source", self.sources.iter().map(|s| s.id.0))?;
        distinct("video", self.videos.iter().map(|v| v.id.0))?;
        distinct("bus", self.busses.iter().map(|b| b.id.0))?;
        distinct("marker", self.markers.iter().map(|m| m.id.0))?;
        distinct("group", self.groups.iter().map(|g| g.id.0))?;
        Ok(())
    }
}

/// The most characters kept of a name read from a file: longer ones can't be shown, and drawing
/// a name of a million characters overflows the renderer's buffers.
pub const MAX_NAME_CHARS: usize = 256;
/// The most characters kept of a comment read from a file.
pub const MAX_COMMENT_CHARS: usize = 4096;

/// Cut `s` to at most `max` characters.
pub fn truncate_chars(s: &mut String, max: usize) {
    if let Some((end, _)) = s.char_indices().nth(max) {
        s.truncate(end);
    }
}

/// Every id in `ids` distinct, or an error naming the namespace and the repeated id.
fn distinct(kind: &str, ids: impl Iterator<Item = u64>) -> Result<(), SessionError> {
    let mut seen = std::collections::HashSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(SessionError::Invalid(format!("duplicate {kind} id {id}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_tracks_names_and_order() {
        let mut s = Session::default();
        let m = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
        let a = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let b = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let v = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Vocal"));
        assert_eq!(s.track(a).map(|t| t.name.as_str()), Some("Audio 1"));
        assert_eq!(s.track(b).map(|t| t.name.as_str()), Some("Audio 2"));
        assert_eq!(s.track(v).map(|t| t.name.as_str()), Some("Vocal"));
        assert_eq!(s.tracks.last().map(|t| t.id), Some(m));
        let v2 = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Vocal"));
        assert_eq!(s.track(v2).map(|t| t.name.as_str()), Some("Vocal 1"));
    }

    #[test]
    fn json_round_trip() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let cid = s.new_clip_id();
        if let Some(p) = s.track_mut(t).and_then(Track::playlist_mut) {
            p.clips.push(Clip::audio(cid, "c", SourceId(99), 0, 10, 100));
        }
        s.add_marker("Verse", MarkerKind::Marker, 48_000, 48_000);
        let text = s.to_json().unwrap();
        let back = Session::from_json(&text).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn session_with_legacy_fit_height_loads() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        if let Some(track) = s.track_mut(t) {
            track.height = crate::TrackHeight::Large;
        }
        let text = s.to_json().unwrap().replace("\"height\": \"Large\"", "\"height\": \"Fit\"");
        assert!(text.contains("\"height\": \"Fit\""));
        let back = Session::from_json(&text).unwrap();
        assert_eq!(back.track(t).map(|tr| tr.height), Some(crate::TrackHeight::Medium));
    }

    #[test]
    fn video_clip_round_trips_and_old_sessions_load() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Video, ChannelFormat::Mono, None);
        let vid = SourceId(s.alloc());
        s.videos.push(VideoSource {
            id: vid,
            name: "cut.mov".into(),
            path: "/films/cut.mov".into(),
            width: 1920,
            height: 1080,
            frame_rate: 23.976,
            duration: 12.5,
            codec: "prores".into(),
            offline: None,
        });
        let cid = s.new_clip_id();
        if let Some(p) = s.track_mut(t).and_then(Track::playlist_mut) {
            p.clips.push(Clip::video(cid, "cut", vid, 480, 0, 600_000));
        }
        let back = Session::from_json(&s.to_json().unwrap()).unwrap();
        assert_eq!(back, s);
        let c = &back.track(t).unwrap().clips()[0];
        assert_eq!((c.video_source(), c.source(), c.source_offset(), c.is_audio()), (Some(vid), None, 480, false));
        assert_eq!(back.track(t).unwrap().kind.label(), "Video Track");
        assert!(back.video(vid).is_some());
        // A session written before video support has no `videos` key.
        let mut v = serde_json::to_value(Session::default()).unwrap();
        v.as_object_mut().unwrap().remove("videos");
        assert!(Session::from_json(&v.to_string()).unwrap().videos.is_empty());
        // Trimming the start keeps the picture anchored.
        let mut c = c.clone();
        c.trim_start_to(100);
        assert_eq!(c.source_offset(), 580);
    }

    #[test]
    fn unique_track_names_take_the_first_free_number() {
        let mut s = Session::default();
        for name in ["Piano", "Piano 1", "Piano 3", "Piano 02", "Pianos 2"] {
            s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
        }
        assert_eq!(s.unique_track_name("Piano"), "Piano 2");
        assert_eq!(s.unique_track_name("Organ"), "Organ");
        assert_eq!(s.unique_track_name("Audio"), "Audio 1");
    }

    #[test]
    fn names_from_a_file_are_cut_to_a_length_that_can_be_drawn() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        if let Some(tr) = s.track_mut(t) {
            tr.name = "é".repeat(1_000);
        }
        s.comments = "c".repeat(10_000);
        let back = Session::from_json(&s.to_json().unwrap()).unwrap();
        assert_eq!(back.track(t).map(|tr| tr.name.chars().count()), Some(MAX_NAME_CHARS));
        assert_eq!(back.comments.len(), MAX_COMMENT_CHARS);
    }

    #[test]
    fn a_bad_sample_rate_or_tempo_is_repaired_on_load() {
        let mut v = serde_json::to_value(Session::default()).unwrap();
        v["sample_rate"] = serde_json::json!(0);
        v["tempo"]["tempos"] = serde_json::json!([{"tick": 0, "bpm": 4e9}]);
        let s = Session::from_json(&v.to_string()).unwrap();
        assert_eq!((s.sample_rate, s.tempo), (SampleRate::HZ_48000, TempoMap::default()));
    }

    #[test]
    fn a_meter_change_far_beyond_the_position_range_is_dropped_on_load() {
        let mut v = serde_json::to_value(Session::default()).unwrap();
        v["tempo"]["meters"] = serde_json::json!([
            {"tick": 0, "numerator": 4, "denominator": 4},
            {"tick": i64::MAX, "numerator": 3, "denominator": 4}
        ]);
        let s = Session::from_json(&v.to_string()).unwrap();
        assert_eq!(s.parse_position("2|1|000", TimeFormat::BarsBeats), Ok(96_000));
    }

    #[test]
    fn rejects_garbage_and_future_versions() {
        assert!(Session::from_json("not json").is_err());
        assert!(Session::from_json("{}").is_err());
        let mut v = serde_json::to_value(Session::default()).unwrap();
        v["format_version"] = serde_json::json!(999);
        assert!(matches!(Session::from_json(&v.to_string()), Err(SessionError::Version(999))));
    }

    #[test]
    fn duplicate_ids_are_rejected_on_load() {
        let mut s = Session::default();
        s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let mut v = serde_json::to_value(&s).unwrap();
        v["tracks"][1]["id"] = v["tracks"][0]["id"].clone();
        let err = Session::from_json(&v.to_string()).unwrap_err();
        assert!(matches!(&err, SessionError::Invalid(m) if m.contains("duplicate track id")), "{err}");
    }

    #[test]
    fn an_exhausted_id_space_is_rejected_on_load() {
        let mut s = Session::default();
        s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let mut v = serde_json::to_value(&s).unwrap();
        v["next_id"] = serde_json::json!(u64::MAX);
        let err = Session::from_json(&v.to_string()).unwrap_err();
        assert!(matches!(&err, SessionError::Invalid(m) if m.contains("exhausted")), "{err}");
        // A maximal existing id is exhausted too, however `next_id` was written: every later
        // allocation would return `u64::MAX` again.
        let mut v = serde_json::to_value(&s).unwrap();
        v["tracks"][0]["id"] = serde_json::json!(u64::MAX);
        let err = Session::from_json(&v.to_string()).unwrap_err();
        assert!(matches!(&err, SessionError::Invalid(m) if m.contains("exhausted")), "{err}");
    }

    #[test]
    fn sanitize_repairs() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        if let Some(tr) = s.track_mut(t) {
            tr.active_playlist = 7;
            tr.mixer.volume_db = f32::NAN;
        }
        s.next_id = 0;
        s.sanitize();
        let tr = s.track(t).unwrap();
        assert_eq!(tr.active_playlist, 0);
        assert_eq!(tr.mixer.volume_db, 0.0);
        assert!(s.next_id > t.0);
    }

    #[test]
    fn loaded_automation_is_clamped_to_its_range() {
        // #884: a session file can carry points outside a lane's range; load clamps them.
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let tr = s.track_mut(t).unwrap();
        for p in [crate::AutoParam::Volume, crate::AutoParam::SendLevel(0)] {
            tr.lane_mut(&p).points.push(crate::AutomationPoint { at: 0, value: 500.0 });
        }
        let s = Session::from_json(&serde_json::to_string(&s).unwrap()).unwrap();
        let tr = s.track(t).unwrap();
        for p in [crate::AutoParam::Volume, crate::AutoParam::SendLevel(0)] {
            assert_eq!(tr.lane(&p).unwrap().points[0].value, 12.0, "{p:?}");
        }
    }

    #[test]
    fn a_loaded_auto_name_scheme_leaves_room_for_its_number() {
        let mut scheme = crate::AutoName { name: "n".repeat(300), suffix: "s".repeat(300), ..Default::default() };
        scheme.sanitize();
        assert_eq!(scheme.name.chars().count() + scheme.suffix.chars().count(), crate::AutoName::MAX_TEXT_CHARS);
    }
}
