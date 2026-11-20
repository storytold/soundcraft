use serde::Serialize;
use std::collections::BTreeMap;

/// Host-side delivery measurements. Time values use microseconds.
#[derive(Default, Serialize)]
pub struct MidiSyncStatus {
    pub sent: u64,
    pub relaunches: u64,
    pub catch_up_sends: u64,
    pub stale_dropped: u64,
    pub lateness_buckets: [u64; 7],
    pub lateness_p50_us: u64,
    pub lateness_p95_us: u64,
    pub lateness_max_us: u64,
    pub reported_lead_us: u64,
    pub lead_clamps: u64,
    pub callback_gap_max_us: u64,
    pub callback_gaps_over_two_buffers: u64,
    pub priority: &'static str,
    pub ports: BTreeMap<String, PortTiming>,
}

#[derive(Serialize)]
pub struct PortTiming {
    pub offset_samples: i64,
    pub total_us: i64,
    pub compensated: bool,
}
