//! End to end: build the test VST3 plugin in `tests/fixture` (a cdylib outside the workspace),
//! install it as a `.vst3` bundle in a temp folder, then scan → instantiate → process → params →
//! notes.

use soundcraft_dsp::{Category, Taper, Unit};

use crate::common::fixture_dir;

fn id_of(name: &str) -> String {
    let found = soundcraft_vst3_host::scan_paths(&[fixture_dir().to_path_buf()]);
    found.into_iter().find(|d| d.name == name).unwrap().id
}

#[test]
fn scan_lists_both_audio_modules_but_not_the_controller_class() {
    let dir = fixture_dir();
    let found = soundcraft_vst3_host::scan_paths(&[dir.to_path_buf()]);
    assert_eq!(found.len(), 8, "{found:?}");
    let g = found.iter().find(|d| d.name == "Fixture Gain").unwrap();
    // `uid()` lays out the first 8 bytes differently on Windows (COM GUID order).
    let want = if cfg!(windows) { "vst3:001D0A5C111122223333444455556666" } else { "vst3:5C0A1D00111122223333444455556666" };
    assert_eq!(g.id, want);
    assert_eq!(g.vendor, "SoundCraft tests", "factory vendor fills an empty class vendor");
    assert_eq!(g.version, "1.2.3");
    assert_eq!(g.sdk_version, "VST 3.8.0");
    assert_eq!(g.sub_categories, vec!["Fx"]);
    assert_eq!(g.category, Category::Other);
    assert!(!g.is_instrument);
    assert!(g.path.ends_with("Fixture.vst3"));
    let s = found.iter().find(|d| d.name == "Fixture Synth").unwrap();
    assert_eq!(s.category, Category::Instrument);
    assert!(s.is_instrument);
    let global = soundcraft_vst3_host::scan();
    assert!(global.iter().any(|d| d.id == g.id) && global.iter().any(|d| d.id == s.id));
}

#[test]
fn info_exposes_visible_params_with_plain_ranges() {
    let gain = id_of("Fixture Gain");
    let info = soundcraft_vst3_host::plugin_info(&gain).unwrap();
    assert_eq!(info.id, gain);
    assert_eq!(info.short_name, "Fixture ");
    let ids: Vec<&str> = info.params.iter().map(|p| p.id).collect();
    assert_eq!(ids, vec!["7", "3", "12", "13", "14"], "hidden 9 and bypass 11 are skipped");
    let g = info.param("7").unwrap();
    assert_eq!((g.name, g.min, g.max, g.default, g.unit, g.taper), ("Gain", 0.0, 2.0, 1.0, Unit::Db, Taper::Linear));
    let mode = info.param("3").unwrap();
    assert_eq!((mode.unit, mode.min, mode.max), (Unit::Choice, 0.0, 2.0));
    assert_eq!(mode.choices, &["A", "B", "C"]);
    let f = info.param("12").unwrap();
    assert_eq!((f.unit, f.taper), (Unit::Hz, Taper::Log));
    assert!((f.min - 20.0).abs() < 1e-3 && (f.max - 20_000.0).abs() < 1e-1, "{f:?}");
    // Cached: the same leaked info every time.
    assert!(std::ptr::eq(info, soundcraft_vst3_host::plugin_info(&gain).unwrap()));
}

/// #599: a plugin whose `normalizedParamToPlain` is the identity (JUCE, the SDK's default
/// `Parameter`) gives SoundCraft a 0..1 range; its real values are only in its own text, which the
/// editor handle reads from the controller on the main thread only (see `editor_main_thread`).
#[test]
fn identity_mapped_param_text_is_read_on_the_main_thread_only() {
    let id = id_of("Fixture Gain");
    let level = soundcraft_vst3_host::plugin_info(&id).unwrap().param("13").unwrap();
    assert_eq!((level.min, level.max, level.unit), (0.0, 1.0, Unit::Db));
    let mut p = soundcraft_vst3_host::instantiate_plugin(&id).unwrap();
    let mut ed = soundcraft_dsp::Plugin::editor(&mut p).expect("editor handle");
    if cfg!(any(target_os = "macos", windows, unix)) {
        assert_eq!(ed.value_text("13", 0.375), None, "test threads are not the main thread");
    } else {
        assert_eq!(ed.value_text("13", 0.375).as_deref(), Some("\u{2212}6.0 dB"));
    }
}

#[test]
fn gain_processes_audio_and_follows_params() {
    let mut p = soundcraft_vst3_host::create(&id_of("Fixture Gain")).unwrap();
    p.prepare(44_100.0, 256, 2);
    assert_eq!(p.latency(), 3);
    // The controller was synced from the component state (gain 0.5), not the declared default.
    assert_eq!(p.param("7"), Some(0.5));
    let mut io = vec![vec![0.5f32; 256], vec![-0.25f32; 256]];
    p.process(&mut io, 256);
    assert!(io[0].iter().all(|&x| (x - 0.25).abs() < 1e-6), "{:?}", &io[0][..4]);
    assert!(p.set_param("7", 2.0));
    assert!(!p.set_param("9", 1.0), "hidden params are not exposed");
    assert!(!p.set_param("Gain", 1.0), "ids are VST3 ParamIDs");
    assert!(p.set_param("7", 5.0), "clamped");
    assert_eq!(p.param("7"), Some(2.0));
    assert!(p.set_param("12", 2000.0));
    assert!((p.param("12").unwrap() - 2000.0).abs() < 1e-2);
    assert!(p.set_param("3", 1.6));
    assert_eq!(p.param("3"), Some(2.0), "choices round");
    // A block larger than max_block is chunked.
    let mut io = vec![vec![0.5f32; 700], vec![-0.25f32; 700]];
    p.process(&mut io, 700);
    assert!(io[0].iter().all(|&x| (x - 1.0).abs() < 1e-6));
    assert!(io[1].iter().all(|&x| (x + 0.5).abs() < 1e-6));
    // Mono strip: the plugin accepts a mono arrangement; the session value is re-sent.
    p.prepare(48_000.0, 128, 1);
    let mut mono = vec![vec![0.25f32; 128]];
    p.process(&mut mono, 128);
    assert!(mono[0].iter().all(|&x| (x - 0.5).abs() < 1e-6), "{:?}", &mono[0][..4]);
    // Hostile shapes don't crash.
    p.process(&mut [], 64);
    let mut short = vec![vec![1.0f32; 4]];
    p.process(&mut short, 1_000_000);
    assert!(p.set_param("7", f32::NAN), "NaN becomes the default");
    assert_eq!(p.param("7"), Some(1.0));
    p.reset();
    p.prepare(f32::NAN, 0, 0);
    let mut io = vec![vec![0.1f32; 8]];
    p.process(&mut io, 8);
}

#[test]
fn synth_plays_notes_at_their_offsets() {
    let id = id_of("Fixture Synth");
    let info = soundcraft_vst3_host::plugin_info(&id).unwrap();
    assert!(info.is_instrument);
    assert!(info.params.is_empty());
    let mut p = soundcraft_vst3_host::create(&id).unwrap();
    p.prepare(48_000.0, 64, 2);
    let mut io = vec![vec![9.0f32; 64], vec![9.0f32; 64]];
    p.note_on(10, 60, 127);
    p.note_off(40, 60);
    p.process(&mut io, 64);
    for (i, &x) in io[0].iter().enumerate() {
        let want = if (10..40).contains(&i) { 1.0 } else { 0.0 };
        assert!((x - want).abs() < 1e-6, "frame {i}: {x}");
    }
    assert_eq!(io[0], io[1]);
    // Offsets past the block are clamped into it; all_notes_off releases held notes.
    p.note_on(1000, 61, 64);
    let mut io = vec![vec![0.0f32; 64], vec![0.0f32; 64]];
    p.process(&mut io, 64);
    assert!(io[0][62].abs() < 1e-6 && (io[0][63] - 64.0 / 127.0).abs() < 1e-6);
    p.all_notes_off();
    p.process(&mut io, 64);
    assert!(io[0].iter().all(|x| x.abs() < 1e-6));
    // MIDI CC 7 reaches the parameter the plugin maps it to, and poly pressure arrives as an event,
    // each at its offset; an unmapped controller is dropped.
    p.midi(10, [0xA0, 60, 32]);
    p.midi(20, [0xB0, 7, 64]);
    p.midi(30, [0xB0, 1, 127]);
    p.process(&mut io, 64);
    assert!(io[0][9].abs() < 1e-6 && (io[0][10] - 32.0 / 127.0).abs() < 1e-6);
    assert!((io[0][20] - 64.0 / 127.0).abs() < 1e-6 && (io[0][63] - 64.0 / 127.0).abs() < 1e-6);
    p.midi(0, [0xB0, 7, 0]);
    p.process(&mut io, 64);
    // Notes in a later sub-block of a chunked call land at the right frame.
    p.prepare(48_000.0, 32, 2);
    let mut io = vec![vec![0.0f32; 96], vec![0.0f32; 96]];
    p.note_on(50, 62, 127);
    p.process(&mut io, 96);
    assert!(io[0][49].abs() < 1e-6 && (io[0][50] - 1.0).abs() < 1e-6 && (io[0][95] - 1.0).abs() < 1e-6);
    // A mono strip downmixes the stereo output.
    p.prepare(48_000.0, 64, 1);
    let mut mono = vec![vec![0.0f32; 64]];
    p.process(&mut mono, 64);
    assert!(mono[0].iter().all(|&x| (x - 1.0).abs() < 1e-6), "the note is still held");
}

/// Id of the CC Synth's channel 1, controller `n` (see the fixture).
fn cc_param(n: u32) -> String {
    (1_000_000 + n).to_string()
}

/// A JUCE plugin plays every change to a MIDI controller stand-in as that controller. Preparing
/// must not re-send the untouched ones: CC 121 ("reset all controllers") at offset 0 would wipe
/// a value the host sends in the same block (MODO BASS 2 lost its CC 5 bend this way).
#[test]
fn prepare_does_not_play_untouched_midi_controller_params() {
    let id = id_of("Fixture CC Synth");
    let info = soundcraft_vst3_host::plugin_info(&id).unwrap();
    assert_eq!(info.params.len(), 16 * 130);
    let block = |p: &mut Box<dyn soundcraft_dsp::Plugin>| {
        let mut io = vec![vec![9.0f32; 64], vec![9.0f32; 64]];
        p.process(&mut io, 64);
        io[0][63]
    };

    // Automation: one value right after prepare (the mixer sends a steady value only once).
    let mut p = soundcraft_vst3_host::create(&id).unwrap();
    p.prepare(48_000.0, 64, 2);
    assert!(p.set_param(&cc_param(5), 1.0));
    assert!((block(&mut p) - 1.0).abs() < 1e-6, "CC 5 from automation was reset");
    assert!((block(&mut p) - 1.0).abs() < 1e-6);
    // A value the host set is part of the session: it comes back after preparing again. The
    // synth keeps its controllers across activation, so move it off that value first.
    p.midi(0, [0xB0, 5, 0]);
    assert!(block(&mut p).abs() < 1e-6, "CC 5 from a clip was not applied");
    p.prepare(48_000.0, 64, 2);
    assert!((block(&mut p) - 1.0).abs() < 1e-6, "CC 5 not re-sent on re-prepare");
    // Also when the host set it back to the default (0) and prepare dropped that waiting change.
    assert!(p.set_param(&cc_param(5), 0.0));
    p.prepare(48_000.0, 64, 2);
    assert!(block(&mut p).abs() < 1e-6, "CC 5 set to its default was lost on re-prepare");

    // A clip's controller message in the first block after prepare.
    let mut p = soundcraft_vst3_host::create(&id).unwrap();
    p.prepare(48_000.0, 64, 2);
    p.midi(0, [0xB0, 5, 64]);
    assert!((block(&mut p) - 64.0 / 127.0).abs() < 1e-6, "CC 5 from a clip was reset");
}

/// More values than one block's parameter queues hold (the resend on prepare alone is 1500) all
/// reach the plugin over the next blocks. Before, the queue filled up on prepare and a value the
/// host set in that block was dropped for good: the mixer sends a steady value only once.
#[test]
fn values_past_one_blocks_queues_are_sent_in_the_next_block() {
    let id = id_of("Fixture Many Params");
    assert_eq!(soundcraft_vst3_host::plugin_info(&id).unwrap().params.len(), 1500);
    let mut p = soundcraft_vst3_host::create(&id).unwrap();
    p.prepare(48_000.0, 64, 2);
    assert!(p.set_param("1499", 1.0));
    let mut io = vec![vec![0.0f32; 64], vec![0.0f32; 64]];
    p.process(&mut io, 64);
    assert!(io[1][63] >= 1000.0, "the first block carries a full set of queues: {}", io[1][63]);
    p.process(&mut io, 64);
    assert_eq!(io[1][63], 1500.0, "every parameter was re-sent");
    assert!((io[0][63] - 1.0).abs() < 1e-6, "the host's value for the last parameter: {}", io[0][63]);
    // Nothing waits any more: a later change goes out in the very next block.
    assert!(p.set_param("1499", 0.25));
    p.process(&mut io, 64);
    assert!((io[0][63] - 0.25).abs() < 1e-6);
}

/// Dense controller input in every block (1024 points for one controller) still leaves room for
/// waiting values: one controller needs one parameter queue, not one per point.
#[test]
fn dense_controller_input_leaves_room_for_waiting_values() {
    let id = id_of("Fixture Many Params");
    let mut p = soundcraft_vst3_host::create(&id).unwrap();
    p.prepare(48_000.0, 64, 2);
    let mut io = vec![vec![0.0f32; 64], vec![0.0f32; 64]];
    for _ in 0..3 {
        for i in 0..1024 {
            p.midi(i % 64, [0xB0, 5, 64]);
        }
        p.process(&mut io, 64);
    }
    assert_eq!(io[1][63], 1500.0, "the resend got through next to the controller points");
}

/// A controller message is newer than a value still waiting for the same parameter, so that
/// value must not go out in a later block and undo it.
#[test]
fn a_waiting_value_does_not_undo_a_newer_controller_message() {
    let id = id_of("Fixture Many Params");
    let mut p = soundcraft_vst3_host::create(&id).unwrap();
    p.prepare(48_000.0, 64, 2);
    // The last parameter waits behind the resend; CC 5 drives it and comes after.
    assert!(p.set_param("1499", 1.0));
    p.midi(0, [0xB0, 5, 32]);
    let mut io = vec![vec![0.0f32; 64], vec![0.0f32; 64]];
    p.process(&mut io, 64);
    p.process(&mut io, 64);
    assert_eq!(io[1][63], 1500.0);
    assert!((io[0][63] - 32.0 / 127.0).abs() < 1e-6, "the older waiting value won: {}", io[0][63]);
}

/// A controller point supersedes a waiting value only from its own time on: in a block of two
/// sub-blocks, the host's value still reaches the plugin in the first, before the point in the
/// second.
#[test]
fn a_waiting_value_still_applies_before_a_later_controller_point() {
    let id = id_of("Fixture Many Params");
    let mut p = soundcraft_vst3_host::create(&id).unwrap();
    p.prepare(48_000.0, 64, 2);
    let mut io = vec![vec![0.0f32; 128], vec![0.0f32; 128]];
    p.process(&mut io, 128);
    assert_eq!(io[1][127], 1499.0, "the resend is through (all but the untouched CC 5 stand-in)");
    assert!(p.set_param("1499", 1.0));
    p.midi(100, [0xB0, 5, 32]);
    p.process(&mut io, 128);
    assert!((io[0][0] - 1.0).abs() < 1e-6, "the host's value was lost before the controller: {}", io[0][0]);
    assert!((io[0][127] - 32.0 / 127.0).abs() < 1e-6, "the controller did not win: {}", io[0][127]);
}

/// A controller point older than a waiting value does not retire it: only the later point that
/// supersedes it does, once its own sub-block is processed.
#[test]
fn an_older_controller_point_does_not_retire_a_value_a_later_one_supersedes() {
    let id = id_of("Fixture Many Params");
    let mut p = soundcraft_vst3_host::create(&id).unwrap();
    p.prepare(48_000.0, 64, 2);
    // Right after prepare the resend fills the first sub-block, but parameter 1499 has a
    // controller point there, so its waiting value goes out in that point's queue. At sample 0
    // the host value is the later call and wins.
    p.midi(0, [0xB0, 5, 32]);
    assert!(p.set_param("1499", 1.0));
    p.midi(164, [0xB0, 5, 64]);
    let mut io = vec![vec![0.0f32; 192], vec![0.0f32; 192]];
    p.process(&mut io, 192);
    assert!((io[0][63] - 1.0).abs() < 1e-6, "the host's value, the later call at sample 0: {}", io[0][63]);
    assert!((io[0][127] - 1.0).abs() < 1e-6, "the host's value was retired by an older point: {}", io[0][127]);
    assert!((io[0][191] - 64.0 / 127.0).abs() < 1e-6, "the later controller did not win: {}", io[0][191]);
}

/// A controller point past the block's end is processed at its last frame, and so is the
/// retirement of the waiting value it supersedes: the next block must not send that value.
#[test]
fn a_controller_point_clamped_to_the_block_end_still_retires_the_value_it_supersedes() {
    let id = id_of("Fixture Many Params");
    let mut p = soundcraft_vst3_host::create(&id).unwrap();
    p.prepare(48_000.0, 64, 2);
    // Right after prepare the resend fills the first sub-block, so parameter 1499 waits.
    assert!(p.set_param("1499", 1.0));
    p.midi(64, [0xB0, 5, 32]);
    let mut io = vec![vec![0.0f32; 64], vec![0.0f32; 64]];
    p.process(&mut io, 64);
    assert!((io[0][63] - 32.0 / 127.0).abs() < 1e-6, "the clamped controller point: {}", io[0][63]);
    p.process(&mut io, 64);
    assert!((io[0][63] - 32.0 / 127.0).abs() < 1e-6, "the older host value undid the controller: {}", io[0][63]);
}

/// Fixture Many Params after the prepare resend has gone out, so parameter 1499 waits for nothing.
fn drained_many_params() -> (Box<dyn soundcraft_dsp::Plugin>, Vec<Vec<f32>>) {
    let mut p = soundcraft_vst3_host::create(&id_of("Fixture Many Params")).unwrap();
    p.prepare(48_000.0, 64, 2);
    let mut io = vec![vec![0.0f32; 64], vec![0.0f32; 64]];
    p.process(&mut io, 64);
    p.process(&mut io, 64);
    assert_eq!(io[1][63], 1499.0, "the resend is through (all but the untouched CC 5 stand-in)");
    (p, io)
}

/// A host value and a controller point for one parameter at the same sample: the later call wins,
/// whichever of the two it is.
#[test]
fn at_a_shared_sample_the_later_of_host_value_and_controller_wins() {
    let cc = 32.0 / 127.0;
    let (mut p, mut io) = drained_many_params();
    p.midi(0, [0xB0, 5, 32]);
    assert!(p.set_param("1499", 1.0));
    p.process(&mut io, 64);
    assert!((io[0][0] - 1.0).abs() < 1e-6, "the newer host value lost to an older controller point: {}", io[0][0]);
    p.process(&mut io, 64);
    assert!((io[0][63] - 1.0).abs() < 1e-6, "the host value did not stay: {}", io[0][63]);

    let (mut p, mut io) = drained_many_params();
    assert!(p.set_param("1499", 1.0));
    p.midi(0, [0xB0, 5, 32]);
    p.process(&mut io, 64);
    assert!((io[0][0] - cc).abs() < 1e-6, "the newer controller point lost to an older host value: {}", io[0][0]);
    p.process(&mut io, 64);
    assert!((io[0][63] - cc).abs() < 1e-6, "the older host value came back: {}", io[0][63]);
}

/// A host value at a sub-block's start and a later controller point in the same sub-block both
/// reach the processor, each at its own offset.
#[test]
fn a_host_value_and_a_later_controller_point_in_one_block_both_arrive() {
    let cc = 32.0 / 127.0;
    let (mut p, mut io) = drained_many_params();
    assert!(p.set_param("1499", 1.0));
    // Past the block's end: processed at its last frame.
    p.midi(64, [0xB0, 5, 32]);
    p.process(&mut io, 64);
    assert!((io[0][0] - 1.0).abs() < 1e-6, "the host value was replaced by the controller point: {}", io[0][0]);
    assert!((io[0][62] - 1.0).abs() < 1e-6, "the controller point came early: {}", io[0][62]);
    assert!((io[0][63] - cc).abs() < 1e-6, "the controller point is missing: {}", io[0][63]);
    p.process(&mut io, 64);
    assert!((io[0][63] - cc).abs() < 1e-6, "the older host value came back: {}", io[0][63]);
}

/// `reset` drops controller points that were never processed, so a value they would have
/// superseded is still the one to send.
#[test]
fn reset_keeps_a_waiting_value_whose_controller_point_it_dropped() {
    let id = id_of("Fixture Many Params");
    let mut p = soundcraft_vst3_host::create(&id).unwrap();
    p.prepare(48_000.0, 64, 2);
    let mut io = vec![vec![0.0f32; 64], vec![0.0f32; 64]];
    p.process(&mut io, 64);
    p.process(&mut io, 64);
    assert_eq!(io[1][63], 1499.0, "the resend is through (all but the untouched CC 5 stand-in)");
    assert!(p.set_param("1499", 1.0));
    p.midi(0, [0xB0, 5, 32]);
    p.reset();
    p.process(&mut io, 64);
    assert!((io[0][63] - 1.0).abs() < 1e-6, "the host's value was lost with the dropped controller: {}", io[0][63]);
}

#[test]
fn reset_after_all_notes_off_still_releases_held_voices() {
    let mut p = soundcraft_vst3_host::create(&id_of("Fixture Synth")).unwrap();
    p.prepare(48_000.0, 64, 2);
    p.note_on(0, 60, 127);
    let mut io = vec![vec![0.0f32; 64], vec![0.0f32; 64]];
    p.process(&mut io, 64);
    assert!((io[0][63] - 1.0).abs() < 1e-6, "the note sounds");
    // Mixer::reset calls these two in a row on an instrument (stop / seek).
    p.all_notes_off();
    p.reset();
    let mut io = vec![vec![9.0f32; 64], vec![9.0f32; 64]];
    p.process(&mut io, 64);
    assert!(io[0].iter().all(|x| x.abs() < 1e-6), "voice still sounding after reset: {}", io[0][63]);
    // A reset alone also releases what the host still holds.
    p.note_on(0, 61, 127);
    p.process(&mut io, 64);
    assert!((io[0][63] - 1.0).abs() < 1e-6);
    p.reset();
    p.process(&mut io, 64);
    assert!(io[0].iter().all(|x| x.abs() < 1e-6), "voice still sounding after reset: {}", io[0][63]);
}

#[test]
fn many_instances_create_and_drop_cleanly() {
    let (g, s) = (id_of("Fixture Gain"), id_of("Fixture Synth"));
    for _ in 0..50 {
        let mut a = soundcraft_vst3_host::create(&g).unwrap();
        let b = soundcraft_vst3_host::create(&s).unwrap();
        let mut io = vec![vec![0.0f32; 32]; 2];
        a.process(&mut io, 32);
        drop(b);
    }
}

/// Smoke test against whatever real VST3 plugins are installed (none are required): scan, then
/// instantiate, prepare and process each one briefly. Set `SOUNDCRAFT_VST3_SMOKE=1` to run it.
#[test]
fn installed_plugins_smoke() {
    if std::env::var_os("SOUNDCRAFT_VST3_SMOKE").is_none() {
        return;
    }
    for d in soundcraft_vst3_host::scan() {
        eprintln!("found {} ({}) {:?} [{}] at {}", d.name, d.vendor, d.category, d.sub_categories.join("|"), d.path.display());
        let info = soundcraft_vst3_host::plugin_info(&d.id);
        eprintln!(
            "  info: {:?}",
            info.map(|i| (i.params.len(), i.params.iter().take(5).map(|p| (p.id, p.name, p.min, p.max, p.unit)).collect::<Vec<_>>()))
        );
        match soundcraft_vst3_host::instantiate(&d.id) {
            Ok(mut p) => {
                p.prepare(48_000.0, 512, 2);
                let mut io = vec![vec![0.1f32; 512], vec![0.1f32; 512]];
                p.process(&mut io, 512);
                eprintln!("  processed; latency {} out[0] {}", p.latency(), io[0][0]);
            }
            Err(e) => eprintln!("  instantiate: {e}"),
        }
    }
}

#[test]
fn state_saves_component_and_controller_and_restores_them() {
    let id = id_of("Fixture Gain");
    let mut p = soundcraft_vst3_host::create(&id).unwrap();
    p.prepare(48_000.0, 64, 2);
    assert!(p.set_param("7", 1.5));
    let mut io = vec![vec![0.5f32; 64]; 2];
    p.process(&mut io, 64);
    let blob = p.save_state().expect("component state");
    assert!(blob.starts_with(b"SCV3"));
    let mut q = soundcraft_vst3_host::create(&id).unwrap();
    q.prepare(48_000.0, 64, 2);
    assert_eq!(q.param("7"), Some(0.5));
    assert!(q.load_state(&blob));
    assert_eq!(q.param("7"), Some(1.5), "controller synced from the restored component state");
    let mut io = vec![vec![0.5f32; 64]; 2];
    q.process(&mut io, 64);
    assert!(io[0].iter().all(|&x| (x - 0.75).abs() < 1e-6), "{:?}", &io[0][..4]);
    // The controller part (Freq) round-trips too: write one by hand.
    let mut custom = b"SCV3".to_vec();
    custom.extend_from_slice(&8u32.to_le_bytes());
    custom.extend_from_slice(&0.25f64.to_le_bytes());
    custom.extend_from_slice(&8u32.to_le_bytes());
    custom.extend_from_slice(&1.0f64.to_le_bytes());
    assert!(q.load_state(&custom));
    assert_eq!(q.param("7"), Some(0.5));
    assert!((q.param("12").unwrap() - 20_000.0).abs() < 1.0, "{:?}", q.param("12"));
    // A bare component state is accepted; hostile blobs are refused.
    assert!(q.load_state(&0.5f64.to_le_bytes()));
    assert_eq!(q.param("7"), Some(1.0));
    for bad in [&b""[..], b"SCV3", b"SCV3\xff\xff\xff\xff", &[0xffu8; 8][..], b"abc"] {
        assert!(!q.load_state(bad), "{bad:?}");
    }
    assert_eq!(q.param("7"), Some(1.0));
}

#[test]
fn state_reader_reads_while_another_thread_processes() {
    let id = id_of("Fixture Gain");
    let mut p = soundcraft_vst3_host::create(&id).unwrap();
    p.prepare(48_000.0, 64, 2);
    assert!(p.set_param("7", 1.5));
    p.process(&mut vec![vec![0.5f32; 64]; 2], 64);
    let mut reader = p.state_reader().expect("every VST3 component has a state");
    let blob = p.save_state().unwrap();
    let audio = std::thread::spawn(move || {
        for _ in 0..2_000 {
            p.process(&mut vec![vec![0.5f32; 64]; 2], 64);
        }
        p
    });
    while !audio.is_finished() {
        assert_eq!(reader.save_state().as_ref(), Some(&blob));
    }
    let p = audio.join().unwrap();
    assert_eq!(reader.save_state(), Some(blob));
    drop(p);
    assert_eq!(reader.save_state(), None);
}

#[test]
fn host_set_controller_param_survives_state_round_trip() {
    // Freq lives only in the controller (the processor ignores it), so processing never carries
    // the host's value there: the state save has to.
    let id = id_of("Fixture Gain");
    let mut p = soundcraft_vst3_host::create(&id).unwrap();
    p.prepare(48_000.0, 64, 2);
    assert!(p.set_param("7", 1.5));
    assert!(p.set_param("12", 200.0));
    let mut p = std::thread::spawn(move || {
        let mut io = vec![vec![0.5f32; 64]; 2];
        p.process(&mut io, 64);
        assert!(io.iter().flatten().all(|&x| (x - 0.75).abs() < 1e-4));
        p
    })
    .join()
    .unwrap();
    assert!((p.param("12").unwrap() - 200.0).abs() < 0.01);
    let blob = p.save_state().expect("state");
    let mut q = soundcraft_vst3_host::create(&id).unwrap();
    assert!(q.load_state(&blob));
    assert!((q.param("7").unwrap() - 1.5).abs() < 1e-4, "{:?}", q.param("7"));
    assert!((q.param("12").unwrap() - 200.0).abs() < 0.01, "{:?}", q.param("12"));
    assert!(q.set_param("12", 300.0));
    assert!(q.load_state(&blob));
    let mut r = soundcraft_vst3_host::create(&id).unwrap();
    assert!(r.load_state(&q.save_state().unwrap()));
    assert!((r.param("12").unwrap() - 200.0).abs() < 0.01, "a loaded state replaces earlier edits: {:?}", r.param("12"));
}

#[test]
fn layouts_beyond_the_host_limits_are_rejected_not_truncated() {
    // Control: exactly at the limits (16 buses of 64 channels) activates and gets its full layout.
    let mut p = soundcraft_vst3_host::create(&id_of("Fixture Wide 16x64")).unwrap();
    p.prepare(44_100.0, 64, 2);
    assert_eq!(p.latency(), 3);
    let mut io = vec![vec![0.5f32; 64]; 2];
    p.process(&mut io, 64);
    assert!(io.iter().flatten().all(|&x| x == 1.0), "16x64 layout delivered as advertised: {:?}", &io[0][..4]);
    // 65 buses, or a 65-channel bus: never activated, never processed with a cut-down layout.
    for name in ["Fixture Wide 65 buses", "Fixture Wide 65 channels"] {
        let mut p = soundcraft_vst3_host::create(&id_of(name)).unwrap();
        p.prepare(44_100.0, 64, 2);
        assert_eq!(p.latency(), 0, "{name}: not activated");
        let mut io = vec![vec![0.5f32; 64]; 2];
        p.process(&mut io, 64);
        assert!(io.iter().flatten().all(|&x| x == 0.5), "{name}: the plugin never ran: {:?}", &io[0][..4]);
    }
}

#[test]
fn offline_renders_set_up_and_process_in_offline_mode() {
    for (offline, want) in [(false, 1.0), (true, 2.0)] {
        let mut p = soundcraft_vst3_host::create(&id_of("Fixture Wide 16x64")).unwrap();
        p.set_offline(offline);
        p.prepare(44_100.0, 64, 2);
        let mut io = vec![vec![0.5f32; 64]; 2];
        p.process(&mut io, 64);
        assert!(io.iter().flatten().all(|&x| x == want), "offline {offline}: {:?}", &io[0][..4]);
    }
}

#[test]
fn many_buses_like_multi_out_instruments_are_activated() {
    // Multi-out instruments declare dozens of buses (SSD Sampler 5: 32); only the main one is
    // active, but the plugin must still load, get buffers for every bus and play (#1247).
    let mut p = soundcraft_vst3_host::create(&id_of("Fixture Wide 32 buses")).unwrap();
    p.prepare(44_100.0, 64, 2);
    assert_eq!(p.latency(), 3, "32-bus plugin activated");
    let mut io = vec![vec![0.5f32; 64]; 2];
    p.process(&mut io, 64);
    assert!(io.iter().flatten().all(|&x| x == 1.0), "32-bus layout delivered as advertised: {:?}", &io[0][..4]);
}

#[test]
fn editor_view_is_probed_and_edits_come_back_as_plain_values() {
    let id = id_of("Fixture Gain");
    let mut p = soundcraft_vst3_host::instantiate_plugin(&id).unwrap();
    // Editor windows are hosted on macOS, Windows and X11 (Linux, BSDs); elsewhere the probe reports "unsupported".
    if cfg!(any(target_os = "macos", windows, unix)) {
        assert_eq!(p.editor_size(), Ok((320, 240)));
    } else {
        assert!(p.editor_size().is_err());
    }
    let mut ed = soundcraft_dsp::Plugin::editor(&mut p).expect("editor handle");
    // The fixture "edits" Gain to 0.9 (normalized) when it gets the component handler.
    assert_eq!(ed.idle(), vec![("7".to_string(), 1.8)]);
    assert!(ed.idle().is_empty());
    if cfg!(any(target_os = "macos", windows, unix)) {
        assert!(ed.open().unwrap_err().contains("main thread"), "test threads are not the main thread");
    } else {
        assert!(ed.open().is_err(), "unsupported elsewhere");
    }
    assert!(!ed.is_open());
    // The synth's controller has no view.
    let synth = soundcraft_vst3_host::instantiate_plugin(&id_of("Fixture Synth")).unwrap();
    assert!(synth.editor_size().is_err());
}
