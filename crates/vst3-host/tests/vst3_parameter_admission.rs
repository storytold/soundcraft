//! Queue admission is distinct from the owned processor's consumption receipt.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use soundcraft_dsp::Plugin;
mod capacity_common;
const P: usize = 1025;
fn create() -> Box<dyn Plugin> {
    let dir = capacity_common::fixture_dir();
    let found = soundcraft_vst3_host::scan_paths(&[dir.to_path_buf()]);
    assert_eq!(found.len(), 1);
    let mut plugin = soundcraft_vst3_host::create(&found[0].id).unwrap();
    plugin.prepare(48_000.0, 256, 2);
    assert_eq!(plugin.info().params.len(), P);
    plugin
}
fn receipt(plugin: &mut dyn Plugin) -> (Vec<f64>, [u64; 5]) {
    let bytes = plugin.save_state().unwrap();
    assert_eq!(&bytes[..4], b"SCV3");
    let length = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let bytes = &bytes[8..8 + length];
    assert_eq!(bytes.len(), P * 8 + 40);
    let values = bytes[..P * 8].chunks_exact(8).map(|part| f64::from_le_bytes(part.try_into().unwrap())).collect();
    let stats = std::array::from_fn(|i| u64::from_le_bytes(bytes[P * 8 + i * 8..P * 8 + (i + 1) * 8].try_into().unwrap()));
    (values, stats)
}
fn process(plugin: &mut dyn Plugin, frames: usize, gain: f32, bias: f32) {
    let mut audio = vec![vec![0.25; frames], vec![-0.5; frames]];
    plugin.process(&mut audio, frames);
    for (channel, input) in audio.iter().zip([0.25, -0.5]) {
        assert!(channel.iter().all(|sample| *sample == input * gain + bias));
    }
}
#[test]
fn full_queue_refuses_without_caching_then_retry_reaches_processor_at_normal_blocks() {
    let mut plugin = create();
    for i in 0..1024 {
        assert!(plugin.set_param(&i.to_string(), if i == 0 { 1.0 } else { 0.0 }));
    }
    assert!(!plugin.set_param("1024", 0.125));
    assert_eq!(plugin.param("1024"), Some(0.0), "rejection cannot advance accepted-value cache");
    // A full queue still accepts a replacement for an already pending ID.
    assert!(plugin.set_param("0", 1.5));
    assert!(!plugin.set_param("unknown", 0.5));
    process(&mut *plugin, 256, 1.5, 0.0);
    let (values, stats) = receipt(&mut *plugin);
    assert_eq!(values[1024], 0.0, "processor never consumed the refused bias");
    assert_eq!(stats[0], 1);
    assert_eq!(stats[1], 1024);
    assert_eq!(stats[4], 256);
    assert!(plugin.set_param("1024", 0.125));
    process(&mut *plugin, 128, 1.5, 0.125);
    let (values, stats) = receipt(&mut *plugin);
    assert_eq!(values[1024], 0.125);
    assert_eq!(stats[0], 2);
    assert_eq!(stats[1], 1);
    assert_eq!(stats[4], 128);
}

#[test]
fn parameter_capacity_refusal_preserves_the_separate_note_queue() {
    let mut plugin = create();
    for i in 0..1024 {
        plugin.note_on(i % 128, (i % 64) as u8, 100);
    }
    assert!(!plugin.set_param("1024", 0.125), "prepare already filled the fixed parameter queue");
    process(&mut *plugin, 256, 1.0, 0.0);
    let (_, stats) = receipt(&mut *plugin);
    assert_eq!(stats[1], 1024);
    assert_eq!(stats[2], 1024);
    assert_eq!(stats[4], 256);
    assert!(plugin.set_param("1024", 0.125));
    process(&mut *plugin, 128, 1.0, 0.125);
    assert_eq!(receipt(&mut *plugin).0[1024], 0.125);
}
