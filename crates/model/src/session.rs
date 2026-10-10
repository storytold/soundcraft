//! The session: the whole document.

use crate::{
    Bus, BusId, ChannelFormat, Clip, ClipId, Group, GroupId, MarkerId, MarkerKind, MemoryLocation, OutputPath, Source, SourceId, SourcePool,
    TRACK_COLORS, Track, TrackId, TrackKind, VideoSource,
};
use soundcraft_time::{FrameRate, GridValue, Range, SampleRate, Samples, TempoMap, TimeFormat};

/// Native session file extension.
pub const SESSION_EXTENSION: &str = "scraft";

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
                .map(|s| s.to_string())
                .collect(),
            values: std::collections::BTreeMap::new(),
        }
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
            edit: EditState::default(),
            next_id: 1,
            comments: String::new(),
            midi_originals: std::collections::BTreeMap::new(),
            pool: SourcePool::default(),
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
        let mut i = 1;
        loop {
            let cand = format!("{base} {i}");
            if !taken(&cand) {
                return cand;
            }
            i += 1;
        }
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
            .chain(self.videos.iter().map(|v| v.id.0))
            .chain(self.busses.iter().map(|b| b.id.0))
            .chain(self.markers.iter().map(|m| m.id.0))
            .chain(self.groups.iter().map(|g| g.id.0))
            .max()
            .unwrap_or(0);
        self.next_id = self.next_id.max(max_id.saturating_add(1));
        for t in &mut self.tracks {
            t.mixer.sanitize();
            if t.playlists.is_empty() {
                t.playlists.push(crate::Playlist::new(t.name.clone()));
            }
            if t.active_playlist >= t.playlists.len() {
                t.active_playlist = 0;
            }
            for p in &mut t.playlists {
                p.clips.retain(|c| c.length > 0);
                for c in &mut p.clips {
                    c.fade_in.len = c.fade_in.len.clamp(0, c.length);
                    c.fade_out.len = c.fade_out.len.clamp(0, c.length);
                    if !c.gain_db.is_finite() {
                        c.gain_db = 0.0;
                    }
                    if !c.stretch.is_finite() || c.stretch <= 0.0 {
                        c.stretch = 1.0;
                    }
                }
                p.sort();
            }
            for l in &mut t.automation {
                l.points.retain(|p| p.value.is_finite());
                l.points.sort_by_key(|p| p.at);
            }
        }
        for v in &mut self.videos {
            if !v.frame_rate.is_finite() || v.frame_rate < 0.0 {
                v.frame_rate = 0.0;
            }
            if !v.duration.is_finite() || v.duration < 0.0 {
                v.duration = 0.0;
            }
        }
        // Drop performances whose clip no longer exists anywhere (any playlist).
        let clip_ids: std::collections::BTreeSet<u64> =
            self.tracks.iter().flat_map(|t| t.playlists.iter().flat_map(|p| p.clips.iter().map(|c| c.id.0))).collect();
        self.midi_originals.retain(|id, _| clip_ids.contains(id));
        if !self.edit.zoom.samples_per_px.is_finite() || self.edit.zoom.samples_per_px <= 0.0 {
            self.edit.zoom.samples_per_px = 1024.0;
        }
    }
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
    fn rejects_garbage_and_future_versions() {
        assert!(Session::from_json("not json").is_err());
        assert!(Session::from_json("{}").is_err());
        let mut v = serde_json::to_value(Session::default()).unwrap();
        v["format_version"] = serde_json::json!(999);
        assert!(matches!(Session::from_json(&v.to_string()), Err(SessionError::Version(999))));
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
}
