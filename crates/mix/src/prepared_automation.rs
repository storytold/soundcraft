//! Document-bound automation lookup tables, prepared outside rendering.

use soundcraft_model::{AutoParam, AutomationLane, Session, Track};
use soundcraft_time::Samples;
use std::cmp::Ordering;
use std::sync::Arc;

/// Maximum admission budget for lookup tables; source audio and points are never copied.
pub const MAX_AUTOMATION_INDEX_BYTES: usize = 32 << 20;

enum Source<'a> {
    Borrowed(&'a Session),
    Owned(Arc<Session>),
}

/// An immutable document and its automation index. The index cannot be paired with another
/// document. Admission or allocation refusal retains the same document with ordinary lookups.
pub struct PreparedSession<'a> {
    source: Source<'a>,
    index: Option<Index>,
}

impl<'a> PreparedSession<'a> {
    /// Prepare a borrowed offline document without cloning its audio, points or document.
    pub fn borrowed(source: &'a Session) -> Self {
        Self::borrowed_with_budget(source, MAX_AUTOMATION_INDEX_BYTES)
    }

    /// Prepare with a smaller lookup budget. A zero budget selects ordinary lookups.
    pub fn borrowed_with_budget(source: &'a Session, bytes: usize) -> Self {
        Self { source: Source::Borrowed(source), index: Index::prepare(source, bytes.min(MAX_AUTOMATION_INDEX_BYTES)) }
    }

    pub fn source(&self) -> &Session {
        match &self.source {
            Source::Borrowed(source) => source,
            Source::Owned(source) => source,
        }
    }

    /// The owned document, when this pair was prepared for publication to playback.
    pub fn owned_source(&self) -> Option<&Arc<Session>> {
        match &self.source {
            Source::Borrowed(_) => None,
            Source::Owned(source) => Some(source),
        }
    }

    pub fn is_indexed(&self) -> bool {
        self.index.is_some()
    }

    pub(crate) fn read<'s>(&'s self, track: &'s Track) -> Read<'s> {
        Read {
            track,
            index: self.index.as_ref().and_then(|index| {
                let key = track as *const Track as usize;
                index.tracks.binary_search_by_key(&key, |(key, _)| *key).ok().and_then(|position| index.tracks.get(position).map(|(_, track)| track))
            }),
        }
    }
}

impl PreparedSession<'static> {
    /// Prepare an owned document on the publishing/control thread.
    pub fn owned(source: Arc<Session>) -> Self {
        Self::owned_with_budget(source, MAX_AUTOMATION_INDEX_BYTES)
    }

    pub fn owned_with_budget(source: Arc<Session>, bytes: usize) -> Self {
        let index = Index::prepare(&source, bytes.min(MAX_AUTOMATION_INDEX_BYTES));
        Self { source: Source::Owned(source), index }
    }
}

struct Index {
    tracks: Vec<(usize, TrackIndex)>,
}

struct TrackIndex {
    lanes: Vec<usize>,
    pan_automated: bool,
    plugin_slots: [bool; 256],
}

impl Index {
    fn prepare(source: &Session, bytes: usize) -> Option<Self> {
        // Admission bounds requested vector storage. No point buffers or parameter strings
        // are allocated: the entries refer into the immutable document. Allocator metadata is
        // outside this policy. The sorted track vector has no hash-table capacity multiplier.
        let lanes = source.tracks.iter().try_fold(0usize, |count, track| count.checked_add(track.automation.len()))?;
        let total = planned_index_bytes(source.tracks.len(), lanes)?;
        if bytes == 0 || total > bytes {
            return None;
        }
        let mut tracks = Vec::new();
        tracks.try_reserve_exact(source.tracks.len()).ok()?;
        for track in &source.tracks {
            let mut lanes = Vec::new();
            lanes.try_reserve_exact(track.automation.len()).ok()?;
            lanes.extend(0..track.automation.len());
            lanes.sort_unstable_by(|a, b| match (track.automation.get(*a), track.automation.get(*b)) {
                (Some(a_lane), Some(b_lane)) => a_lane.param.cmp(&b_lane.param).then(a.cmp(b)),
                _ => a.cmp(b),
            });
            // Keep the first lane even when it is empty or disabled, just like Track::lane.
            lanes.dedup_by(|a, b| track.automation.get(*a).zip(track.automation.get(*b)).is_some_and(|(a, b)| a.param == b.param));
            let mut plugin_slots = [false; 256];
            for lane in &track.automation {
                if let AutoParam::Plugin { slot, .. } = lane.param
                    && !lane.points.is_empty()
                    && let Some(automated) = plugin_slots.get_mut(usize::from(slot))
                {
                    *automated = true;
                }
            }
            let pan_automated = track.automation.iter().any(|lane| matches!(lane.param, AutoParam::Pan(_)) && !lane.points.is_empty());
            tracks.push((track as *const Track as usize, TrackIndex { lanes, pan_automated, plugin_slots }));
        }
        tracks.sort_unstable_by_key(|(key, _)| *key);
        Some(Self { tracks })
    }
}

fn planned_index_bytes(tracks: usize, lanes: usize) -> Option<usize> {
    tracks.checked_mul(512)?.checked_add(lanes.checked_mul(2 * size_of::<usize>())?)
}

#[derive(Clone, Copy)]
pub(crate) struct Read<'a> {
    pub(crate) track: &'a Track,
    index: Option<&'a TrackIndex>,
}

impl<'a> Read<'a> {
    pub(crate) fn ordinary(track: &'a Track) -> Self {
        Self { track, index: None }
    }

    pub(crate) fn lane(self, param: &AutoParam) -> Option<&'a AutomationLane> {
        let Some(index) = self.index else { return self.track.lane(param) };
        let position =
            index.lanes.binary_search_by(|index| self.track.automation.get(*index).map_or(Ordering::Less, |lane| lane.param.cmp(param))).ok()?;
        index.lanes.get(position).and_then(|index| self.track.automation.get(*index))
    }

    pub(crate) fn plugin_lane(self, slot: u8, param: &str) -> Option<&'a AutomationLane> {
        let matches =
            |lane: &&AutomationLane| matches!(&lane.param, AutoParam::Plugin { slot: stored, param: key } if *stored == slot && key == param);
        let Some(index) = self.index else { return self.track.automation.iter().find(matches) };
        let position = index
            .lanes
            .binary_search_by(|index| match self.track.automation.get(*index).map(|lane| &lane.param) {
                Some(AutoParam::Plugin { slot: stored, param: key }) => stored.cmp(&slot).then_with(|| key.as_str().cmp(param)),
                _ => Ordering::Less,
            })
            .ok()?;
        index.lanes.get(position).and_then(|index| self.track.automation.get(*index))
    }

    pub(crate) fn pan_automated(self) -> bool {
        self.index.map_or_else(
            || self.track.automation.iter().any(|lane| matches!(lane.param, AutoParam::Pan(_)) && !lane.points.is_empty()),
            |index| index.pan_automated,
        )
    }

    pub(crate) fn plugin_automated(self, slot: u8) -> bool {
        self.index.map_or_else(
            || {
                self.track
                    .automation
                    .iter()
                    .any(|lane| matches!(&lane.param, AutoParam::Plugin { slot: stored, .. } if *stored == slot) && !lane.points.is_empty())
            },
            |index| index.plugin_slots.get(usize::from(slot)).copied().unwrap_or(false),
        )
    }
}

/// Keep the existing f32 interpolation and point order. Only timestamp subtraction that would
/// otherwise overflow uses a widened difference; no sorting, clamping or sanitization occurs.
pub(crate) fn value_at(lane: &AutomationLane, at: Samples, default: f32) -> f32 {
    let points = &lane.points;
    let Some(first) = points.first() else { return default };
    if at <= first.at {
        return first.value;
    }
    let index = points.partition_point(|point| point.at <= at);
    match (points.get(index.wrapping_sub(1)), points.get(index)) {
        (Some(a), Some(b)) if b.at > a.at => {
            if matches!(lane.param, AutoParam::Mute | AutoParam::SendMute(_)) {
                return a.value;
            }
            let offset = at.checked_sub(a.at).map_or_else(|| (i128::from(at) - i128::from(a.at)) as f32, |difference| difference as f32);
            let span = b.at.checked_sub(a.at).map_or_else(|| (i128::from(b.at) - i128::from(a.at)) as f32, |difference| difference as f32);
            a.value + (b.value - a.value) * (offset / span)
        }
        (Some(a), _) => a.value,
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{AutomationPoint, ChannelFormat, TrackKind};

    fn fixture() -> Session {
        let mut source = Session::default();
        source.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Prepared lookup"));
        source
    }

    fn lane(param: AutoParam, values: &[(Samples, f32)]) -> AutomationLane {
        AutomationLane { param, points: values.iter().map(|&(at, value)| AutomationPoint { at, value }).collect(), enabled: false }
    }

    #[test]
    fn first_empty_disabled_duplicate_lane_remains_the_read_target() {
        let mut source = fixture();
        source.tracks[0].automation =
            vec![lane(AutoParam::Volume, &[]), lane(AutoParam::Pan(0), &[(0, -1.0)]), lane(AutoParam::Volume, &[(0, -144.0)])];
        let prepared = PreparedSession::borrowed(&source);
        assert!(prepared.is_indexed());
        let read = prepared.read(&source.tracks[0]);
        assert!(read.index.is_some(), "a track borrowed from the paired source hits its prepared index");
        let unrelated = source.tracks[0].clone();
        assert!(prepared.read(&unrelated).index.is_none(), "foreign track addresses cannot reuse this document index");
        assert!(std::ptr::eq(read.lane(&AutoParam::Volume).unwrap(), &source.tracks[0].automation[0]));
        assert!(read.lane(&AutoParam::Volume).unwrap().points.is_empty());
        assert!(!read.lane(&AutoParam::Pan(0)).unwrap().enabled);
    }

    #[test]
    fn plugin_keys_remain_case_sensitive_and_first_empty_key_wins() {
        let mut source = fixture();
        source.tracks[0].automation = vec![
            lane(AutoParam::Plugin { slot: 0, param: "Gain".into() }, &[(0, 2.0)]),
            lane(AutoParam::Plugin { slot: 0, param: "gain".into() }, &[]),
            lane(AutoParam::Plugin { slot: 0, param: "gain".into() }, &[(0, 3.0)]),
        ];
        let prepared = PreparedSession::borrowed(&source);
        let read = prepared.read(&source.tracks[0]);
        assert!(read.plugin_automated(0));
        assert!(read.plugin_lane(0, "gain").unwrap().points.is_empty());
        assert_eq!(value_at(read.plugin_lane(0, "Gain").unwrap(), 0, 0.0), 2.0);
        assert!(read.plugin_lane(0, "GAIN").is_none());
    }

    #[test]
    fn budget_refusal_keeps_the_exact_owned_document_and_source() {
        let source = Arc::new(fixture());
        let pair = PreparedSession::owned_with_budget(Arc::clone(&source), 0);
        assert!(!pair.is_indexed());
        assert!(Arc::ptr_eq(pair.owned_source().unwrap(), &source));
        assert!(std::ptr::eq(pair.source(), source.as_ref()));
        let indexed = PreparedSession::owned_with_budget(Arc::clone(&source), usize::MAX);
        assert!(indexed.is_indexed());
        assert!(Arc::ptr_eq(indexed.owned_source().unwrap(), &source));
    }

    #[test]
    fn admission_counts_tracks_and_lane_indices_without_copying_points() {
        let mut source = fixture();
        source.tracks[0].automation = vec![lane(AutoParam::Volume, &[(0, 0.0); 128])];
        let bytes = 512 + 2 * size_of::<usize>();
        assert!(!PreparedSession::borrowed_with_budget(&source, bytes - 1).is_indexed());
        let pair = PreparedSession::borrowed_with_budget(&source, bytes);
        assert!(pair.is_indexed());
        assert!(std::ptr::eq(
            pair.read(&source.tracks[0]).lane(&AutoParam::Volume).unwrap().points.as_ptr(),
            source.tracks[0].automation[0].points.as_ptr()
        ));
    }

    #[test]
    fn requested_index_storage_is_bounded_and_checked_at_capacity_edges() {
        assert!(size_of::<(usize, TrackIndex)>() <= 512);
        let tracks = MAX_AUTOMATION_INDEX_BYTES / 512;
        assert_eq!(planned_index_bytes(tracks, 0), Some(MAX_AUTOMATION_INDEX_BYTES));
        assert_eq!(planned_index_bytes(tracks + 1, 0), Some(MAX_AUTOMATION_INDEX_BYTES + 512));
        assert!(planned_index_bytes(57_345, 0).unwrap() >= 57_345 * size_of::<(usize, TrackIndex)>());
        assert!(planned_index_bytes(usize::MAX, 0).is_none());
        assert!(planned_index_bytes(0, usize::MAX).is_none());
    }

    #[test]
    fn equal_track_ids_do_not_cross_bind_their_lanes() {
        let mut source = fixture();
        source.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Other lookup"));
        source.tracks[1].id = source.tracks[0].id;
        source.tracks[0].automation = vec![lane(AutoParam::Volume, &[(0, -3.0)])];
        source.tracks[1].automation = vec![lane(AutoParam::Volume, &[(0, -9.0)])];
        let pair = PreparedSession::borrowed(&source);
        assert_eq!(value_at(pair.read(&source.tracks[0]).lane(&AutoParam::Volume).unwrap(), 0, 0.0), -3.0);
        assert_eq!(value_at(pair.read(&source.tracks[1]).lane(&AutoParam::Volume).unwrap(), 0, 0.0), -9.0);
    }

    #[test]
    fn prepared_index_preserves_point_order_duplicates_and_nonfinite_bits() {
        let mut source = fixture();
        source.tracks[0].automation = vec![lane(AutoParam::Volume, &[(30, 50.0), (10, f32::NAN), (10, f32::INFINITY), (20, -300.0)])];
        let pair = PreparedSession::borrowed(&source);
        assert!(pair.is_indexed());
        let indexed = pair.read(&source.tracks[0]).lane(&AutoParam::Volume).unwrap();
        assert!(std::ptr::eq(indexed, &source.tracks[0].automation[0]));
        assert_eq!(
            indexed.points.iter().map(|point| (point.at, point.value.to_bits())).collect::<Vec<_>>(),
            vec![(30, 50.0f32.to_bits()), (10, f32::NAN.to_bits()), (10, f32::INFINITY.to_bits()), (20, (-300.0f32).to_bits())]
        );
    }

    #[test]
    fn normal_interpolation_is_bit_exact_and_extreme_timestamps_are_bounded() {
        for param in [AutoParam::Volume, AutoParam::Mute, AutoParam::SendMute(0)] {
            let curve = lane(param, &[(-7, -12.0), (3, 2.0), (3, 3.0), (19, -1.0)]);
            for at in -10..25 {
                assert_eq!(value_at(&curve, at, 0.0).to_bits(), curve.value_at(at, 0.0).to_bits());
            }
        }
        let curve = lane(AutoParam::Volume, &[(i64::MIN, -12.0), (i64::MAX, 0.0)]);
        assert_eq!(value_at(&curve, 0, 0.0), -6.0);
        assert_eq!(value_at(&curve, i64::MIN, 0.0), -12.0);
        assert_eq!(value_at(&curve, i64::MAX, 0.0), 0.0);
    }
}
