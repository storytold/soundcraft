use super::{BusId, Route, Session, Track, TrackKind};
use std::collections::HashMap;
use std::sync::Arc;

/// Hash of everything `sync`/`processing_order` depend on (tracks, kinds, routing, plugins, sends).
pub(crate) fn structure_fingerprint(s: &Session) -> usize {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for t in &s.tracks {
        t.id.hash(&mut h);
        t.kind.hash(&mut h);
        t.format.hash(&mut h);
        t.inactive.hash(&mut h);
        t.mixer.inserts.len().hash(&mut h);
        t.mixer.input.hash(&mut h);
        t.mixer.output.hash(&mut h);
        for i in &t.mixer.inserts {
            i.as_ref().map(|x| x.plugin.as_str()).hash(&mut h);
        }
        for snd in t.mixer.sends.iter().flatten() {
            snd.target.hash(&mut h);
        }
        t.instrument.as_ref().map(|x| x.plugin.as_str()).hash(&mut h);
        let sample = t.instrument.as_ref().and_then(|x| x.sample);
        sample.hash(&mut h);
        // Which decoded audio it is, so new audio under the same id (another session) re-syncs.
        sample.and_then(|id| s.pool.get(id)).map(|a| Arc::as_ptr(a) as usize).hash(&mut h);
    }
    for b in &s.busses {
        b.id.hash(&mut h);
        b.format.hash(&mut h);
    }
    s.main_format().hash(&mut h);
    h.finish() as usize
}

/// Audio/instrument tracks first, then auxes ordered so bus producers come before consumers.
pub(crate) fn processing_order(s: &Session) -> Vec<usize> {
    let mut order: Vec<usize> =
        (0..s.tracks.len()).filter(|&i| s.tracks.get(i).is_some_and(|t| !matches!(t.kind, TrackKind::Aux | TrackKind::Folder))).collect();
    let mut auxes: Vec<usize> =
        (0..s.tracks.len()).filter(|&i| s.tracks.get(i).is_some_and(|t| matches!(t.kind, TrackKind::Aux | TrackKind::Folder))).collect();
    if let Some(depths) = acyclic_aux_depths(s, &auxes) {
        // Stable sorting preserves session order among auxes at the same longest-path depth.
        auxes.sort_by_key(|&i| depths.get(i).copied().unwrap_or(0));
    } else {
        // A cycle keeps the existing bounded ordering for the entire graph, including any
        // independent acyclic branches. Feedback does not have a finite longest-path depth.
        auxes.sort_by_key(|&i| bounded_aux_depth(s, i));
    }
    order.extend(auxes);
    order
}

/// Exact aux-hop depths when the whole Aux/Folder graph is acyclic. Bus joins retain only
/// producer counts and consumers, avoiding a producer-by-consumer adjacency matrix.
fn acyclic_aux_depths(s: &Session, auxes: &[usize]) -> Option<Vec<usize>> {
    #[derive(Default)]
    struct BusDependencies {
        remaining: usize,
        depth: usize,
        last_producer: Option<usize>,
        last_processed: Option<usize>,
        consumers: Vec<usize>,
    }
    let mut busses = HashMap::<BusId, BusDependencies>::new();
    for &i in auxes {
        for bus in aux_output_busses(s.tracks.get(i)?) {
            let dependency = busses.entry(bus).or_default();
            // Output and multiple sends from this producer may name the same bus.
            if dependency.last_producer != Some(i) {
                dependency.last_producer = Some(i);
                dependency.remaining = dependency.remaining.checked_add(1)?;
            }
        }
    }
    let mut depths = vec![0usize; s.tracks.len()];
    let mut ready = Vec::with_capacity(auxes.len());
    for &i in auxes {
        if let Route::Bus(bus) = &s.tracks.get(i)?.mixer.input {
            let dependency = busses.entry(*bus).or_default();
            dependency.consumers.push(i);
            if dependency.remaining > 0 {
                continue;
            }
        }
        ready.push(i);
    }
    let mut processed = 0usize;
    while let Some(&i) = ready.get(processed) {
        processed = processed.checked_add(1)?;
        let next_depth = depths.get(i)?.checked_add(1)?;
        for bus in aux_output_busses(s.tracks.get(i)?) {
            let dependency = busses.get_mut(&bus)?;
            if dependency.last_processed == Some(i) {
                continue;
            }
            dependency.last_processed = Some(i);
            dependency.depth = dependency.depth.max(next_depth);
            dependency.remaining = dependency.remaining.checked_sub(1)?;
            if dependency.remaining == 0 {
                for &consumer in &dependency.consumers {
                    *depths.get_mut(consumer)? = dependency.depth;
                    ready.push(consumer);
                }
            }
        }
    }
    (processed == auxes.len()).then_some(depths)
}

fn aux_output_busses(t: &Track) -> impl Iterator<Item = BusId> + '_ {
    std::iter::once(&t.mixer.output)
        .chain(t.mixer.sends.iter().flatten().map(|send| &send.target))
        .filter_map(|route| if let Route::Bus(bus) = route { Some(*bus) } else { None })
}

/// The previous bounded depth calculation, retained unchanged for any cyclic graph.
fn bounded_aux_depth(s: &Session, i: usize) -> usize {
    let mut d = 0;
    let mut cur = vec![i];
    for _ in 0..8 {
        let inputs: Vec<&Route> = cur.iter().filter_map(|&k| s.tracks.get(k)).map(|t| &t.mixer.input).collect();
        let feeders: Vec<usize> = auxes_feeding(s, &inputs);
        if feeders.is_empty() {
            break;
        }
        d += 1;
        cur = feeders;
    }
    d
}

fn auxes_feeding(s: &Session, inputs: &[&Route]) -> Vec<usize> {
    s.tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| matches!(t.kind, TrackKind::Aux | TrackKind::Folder))
        .filter(|(_, t)| {
            inputs.iter().any(|r| matches!(r, Route::Bus(_)) && (&&t.mixer.output == r || t.mixer.sends.iter().flatten().any(|sn| &&sn.target == r)))
        })
        .map(|(i, _)| i)
        .collect()
}

/// Implicit solo: an aux is not muted when a soloed track feeds its input bus.
pub(crate) fn receives_solo(s: &Session, t: &Track) -> bool {
    let Route::Bus(b) = &t.mixer.input else { return false };
    s.tracks
        .iter()
        .any(|o| o.mixer.solo && (o.mixer.output == Route::Bus(*b) || o.mixer.sends.iter().flatten().any(|sn| sn.target == Route::Bus(*b))))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod processing_order_tests {
    use super::*;
    use soundcraft_model::ChannelFormat;
    use soundcraft_model::SendSlot;

    #[test]
    fn acyclic_bus_joins_use_longest_depth_and_keep_session_order_for_ties() {
        let mut s = Session::default();
        s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let consumer = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
        let first_root = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
        let second_root = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
        let middle = s.add_track(TrackKind::Folder, ChannelFormat::Stereo, None);
        let first_bus = s.add_bus("First", ChannelFormat::Stereo);
        let join = s.add_bus("Join", ChannelFormat::Stereo);
        s.track_mut(consumer).unwrap().mixer.input = Route::Bus(join);
        let root = s.track_mut(first_root).unwrap();
        root.mixer.output = Route::Main;
        root.mixer.sends[0] = Some(SendSlot::new(Route::Bus(first_bus)));
        let root = s.track_mut(second_root).unwrap();
        root.mixer.output = Route::Bus(join);
        root.mixer.sends[0] = Some(SendSlot::new(Route::Bus(join)));
        root.mixer.sends[1] = Some(SendSlot::new(Route::Bus(join)));
        let folder = s.track_mut(middle).unwrap();
        folder.mixer.input = Route::Bus(first_bus);
        folder.mixer.output = Route::Bus(join);
        folder.mixer.sends[0] = Some(SendSlot::new(Route::Bus(join)));

        assert_eq!(processing_order(&s), vec![0, 2, 3, 4, 1]);
    }

    #[test]
    fn a_cycle_preserves_bounded_order_for_the_whole_graph() {
        let mut s = Session::default();
        let busses: Vec<_> = (0..10).map(|i| s.add_bus(&format!("Chain {i}"), ChannelFormat::Stereo)).collect();
        let tracks: Vec<_> = (0..10).map(|_| s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None)).collect();
        for (i, id) in tracks.iter().enumerate() {
            let track = s.track_mut(*id).unwrap();
            track.mixer.input = Route::Bus(busses[i]);
            track.mixer.output = if i == 0 { Route::Main } else { Route::Bus(busses[i - 1]) };
        }
        assert_eq!(processing_order(&s), vec![9, 8, 7, 6, 5, 4, 3, 2, 1, 0]);
        let cycle_bus = s.add_bus("Feedback", ChannelFormat::Stereo);
        let cycle = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
        let track = s.track_mut(cycle).unwrap();
        track.mixer.input = Route::Bus(cycle_bus);
        track.mixer.output = Route::Bus(cycle_bus);

        // Both deepest independent returns tie with the cycle at the legacy eight-hop cap.
        assert_eq!(processing_order(&s), vec![9, 8, 7, 6, 5, 4, 3, 2, 0, 1, 10]);
    }
}
