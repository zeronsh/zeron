//! Desktop-only bounded audio. PCM is ephemeral; no logs, files or network I/O.
#[cfg(feature = "aec")]
pub mod aec;
pub mod dsp;
#[cfg(feature = "native")]
pub mod native;
#[cfg(all(feature = "aec", feature = "native"))]
pub mod worker;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use zeron_proto::voice::*;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("audio device unavailable or unsupported")]
    Device,
    #[error("audio buffer overflow")]
    Overflow,
    #[error("invalid audio format or frame")]
    Frame,
}
pub fn decode(frame: &VoiceFrame) -> Result<Vec<f32>, AudioError> {
    if frame.data.len() > MAX_AUDIO_BYTES.div_ceil(3) * 4 {
        return Err(AudioError::Frame);
    }
    let bytes = STANDARD
        .decode(&frame.data)
        .map_err(|_| AudioError::Frame)?;
    if !frame.format.validate(bytes.len()) {
        return Err(AudioError::Frame);
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect())
}
pub fn encode(samples: &[f32], generation: u64, sequence: u64) -> Result<VoiceFrame, AudioError> {
    if samples.is_empty() || samples.len() * 2 > MAX_AUDIO_BYTES {
        return Err(AudioError::Frame);
    }
    let bytes: Vec<_> = samples
        .iter()
        .flat_map(|s| {
            let value = if s.is_finite() {
                (s.clamp(-1.0, 1.0) * 32767.0) as i16
            } else {
                0
            };
            value.to_le_bytes()
        })
        .collect();
    Ok(VoiceFrame {
        generation,
        sequence,
        format: VoiceFormat {
            encoding: VoiceEncoding::Pcm16Le,
            sample_rate: 24_000,
            channels: 1,
        },
        data: STANDARD.encode(bytes),
        item_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codec_bounds_endian_and_nan() {
        let f = encode(&[0.0, 0.5, -0.5, f32::NAN], 7, 8).unwrap();
        let samples = decode(&f).unwrap();
        assert!((samples[1] - 0.5).abs() < 0.0001);
        assert!((samples[2] + 0.5).abs() < 0.0001);
        assert_eq!(samples[3], 0.0);
        let mut invalid = f.clone();
        invalid.data = "A".into();
        assert!(decode(&invalid).is_err());
        assert!(encode(&vec![0.0; MAX_AUDIO_BYTES], 1, 1).is_err());
    }
}
