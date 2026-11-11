use super::args::ConvertArgs;
use super::error::{CliError, ConvertError};
use std::io::Write;
use std::num::NonZeroU32;

pub(crate) fn convert(args: &ConvertArgs, out: &mut dyn Write) -> Result<(), CliError> {
    let (input, output) = (&args.input, &args.output);
    let rate = args.rate.map(NonZeroU32::get);
    let bit_depth = args.bit_depth.into();
    let dither_flag = match (args.dither, args.no_dither) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    };
    let bytes = std::fs::read(input).map_err(|source| CliError::File { path: input.clone(), source })?;
    let ext = input.extension().and_then(std::ffi::OsStr::to_str);
    let (info, mut buf) = craft_audio::decode(&bytes, ext).map_err(|source| CliError::Audio { path: input.clone(), source })?;
    let source_rate = buf.sample_rate;
    if let Some(rate) = rate {
        buf.channels = soundcraft_dsp::offline::resample(&buf.channels, buf.sample_rate, rate);
        buf.sample_rate = rate;
    }
    let normalize = args.normalize;
    if normalize {
        soundcraft_dsp::offline::normalize(&mut buf.channels, -0.1, false);
    }
    let processed = rate.is_some_and(|r| r != info.sample_rate) || normalize;
    let dither = dither_flag.unwrap_or_else(|| auto_dither(info.sample_format, bit_depth, processed));
    // Lossy only for the message: an extension that is not valid Unicode names no format.
    let out_ext = output.extension().map_or_else(|| "wav".into(), |e| e.to_string_lossy().to_ascii_lowercase());
    let Some(format) = craft_audio::encode_format_for(&out_ext) else {
        return Err(ConvertError::OutputFormat { output: output.clone(), ext: out_ext }.into());
    };
    // A BWF source keeps its `bext` (written for WAV output only). The time reference counts
    // samples, so it moves to the output rate.
    let mut opts = craft_audio::EncodeOptions::new(format).with_bit_depth(bit_depth).with_dither(dither);
    opts.bwf = info.bwf.map(|mut b| {
        b.time_reference = at_rate(b.time_reference, source_rate, buf.sample_rate);
        b
    });
    let bytes = craft_audio::encode(&buf, &opts).map_err(|source| CliError::Audio { path: output.clone(), source })?;
    write_replacing(&args.write_path(), &bytes).map_err(|source| CliError::File { path: output.clone(), source })?;
    outln!(out, "wrote {} ({} ch, {} Hz, {:.2} s)", output.display(), buf.num_channels(), buf.sample_rate, buf.duration_secs());
    Ok(())
}

/// The sample position `pos` at `from` Hz as the nearest sample at `to` Hz.
fn at_rate(pos: u64, from: u32, to: u32) -> u64 {
    if from == 0 || from == to {
        return pos;
    }
    let scaled = (u128::from(pos) * u128::from(to) + u128::from(from) / 2) / u128::from(from);
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

/// Writes `bytes` to a new file next to `target`, flushes it and renames it over `target`, so `target`
/// (the input, for --in-place) holds either its old or its new contents. The new file is removed when
/// writing or renaming fails.
fn write_replacing(target: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let name = target.file_name().map_or_else(|| "output".into(), |n| n.to_string_lossy().into_owned());
    let tmp = target.with_file_name(format!(".{name}.{}.part", std::process::id()));
    let written = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Whether `convert` dithers when not told: only when writing integer samples that cannot hold the
/// source exactly, i.e. the output has fewer bits than the source, the source is float or lossy, or
/// resampling/normalizing produced new sample values. A 16-bit file written as 16 or 24-bit, or
/// anything written as 32-bit float, is copied sample for sample (digital silence stays silent).
fn auto_dither(source: craft_audio::SampleFormat, target: craft_audio::BitDepth, processed: bool) -> bool {
    use craft_audio::{BitDepth, SampleFormat};
    let target_bits = match target {
        BitDepth::Float32 => return false,
        BitDepth::Int16 => 16,
        BitDepth::Int24 => 24,
        BitDepth::Int32 => 32,
        // A depth added to craft-audio later: dither, as for any step that may lose resolution.
        _ => return true,
    };
    if processed {
        return true;
    }
    match source {
        SampleFormat::Int8 => target_bits < 8,
        SampleFormat::Int16 => target_bits < 16,
        SampleFormat::Int24 => target_bits < 24,
        // Decoded through f32, which holds 24 bits: a 32-bit integer source has already lost
        // resolution, so treat it like float (as is any format added to craft-audio later).
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_dithers_only_when_resolution_is_lost() {
        use craft_audio::{BitDepth, SampleFormat};
        // Same depth or wider: the samples fit exactly.
        assert!(!auto_dither(SampleFormat::Int16, BitDepth::Int16, false));
        assert!(!auto_dither(SampleFormat::Int16, BitDepth::Int24, false));
        assert!(!auto_dither(SampleFormat::Int24, BitDepth::Int24, false));
        assert!(!auto_dither(SampleFormat::Int8, BitDepth::Int16, false));
        // Float output never dithers.
        assert!(!auto_dither(SampleFormat::Float32, BitDepth::Float32, true));
        assert!(!auto_dither(SampleFormat::Int24, BitDepth::Float32, false));
        // Fewer bits, a float or lossy source, or processed samples: dither.
        assert!(auto_dither(SampleFormat::Int24, BitDepth::Int16, false));
        assert!(auto_dither(SampleFormat::Float32, BitDepth::Int24, false));
        assert!(auto_dither(SampleFormat::Compressed, BitDepth::Int16, false));
        assert!(auto_dither(SampleFormat::Int32, BitDepth::Int24, false));
        assert!(auto_dither(SampleFormat::Int16, BitDepth::Int16, true));
    }
}
