//! Tracks and playlists.

use crate::{AutoParam, AutomationLane, ChannelFormat, Clip, ClipId, Mixer, TrackId};
use soundcraft_time::{Range, Samples};

/// Track types (Track › New).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum TrackKind {
    #[default]
    Audio,
    Aux,
    Master,
    Midi,
    Instrument,
    Vca,
    Folder,
    /// Picture: clips reference a movie ([`crate::VideoSource`]); no audio path.
    Video,
}

impl TrackKind {
    pub const ALL: [TrackKind; 8] = [
        TrackKind::Audio,
        TrackKind::Aux,
        TrackKind::Master,
        TrackKind::Midi,
        TrackKind::Instrument,
        TrackKind::Vca,
        TrackKind::Folder,
        TrackKind::Video,
    ];
    pub fn label(self) -> &'static str {
        match self {
            TrackKind::Audio => "Audio Track",
            TrackKind::Aux => "Aux Input",
            TrackKind::Master => "Master Fader",
            TrackKind::Midi => "MIDI Track",
            TrackKind::Instrument => "Instrument Track",
            TrackKind::Vca => "VCA Master",
            TrackKind::Folder => "Routing Folder Track",
            TrackKind::Video => "Video Track",
        }
    }
    pub fn default_name(self) -> &'static str {
        match self {
            TrackKind::Audio => "Audio",
            TrackKind::Aux => "Aux",
            TrackKind::Master => "Master",
            TrackKind::Midi => "MIDI",
            TrackKind::Instrument => "Inst",
            TrackKind::Vca => "VCA",
            TrackKind::Folder => "Folder",
            TrackKind::Video => "Video",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            TrackKind::Audio => "audio",
            TrackKind::Aux => "aux",
            TrackKind::Master => "master",
            TrackKind::Midi => "midi",
            TrackKind::Instrument => "instrument",
            TrackKind::Vca => "vca",
            TrackKind::Folder => "folder",
            TrackKind::Video => "video",
        }
    }
    pub fn from_id(s: &str) -> Option<TrackKind> {
        let s = s.to_ascii_lowercase();
        TrackKind::ALL.into_iter().find(|k| k.id() == s || k.label().eq_ignore_ascii_case(&s))
    }
    /// Track kinds that hold clips on a timeline.
    pub fn has_playlist(self) -> bool {
        matches!(self, TrackKind::Audio | TrackKind::Midi | TrackKind::Instrument | TrackKind::Video)
    }
    pub fn is_midi(self) -> bool {
        matches!(self, TrackKind::Midi | TrackKind::Instrument)
    }
    /// Kinds that pass audio through the mixer.
    pub fn is_audio_path(self) -> bool {
        matches!(self, TrackKind::Audio | TrackKind::Aux | TrackKind::Master | TrackKind::Instrument | TrackKind::Folder)
    }
}

/// Track heights (View › Track Height presets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
#[serde(from = "TrackHeightRepr")]
pub enum TrackHeight {
    /// Any height from Micro to Extreme, in points.
    Custom(u16),
    Micro,
    Mini,
    Small,
    #[default]
    Medium,
    Large,
    Jumbo,
    Extreme,
    /// Fit To Window: this many points.
    Fit(u16),
}

/// What a saved track height may look like: the current form, or the bare `"Fit"` string older sessions used.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum TrackHeightRepr {
    Current(TrackHeightCurrent),
    LegacyFit(LegacyFit),
}

#[derive(serde::Deserialize)]
enum LegacyFit {
    Fit,
}

/// Mirror of `TrackHeight` that derives the current serialized form.
#[derive(serde::Deserialize)]
enum TrackHeightCurrent {
    Custom(u16),
    Micro,
    Mini,
    Small,
    Medium,
    Large,
    Jumbo,
    Extreme,
    Fit(u16),
}

impl From<TrackHeightRepr> for TrackHeight {
    fn from(repr: TrackHeightRepr) -> Self {
        match repr {
            TrackHeightRepr::Current(TrackHeightCurrent::Custom(p)) => TrackHeight::Custom(p),
            TrackHeightRepr::Current(TrackHeightCurrent::Micro) => TrackHeight::Micro,
            TrackHeightRepr::Current(TrackHeightCurrent::Mini) => TrackHeight::Mini,
            TrackHeightRepr::Current(TrackHeightCurrent::Small) => TrackHeight::Small,
            TrackHeightRepr::Current(TrackHeightCurrent::Medium) => TrackHeight::Medium,
            TrackHeightRepr::Current(TrackHeightCurrent::Large) => TrackHeight::Large,
            TrackHeightRepr::Current(TrackHeightCurrent::Jumbo) => TrackHeight::Jumbo,
            TrackHeightRepr::Current(TrackHeightCurrent::Extreme) => TrackHeight::Extreme,
            TrackHeightRepr::Current(TrackHeightCurrent::Fit(p)) => TrackHeight::Fit(p),
            // Older sessions stored Fit with no size, drawn 88 points tall: the default Medium height.
            TrackHeightRepr::LegacyFit(LegacyFit::Fit) => TrackHeight::Medium,
        }
    }
}

impl TrackHeight {
    pub const ALL: [TrackHeight; 7] = [
        TrackHeight::Micro,
        TrackHeight::Mini,
        TrackHeight::Small,
        TrackHeight::Medium,
        TrackHeight::Large,
        TrackHeight::Jumbo,
        TrackHeight::Extreme,
    ];
    pub fn points(self) -> f32 {
        match self {
            TrackHeight::Custom(points) => f32::from(points).clamp(TrackHeight::Micro.points(), TrackHeight::Extreme.points()),
            TrackHeight::Micro => 16.0,
            TrackHeight::Mini => 24.0,
            TrackHeight::Small => 46.0,
            TrackHeight::Medium => 88.0,
            TrackHeight::Large => 132.0,
            TrackHeight::Jumbo => 220.0,
            TrackHeight::Extreme => 440.0,
            TrackHeight::Fit(points) => f32::from(points.max(16)),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            TrackHeight::Custom(_) => "custom",
            TrackHeight::Micro => "micro",
            TrackHeight::Mini => "mini",
            TrackHeight::Small => "small",
            TrackHeight::Medium => "medium",
            TrackHeight::Large => "large",
            TrackHeight::Jumbo => "jumbo",
            TrackHeight::Extreme => "extreme",
            TrackHeight::Fit(_) => "fit to window",
        }
    }
    pub fn from_id(s: &str) -> Option<TrackHeight> {
        TrackHeight::ALL.into_iter().find(|h| h.label().eq_ignore_ascii_case(s))
    }
    /// The preset this many points tall, else a custom height clamped to Micro..Extreme.
    pub fn from_points(points: f32) -> TrackHeight {
        let p = points.max(TrackHeight::Micro.points()).min(TrackHeight::Extreme.points()).round() as u16;
        TrackHeight::ALL.into_iter().find(|h| h.points() == f32::from(p)).unwrap_or(TrackHeight::Custom(p))
    }
    /// The next preset taller (or shorter) than this height; Extreme (or Micro) stays.
    pub fn step(self, taller: bool) -> TrackHeight {
        let p = self.points();
        let next =
            if taller { TrackHeight::ALL.into_iter().find(|h| h.points() > p) } else { TrackHeight::ALL.into_iter().rev().find(|h| h.points() < p) };
        next.unwrap_or(self)
    }
}

/// The default track colour palette (original, chosen for SoundCraft).
pub const TRACK_COLORS: [[u8; 3]; 16] = [
    [86, 132, 214],
    [94, 170, 214],
    [80, 190, 168],
    [104, 186, 98],
    [168, 196, 76],
    [222, 196, 72],
    [230, 152, 62],
    [222, 104, 72],
    [214, 82, 110],
    [196, 86, 168],
    [150, 98, 206],
    [110, 104, 214],
    [130, 150, 170],
    [178, 140, 110],
    [120, 170, 140],
    [190, 120, 150],
];

/// A playlist: an alternative take of a track's clips.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Playlist {
    pub name: String,
    /// Clips sorted by start; non-overlapping except at crossfades.
    pub clips: Vec<Clip>,
}

impl Playlist {
    pub fn new(name: impl Into<String>) -> Self {
        Playlist { name: name.into(), clips: Vec::new() }
    }
    pub fn sort(&mut self) {
        self.clips.sort_by_key(|c| (c.start, c.id.0));
    }
    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        self.clips.iter().find(|c| c.id == id)
    }
    pub fn clip_mut(&mut self, id: ClipId) -> Option<&mut Clip> {
        self.clips.iter_mut().find(|c| c.id == id)
    }
    pub fn clips_in(&self, r: Range) -> impl Iterator<Item = &Clip> {
        self.clips.iter().filter(move |c| c.range().overlaps(&r))
    }
    pub fn clip_at(&self, at: Samples) -> Option<&Clip> {
        self.clips.iter().rev().find(|c| c.range().contains(at))
    }
    pub fn end(&self) -> Samples {
        self.clips.iter().map(Clip::end).max().unwrap_or(0)
    }
}

/// A track.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub name: String,
    pub kind: TrackKind,
    pub format: ChannelFormat,
    pub color: [u8; 3],
    pub height: TrackHeight,
    pub playlists: Vec<Playlist>,
    pub active_playlist: usize,
    pub mixer: Mixer,
    pub automation: Vec<AutomationLane>,
    /// Which lane the track shows: `waveform`/`blocks`/`notes`/`regions` or an automation param id.
    pub view: String,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub inactive: bool,
    #[serde(default)]
    pub frozen: bool,
    #[serde(default)]
    pub comments: String,
    /// Folder that contains this track.
    #[serde(default)]
    pub folder: Option<TrackId>,
    #[serde(default = "yes")]
    pub folder_open: bool,
    /// Instrument plugin (instrument tracks) — registry id.
    #[serde(default)]
    pub instrument: Option<crate::Insert>,
    /// Ticks (musical) or samples timebase for clips.
    #[serde(default)]
    pub ticks_timebase: bool,
    /// Elastic Audio algorithm, if enabled.
    #[serde(default)]
    pub elastic: Option<String>,
    /// Pinned to top in the Edit window.
    #[serde(default)]
    pub pinned: bool,
}

fn yes() -> bool {
    true
}

impl Track {
    pub fn new(id: TrackId, name: impl Into<String>, kind: TrackKind, format: ChannelFormat, color: [u8; 3]) -> Self {
        let name = name.into();
        let format = if kind.is_midi() && kind == TrackKind::Midi { ChannelFormat::Mono } else { format };
        let view = if kind.is_midi() {
            "notes"
        } else if kind.has_playlist() {
            "waveform"
        } else {
            "volume"
        };
        Track {
            id,
            playlists: vec![Playlist::new(name.clone())],
            name,
            kind,
            format,
            color,
            height: TrackHeight::Medium,
            active_playlist: 0,
            mixer: Mixer::new(format),
            automation: Vec::new(),
            view: view.into(),
            hidden: false,
            inactive: false,
            frozen: false,
            comments: String::new(),
            folder: None,
            folder_open: true,
            instrument: None,
            ticks_timebase: kind.is_midi(),
            elastic: None,
            pinned: false,
        }
    }

    pub fn channels(&self) -> usize {
        self.format.channels()
    }

    pub fn playlist(&self) -> Option<&Playlist> {
        self.playlists.get(self.active_playlist)
    }

    pub fn playlist_mut(&mut self) -> Option<&mut Playlist> {
        self.playlists.get_mut(self.active_playlist)
    }

    pub fn clips(&self) -> &[Clip] {
        self.playlist().map_or(&[], |p| p.clips.as_slice())
    }

    pub fn lane(&self, p: &AutoParam) -> Option<&AutomationLane> {
        self.automation.iter().find(|l| &l.param == p)
    }

    pub fn lane_mut(&mut self, p: &AutoParam) -> &mut AutomationLane {
        let idx = match self.automation.iter().position(|l| &l.param == p) {
            Some(i) => i,
            None => {
                self.automation.push(AutomationLane::new(p.clone()));
                self.automation.len() - 1
            }
        };
        let len = self.automation.len();
        // `idx` is in range: it came from `position` or is the index of the element just pushed.
        &mut self.automation[idx.min(len - 1)]
    }

    pub fn is_folder(&self) -> bool {
        self.kind == TrackKind::Folder
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_fit_string_loads_as_default_height() {
        assert_eq!(serde_json::from_str::<TrackHeight>("\"Fit\"").unwrap(), TrackHeight::Medium);
        assert_eq!(serde_json::from_str::<TrackHeight>("{\"Fit\":120}").unwrap(), TrackHeight::Fit(120));
        assert_eq!(serde_json::to_string(&TrackHeight::Fit(120)).unwrap(), "{\"Fit\":120}");
        assert_eq!(serde_json::from_str::<TrackHeight>("\"Large\"").unwrap(), TrackHeight::Large);
        assert_eq!(serde_json::from_str::<TrackHeight>("{\"Custom\":50}").unwrap(), TrackHeight::Custom(50));
    }

    #[test]
    fn new_track_defaults() {
        let t = Track::new(TrackId(1), "Gtr", TrackKind::Audio, ChannelFormat::Stereo, TRACK_COLORS[0]);
        assert_eq!(t.channels(), 2);
        assert_eq!(t.view, "waveform");
        assert_eq!(t.playlists.len(), 1);
        let m = Track::new(TrackId(2), "Keys", TrackKind::Midi, ChannelFormat::Stereo, TRACK_COLORS[0]);
        assert_eq!(m.format, ChannelFormat::Mono);
        assert_eq!(m.view, "notes");
    }

    #[test]
    fn lane_mut_creates_once() {
        let mut t = Track::new(TrackId(1), "Gtr", TrackKind::Audio, ChannelFormat::Mono, TRACK_COLORS[0]);
        t.lane_mut(&AutoParam::Volume).set_point(0, -3.0);
        t.lane_mut(&AutoParam::Volume).set_point(10, -6.0);
        assert_eq!(t.automation.len(), 1);
        assert_eq!(t.lane(&AutoParam::Volume).map(|l| l.points.len()), Some(2));
    }

    #[test]
    fn kind_ids() {
        for k in TrackKind::ALL {
            assert_eq!(TrackKind::from_id(k.id()), Some(k));
        }
    }

    #[test]
    fn custom_heights_round_to_presets_and_step_through_them() {
        assert_eq!(TrackHeight::from_points(88.0), TrackHeight::Medium);
        assert_eq!(TrackHeight::from_points(57.4), TrackHeight::Custom(57));
        assert_eq!(TrackHeight::from_points(9999.0), TrackHeight::Extreme);
        assert_eq!(TrackHeight::from_points(f32::NAN), TrackHeight::Micro);
        assert_eq!(TrackHeight::Custom(u16::MAX).points(), 440.0);
        assert_eq!(TrackHeight::Custom(57).step(true), TrackHeight::Medium);
        assert_eq!(TrackHeight::Custom(57).step(false), TrackHeight::Small);
        assert_eq!(TrackHeight::Extreme.step(true), TrackHeight::Extreme);
        assert_eq!(TrackHeight::Micro.step(false), TrackHeight::Micro);
        let json = serde_json::to_string(&TrackHeight::Custom(57)).unwrap();
        assert_eq!(serde_json::from_str::<TrackHeight>(&json).unwrap(), TrackHeight::Custom(57));
        assert_eq!(serde_json::from_str::<TrackHeight>("\"Large\"").unwrap(), TrackHeight::Large);
    }
}
