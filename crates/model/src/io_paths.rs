//! Physical input paths (Setup › I/O › Input): named groups of hardware input channels, as Pro
//! Tools' I/O Setup defines them. A track's input route names one (`Route::Hardware(name)`); the
//! path says which interface channels feed it.

use crate::{Bus, BusSubId, BusSubPath, ChannelFormat, MAX_NAME_CHARS, OutputPath, Route, Session, truncate_chars};

/// The highest hardware input channel (0-based) a path may name: more than any interface offers.
pub const MAX_INPUT_CHANNEL: u16 = 1023;
/// Main input paths a session keeps, and sub-paths a main path keeps.
pub const MAX_INPUT_PATHS: usize = 1024;
pub const MAX_SUB_PATHS: usize = 64;

/// A main input path: a name, a channel format, the hardware input channel (0-based) feeding each
/// of its channels (`None` while unassigned) and its sub-paths. A sub-path is a path within its
/// main path: a stereo `In 1-2` has the mono sub-paths `In 1` and `In 2`. An inactive path stays
/// in the session but feeds nothing.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InputPath {
    pub name: String,
    pub format: ChannelFormat,
    pub channels: Vec<Option<u16>>,
    #[serde(default = "yes")]
    pub active: bool,
    #[serde(default)]
    pub sub_paths: Vec<InputPath>,
}

fn yes() -> bool {
    true
}

impl InputPath {
    /// A path with nothing assigned yet.
    pub fn new(name: impl Into<String>, format: ChannelFormat) -> InputPath {
        InputPath { name: name.into(), format, channels: vec![None; format.channels()], active: true, sub_paths: Vec::new() }
    }

    /// A path fed by the hardware channels from `first` on, in order.
    pub fn assigned(name: impl Into<String>, format: ChannelFormat, first: u16) -> InputPath {
        let mut p = InputPath::new(name, format);
        p.assign_from(first);
        p
    }

    /// Feeds the channels from hardware channel `first` on, in order (the I/O Setup grid's click:
    /// "clicking under channel 1 fills both channel 1 and 2, left to 1, right to 2"). Capped at
    /// [`MAX_INPUT_CHANNEL`].
    pub fn assign_from(&mut self, first: u16) {
        for (k, c) in self.channels.iter_mut().enumerate() {
            *c = Some(first.saturating_add(u16::try_from(k).unwrap_or(u16::MAX)).min(MAX_INPUT_CHANNEL));
        }
    }

    /// Changes the format; the assignment is kept as far as it fits.
    pub fn set_format(&mut self, format: ChannelFormat) {
        self.format = format;
        self.channels.resize(format.channels(), None);
    }

    /// Channels in the path.
    pub fn width(&self) -> usize {
        self.channels.len()
    }

    /// Every channel assigned?
    pub fn complete(&self) -> bool {
        self.channels.iter().all(Option::is_some)
    }

    /// The hardware channel a path channel reads, 0-based.
    pub fn hardware(&self, k: usize) -> Option<u16> {
        self.channels.get(k).copied().flatten()
    }

    /// Which path channel hardware channel `hw` feeds, if any.
    pub fn channel_on(&self, hw: u16) -> Option<usize> {
        self.channels.iter().position(|c| *c == Some(hw))
    }

    /// Keeps a path read from a file consistent: as many channels as the format, each in range
    /// and used once, names bounded, sub-paths bounded, one level deep and within the main path.
    pub fn sanitize(&mut self, is_main: bool) {
        truncate_chars(&mut self.name, MAX_NAME_CHARS);
        self.channels.resize(self.format.channels(), None);
        let mut seen = std::collections::BTreeSet::new();
        for c in &mut self.channels {
            if let Some(ch) = *c
                && (ch > MAX_INPUT_CHANNEL || !seen.insert(ch))
            {
                *c = None;
            }
        }
        if !is_main {
            self.sub_paths.clear();
            return;
        }
        self.sub_paths.truncate(MAX_SUB_PATHS);
        let main: Vec<Option<u16>> = self.channels.clone();
        for sp in &mut self.sub_paths {
            sp.sub_paths.clear();
            sp.sanitize(false);
            for c in &mut sp.channels {
                if c.is_some() && !main.contains(c) {
                    *c = None;
                }
            }
        }
    }
}

/// The sub-paths Pro Tools makes for a new main path ("Auto-Create Sub-Paths"): a stereo path
/// gets `.L` and `.R` mono sub-paths; a wider one a stereo `.LR` and a mono sub-path per channel,
/// named by the channel (`.C`, `.LFE`, `.Ls`…). A mono path has none.
pub fn default_sub_paths(main: &InputPath) -> Vec<InputPath> {
    let speakers = main.format.speakers();
    if speakers.len() < 2 || main.format.is_ambisonic() {
        return Vec::new();
    }
    let sub = |name: String, format: ChannelFormat, from: &[usize]| {
        let mut p = InputPath::new(name, format);
        for (c, k) in p.channels.iter_mut().zip(from) {
            *c = main.hardware(*k);
        }
        p
    };
    let mut out = Vec::new();
    if speakers.len() > 2 {
        out.push(sub(format!("{}.LR", main.name), ChannelFormat::Stereo, &[0, 1]));
    }
    for (k, sp) in speakers.iter().enumerate() {
        out.push(sub(format!("{}.{}", main.name, sp.label()), ChannelFormat::Mono, &[k]));
    }
    out
}

impl Session {
    /// The input paths of an interface with `channels` inputs, as Pro Tools' Stereo Mix settings
    /// make them: a stereo main path per pair (`In 1-2`) with a mono sub-path per channel
    /// (`In 1`, `In 2`); an odd last channel is a mono main path.
    pub fn default_input_paths(channels: usize) -> Vec<InputPath> {
        let channels = channels.clamp(1, MAX_INPUT_CHANNEL as usize + 1);
        let mut out = Vec::new();
        let mut c = 0usize;
        while c < channels {
            let first = c as u16;
            if c + 1 < channels {
                let mut main = InputPath::assigned(format!("In {}-{}", c + 1, c + 2), ChannelFormat::Stereo, first);
                main.sub_paths = vec![
                    InputPath::assigned(format!("In {}", c + 1), ChannelFormat::Mono, first),
                    InputPath::assigned(format!("In {}", c + 2), ChannelFormat::Mono, first + 1),
                ];
                out.push(main);
                c += 2;
            } else {
                out.push(InputPath::assigned(format!("In {}", c + 1), ChannelFormat::Mono, first));
                c += 1;
            }
        }
        out
    }

    /// Where the input path named `name` is: the main path's index and, for a sub-path, its index
    /// within it. Names are matched ignoring case.
    pub fn find_input_path(&self, name: &str) -> Option<(usize, Option<usize>)> {
        self.inputs.iter().enumerate().find_map(|(i, m)| {
            if m.name.eq_ignore_ascii_case(name) {
                return Some((i, None));
            }
            m.sub_paths.iter().position(|s| s.name.eq_ignore_ascii_case(name)).map(|j| (i, Some(j)))
        })
    }

    /// The input path named `name`, main or sub.
    pub fn input_path(&self, name: &str) -> Option<&InputPath> {
        let (i, j) = self.find_input_path(name)?;
        let m = self.inputs.get(i)?;
        match j {
            None => Some(m),
            Some(j) => m.sub_paths.get(j),
        }
    }

    /// Every input path in I/O Setup order, each main path followed by its sub-paths, with
    /// whether it (and its main path) is active.
    pub fn input_paths_flat(&self) -> Vec<(&InputPath, bool)> {
        self.inputs.iter().flat_map(|m| std::iter::once((m, m.active)).chain(m.sub_paths.iter().map(move |s| (s, m.active && s.active)))).collect()
    }

    /// The hardware channels an input route reads, one per track channel, `None` where nothing
    /// feeds it: from the named input path when the session has it and it is active (a path
    /// narrower than the track repeats), else, for a name no path has, from the `In N` /
    /// `In N-M` naming the default paths use. A track without a hardware input reads from the
    /// first channels, as it always has.
    pub fn input_route_channels(&self, route: &Route, channels: usize) -> Vec<Option<u16>> {
        let Route::Hardware(name) = route else {
            return (0..channels).map(|k| u16::try_from(k).ok()).collect();
        };
        if let Some((i, j)) = self.find_input_path(name)
            && let Some(m) = self.inputs.get(i)
        {
            let p = match j {
                None => m,
                Some(j) => match m.sub_paths.get(j) {
                    Some(s) => s,
                    None => return vec![None; channels],
                },
            };
            if !m.active || !p.active || p.channels.is_empty() {
                return vec![None; channels];
            }
            return (0..channels).map(|k| p.channels.get(k % p.channels.len()).copied().flatten()).collect();
        }
        let first = name
            .trim()
            .get(3..)
            .filter(|_| name.trim().get(..3).is_some_and(|p| p.eq_ignore_ascii_case("in ")))
            .and_then(|rest| rest.trim_start().split(['-', ' ']).next())
            .and_then(|n| n.parse::<u16>().ok())
            .map_or(0, |n| n.saturating_sub(1));
        (0..channels).map(|k| u16::try_from(k).ok().map(|k| first.saturating_add(k))).collect()
    }
}

impl OutputPath {
    /// A path with nothing assigned yet.
    pub fn new(name: impl Into<String>, format: ChannelFormat) -> OutputPath {
        OutputPath { name: name.into(), first_channel: 0, format, channels: vec![None; format.channels()], active: true }
    }

    /// A path driving the hardware channels from `first` on, in order.
    pub fn assigned(name: impl Into<String>, format: ChannelFormat, first: u16) -> OutputPath {
        let mut p = OutputPath::new(name, format);
        p.assign_from(first);
        p
    }

    /// Drives the channels from hardware channel `first` on, in order (the grid's click), capped
    /// at [`MAX_INPUT_CHANNEL`].
    pub fn assign_from(&mut self, first: u16) {
        for (k, c) in self.channels.iter_mut().enumerate() {
            *c = Some(first.saturating_add(u16::try_from(k).unwrap_or(u16::MAX)).min(MAX_INPUT_CHANNEL));
        }
        self.first_channel = first.min(MAX_INPUT_CHANNEL);
    }

    /// Changes the format; the assignment is kept as far as it fits.
    pub fn set_format(&mut self, format: ChannelFormat) {
        self.format = format;
        self.channels.resize(format.channels(), None);
    }

    pub fn width(&self) -> usize {
        self.channels.len()
    }

    pub fn complete(&self) -> bool {
        self.channels.iter().all(Option::is_some)
    }

    /// The hardware channel a path channel drives, 0-based.
    pub fn hardware(&self, k: usize) -> Option<u16> {
        self.channels.get(k).copied().flatten()
    }

    /// Which path channel drives hardware channel `hw`, if any.
    pub fn channel_on(&self, hw: u16) -> Option<usize> {
        self.channels.iter().position(|c| *c == Some(hw))
    }

    /// Keeps a path read from a file consistent: as many channels as the format (a file from
    /// before the map existed gets them in order from `first_channel`), each in range and used
    /// once; `first_channel` follows the map.
    pub fn sanitize(&mut self) {
        truncate_chars(&mut self.name, MAX_NAME_CHARS);
        if self.channels.is_empty() && self.format.channels() > 0 {
            self.channels = vec![None; self.format.channels()];
            let first = self.first_channel;
            self.assign_from(first);
        }
        self.channels.resize(self.format.channels(), None);
        let mut seen = std::collections::BTreeSet::new();
        for c in &mut self.channels {
            if let Some(ch) = *c
                && (ch > MAX_INPUT_CHANNEL || !seen.insert(ch))
            {
                *c = None;
            }
        }
        self.first_channel = self.channels.iter().flatten().copied().min().unwrap_or(0);
    }
}

impl BusSubPath {
    /// A sub-path of a bus reading nothing yet.
    pub fn new(id: BusSubId, name: impl Into<String>, format: ChannelFormat) -> BusSubPath {
        BusSubPath { id, name: name.into(), format, channels: vec![None; format.channels()] }
    }

    pub fn width(&self) -> usize {
        self.channels.len()
    }

    /// Reads the bus channels from `first` on, in order (the grid's click); channels past the
    /// bus's width stay unassigned.
    pub fn assign_from(&mut self, first: u8, bus_width: usize) {
        for (k, c) in self.channels.iter_mut().enumerate() {
            let ch = usize::from(first).saturating_add(k);
            *c = (ch < bus_width).then(|| u8::try_from(ch).unwrap_or(u8::MAX));
        }
    }

    /// Which sub-path channel reads bus channel `ch`, if any.
    pub fn channel_on(&self, ch: u8) -> Option<usize> {
        self.channels.iter().position(|c| *c == Some(ch))
    }

    /// The bus channels in sub-path order, `None` where unassigned.
    pub fn bus_channels(&self) -> &[Option<u8>] {
        &self.channels
    }

    fn sanitize(&mut self, bus_width: usize) {
        truncate_chars(&mut self.name, MAX_NAME_CHARS);
        self.channels.resize(self.format.channels(), None);
        let mut seen = std::collections::BTreeSet::new();
        for c in &mut self.channels {
            if let Some(ch) = *c
                && (usize::from(ch) >= bus_width || !seen.insert(ch))
            {
                *c = None;
            }
        }
    }
}

impl Bus {
    pub fn width(&self) -> usize {
        self.format.channels()
    }

    /// Keeps a bus read from a file consistent: bounded names and sub-paths, each sub-path within
    /// the bus.
    pub fn sanitize(&mut self) {
        truncate_chars(&mut self.name, MAX_NAME_CHARS);
        if let Some(o) = &mut self.output {
            truncate_chars(o, MAX_NAME_CHARS);
        }
        self.sub_paths.truncate(MAX_SUB_PATHS);
        let w = self.width();
        self.sub_paths.iter_mut().for_each(|sp| sp.sanitize(w));
    }
}

/// The sub-paths Pro Tools makes for a new bus, as for an input path: `.L` and `.R` for stereo,
/// a stereo `.LR` and a mono per channel for wider formats, none for mono. Ids come from `alloc`.
pub fn default_bus_sub_paths(bus: &Bus, mut alloc: impl FnMut() -> u64) -> Vec<BusSubPath> {
    let speakers = bus.format.speakers();
    if speakers.len() < 2 || bus.format.is_ambisonic() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut sub = |name: String, format: ChannelFormat, from: &[usize]| {
        let mut p = BusSubPath::new(BusSubId(alloc()), name, format);
        for (c, k) in p.channels.iter_mut().zip(from) {
            *c = u8::try_from(*k).ok();
        }
        p
    };
    if speakers.len() > 2 {
        out.push(sub(format!("{}.LR", bus.name), ChannelFormat::Stereo, &[0, 1]));
    }
    for (k, sp) in speakers.iter().enumerate() {
        out.push(sub(format!("{}.{}", bus.name, sp.label()), ChannelFormat::Mono, &[k]));
    }
    out
}

impl Session {
    /// The output paths of an interface with `channels` outputs, as Pro Tools' Stereo Mix
    /// settings make them: a stereo path per pair (`Out 1-2`), an odd last channel mono. The
    /// first is the main (monitor) path.
    pub fn default_output_paths(channels: usize) -> Vec<OutputPath> {
        let channels = channels.clamp(1, MAX_INPUT_CHANNEL as usize + 1);
        let mut out = Vec::new();
        let mut c = 0usize;
        while c < channels {
            let first = c as u16;
            if c + 1 < channels {
                out.push(OutputPath::assigned(format!("Out {}-{}", c + 1, c + 2), ChannelFormat::Stereo, first));
                c += 2;
            } else {
                out.push(OutputPath::assigned(format!("Out {}", c + 1), ChannelFormat::Mono, first));
                c += 1;
            }
        }
        out
    }

    /// Where the output path named `name` is (ignoring case).
    pub fn find_output_path(&self, name: &str) -> Option<usize> {
        self.outputs.iter().position(|o| o.name.eq_ignore_ascii_case(name))
    }

    pub fn output_path(&self, name: &str) -> Option<&OutputPath> {
        self.outputs.iter().find(|o| o.name.eq_ignore_ascii_case(name))
    }

    /// The output path the main mix drives: the one the I/O Setup names (when it exists and is
    /// active), else the first active path.
    pub fn monitor_output_path(&self) -> Option<&OutputPath> {
        self.monitor_output.as_deref().and_then(|n| self.output_path(n)).filter(|o| o.active).or_else(|| self.outputs.iter().find(|o| o.active))
    }

    /// The output path the main meter shows: the one the I/O Setup names, else the monitor path.
    pub fn meter_output_path(&self) -> Option<&OutputPath> {
        self.meter_output.as_deref().and_then(|n| self.output_path(n)).filter(|o| o.active).or_else(|| self.monitor_output_path())
    }

    /// Whether `name` is the monitor path (the main mix), so a track routed to it by name is
    /// routed to the main mix.
    pub fn is_monitor_output(&self, name: &str) -> bool {
        self.monitor_output_path().is_some_and(|o| o.name.eq_ignore_ascii_case(name))
    }

    /// The bus a sub-path belongs to, and the sub-path.
    pub fn bus_sub(&self, id: BusSubId) -> Option<(&Bus, &BusSubPath)> {
        self.busses.iter().find_map(|b| b.sub_paths.iter().find(|sp| sp.id == id).map(|sp| (b, sp)))
    }

    /// A bus or bus sub-path by name (ignoring case): the bus, and the sub-path when it is one.
    pub fn bus_path_by_name(&self, name: &str) -> Option<(&Bus, Option<&BusSubPath>)> {
        self.busses.iter().find_map(|b| {
            if b.name.eq_ignore_ascii_case(name) {
                return Some((b, None));
            }
            b.sub_paths.iter().find(|sp| sp.name.eq_ignore_ascii_case(name)).map(|sp| (b, Some(sp)))
        })
    }

    /// Internal mix busses as Pro Tools' defaults name them: `Bus 1-2`, `Bus 3-4`… (`count`
    /// stereo busses), each with `.L`/`.R` sub-paths. Busses that exist already (by name) are kept.
    pub fn add_default_busses(&mut self, count: usize) -> Vec<crate::BusId> {
        let mut out = Vec::new();
        for i in 0..count.min(128) {
            let name = format!("Bus {}-{}", 2 * i + 1, 2 * i + 2);
            if self.bus_by_name(&name).is_some() {
                continue;
            }
            let id = self.add_bus(&name, ChannelFormat::Stereo);
            let subs = self.bus(id).map(|b| default_bus_sub_paths(b, || 0)).unwrap_or_default();
            let subs: Vec<BusSubPath> = subs
                .into_iter()
                .map(|mut sp| {
                    sp.id = BusSubId(self.alloc());
                    sp
                })
                .collect();
            if let Some(b) = self.busses.iter_mut().find(|b| b.id == id) {
                b.sub_paths = subs;
            }
            out.push(id);
        }
        out
    }

    /// Output busses as Pro Tools makes them: a bus named after each active output path, of its
    /// format, mapped to it. Existing busses of those names are mapped, not duplicated.
    pub fn add_output_busses(&mut self) -> Vec<crate::BusId> {
        let paths: Vec<(String, ChannelFormat)> = self.outputs.iter().filter(|o| o.active).map(|o| (o.name.clone(), o.format)).collect();
        let mut out = Vec::new();
        for (name, format) in paths {
            let id = self.add_bus(&name, format);
            if let Some(b) = self.busses.iter_mut().find(|b| b.id == id) {
                b.output = Some(name);
            }
            out.push(id);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hw(p: &InputPath) -> Vec<Option<u16>> {
        p.channels.clone()
    }

    #[test]
    fn default_paths_pair_the_channels_with_mono_sub_paths() {
        let paths = Session::default_input_paths(5);
        let names: Vec<&str> = paths.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["In 1-2", "In 3-4", "In 5"]);
        assert_eq!(hw(&paths[1]), [Some(2), Some(3)]);
        let subs: Vec<(&str, Vec<Option<u16>>)> = paths[1].sub_paths.iter().map(|s| (s.name.as_str(), hw(s))).collect();
        assert_eq!(subs, [("In 3", vec![Some(2)]), ("In 4", vec![Some(3)])]);
        assert_eq!(paths[2].format, ChannelFormat::Mono);
        assert!(paths[2].sub_paths.is_empty());
        assert_eq!(Session::default_input_paths(0).len(), 1, "at least one channel");
        assert_eq!(Session::default_input_paths(usize::MAX).len(), 512, "capped");
    }

    #[test]
    fn a_new_session_has_the_eight_channel_defaults_and_finds_paths_by_name() {
        let s = Session::default();
        assert_eq!(s.inputs.len(), 4);
        assert_eq!(s.find_input_path("in 3-4"), Some((1, None)));
        assert_eq!(s.find_input_path("IN 4"), Some((1, Some(1))));
        assert_eq!(s.find_input_path("In 9"), None);
        assert_eq!(s.input_path("In 7").map(hw), Some(vec![Some(6)]));
        assert_eq!(s.input_paths_flat().len(), 12);
    }

    #[test]
    fn route_channels_come_from_the_path_or_the_in_n_naming() {
        let mut s = Session::default();
        assert_eq!(s.input_route_channels(&Route::Hardware("In 3-4".into()), 2), [Some(2), Some(3)]);
        assert_eq!(s.input_route_channels(&Route::Hardware("In 4".into()), 2), [Some(3), Some(3)], "a mono path repeats on a stereo track");
        assert_eq!(s.input_route_channels(&Route::None, 2), [Some(0), Some(1)]);
        // A name no path has: the default naming still works, as it did before paths existed.
        assert_eq!(s.input_route_channels(&Route::Hardware("In 11-12".into()), 2), [Some(10), Some(11)]);
        assert_eq!(s.input_route_channels(&Route::Hardware("Vocal".into()), 1), [Some(0)]);
        // Inactive paths feed nothing; neither do their sub-paths.
        s.inputs[1].active = false;
        assert_eq!(s.input_route_channels(&Route::Hardware("In 3-4".into()), 2), [None, None]);
        assert_eq!(s.input_route_channels(&Route::Hardware("In 3".into()), 1), [None]);
        s.inputs[1].active = true;
        s.inputs[1].channels[1] = None;
        assert_eq!(s.input_route_channels(&Route::Hardware("In 3-4".into()), 2), [Some(2), None]);
    }

    #[test]
    fn sub_paths_follow_the_format_table() {
        let stereo = InputPath::assigned("A 1-2", ChannelFormat::Stereo, 0);
        let subs: Vec<(String, Vec<Option<u16>>)> = default_sub_paths(&stereo).into_iter().map(|p| (p.name.clone(), hw(&p))).collect();
        assert_eq!(subs, [("A 1-2.L".to_string(), vec![Some(0)]), ("A 1-2.R".to_string(), vec![Some(1)])]);
        let five = InputPath::assigned("Surr", ChannelFormat::Surround51, 8);
        let subs = default_sub_paths(&five);
        assert_eq!(subs.len(), 7);
        assert_eq!((subs[0].name.as_str(), hw(&subs[0])), ("Surr.LR", vec![Some(8), Some(9)]));
        assert_eq!((subs[4].name.as_str(), hw(&subs[4])), ("Surr.LFE", vec![Some(11)]));
        assert!(default_sub_paths(&InputPath::new("M", ChannelFormat::Mono)).is_empty());
        let unassigned = InputPath::new("U", ChannelFormat::Stereo);
        assert_eq!(hw(&default_sub_paths(&unassigned)[0]), [None]);
    }

    #[test]
    fn sanitize_bounds_a_hostile_path() {
        let mut p = InputPath {
            name: "x".repeat(1000),
            format: ChannelFormat::Stereo,
            channels: vec![Some(5000), Some(1), Some(1)],
            active: true,
            sub_paths: Vec::new(),
        };
        let mut sub = InputPath::new("s", ChannelFormat::Mono);
        sub.channels = vec![Some(7)];
        sub.sub_paths.push(InputPath::new("deep", ChannelFormat::Mono));
        p.sub_paths = vec![sub; MAX_SUB_PATHS + 5];
        p.sanitize(true);
        assert_eq!(p.name.chars().count(), MAX_NAME_CHARS);
        assert_eq!(p.channels, [None, Some(1)], "out of range and the duplicate dropped, width from the format");
        assert_eq!(p.sub_paths.len(), MAX_SUB_PATHS);
        assert!(p.sub_paths.iter().all(|s| s.sub_paths.is_empty() && s.channels == [None]), "a sub-path reads only its main path's channels");
        let mut m = InputPath::assigned("m", ChannelFormat::Mono, u16::MAX);
        m.assign_from(MAX_INPUT_CHANNEL);
        assert_eq!(m.channels, [Some(MAX_INPUT_CHANNEL)]);
    }

    #[test]
    fn output_paths_default_per_pair_and_the_first_is_the_monitor() {
        let s = Session::default();
        assert_eq!(s.outputs.len(), 1);
        assert_eq!(s.outputs[0].channels, [Some(0), Some(1)]);
        assert_eq!(s.outputs[0].first_channel, 0);
        let paths = Session::default_output_paths(5);
        let names: Vec<&str> = paths.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Out 1-2", "Out 3-4", "Out 5"]);
        assert_eq!(paths[1].channels, [Some(2), Some(3)]);
        assert_eq!(paths[1].first_channel, 2);
        let mut s = Session::new("x", Default::default());
        s.outputs = Session::default_output_paths(4);
        assert_eq!(s.monitor_output_path().map(|o| o.name.as_str()), Some("Out 1-2"));
        assert!(s.is_monitor_output("out 1-2"));
        s.monitor_output = Some("Out 3-4".into());
        assert_eq!(s.monitor_output_path().map(|o| o.name.as_str()), Some("Out 3-4"));
        assert_eq!(s.meter_output_path().map(|o| o.name.as_str()), Some("Out 3-4"), "the meter follows the monitor unless named");
        s.meter_output = Some("Out 1-2".into());
        assert_eq!(s.meter_output_path().map(|o| o.name.as_str()), Some("Out 1-2"));
        s.outputs[1].active = false;
        assert_eq!(s.monitor_output_path().map(|o| o.name.as_str()), Some("Out 1-2"), "an inactive monitor path falls back to the first active");
        s.outputs[0].format = ChannelFormat::Surround51;
        assert_eq!(s.main_format(), ChannelFormat::Surround51, "the main format is the monitor path's");
    }

    #[test]
    fn an_output_path_from_before_the_map_gets_it_from_its_first_channel() {
        let mut p = OutputPath { name: "Out 3-4".into(), first_channel: 2, format: ChannelFormat::Stereo, channels: Vec::new(), active: true };
        p.sanitize();
        assert_eq!(p.channels, [Some(2), Some(3)]);
        let mut hostile =
            OutputPath { name: "x".repeat(400), first_channel: 9, format: ChannelFormat::Mono, channels: vec![Some(5000), Some(1)], active: true };
        hostile.sanitize();
        assert_eq!((hostile.channels.clone(), hostile.first_channel), (vec![None], 0));
        assert_eq!(hostile.name.chars().count(), MAX_NAME_CHARS);
        let mut dup = OutputPath::assigned("d", ChannelFormat::Stereo, 4);
        dup.channels[1] = Some(4);
        dup.sanitize();
        assert_eq!(dup.channels, [Some(4), None]);
    }

    #[test]
    fn bus_sub_paths_read_within_the_bus_and_are_found_by_id_and_name() {
        let mut s = Session::default();
        let id = s.add_bus("Verb", ChannelFormat::Stereo);
        let subs = default_bus_sub_paths(s.bus(id).unwrap(), || 77);
        assert_eq!(subs.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Verb.L", "Verb.R"]);
        assert_eq!(subs[1].channels, [Some(1)]);
        s.busses[0].sub_paths = subs;
        s.busses[0].sub_paths[0].id = BusSubId(5);
        s.busses[0].sub_paths[1].id = BusSubId(6);
        assert_eq!(s.bus_sub(BusSubId(6)).map(|(b, sp)| (b.name.as_str(), sp.name.as_str())), Some(("Verb", "Verb.R")));
        assert!(s.bus_sub(BusSubId(9)).is_none());
        assert_eq!(s.bus_path_by_name("verb.l").map(|(b, sp)| (b.id, sp.map(|sp| sp.id))), Some((id, Some(BusSubId(5)))));
        assert_eq!(s.bus_path_by_name("VERB").map(|(b, sp)| (b.id, sp.is_none())), Some((id, true)));
        let mut sp = BusSubPath::new(BusSubId(1), "wide", ChannelFormat::Stereo);
        sp.assign_from(1, 2);
        assert_eq!(sp.channels, [Some(1), None], "the second channel falls off the bus");
        s.busses[0].sub_paths.push(BusSubPath { id: BusSubId(2), name: "bad".into(), format: ChannelFormat::Mono, channels: vec![Some(9)] });
        s.sanitize();
        assert_eq!(s.busses[0].sub_paths[2].channels, [None]);
        assert!(s.next_id > 6, "sub-path ids count toward the allocator");
    }

    #[test]
    fn default_busses_and_output_busses() {
        let mut s = Session::new("x", Default::default());
        s.outputs = Session::default_output_paths(4);
        let made = s.add_default_busses(2);
        assert_eq!(made.len(), 2);
        assert_eq!(s.busses.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["Bus 1-2", "Bus 3-4"]);
        assert_eq!(s.busses[0].sub_paths.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Bus 1-2.L", "Bus 1-2.R"]);
        let ids: std::collections::BTreeSet<u64> = s.busses.iter().flat_map(|b| b.sub_paths.iter().map(|p| p.id.0)).collect();
        assert_eq!(ids.len(), 4, "every sub-path has its own id");
        assert_eq!(s.add_default_busses(2).len(), 0, "already there");
        let out = s.add_output_busses();
        assert_eq!(out.len(), 2);
        assert_eq!(s.bus_by_name("Out 3-4").and_then(|b| b.output.as_deref()), Some("Out 3-4"));
        assert_eq!(s.add_output_busses().len(), 2, "mapped again, not duplicated");
        assert_eq!(s.busses.len(), 4);
    }
}
