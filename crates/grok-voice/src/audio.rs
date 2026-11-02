//! Speaker audio waits until the device ring has room, so a burst plays in
//! order instead of chopped. The microphone stays open during playback so the
//! caller can interrupt. `clear_speaker` drops queued audio at once.
//! There is no acoustic echo canceller and no second device stack.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
#[cfg(test)]
use std::sync::{Arc, Mutex};

use zeron_audio::dsp::{Resampler, level};
use zeron_audio::native::AudioIo;
use zeron_proto::voice::VoiceRejection;

/// 20 ms at 24 kHz. Matches one mic frame.
const SPEAKER_CHUNK: usize = 480;
/// One minute of 24 kHz mono. A stalled device cannot grow past this.
const MAX_PENDING_SAMPLES: usize = 24_000 * 60;

pub trait Duplex: Send {
    fn open(&mut self) -> Result<(), VoiceRejection>;
    fn set_muted(&mut self, muted: bool);
    /// Append newly captured mic audio as 24 kHz mono f32. Drain the device
    /// ring every call so it cannot overflow. The caller sends the samples
    /// unless the user muted the microphone.
    fn pull_mic(&mut self, out: &mut Vec<f32>) -> Result<(), VoiceRejection>;
    fn push_speaker(&mut self, pcm: &[i16]);
    /// Drop queued speaker audio now. Does not change the playback generation,
    /// so later audio still enqueues.
    fn clear_speaker(&mut self);
    fn levels(&mut self) -> (u16, u16);
    fn speaker_queued_ms(&self) -> u32;
    /// Switch the capture device. `None` is the system default. A device that
    /// cannot be opened leaves the current one running.
    fn set_input(&mut self, _id: Option<&str>) -> Result<(), VoiceRejection> {
        Ok(())
    }
    fn close(&mut self);
}

#[derive(Default)]
pub struct DesktopDuplex {
    io: Option<AudioIo>,
    resampler: Option<Resampler>,
    mic_level: u16,
    speaker_level: u16,
    sequence: u64,
    /// cpal device id, or the system default when empty.
    input_id: Option<String>,
    /// 24 kHz mono waiting for a free slot in the device ring (~200 ms).
    pending: VecDeque<f32>,
}

impl DesktopDuplex {
    /// Capture from this device on the next [`Duplex::open`]. Empty uses the default.
    pub fn prefer_input(&mut self, id: Option<String>) {
        self.input_id = id.filter(|id| !id.trim().is_empty());
    }
}

impl Duplex for DesktopDuplex {
    fn open(&mut self) -> Result<(), VoiceRejection> {
        self.close();
        let io = AudioIo::open_input(1, self.input_id.as_deref())
            .map_err(|_| VoiceRejection::DeviceUnavailable)?;
        if io.start().is_err() {
            return Err(VoiceRejection::DeviceUnavailable);
        }
        let resampler =
            Resampler::new(io.input_rate, 24_000).ok_or(VoiceRejection::DeviceUnavailable)?;
        self.io = Some(io);
        self.resampler = Some(resampler);
        self.sequence = 0;
        self.mic_level = 0;
        self.speaker_level = 0;
        Ok(())
    }

    fn set_muted(&mut self, muted: bool) {
        // `AudioIo::mute` drains the capture ring on every call, including unmute.
        if let Some(io) = self.io.as_ref() {
            if io.muted.load(Ordering::Acquire) != muted {
                io.mute(muted);
            }
        }
        if muted {
            self.mic_level = 0;
        }
    }

    fn pull_mic(&mut self, out: &mut Vec<f32>) -> Result<(), VoiceRejection> {
        self.feed_speaker();
        let raw = {
            let Some(io) = self.io.as_ref() else {
                return Err(VoiceRejection::DeviceUnavailable);
            };
            // The output callback also fills `reference`. Leaving it full sets
            // the device failed flag. There is no AEC; drain and drop.
            drain_reference(io);
            let mut raw = Vec::new();
            while let Some(sample) = io.capture.pop() {
                raw.push(sample);
            }
            if io.failed.load(Ordering::Acquire) {
                return Err(VoiceRejection::DeviceUnavailable);
            }
            raw
        };
        let start = out.len();
        {
            let Some(resampler) = self.resampler.as_mut() else {
                return Err(VoiceRejection::DeviceUnavailable);
            };
            for sample in raw {
                resampler.push(sample, out);
            }
        }
        self.mic_level = level_u16(&out[start..]);
        Ok(())
    }

    fn push_speaker(&mut self, pcm: &[i16]) {
        if pcm.is_empty() || self.io.is_none() {
            return;
        }
        let samples: Vec<f32> = pcm.iter().map(|sample| *sample as f32 / 32_768.0).collect();
        self.speaker_level = level_u16(&samples);
        self.pending.extend(samples);
        trim_pending(&mut self.pending);
        self.feed_speaker();
    }

    fn clear_speaker(&mut self) {
        self.pending.clear();
        self.speaker_level = 0;
        if let Some(io) = self.io.as_mut() {
            io.clear_playout();
        }
    }

    fn levels(&mut self) -> (u16, u16) {
        let queued = self.speaker_queued_ms();
        let muted = self
            .io
            .as_ref()
            .is_some_and(|io| io.muted.load(Ordering::Acquire));
        let mic = if muted { 0 } else { self.mic_level };
        let speaker = if queued == 0 { 0 } else { self.speaker_level };
        (mic, speaker)
    }

    fn speaker_queued_ms(&self) -> u32 {
        let Some(io) = self.io.as_ref() else {
            return playback_queued_ms(self.pending.len(), 0, 24_000);
        };
        drain_reference(io);
        playback_queued_ms(self.pending.len(), io.output_queued(), io.output_rate)
    }

    fn set_input(&mut self, id: Option<&str>) -> Result<(), VoiceRejection> {
        let next = id
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        if self.input_id == next {
            return Ok(());
        }
        let Some(io) = self.io.as_mut() else {
            self.input_id = next;
            return Ok(());
        };
        if io.replace_input(next.as_deref()).is_err() {
            tracing::debug!("grok voice kept the current microphone");
            return Ok(());
        }
        let rate = io.input_rate;
        self.resampler =
            Some(Resampler::new(rate, 24_000).ok_or(VoiceRejection::DeviceUnavailable)?);
        self.input_id = next;
        self.mic_level = 0;
        Ok(())
    }

    fn close(&mut self) {
        self.io = None;
        self.resampler = None;
        self.mic_level = 0;
        self.speaker_level = 0;
        self.pending.clear();
    }
}

impl DesktopDuplex {
    /// Move queued 24 kHz audio into the device ring, stopping at the first
    /// chunk that does not fit. The rest stays queued for the next tick.
    fn feed_speaker(&mut self) {
        let Some(io) = self.io.as_mut() else {
            return;
        };
        drain_reference(io);
        let rate = io.output_rate;
        loop {
            let Some(chunk) = take_ready_chunk(&mut self.pending, io.output_space(), rate) else {
                break;
            };
            self.sequence = self.sequence.wrapping_add(1);
            let Ok(frame) = zeron_audio::encode(&chunk, 1, self.sequence) else {
                restore_chunk(&mut self.pending, chunk);
                self.sequence = self.sequence.wrapping_sub(1);
                break;
            };
            if io.enqueue(&frame).is_err() {
                restore_chunk(&mut self.pending, chunk);
                self.sequence = self.sequence.wrapping_sub(1);
                break;
            }
        }
    }
}

fn drain_reference(io: &AudioIo) {
    while io.reference.pop().is_some() {}
}

fn trim_pending(pending: &mut VecDeque<f32>) {
    if pending.len() > MAX_PENDING_SAMPLES {
        let extra = pending.len() - MAX_PENDING_SAMPLES;
        pending.drain(..extra);
    }
}

/// Next chunk that fits in `space` device-rate samples, or `None` if the
/// pending audio has to wait. Does not drop samples.
fn take_ready_chunk(
    pending: &mut VecDeque<f32>,
    space: usize,
    output_rate: u32,
) -> Option<Vec<f32>> {
    let n = pending.len().min(SPEAKER_CHUNK);
    if n == 0 || space < Resampler::max_outputs(24_000, output_rate, n) {
        return None;
    }
    Some(pending.drain(..n).collect())
}

fn restore_chunk(pending: &mut VecDeque<f32>, chunk: Vec<f32>) {
    let mut restored = VecDeque::from(chunk);
    restored.append(pending);
    *pending = restored;
}

fn playback_queued_ms(pending_samples: usize, device_samples: usize, device_rate: u32) -> u32 {
    let pending_ms = pending_samples as u64 * 1_000 / 24_000;
    let device_ms = device_samples as u64 * 1_000 / u64::from(device_rate.max(1));
    u32::try_from(pending_ms.saturating_add(device_ms)).unwrap_or(u32::MAX)
}

fn level_u16(samples: &[f32]) -> u16 {
    (level(samples) * u16::MAX as f32) as u16
}

/// Records speaker PCM and reports an empty playback queue.
#[cfg(test)]
pub(crate) struct NullDuplex {
    pub pcm: Arc<Mutex<Vec<i16>>>,
    muted: bool,
}

#[cfg(test)]
impl Default for NullDuplex {
    fn default() -> Self {
        Self {
            pcm: Arc::new(Mutex::new(Vec::new())),
            muted: false,
        }
    }
}

#[cfg(test)]
impl Duplex for NullDuplex {
    fn open(&mut self) -> Result<(), VoiceRejection> {
        Ok(())
    }

    fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    fn pull_mic(&mut self, _out: &mut Vec<f32>) -> Result<(), VoiceRejection> {
        Ok(())
    }

    fn push_speaker(&mut self, pcm: &[i16]) {
        self.pcm.lock().expect("pcm").extend_from_slice(pcm);
    }

    fn clear_speaker(&mut self) {
        self.pcm.lock().expect("pcm").clear();
    }

    fn levels(&mut self) -> (u16, u16) {
        let _ = self.muted;
        (0, 0)
    }

    fn speaker_queued_ms(&self) -> u32 {
        0
    }

    fn close(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_one_second_burst_plays_in_order_when_the_ring_is_small() {
        let mut pending = VecDeque::new();
        pending.extend((0..24_000).map(|i| i as f32));
        let mut played = Vec::new();
        // One 20 ms chunk fits; the other 980 ms has to wait.
        let room = Resampler::max_outputs(24_000, 48_000, SPEAKER_CHUNK);
        let first = take_ready_chunk(&mut pending, room, 48_000).expect("first chunk");
        assert_eq!(first.len(), SPEAKER_CHUNK);
        played.extend(first);
        assert!(take_ready_chunk(&mut pending, 0, 48_000).is_none());
        assert_eq!(pending.len(), 24_000 - SPEAKER_CHUNK);
        while let Some(chunk) = take_ready_chunk(&mut pending, usize::MAX, 48_000) {
            played.extend(chunk);
        }
        assert_eq!(played.len(), 24_000);
        assert_eq!(played[0], 0.0);
        assert_eq!(played[23_999], 23_999.0);
    }

    #[test]
    fn queued_time_includes_audio_the_device_has_not_taken_yet() {
        assert_eq!(playback_queued_ms(24_000, 0, 48_000), 1_000);
        assert_eq!(playback_queued_ms(0, 48_000, 48_000), 1_000);
        assert_eq!(playback_queued_ms(24_000, 48_000, 48_000), 2_000);
    }

    #[test]
    fn the_speaker_queue_drops_the_oldest_past_one_minute() {
        let mut pending = VecDeque::new();
        pending.push_back(1.0);
        pending.extend(std::iter::repeat_n(0.0, MAX_PENDING_SAMPLES));
        trim_pending(&mut pending);
        assert_eq!(pending.len(), MAX_PENDING_SAMPLES);
        assert_eq!(pending[0], 0.0);
    }
}
