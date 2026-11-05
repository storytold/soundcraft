//! Min:Secs shows exact millisecond positions exactly (no float-floor off-by-one).

use soundcraft_time::{FrameRate, SampleRate, TempoMap, TimeFormat, format_position};

fn min_secs(s: i64, sr: SampleRate) -> String {
    format_position(s, TimeFormat::MinSecs, sr, &TempoMap::default(), FrameRate::default(), 0, 0)
}

fn expected(ms_total: i64) -> String {
    format!("{}:{:02}.{:03}", ms_total / 60_000, (ms_total / 1000) % 60, ms_total % 1000)
}

#[test]
fn known_positions_at_44100() {
    assert_eq!(min_secs(88_641, SampleRate::HZ_44100), "0:02.010");
    assert_eq!(min_secs(89_523, SampleRate::HZ_44100), "0:02.030");
}

#[test]
fn negative_position_at_44100() {
    assert_eq!(min_secs(-88_641, SampleRate::HZ_44100), "-0:02.010");
}

#[test]
fn every_exact_millisecond_displays_exactly() {
    for sr in [SampleRate::HZ_44100, SampleRate::HZ_48000] {
        let hz = i64::from(sr.hz());
        for ms in 0..=600_000_i64 {
            if (ms * hz) % 1000 != 0 {
                continue;
            }
            let s = ms * hz / 1000;
            assert_eq!(min_secs(s, sr), expected(ms), "{hz} Hz, sample {s}");
        }
    }
}
