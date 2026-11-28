use super::*;
use log::Log;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, UNIX_EPOCH};

fn temp_dir(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("soundcraft-logging-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("read log")
}

fn record(logger: &AppLogger, level: log::Level, target: &'static str, msg: &str) {
    logger.log(&log::Record::builder().level(level).target(target).args(format_args!("{msg}")).build());
}

/// Run `f` on a thread the playback crate marks as the realtime audio thread.
fn on_audio_thread<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| {
        s.spawn(|| {
            soundcraft_playback::mark_audio_thread();
            f()
        })
        .join()
        .expect("audio thread")
    })
}

#[test]
fn the_default_filter_shows_soundcraft_info_and_other_crates_warnings() {
    let f = Filter::parse(DEFAULT_FILTER);
    assert_eq!(f.level_for("soundcraft"), LevelFilter::Info);
    assert_eq!(f.level_for("soundcraft::control_server"), LevelFilter::Info);
    assert_eq!(f.level_for("soundcraft_playback"), LevelFilter::Info);
    assert_eq!(f.level_for("soundcraft_clap_host::ffi"), LevelFilter::Info);
    assert_eq!(f.level_for("wgpu_core::device"), LevelFilter::Warn);
    assert_eq!(f.level_for("cpal"), LevelFilter::Warn);
    assert_eq!(f.max(), LevelFilter::Info);
}

#[test]
fn directives_follow_env_logger_and_the_most_specific_one_wins() {
    let f = Filter::parse("info,wgpu_core=error,soundcraft_mix=trace,soundcraft_mix::pdc=off");
    assert_eq!(f.level_for("eframe"), LevelFilter::Info);
    assert_eq!(f.level_for("wgpu_core::instance"), LevelFilter::Error);
    assert_eq!(f.level_for("soundcraft_mix::render"), LevelFilter::Trace);
    assert_eq!(f.level_for("soundcraft_mix::pdc"), LevelFilter::Off);
    assert_eq!(f.max(), LevelFilter::Trace);
    // A module name is matched at `::` boundaries, not as a bare prefix.
    assert_eq!(f.level_for("wgpu_core_extra"), LevelFilter::Info);
    // A trailing `*` is a prefix.
    assert_eq!(Filter::parse("warn,soundcraft_ui*=debug").level_for("soundcraft_ui_egui::mix_window"), LevelFilter::Debug);
    assert_eq!(Filter::parse("warn,soundcraft_ui*=debug").level_for("soundcraft_engine"), LevelFilter::Warn);
    // A bare target name sets that target to the most verbose level, as env_logger does.
    assert_eq!(Filter::parse("naga").level_for("naga::front"), LevelFilter::Trace);
    assert_eq!(Filter::parse("naga").level_for("eframe"), LevelFilter::Error);
    // Levels are case-insensitive; the last bare level wins.
    assert_eq!(Filter::parse("debug,WARN").level_for("x"), LevelFilter::Warn);
}

#[test]
fn hostile_specs_never_panic_and_fall_back_sensibly() {
    let long = "a".repeat(100_000);
    let specs = [
        "",
        ",,,",
        "=",
        "==",
        "=debug",
        "soundcraft=",
        "nonsense=loud",
        "🦀=info",
        "*",
        "*=debug",
        "a=b=c",
        "warn,,soundcraft=DEBUG",
        " \t ",
        "\u{0}",
        "é*=trace",
        &long,
    ];
    for spec in specs {
        let f = Filter::parse(spec);
        for target in ["soundcraft", "", "🦀", "é", "a::b"] {
            let _ = f.level_for(target);
        }
        let _ = f.max();
    }
    assert_eq!(Filter::parse("warn,,soundcraft=DEBUG").level_for("soundcraft"), LevelFilter::Debug);
    assert_eq!(Filter::parse("nonsense=loud").level_for("nonsense"), LevelFilter::Error);
    assert_eq!(Filter::parse("=debug").max(), LevelFilter::Error, "a directive without a name is skipped");
    assert_eq!(Filter::parse("*=debug").level_for("anything"), LevelFilter::Debug, "a lone `*` matches every target");
    assert_eq!(Filter::parse("é*=trace").level_for("éa"), LevelFilter::Trace);
}

#[test]
fn timestamps_are_utc_with_milliseconds() {
    assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
    assert_eq!(timestamp(UNIX_EPOCH + Duration::from_millis(1_791_446_357_728)), "2026-10-08T07:59:17.728Z");
    // Leap day, and the day after it.
    assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(1_709_164_800)), "2024-02-29T00:00:00.000Z");
    assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(1_709_251_199)), "2024-02-29T23:59:59.000Z");
    assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(1_709_251_200)), "2024-03-01T00:00:00.000Z");
    // 2000 is a leap year (divisible by 400), 2100 is not.
    assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29T00:00:00.000Z");
    assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(4_107_542_400)), "2100-03-01T00:00:00.000Z");
    // Before the epoch (a clock set wrong) clamps instead of panicking.
    assert_eq!(timestamp(UNIX_EPOCH - Duration::from_secs(5)), "1970-01-01T00:00:00.000Z");
    // Past chrono's year range still converts via the 400-year cycle (the old code did too,
    // through `civil_from_days`): never a silent reset to the epoch. Platform-safe: Windows'
    // SystemTime can't hold this instant, so it is only checked where it exists.
    // `u64::MAX / 2` seconds == `i64::MAX` seconds → year 292277026596 (old output).
    if let Some(far) = UNIX_EPOCH.checked_add(Duration::from_secs(u64::MAX / 2)) {
        assert_eq!(timestamp(far), "292277026596-12-04T15:30:07.000Z");
    }
    // Years past 9999 keep the plain zero-padded year (no `+`).
    assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(253_402_300_800)), "10000-01-01T00:00:00.000Z");
    // 10_000_000_000_000 s is far beyond chrono's range; old output is year 318857.
    // Windows' SystemTime holds only up to about year 30828, so this is checked where it fits.
    if let Some(far) = UNIX_EPOCH.checked_add(Duration::from_secs(10_000_000_000_000)) {
        assert_eq!(timestamp(far), "318857-05-20T17:46:40.000Z");
    }
}

#[test]
fn rotation_keeps_the_previous_logs_and_starts_an_empty_file() {
    let dir = temp_dir("rotate");
    for run in 1..=4 {
        let (path, _file) = rotate(&dir).expect("rotate");
        assert_eq!(path, dir.join(LOG_FILE));
        assert_eq!(read(&path), "");
        std::fs::write(&path, format!("run {run}")).expect("write");
    }
    assert_eq!(read(&dir.join(LOG_FILE)), "run 4");
    assert_eq!(read(&dir.join("soundcraft.1.log")), "run 3");
    assert_eq!(read(&dir.join("soundcraft.2.log")), "run 2");
    assert!(!dir.join("soundcraft.3.log").exists(), "only {KEEP} old logs are kept");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rotation_into_an_unusable_directory_is_an_error_not_a_panic() {
    let dir = temp_dir("blocked");
    std::fs::create_dir_all(dir.parent().expect("parent")).expect("tmp");
    std::fs::write(&dir, "a file where the directory should be").expect("block");
    assert!(rotate(&dir).is_err());
    assert!(rotate(&dir.join("below")).is_err());
    let _ = std::fs::remove_file(&dir);
}

#[test]
fn a_line_carries_time_level_thread_target_and_message() {
    assert_eq!(
        format_line("2026-10-08T07:59:17.728Z", log::Level::Warn, "main", "soundcraft_mix", "eq_7band: the plugin did not accept its stored state"),
        "2026-10-08T07:59:17.728Z WARN  [main] soundcraft_mix: eq_7band: the plugin did not accept its stored state\n"
    );
    // A multi-line message keeps its lines; the record still ends in exactly one newline.
    assert!(format_line("t", log::Level::Error, "w", "x", "a\nb\n").ends_with("x: a\nb\n"));
}

#[test]
fn lines_logged_before_the_file_exists_are_written_when_it_is_attached() {
    let dir = temp_dir("pending");
    let (path, file) = rotate(&dir).expect("rotate");
    let mut sink = Sink::new(MAX_FILE_BYTES);
    sink.write("early 1\n");
    sink.write("early 2\n");
    sink.attach(file);
    sink.write("late\n");
    assert_eq!(read(&path), "early 1\nearly 2\nlate\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_pending_buffer_is_bounded_and_says_how_much_it_dropped() {
    let dir = temp_dir("bounded");
    let (path, file) = rotate(&dir).expect("rotate");
    let mut sink = Sink::new(MAX_FILE_BYTES);
    for i in 0..MAX_PENDING + 5 {
        sink.write(&format!("line {i}\n"));
    }
    sink.attach(file);
    let text = read(&path);
    assert_eq!(text.lines().filter(|l| l.starts_with("line ")).count(), MAX_PENDING);
    assert_eq!(text.matches("5 earlier log lines were dropped").count(), 1, "{text}");
    assert!(text.contains("line 0\n") && !text.contains(&format!("line {MAX_PENDING}\n")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn exactly_max_pending_lines_drop_nothing() {
    let dir = temp_dir("exact");
    let (path, file) = rotate(&dir).expect("rotate");
    let mut sink = Sink::new(MAX_FILE_BYTES);
    for i in 0..MAX_PENDING {
        sink.write(&format!("line {i}\n"));
    }
    sink.attach(file);
    let text = read(&path);
    assert_eq!(text.lines().count(), MAX_PENDING);
    assert!(!text.contains("dropped"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_file_stops_at_its_size_cap_with_one_note() {
    let dir = temp_dir("cap");
    let (path, file) = rotate(&dir).expect("rotate");
    let mut sink = Sink::new(100);
    sink.attach(file);
    for i in 0..50 {
        sink.write(&format!("line {i:02} with some padding\n"));
    }
    let text = read(&path);
    assert_eq!(text.lines().filter(|l| l.starts_with("line ")).count(), 3, "{text}");
    assert_eq!(text.matches("log file reached 100 bytes").count(), 1, "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_logger_filters_records_and_writes_them_to_the_attached_file() {
    let dir = temp_dir("logger");
    let logger = AppLogger::new(Filter::parse(DEFAULT_FILTER), false);
    record(&logger, log::Level::Info, "soundcraft", "SoundCraft starting");
    record(&logger, log::Level::Info, "wgpu_core::device", "too chatty");
    let path = logger.attach_dir(&dir).expect("attach");
    assert_eq!(path, dir.join(LOG_FILE));
    record(&logger, log::Level::Warn, "wgpu_hal::vulkan", "a real warning");
    record(&logger, log::Level::Debug, "soundcraft", "below info");
    let text = read(&path);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(lines.first().is_some_and(|l| l.contains(" INFO  [") && l.ends_with("soundcraft: SoundCraft starting")), "{text}");
    assert!(lines.get(1).is_some_and(|l| l.ends_with("wgpu_hal::vulkan: a real warning")), "{text}");
    assert!(logger.enabled(&log::Metadata::builder().level(log::Level::Info).target("soundcraft_engine").build()));
    assert!(!logger.enabled(&log::Metadata::builder().level(log::Level::Info).target("naga").build()));
    // A second launch rotates: the first log becomes `.1`.
    let second = AppLogger::new(Filter::parse(DEFAULT_FILTER), false);
    assert_eq!(second.attach_dir(&dir).expect("attach again"), dir.join(LOG_FILE));
    assert_eq!(read(&dir.join("soundcraft.1.log")), text);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn records_from_many_threads_are_whole_lines() {
    let dir = temp_dir("threads");
    let logger = AppLogger::new(Filter::parse("info"), false);
    let path = logger.attach_dir(&dir).expect("attach");
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| {
                for _ in 0..100 {
                    record(&logger, log::Level::Info, "soundcraft", "from a worker");
                }
            });
        }
    });
    let text = read(&path);
    assert_eq!(text.lines().count(), 400);
    assert!(text.lines().all(|l| l.ends_with("soundcraft: from a worker")), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn attaching_to_an_unusable_directory_reports_an_error_and_keeps_logging() {
    let dir = temp_dir("unusable");
    std::fs::write(&dir, "not a directory").expect("block");
    let logger = AppLogger::new(Filter::parse("info"), false);
    record(&logger, log::Level::Info, "soundcraft", "kept for a file");
    let e = logger.attach_dir(&dir).expect_err("a file is not a log directory");
    assert!(e.contains(&dir.display().to_string()), "{e}");
    record(&logger, log::Level::Error, "x", "still fine");
    let sink = logger.sink.lock().expect("sink");
    assert!(sink.file.is_none() && sink.pending.is_empty(), "nothing is kept for a file that will not come");
    drop(sink);
    let _ = std::fs::remove_file(&dir);
}

#[test]
fn a_run_without_a_log_file_keeps_nothing_for_one() {
    let logger = AppLogger::new(Filter::parse("info"), false);
    record(&logger, log::Level::Info, "soundcraft", "early");
    logger.no_file();
    record(&logger, log::Level::Info, "soundcraft", "late");
    let sink = logger.sink.lock().expect("sink");
    assert!(sink.file.is_none() && sink.pending.is_empty() && sink.dropped == 0);
}

#[test]
fn a_poisoned_lock_does_not_stop_logging() {
    let dir = temp_dir("poison");
    let logger = AppLogger::new(Filter::parse("info"), false);
    let path = logger.attach_dir(&dir).expect("attach");
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = logger.sink.lock();
        panic!("poison the sink");
    }));
    assert!(logger.sink.is_poisoned());
    record(&logger, log::Level::Info, "soundcraft", "after the panic");
    assert!(read(&path).contains("after the panic"));
    let _ = std::fs::remove_dir_all(&dir);
}

// Realtime audio: the audio thread never takes the sink lock or touches the file.

#[test]
fn an_audio_thread_record_never_waits_for_the_sink() {
    let dir = temp_dir("rt-nowait");
    let logger = AppLogger::new(Filter::parse("info"), false);
    let path = logger.attach_dir(&dir).expect("attach");
    // Another thread holds the sink (a slow disk, say) while the audio thread logs.
    let held = logger.sink.lock().expect("sink");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|s| {
        s.spawn(|| {
            soundcraft_playback::mark_audio_thread();
            record(&logger, log::Level::Warn, "soundcraft_playback", "audio stream error: device unplugged");
            let _ = tx.send(());
        });
        let done = rx.recv_timeout(Duration::from_secs(10)).is_ok();
        drop(held);
        assert!(done, "the audio thread blocked on the sink lock");
    });
    assert_eq!(read(&path), "", "nothing is written from the audio thread itself");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_first_audio_thread_record_is_written_later_with_a_count_of_the_rest() {
    let dir = temp_dir("rt-report");
    let logger = AppLogger::new(Filter::parse(DEFAULT_FILTER), false);
    let path = logger.attach_dir(&dir).expect("attach");
    on_audio_thread(|| {
        record(&logger, log::Level::Warn, "soundcraft_playback", "audio stream error: device unplugged");
        for _ in 0..41 {
            record(&logger, log::Level::Warn, "soundcraft_playback", "audio stream error: still unplugged");
        }
        // Filtered out: neither kept nor counted.
        record(&logger, log::Level::Debug, "soundcraft_clap_host::plugin", "process failed");
    });
    assert_eq!(read(&path), "");
    logger.report_audio_thread();
    let text = read(&path);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(lines.first().is_some_and(|l| l.contains(" WARN  [audio] soundcraft_playback: audio stream error: device unplugged")), "{text}");
    assert!(lines.get(1).is_some_and(|l| l.contains("41 more log records from the realtime audio thread were not written")), "{text}");
    // Reported once.
    logger.report_audio_thread();
    assert_eq!(read(&path), text);
    // The next record from any other thread reports what the audio thread kept since, first.
    on_audio_thread(|| record(&logger, log::Level::Error, "soundcraft_clap_host::ffi", "clap plugin: out of voices"));
    record(&logger, log::Level::Info, "soundcraft", "UI thread");
    let text = read(&path);
    let tail: Vec<&str> = text.lines().skip(2).collect();
    assert_eq!(tail.len(), 2, "{text}");
    assert!(tail.first().is_some_and(|l| l.contains(" ERROR [audio] soundcraft_clap_host::ffi: clap plugin: out of voices")), "{text}");
    assert!(tail.get(1).is_some_and(|l| l.ends_with("soundcraft: UI thread")), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_long_audio_thread_record_is_cut_at_a_character_boundary() {
    let dir = temp_dir("rt-long");
    let logger = AppLogger::new(Filter::parse("info"), false);
    let path = logger.attach_dir(&dir).expect("attach");
    let long = "é".repeat(REALTIME_MESSAGE_BYTES);
    on_audio_thread(|| record(&logger, log::Level::Warn, "soundcraft", &long));
    logger.report_audio_thread();
    let text = read(&path);
    let message = text.split("soundcraft: ").nth(1).expect("message");
    assert!(message.ends_with("…\n"), "{text}");
    assert!(message.len() <= REALTIME_MESSAGE_BYTES + "…\n".len(), "{}", message.len());
    assert!(message.trim_end_matches(['…', '\n']).chars().all(|c| c == 'é'));
    let _ = std::fs::remove_dir_all(&dir);
}
