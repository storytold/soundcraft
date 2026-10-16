use soundcraft_dsp::offline::{MAX_OFFLINE_SAMPLES, resample, resample_to_frames};

#[test]
fn exact_delivery_uses_existing_sinc_signal_with_an_explicit_half_frame_length() {
    let input = [vec![0.25], vec![-0.125]];
    let output = resample_to_frames(&input, 48000, 24000, 1).unwrap();
    assert_eq!(output, resample(&input, 48000, 24000));
    assert_eq!(output.len(), 2);
    assert_eq!(output[0].len(), 1);
    assert!(output[0][0] > 0.0 && output[1][0] < 0.0);
    let stable = [vec![0.0, -0.0, 0.25]];
    assert_eq!(
        resample_to_frames(&stable, 48000, 48000, 3).unwrap()[0].iter().map(|n| n.to_bits()).collect::<Vec<_>>(),
        stable[0].iter().map(|n| n.to_bits()).collect::<Vec<_>>()
    );
}

#[test]
fn invalid_or_oversized_delivery_is_rejected_without_allocating_output() {
    let input = [vec![0.25], vec![-0.125]];
    for (from, to, frames) in [
        (0, 48000, 1),
        (48000, 0, 1),
        (7999, 48000, 1),
        (48000, 768001, 1),
        (48000, 24000, 0),
        (48000, 24000, MAX_OFFLINE_SAMPLES / 2 + 1),
        (48000, 24000, usize::MAX),
        (48000, 48000, 2),
    ] {
        assert!(resample_to_frames(&input, from, to, frames).is_err());
    }
    for input in [vec![], vec![vec![]], vec![vec![1.0], vec![]], vec![vec![1.0]; 65]] {
        assert!(resample_to_frames(&input, 48000, 24000, 1).is_err());
    }
}

#[test]
fn sliced_delivery_matches_whole_sinc_bits_and_yields_through_kernel_and_samples() {
    use soundcraft_dsp::offline::DeliveryResampler;
    let input: Vec<Vec<f32>> = (0..3)
        .map(|channel| (0..150).map(|frame| if frame == 0 { -0.0 } else { ((frame * (channel + 1)) as f32 * 0.031).sin() * 0.25 }).collect())
        .collect();
    for (from, to) in [(48000, 44100), (44100, 96000), (96000, 8000), (48000, 48000)] {
        let frames = ((150u64 * to as u64 + from as u64 / 2) / from as u64) as usize;
        let expected = resample_to_frames(&input, from, to, frames).unwrap();
        let mut delivery = DeliveryResampler::new(input.clone(), from, to, frames).unwrap();
        let mut progress = vec![delivery.progress()];
        let mut steps = 0;
        loop {
            steps += 1;
            let finished = delivery.step().unwrap();
            progress.push(delivery.progress());
            if finished {
                break;
            }
            assert!(steps < 10_000);
        }
        assert!(steps > 1);
        assert!(progress.is_sorted() && progress.iter().all(|p| *p >= 0.0 && *p <= 1.0));
        assert_eq!(progress.last(), Some(&1.0));
        let actual = delivery.finish().unwrap();
        for (actual, expected) in actual.iter().zip(&expected) {
            assert_eq!(actual.iter().map(|x| x.to_bits()).collect::<Vec<_>>(), expected.iter().map(|x| x.to_bits()).collect::<Vec<_>>());
        }
    }
}
