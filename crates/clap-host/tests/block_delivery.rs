//! Original loaded-processor evidence for explicit whole-block delivery. No installed plugins.
use soundcraft_dsp::{
    BlockDeliveryCapacity as Capacity, BlockDeliveryError as Error, BlockNote as Note, BlockNoteMessage as Message, BlockProcessStatus as Status,
    BlockTarget as Target, Plugin,
};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

const ID: &str = "clap:org.soundcraft.test.block-delivery";
const FULL: Capacity = Capacity { targets: 4096, notes: 1024 };

fn fixture() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("block-delivery-fixture-target");
        let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args(["build", "--quiet", "--locked", "--manifest-path"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/block_delivery_fixture/Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .env_remove("RUSTFLAGS")
            .status()
            .unwrap();
        assert!(status.success(), "original block-delivery fixture builds");
        let lib = if cfg!(target_os = "macos") {
            "libblock_delivery_fixture.dylib"
        } else if cfg!(windows) {
            "block_delivery_fixture.dll"
        } else {
            "libblock_delivery_fixture.so"
        };
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("block-delivery-install-{}", std::process::id()));
        let dest =
            if cfg!(target_os = "macos") { dir.join("BlockDelivery.clap/Contents/MacOS/BlockDelivery") } else { dir.join("BlockDelivery.clap") };
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(target.join("debug").join(lib), &dest).unwrap();
        assert_eq!(soundcraft_clap_host::add_search_dir(&dir), 1);
        dir
    })
}
fn plugin() -> Box<dyn Plugin> {
    fixture();
    let mut p = soundcraft_clap_host::create(ID).unwrap();
    p.prepare(48_000.0, 256, 2);
    p.prepare_block_delivery(FULL).unwrap();
    p
}
#[derive(Debug, PartialEq)]
struct Receipt {
    words: [u64; 8],
    values: Vec<f64>,
}
fn observed(p: &mut dyn Plugin) -> Receipt {
    let bytes = p.save_state().unwrap();
    assert_eq!(bytes.len(), 64 + 4096 * 8);
    let words = std::array::from_fn(|i| u64::from_le_bytes(bytes[i * 8..i * 8 + 8].try_into().unwrap()));
    let values = bytes[64..].chunks_exact(8).map(|b| f64::from_le_bytes(b.try_into().unwrap())).collect();
    Receipt { words, values }
}
fn mode(p: &mut dyn Plugin, value: u32) {
    let mut bytes = b"BD01".to_vec();
    bytes.extend_from_slice(&value.to_le_bytes());
    assert!(p.load_state(&bytes));
}
fn audio(n: usize) -> Vec<Vec<f32>> {
    vec![vec![0.5; n], vec![-0.25; n]]
}
fn pcm(io: &[Vec<f32>], gain: f32, bias: f32) {
    for (ch, input) in io.iter().zip([0.5, -0.25]) {
        assert!(ch.iter().all(|v| (*v - (input * gain + bias)).abs() < 1e-6));
    }
}

#[test]
fn exact_catalog_and_event_limits_reach_one_real_call_then_acknowledge() {
    let mut p = plugin();
    assert_eq!(p.info().params.len(), 4096);
    let ids: Vec<String> = (0..4096).map(|i| i.to_string()).collect();
    let targets: Vec<_> = ids.iter().map(|id| Target { id, value: 1.5 }).collect();
    let notes: Vec<_> = (0..1024)
        .map(|i| Note {
            offset: i / 4,
            message: match i % 4 {
                0 => Message::On { note: 60, velocity: 127 },
                1 => Message::Off { note: 60 },
                _ => Message::Midi([0xb0, 7, 0]),
            },
        })
        .collect();
    let mut io = audio(256);
    let r = p.process_block_delivery(&mut io, 256, &targets, &notes).unwrap();
    assert_eq!((r.frames, r.targets, r.notes, r.status), (256, 4096, 1024, Status::Continue));
    pcm(&io, 1.5, 0.1875);
    let got = observed(&mut *p);
    assert_eq!(got.words, [1, 256, 256, 4096, 256, 256, 512, 0]);
    assert!(got.values.iter().all(|v| *v == 1.5));
    let mut io = audio(128);
    let r = p.process_block_delivery(&mut io, 128, &targets, &[]).unwrap();
    assert_eq!((r.frames, r.targets, r.notes), (128, 0, 0));
    pcm(&io, 1.5, 0.1875);
    assert_eq!(observed(&mut *p).words, [2, 384, 128, 0, 0, 0, 0, 0]);
}

#[test]
fn explicit_event_offsets_shape_pcm_and_all_statuses_are_truthful() {
    let mut p = plugin();
    for (config, status) in [(4, Status::Continue), (5, Status::ContinueIfNotQuiet), (6, Status::Tail), (7, Status::Sleep)] {
        mode(&mut *p, config);
        let notes = [
            Note { offset: 2, message: Message::On { note: 60, velocity: 127 } },
            Note { offset: 5, message: Message::Off { note: 60 } },
            Note { offset: 7, message: Message::Midi([0xb0, 7, 127]) },
            Note { offset: 9, message: Message::Midi([0xb0, 7, 0]) },
        ];
        let mut io = audio(12);
        let r = p.process_block_delivery(&mut io, 12, &[], &notes).unwrap();
        assert_eq!(r.status, status);
        for (ch, input) in io.iter().zip([0.5, -0.25]) {
            for (i, v) in ch.iter().enumerate() {
                let level = if (2..5).contains(&i) || (7..9).contains(&i) { 1.0 } else { 0.0 };
                assert!((*v - (input + 0.125 + level)).abs() < 1e-6);
            }
        }
        let got = observed(&mut *p);
        assert_eq!(&got.words[2..], [12, 0, 1, 1, 2, 0]);
    }
}

#[test]
fn whole_batch_and_span_refusals_leave_processor_and_audio_untouched() {
    let mut p = plugin();
    let before = observed(&mut *p);
    let bad_targets = [vec![Target { id: "0", value: 2.0 }, Target { id: "no-such-id", value: 1.0 }], vec![Target { id: "00000000000", value: 1.0 }]];
    for targets in bad_targets {
        let mut io = audio(32);
        let original = io.clone();
        assert_eq!(p.process_block_delivery(&mut io, 32, &targets, &[]), Err(Error::InvalidTarget));
        assert_eq!(io, original);
        assert_eq!(observed(&mut *p), before);
    }
    for notes in [
        vec![Note { offset: 32, message: Message::Off { note: 60 } }],
        vec![Note { offset: 3, message: Message::Off { note: 60 } }, Note { offset: 2, message: Message::Off { note: 60 } }],
    ] {
        let mut io = audio(32);
        let original = io.clone();
        assert_eq!(p.process_block_delivery(&mut io, 32, &[Target { id: "0", value: 2.0 }], &notes), Err(Error::InvalidBlock));
        assert_eq!(io, original);
        assert_eq!(observed(&mut *p), before);
    }
    let mut io = audio(32);
    assert_eq!(
        p.process_block_delivery(&mut io, 32, &[], &[Note { offset: 0, message: Message::On { note: 128, velocity: 127 } }]),
        Err(Error::InvalidTarget)
    );
    for frames in [0, 33, 257, usize::MAX] {
        let mut io = audio(32);
        let original = io.clone();
        assert_eq!(p.process_block_delivery(&mut io, frames, &[], &[]), Err(Error::InvalidBlock));
        assert_eq!(io, original);
        assert_eq!(observed(&mut *p), before);
    }
    let mut io = vec![vec![1.0; 32]];
    assert_eq!(p.process_block_delivery(&mut io, 32, &[], &[]), Err(Error::InvalidBlock));
    assert_eq!(observed(&mut *p), before);
    let mut io = audio(32);
    let r = p.process_block_delivery(&mut io, 32, &[], &[]).unwrap();
    assert_eq!(r.targets, 0);
    pcm(&io, 1.0, 0.125);
}

#[test]
fn capacity_limits_and_latest_union_are_atomic() {
    let mut p = plugin();
    for capacity in [Capacity { targets: 4097, notes: 0 }, Capacity { targets: 0, notes: 1025 }, Capacity { targets: usize::MAX, notes: usize::MAX }]
    {
        assert_eq!(p.prepare_block_delivery(capacity), Err(Error::InvalidCapacity));
    }
    p.prepare_block_delivery(Capacity { targets: 1, notes: 1 }).unwrap();
    let mut io = audio(32);
    p.process_block_delivery(&mut io, 32, &[Target { id: "0", value: 1.5 }], &[]).unwrap();
    let before = observed(&mut *p);
    let mut io = audio(32);
    let original = io.clone();
    assert_eq!(p.process_block_delivery(&mut io, 32, &[Target { id: "1", value: 0.5 }], &[]), Err(Error::InvalidCapacity));
    assert_eq!(io, original);
    assert_eq!(observed(&mut *p), before);
    let notes = [Note { offset: 0, message: Message::Off { note: 60 } }; 2];
    assert_eq!(p.process_block_delivery(&mut io, 32, &[], &notes), Err(Error::InvalidCapacity));
    assert_eq!(observed(&mut *p), before);
    p.clear_block_delivery_targets().unwrap();
    let r = p.process_block_delivery(&mut io, 32, &[Target { id: "1", value: 0.5 }], &[]).unwrap();
    assert_eq!(r.targets, 1);
    let got = observed(&mut *p);
    assert_eq!(got.values[0], 1.5);
    assert_eq!(got.values[1], 0.5);
}

#[test]
fn uncertain_process_or_status_retries_absolutes_without_replaying_notes() {
    for config in [1, 2] {
        let mut p = plugin();
        mode(&mut *p, config);
        let mut io = audio(32);
        let notes = [Note { offset: 0, message: Message::On { note: 60, velocity: 127 } }, Note { offset: 31, message: Message::Off { note: 60 } }];
        assert_eq!(
            p.process_block_delivery(&mut io, 32, &[Target { id: "0", value: 1.5 }, Target { id: "4095", value: 0.5 }], &notes),
            Err(Error::ProcessFailed)
        );
        assert!(io.iter().flatten().all(|v| *v == 0.0));
        let got = observed(&mut *p);
        assert_eq!(&got.words[..7], &[1, 32, 32, 2, 1, 1, 0]);
        assert_eq!(got.values[0], 1.5);
        let mut io = audio(32);
        let r = p.process_block_delivery(&mut io, 32, &[], &[]).unwrap();
        assert_eq!((r.targets, r.notes), (2, 0));
        pcm(&io, 1.5, 0.0625);
        assert_eq!(&observed(&mut *p).words[..7], &[2, 64, 32, 2, 0, 0, 0]);
        let mut io = audio(32);
        assert_eq!(p.process_block_delivery(&mut io, 32, &[], &[]).unwrap().targets, 0);
    }
}

#[test]
fn failed_start_and_newer_absolute_target_supersede_uncertain_values() {
    let mut p = plugin();
    mode(&mut *p, 3);
    let mut io = audio(32);
    assert_eq!(
        p.process_block_delivery(
            &mut io,
            32,
            &[Target { id: "0", value: 1.5 }],
            &[Note { offset: 0, message: Message::On { note: 60, velocity: 127 } }]
        ),
        Err(Error::ProcessFailed)
    );
    assert!(io.iter().flatten().all(|v| *v == 0.0));
    assert_eq!(observed(&mut *p).words, [0; 8]);
    let mut io = audio(32);
    let r = p.process_block_delivery(&mut io, 32, &[Target { id: "0", value: 0.25 }], &[]).unwrap();
    assert_eq!((r.targets, r.notes), (1, 0));
    pcm(&io, 0.25, 0.125);
    assert_eq!(observed(&mut *p).values[0], 0.25);
}

#[test]
fn lifecycle_preserves_or_clears_latest_according_to_ownership() {
    let mut p = plugin();
    let mut io = audio(32);
    p.process_block_delivery(&mut io, 32, &[Target { id: "0", value: 1.5 }], &[]).unwrap();
    p.reset();
    let mut io = audio(32);
    assert_eq!(p.process_block_delivery(&mut io, 32, &[], &[]).unwrap().targets, 1);
    pcm(&io, 1.5, 0.125);
    p.prepare(48_000.0, 256, 2);
    let mut io = audio(32);
    assert_eq!(p.process_block_delivery(&mut io, 32, &[], &[]), Err(Error::NotPrepared));
    p.prepare_block_delivery(FULL).unwrap();
    assert_eq!(p.process_block_delivery(&mut io, 32, &[], &[]).unwrap().targets, 1);
    mode(&mut *p, 0);
    let mut io = audio(32);
    assert_eq!(p.process_block_delivery(&mut io, 32, &[], &[]).unwrap().targets, 0);
    pcm(&io, 1.5, 0.125);
    let mut io = audio(32);
    p.process_block_delivery(&mut io, 32, &[Target { id: "0", value: 0.5 }], &[]).unwrap();
    p.clear_block_delivery_targets().unwrap();
    p.reset();
    let mut io = audio(32);
    assert_eq!(p.process_block_delivery(&mut io, 32, &[], &[]).unwrap().targets, 0);
    pcm(&io, 0.5, 0.125);
}

#[test]
fn legacy_intervention_requires_explicit_repreparation_and_pending_drain() {
    let mut p = plugin();
    p.process_block_delivery(&mut audio(32), 32, &[Target { id: "0", value: 1.5 }], &[]).unwrap();
    assert!(p.set_param("0", 0.5));
    assert_eq!(p.prepare_block_delivery(FULL), Err(Error::MixedLegacy));
    let mut io = audio(32);
    assert_eq!(p.process_block_delivery(&mut io, 32, &[], &[]), Err(Error::NotPrepared));
    p.process(&mut io, 32);
    pcm(&io, 0.5, 0.125);
    p.prepare_block_delivery(FULL).unwrap();
    let mut io = audio(32);
    assert_eq!(p.process_block_delivery(&mut io, 32, &[], &[]).unwrap().targets, 0);
    pcm(&io, 0.5, 0.125);
    for intervention in 0..5 {
        match intervention {
            0 => p.note_on(0, 60, 127),
            1 => p.note_off(0, 60),
            2 => p.midi(0, [0xb0, 7, 0]),
            3 => p.all_notes_off(),
            _ => p.process(&mut audio(32), 32),
        }
        assert_eq!(p.process_block_delivery(&mut audio(32), 32, &[], &[]), Err(Error::NotPrepared));
        p.process(&mut audio(32), 32);
        p.prepare_block_delivery(FULL).unwrap();
    }
}

#[test]
fn trait_object_default_backend_refuses_opt_in_without_audio_mutation() {
    let mut p: Box<dyn Plugin> = soundcraft_dsp::create("gain").unwrap();
    p.prepare(48_000.0, 32, 2);
    let mut io = audio(32);
    let original = io.clone();
    assert_eq!(p.prepare_block_delivery(FULL), Err(Error::Unsupported));
    assert_eq!(p.process_block_delivery(&mut io, 32, &[], &[]), Err(Error::Unsupported));
    assert_eq!(p.clear_block_delivery_targets(), Err(Error::Unsupported));
    assert_eq!(io, original);
}

#[test]
fn invalid_batch_after_failure_preserves_latest_and_newer_update_wins() {
    let mut p = plugin();
    mode(&mut *p, 1);
    assert_eq!(
        p.process_block_delivery(&mut audio(32), 32, &[Target { id: "0", value: 1.5 }, Target { id: "4095", value: 0.5 }], &[]),
        Err(Error::ProcessFailed)
    );
    let before = observed(&mut *p);
    assert_eq!(
        p.process_block_delivery(&mut audio(32), 32, &[Target { id: "0", value: 0.25 }, Target { id: "bad", value: 1.0 }], &[]),
        Err(Error::InvalidTarget)
    );
    assert_eq!(observed(&mut *p), before);
    let mut io = audio(32);
    assert_eq!(p.process_block_delivery(&mut io, 32, &[], &[]).unwrap().targets, 2);
    pcm(&io, 1.5, 0.0625);
    mode(&mut *p, 1);
    assert_eq!(p.process_block_delivery(&mut audio(32), 32, &[Target { id: "0", value: 1.75 }], &[]), Err(Error::ProcessFailed));
    let mut io = audio(32);
    assert_eq!(p.process_block_delivery(&mut io, 32, &[Target { id: "0", value: 0.25 }], &[]).unwrap().targets, 1);
    pcm(&io, 0.25, 0.0625);
    assert_eq!(observed(&mut *p).values[0], 0.25);
}

#[test]
fn rejected_state_load_keeps_latest_but_invalidates_acknowledgement() {
    let mut p = plugin();
    p.process_block_delivery(&mut audio(32), 32, &[Target { id: "0", value: 0.5 }], &[]).unwrap();
    assert!(!p.load_state(b"invalid opaque state"));
    let mut io = audio(32);
    assert_eq!(p.process_block_delivery(&mut io, 32, &[], &[]).unwrap().targets, 1);
    pcm(&io, 0.5, 0.125);
}

#[test]
fn delivery_requires_explicit_reservation_and_rejects_short_channels() {
    fixture();
    let mut p: Box<dyn Plugin> = soundcraft_clap_host::create(ID).unwrap();
    let mut io = audio(32);
    assert_eq!(p.process_block_delivery(&mut io, 32, &[], &[]), Err(Error::NotPrepared));
    p.prepare(48_000.0, 256, 2);
    p.prepare_block_delivery(FULL).unwrap();
    let before = observed(&mut *p);
    let mut short = vec![vec![1.0; 32], vec![1.0; 31]];
    let original = short.clone();
    assert_eq!(p.process_block_delivery(&mut short, 32, &[], &[]), Err(Error::InvalidBlock));
    assert_eq!(short, original);
    assert_eq!(observed(&mut *p), before);
}

#[test]
fn duplicate_targets_use_last_value_and_declared_clamping_at_processor() {
    let mut p = plugin();
    let targets = [
        Target { id: "0", value: 0.25 },
        Target { id: "0", value: 1.75 },
        Target { id: "4095", value: f32::NAN },
        Target { id: "1", value: f32::INFINITY },
        Target { id: "2", value: f32::NEG_INFINITY },
    ];
    let mut io = audio(32);
    assert_eq!(p.process_block_delivery(&mut io, 32, &targets, &[]).unwrap().targets, 4);
    pcm(&io, 1.75, 0.125);
    let got = observed(&mut *p);
    assert_eq!(got.words[3], 4);
    assert_eq!((got.values[0], got.values[1], got.values[2], got.values[4095]), (1.75, 2.0, 0.0, 1.0));
}
