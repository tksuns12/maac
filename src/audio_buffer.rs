//! Immutable decoded PCM and bounded, frame-oriented source views.

use crate::{audio_asset::AudioAsset, plan::PlanError};
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct AudioBuffer {
    rate_hz: u32,
    channels: u8,
    frames: u64,
    samples: Arc<[f32]>,
}

impl AudioBuffer {
    pub(crate) fn from_asset(asset: &AudioAsset) -> Result<Self, PlanError> {
        let samples = asset.decode()?;
        Ok(Self {
            rate_hz: asset.rate_hz,
            channels: asset.channels,
            frames: asset.frames,
            samples,
        })
    }
    pub(crate) fn rate_hz(&self) -> u32 {
        self.rate_hz
    }
    pub(crate) fn channels(&self) -> u8 {
        self.channels
    }
    pub(crate) fn frames(&self) -> u64 {
        self.frames
    }
    pub(crate) fn byte_len(&self) -> usize {
        self.samples.len() * 4
    }

    pub(crate) fn slice(
        &self,
        start: u64,
        end: u64,
        reverse: bool,
    ) -> Result<AudioSlice<'_>, PlanError> {
        if start >= end || end > self.frames {
            return Err(PlanError {
                code: "E_RANGE".into(),
                path: "audio.source".into(),
                message: "source slice must satisfy 0 <= start < end <= asset frames".into(),
                span: None,
            });
        }
        Ok(AudioSlice {
            buffer: self,
            start,
            end,
            reverse,
        })
    }

    /// Own a validated view without copying or decoding the shared PCM again.
    pub(crate) fn owned_slice(
        self: &Arc<Self>,
        start: u64,
        end: u64,
        reverse: bool,
    ) -> Result<OwnedAudioSlice, PlanError> {
        self.slice(start, end, reverse)?;
        Ok(OwnedAudioSlice {
            buffer: Arc::clone(self),
            start,
            end,
            reverse,
        })
    }

    /// Whole-buffer sampling also supports the empty assets accepted by kits.
    pub(crate) fn interpolate(&self, index: u64, fraction: f64, channel: usize) -> f64 {
        AudioSlice {
            buffer: self,
            start: 0,
            end: self.frames,
            reverse: false,
        }
        .interpolate(index, fraction, channel)
    }
}

#[derive(Debug)]
pub(crate) struct OwnedAudioSlice {
    buffer: Arc<AudioBuffer>,
    start: u64,
    end: u64,
    reverse: bool,
}
impl OwnedAudioSlice {
    pub(crate) fn channels(&self) -> u8 {
        self.buffer.channels
    }
    pub(crate) fn interpolate(&self, index: u64, fraction: f64, channel: usize) -> f64 {
        AudioSlice {
            buffer: &self.buffer,
            start: self.start,
            end: self.end,
            reverse: self.reverse,
        }
        .interpolate(index, fraction, channel)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AudioSlice<'a> {
    buffer: &'a AudioBuffer,
    start: u64,
    end: u64,
    reverse: bool,
}

impl AudioSlice<'_> {
    fn sample(&self, index: u64, channel: usize) -> f64 {
        if index >= self.end - self.start {
            return 0.;
        }
        let frame = if self.reverse {
            self.end - 1 - index
        } else {
            self.start + index
        };
        f64::from(self.buffer.samples[frame as usize * self.buffer.channels as usize + channel])
    }

    /// The caller supplies a normalized fractional coordinate and valid channel.
    /// These are programmer invariants, not unchecked source-document input.
    pub(crate) fn interpolate(&self, index: u64, fraction: f64, channel: usize) -> f64 {
        assert!(fraction.is_finite() && (0.0..1.0).contains(&fraction));
        assert!(channel < self.buffer.channels as usize);
        if index >= self.end - self.start {
            return 0.;
        }
        let left = self.sample(index, channel);
        let right = self.sample(index + 1, channel);
        (1. - fraction) * left + fraction * right
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        audio_asset::{AudioAsset, CORE_AUDIO_FORMAT},
        bundle::sha256_digest,
    };

    fn buffer(channels: u8, values: &[f32]) -> AudioBuffer {
        let bytes: Vec<_> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        AudioBuffer::from_asset(&AudioAsset {
            id: "sample".into(),
            format: CORE_AUDIO_FORMAT.into(),
            rate_hz: 24000,
            channels,
            frames: (values.len() / channels as usize) as u64,
            hash: sha256_digest(&bytes),
            bytes,
        })
        .unwrap()
    }

    #[test]
    fn owned_slices_share_one_decoded_buffer_and_validate_once() {
        let buffer = Arc::new(buffer(1, &[1., 2., 4.]));
        let first = buffer.owned_slice(0, 3, false).unwrap();
        let second = buffer.owned_slice(1, 3, true).unwrap();
        assert!(Arc::ptr_eq(&first.buffer, &buffer));
        assert!(Arc::ptr_eq(&second.buffer, &buffer));
        assert_eq!(Arc::strong_count(&buffer), 3);
        assert_eq!(second.interpolate(0, 0.5, 0), 3.);
        assert!(buffer.owned_slice(1, 4, false).is_err());
    }
    #[test]
    fn mono_slice_interpolation_has_zero_neighbors_in_both_directions() {
        let b = buffer(1, &[99., 2., 6., 88.]);
        for (reverse, expected) in [(false, [2., 4., 6., 3.]), (true, [6., 4., 2., 1.])] {
            let slice = b.slice(1, 3, reverse).unwrap();
            for ((index, fraction), value) in [(0, 0.), (0, 0.5), (1, 0.), (1, 0.5)]
                .into_iter()
                .zip(expected)
            {
                assert_eq!(slice.interpolate(index, fraction, 0), value);
            }
            assert_eq!(slice.interpolate(2, 0.5, 0), 0.);
            assert_eq!(slice.interpolate(u64::MAX, 0.5, 0), 0.);
        }
    }
    #[test]
    fn stereo_reverse_preserves_channel_order_and_slice_edges() {
        let b = buffer(2, &[99., 98., 2., 20., 6., 60., 88., 87.]);
        for (reverse, first, last) in [(false, [2., 20.], [6., 60.]), (true, [6., 60.], [2., 20.])]
        {
            let slice = b.slice(1, 3, reverse).unwrap();
            for channel in 0..2 {
                assert_eq!(slice.interpolate(0, 0., channel), first[channel]);
                assert_eq!(slice.interpolate(0, 0.5, channel), [4., 40.][channel]);
                assert_eq!(slice.interpolate(1, 0.5, channel), last[channel] * 0.5);
            }
        }
    }
    #[test]
    fn decoded_bits_and_metadata_are_preserved() {
        let bits = [
            0,
            0x80000000,
            1,
            0x80000001,
            2f32.to_bits(),
            (-3f32).to_bits(),
        ];
        let values: Vec<_> = bits.into_iter().map(f32::from_bits).collect();
        let b = buffer(2, &values);
        assert_eq!(
            (b.rate_hz(), b.channels(), b.frames(), b.byte_len()),
            (24000, 2, 3, 24)
        );
        assert_eq!(
            b.samples.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
            bits
        );
    }
    #[test]
    fn rejects_empty_reversed_and_out_of_asset_slices() {
        let b = buffer(1, &[1., 2.]);
        for (a, z) in [(0, 0), (1, 1), (2, 1), (0, 3), (u64::MAX, u64::MAX)] {
            assert_eq!(b.slice(a, z, false).unwrap_err().code, "E_RANGE");
        }
        assert!(buffer(1, &[]).slice(0, 1, false).is_err());
    }
}
