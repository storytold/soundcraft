use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputLeadSource {
    #[default]
    Unobserved,
    Reported,
    Retained,
    Unknown,
}

impl OutputLeadSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unobserved => "unobserved",
            Self::Reported => "reported",
            Self::Retained => "retained",
            Self::Unknown => "unknown",
        }
    }
}

/// The latest hardware callback had no usable presentation timestamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputTimingIssue {
    Retained { lead_us: u64 },
    Unknown,
}

/// Host-side delivery measurements. Time values use microseconds.
#[derive(Default, Serialize)]
pub struct MidiSyncStatus {
    pub sent: u64,
    pub pending_transition_timeouts: u64,
    pub oversized_dropped: u64,
    pub error: Option<String>,
    pub relaunches: u64,
    pub session_backlog_drops: u64,
    pub catch_up_sends: u64,
    pub stale_dropped: u64,
    pub lateness_buckets: [u64; 7],
    pub lateness_p50_us: u64,
    pub lateness_p95_us: u64,
    pub lateness_max_us: u64,
    pub reported_lead_us: Option<u64>,
    pub lead_source: OutputLeadSource,
    pub lead_clamps: u64,
    pub callback_gap_max_us: u64,
    pub callback_gaps_over_two_buffers: u64,
    pub priority: &'static str,
    pub ports: BTreeMap<String, PortTiming>,
}

#[derive(Serialize)]
pub struct PortTiming {
    pub connected: bool,
    pub device_lead_us: Option<u64>,
    pub lead_source: OutputLeadSource,
    pub system_delay_samples: u64,
    pub advance_limited: bool,
    pub effective_total_us: Option<i64>,
    pub requested_offset_samples: i64,
    pub offset_samples: i64,
    pub beat_clock_enabled: bool,
    pub mtc_enabled: bool,
    pub beat_clock_total_us: Option<i64>,
    pub mtc_total_us: Option<i64>,
}
