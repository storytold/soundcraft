# External MIDI Beat Clock response

These are 48 kHz software-rendering measurements for external MIDI Clock follow. The input is a deterministic sequence of timestamped F8 messages. The measurements use actual sampler attacks, releases and metronome PCM. A positive error means the attack was late relative to the source score. Device buffering is simulated in these tests.

## Abrupt tempo changes

Each case has eight post-change beats of eighth notes. Pulse 384 is a bar line and pulse 396 is halfway through a beat. Each cell gives the signed deviation of the first eighth after the cut (ms), maximum absolute deviation from the second beat onward (ms), and number of received F8s until confirmation. The packed input cycles through groups of three, four and five pulses within 1 ms with gaps between groups. A batch is timestamped at its first pulse.

| Step (BPM) | Cut (F8) | ±1 ms | ±2 ms | 3–5 packed |
| --- | --- | --- | --- | --- |
| 130→80 | 384 | +0.1667 / 3.4792 / 2 | +1.9792 / 4.1875 / 2 | -144.2500 / 0.0000 / 7 |
| 130→80 | 396 | +0.7292 / 2.6042 / 2 | -5.7083 / 2.3958 / 2 | -144.2292 / 0.0000 / 7 |
| 120→100 | 384 | +0.0208 / 0.3125 / 4 | +1.6042 / 1.1250 / 4 | -50.0000 / 0.0000 / 15 |
| 120→100 | 396 | +0.0208 / 3.0208 / 4 | +2.9583 / 2.7708 / 4 | -50.0000 / 0.0000 / 15 |
| 180→60 | 384 | +6.6042 / 4.7500 / 2 | -0.6875 / 4.1667 / 2 | -196.8958 / 196.8958 / 7 |
| 180→60 | 396 | -4.9583 / 2.6042 / 2 | +5.8750 / 1.4583 / 2 | -196.8958 / 196.8958 / 7 |
| 100→120 | 384 | +3.7708 / 2.7292 / 3 | +4.5417 / 1.7500 / 3 | +50.0000 / 0.0000 / 15 |
| 100→120 | 396 | +0.6875 / 0.9583 / 4 | +3.5833 / 2.8333 / 3 | +50.0000 / 0.0000 / 15 |
| 80→130 | 384 | +0.7917 / 0.8958 / 2 | +1.5417 / 2.4167 / 2 | +0.0000 / 0.0208 / 7 |
| 80→130 | 396 | +0.3542 / 2.3958 / 2 | +1.1042 / 1.8750 / 2 | +0.0000 / 0.0208 / 7 |
| 60→180 | 384 | +0.1667 / 0.3750 / 2 | +1.5833 / 1.2292 / 2 | +0.0000 / 0.0000 / 7 |
| 60→180 | 396 | +0.8333 / 0.4583 / 2 | +0.3125 / 0.6667 / 2 | +0.0000 / 0.0000 / 7 |

Across the 36 cases all 1,782 expected notes had a single attack and the complete 96-sample voice. At ±1 and ±2 ms, the two-interval path confirmed 8 of 12 changes in each profile. The four-interval path handled the remaining four at the third or fourth incoming F8 after the cut (an interval can span the cut). Under grouped delivery, two or four independent group intervals took seven or fifteen received F8s. Each changed train produced one confirmation. No confirmation preceded the change. The 10/4 ms ideal-input thresholds remain a separate regression. With timestamp jitter three later peaks exceeded 4 ms. The jitter regression enforces 10 ms on the first eighth and 6 ms thereafter; the ideal-wire regression still enforces 10/4 ms. The most extreme packed slowdown was 196.896 ms early.

Four additional ten-minute, 120 BPM runs used group widths of 3, 4, 5 and cycling 3–5. Each rendered 2,400 intact eighth notes with zero false steps, dropouts or onset error. The prior head produced 194 loss indications and 88 empty/88 overfull eighth-note slots across the twelve variable-burst changes. Shifted onsets can leave a slot empty even when the raw attack count is unchanged.

## Output deadlines

A locked follower may project as far as the end of the audio the device actually requests. A callback shorter than one F8, missing presentation timing, an overdue pulse, a detected rate change or non-uniform packet delivery uses the existing wire limit. A later transition into Coast follows the configured loss policy. The sampler and click continue to render their tails at the device rate.

Each steady range covers individual note and click attacks. Hard uses the last received F8 plus one pulse. Selected allows bounded prediction. For the slowdown, the source rate drops by 20% (200→160, 240→192, 300→240). Each row sweeps all 24 pulse positions in the beat and four callback alignments. Offbeat is the first subsequent offbeat and Beat is the next quarter-note boundary strictly after the cut. Values are worst signed errors by magnitude in ms across the 96 alignments. If the change lands exactly on a quarter-note boundary, the attack on that boundary spans +20.646…+82.875 ms with the hard cap and 0…+57.979 ms with the selected policy. The reference includes presentation lead, so adding driver latency again would double-count it.

| BPM | Frames | Lead (ms) | Steady hard | Steady selected | Offbeat hard | Offbeat selected | Beat hard | Beat selected |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 200 | 128 | 30 | +20.646…+22.646 | +20.646…+22.646 | +19.646 | +19.646 | +19.646 | +19.646 |
| 200 | 128 | 50 | +40.646…+42.646 | +40.646…+42.646 | +39.646 | +39.646 | +39.646 | +39.646 |
| 200 | 512 | 30 | +28.646…+38.646 | +28.646…+38.646 | +35.646 | +35.646 | +35.646 | +35.646 |
| 200 | 512 | 50 | +48.646…+58.646 | +48.646…+58.646 | +55.646 | +55.646 | +55.646 | +55.646 |
| 200 | 2048 | 30 | +28.646…+61.979 | +0.000…+0.000 | +60.542 | +60.542 | +60.458 | +60.458 |
| 200 | 2048 | 50 | +48.646…+81.979 | +0.000…+0.000 | +80.542 | +80.542 | +80.458 | +80.458 |
| 240 | 128 | 30 | +22.312…+24.646 | +22.312…+24.646 | +22.271 | +22.271 | +22.250 | +22.250 |
| 240 | 128 | 50 | +42.312…+44.646 | +42.312…+44.646 | +42.271 | +42.271 | +42.250 | +42.250 |
| 240 | 512 | 30 | +30.312…+40.062 | +0.000…+0.000 | +37.729 | +37.729 | +38.250 | +38.250 |
| 240 | 512 | 50 | +50.312…+60.062 | +0.000…+0.000 | +57.729 | +57.729 | +58.250 | +58.250 |
| 240 | 2048 | 30 | +30.312…+62.646 | +0.000…+0.000 | +61.667 | +61.667 | +61.812 | +60.771 |
| 240 | 2048 | 50 | +50.312…+82.646 | +0.000…+0.000 | +81.667 | +81.667 | +81.812 | +81.375 |
| 300 | 128 | 30 | +24.646…+25.979 | +24.646…+25.979 | +24.812 | +24.812 | +24.812 | +24.812 |
| 300 | 128 | 50 | +44.646…+45.979 | +44.646…+45.979 | +44.812 | +44.812 | +44.812 | +44.812 |
| 300 | 512 | 30 | +32.646…+36.979 | +0.000…+0.000 | +40.062 | +40.062 | +40.229 | +40.229 |
| 300 | 512 | 50 | +52.646…+56.979 | +0.000…+0.000 | +60.062 | +60.062 | +60.229 | +60.229 |
| 300 | 2048 | 30 | +32.646…+65.104 | +0.000…+0.000 | +63.875 | +63.875 | +64.042 | +63.271 |
| 300 | 2048 | 50 | +52.646…+85.104 | +0.000…+0.000 | +86.500 | +86.500 | +84.042 | +83.771 |

Across both policies the slowdown sweep verified 58,752 attacks and their complete waveforms. An initial projection implementation muted 201 sampler notes and damaged 28 tails on the return from a slowdown. Keeping overdue MIDI events ordered when prediction resumes removed all 229 failures. The selected policy’s first offbeat spanned −25.000 to +86.500 ms, and the next beat −25.000 to +83.771 ms. The −25.000 ms attack was already in the device queue before an old-rate pulse first became overdue. The largest positive error occurred at 300 BPM, 2,048 frames and 50 ms lead after falling back to the wire limit.

Once PCM is committed to the device, a tempo change cannot revise it. An early attack in the device queue and a late attack held by the strict wire limit have different causes. The measured maxima describe these settings and phases. Other drivers, block alignment and clock sources can give different results.

## Continuous tempo and stability

The 124 BPM round trip took 32 bars out and 32 bars back for each ±8% excursion. First-pulse estimation was excluded from the BPM display statistic. The regression requires the transport to be locked and one beat of estimates to have arrived. The linear period/phase fit continues to reject jitter. When the long fit detects sustained curvature above the residual noise floor, the writable tempo grid makes smaller, more frequent corrections.

| Source excursion | Clean | ±1 ms | ±2 ms |
| --- | ---: | ---: | ---: |
| +8% and return | 2.3542 | 2.4375 | 3.8333 |
| −8% and return | 2.7708 | 2.9167 | 3.7500 |
| 124.0→124.3→124.0 BPM | 3.1042 | 3.0625 | 3.7917 |

Values are peak absolute PCM onset error in ms. The six ±8% runs each rendered 513 complete notes. The three smaller drift runs each rendered 257 complete notes with zero continuous-ramp or abrupt-step detections. The previous worst was −435 samples (−9.0625 ms). The maximum post-acquisition change between consecutive BPM estimates was 0.09139 BPM.

The existing eight-hour flutter simulation retained its 148-sample grid peak. A separate chronological eight-hour sampler render had identical before/after records for all 115,201 attacks, peak 234 samples (4.875 ms), zero false steps and every 96-sample voice intact. The nine steady BPM and phase p95 pairs were unchanged. The 12 ideal abrupt changes, the 128↔140 BPM controls and Coast relock are checked separately.

## Reproduction

Run each command from the workspace root. The packet trace option emits signed sample errors for every eighth and the pulse positions at which the detector confirmed a step.

```sh
cargo test -p soundcraft-playback --lib clock_follow::render_lead -- --nocapture
cargo test -p soundcraft-ui-egui --lib packet_steps -- --include-ignored --nocapture
cargo test -p soundcraft-ui-egui --lib slowdown_device_phase_sweep -- --ignored --nocapture
cargo test -p soundcraft-ui-egui --lib ramp_response -- --nocapture
cargo test -p soundcraft-ui-egui --lib large_steps -- --nocapture
cargo test -p soundcraft-ui-egui --lib returning_clock_without -- --nocapture
cargo test -p soundcraft-ui-egui --lib eight_hours_of_clock_flutter -- --ignored --nocapture
```

Set `SOUNDCRAFT_PACKET_TRACE=1` before the packet run for the individual attacks. Set `SOUNDCRAFT_SWEEP_DIR` to a directory to export both per-attack slowdown matrices. The acceptance thresholds are SoundCraft targets. Avid describes Beat Clock output and measuring fixed offsets in the [Pro Tools Reference Guide 2026.4, printed pp. 743–744](https://resources.avid.com/SupportFiles/PT/Pro_Tools_Reference_Guide_2026.4.pdf#page=845). Bitwig offers a synchronization response control that trades rapid tracking for smoothing of unsteady clocks. [Bitwig Studio User Guide, Dashboard › Synchronization Settings](https://www.bitwig.com/userguide/latest/the_dashboard/)
