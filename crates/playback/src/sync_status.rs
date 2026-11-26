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

/// Host-side delivery measurements of the shared MIDI output worker. Time values use
/// microseconds.
#[derive(Default, Serialize)]
pub struct MidiSyncStatus {
    pub sent: u64,
    pub pending_transition_timeouts: u64,
    pub oversized_dropped: u64,
    /// The message of every failing destination (for example
    /// `"MIDI output <port> unavailable"`) joined with `"; "`, or a worker startup
    /// error. `None` while every destination works.
    pub error: Option<String>,
    pub session_backlog_drops: u64,
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
    /// Live input played through to the tracks' MIDI outputs (MIDI Thru).
    pub thru: ThruStatus,
}

/// MIDI Thru to external gear: what was sent and how soon. Time values use microseconds.
#[derive(Default, Serialize)]
pub struct ThruStatus {
    /// Messages handed to an output port.
    pub sent: u64,
    /// Messages that came back on an input of the device SoundCraft had just sent them to (a
    /// MIDI loop) and were not sent on: echoes of thru (dropped) and of the clips (not thru'd).
    pub echoes_dropped: u64,
    /// From a message's arrival at the input to its hand-off to the output port, counted in the
    /// buckets of `lateness_buckets` (up to 0.5, 1, 2, 5, 10, 20 ms, and above).
    pub latency_buckets: [u64; 7],
    /// Median and 95th percentile, estimated within their bucket (as if its values were spread
    /// evenly up to the bucket's bound or `latency_max_us`, whichever is lower): never above
    /// `latency_max_us`.
    pub latency_p50_us: u64,
    pub latency_p95_us: u64,
    pub latency_max_us: u64,
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
    /// MIDI or instrument tracks play their clips to this port.
    pub notes_enabled: bool,
    pub notes_total_us: Option<i64>,
}
