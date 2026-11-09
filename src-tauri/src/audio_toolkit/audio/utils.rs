use anyhow::Result;
use hound::{WavReader, WavSpec, WavWriter};
use log::debug;
use std::io::{Cursor, Seek, Write};
use std::path::Path;

/// Read a WAV file and return normalised f32 samples.
pub fn read_wav_samples<P: AsRef<Path>>(file_path: P) -> Result<Vec<f32>> {
    let reader = WavReader::open(file_path.as_ref())?;
    let samples = reader
        .into_samples::<i16>()
        .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
        .collect::<Result<Vec<f32>, _>>()?;
    Ok(samples)
}

/// Verify a WAV file by reading it back and checking the sample count.
pub fn verify_wav_file<P: AsRef<Path>>(file_path: P, expected_samples: usize) -> Result<()> {
    let reader = WavReader::open(file_path.as_ref())?;
    let actual_samples = reader.len() as usize;
    if actual_samples != expected_samples {
        anyhow::bail!(
            "WAV sample count mismatch: expected {}, got {}",
            expected_samples,
            actual_samples
        );
    }
    Ok(())
}

/// The format Handy saves recordings in: 16 kHz mono 16-bit PCM.
const RECORDING_SPEC: WavSpec = WavSpec {
    channels: 1,
    sample_rate: 16000,
    bits_per_sample: 16,
    sample_format: hound::SampleFormat::Int,
};

/// Write f32 samples as i16 and finish the file.
fn write_samples<W: Write + Seek>(mut writer: WavWriter<W>, samples: &[f32]) -> Result<()> {
    for sample in samples {
        writer.write_sample((sample * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    Ok(())
}

/// Save audio samples as a WAV file
pub fn save_wav_file<P: AsRef<Path>>(file_path: P, samples: &[f32]) -> Result<()> {
    write_samples(
        WavWriter::create(file_path.as_ref(), RECORDING_SPEC)?,
        samples,
    )?;
    debug!("Saved WAV file: {:?}", file_path.as_ref());
    Ok(())
}

/// The same WAV as [`save_wav_file`] writes, in memory, e.g. to upload.
pub fn encode_wav(samples: &[f32]) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(Vec::new());
    write_samples(WavWriter::new(&mut cursor, RECORDING_SPEC)?, samples)?;
    Ok(cursor.into_inner())
}
