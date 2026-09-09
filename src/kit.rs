//! Native one-shot sample voices, independent of score scheduling.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use crate::plan::{PlanError, PlanLimits};

const ENGINE_RATE: u128 = 48_000;

pub(crate) type KitSample = crate::audio_buffer::AudioBuffer;

#[derive(Debug)]
struct Voice {
    sample: Arc<KitSample>,
    onset: u64,
    velocity: f64,
    address: String,
}
impl Voice {
    fn coordinate(&self, frame: u64) -> Option<(u64, f64)> {
        let elapsed = frame.checked_sub(self.onset)?;
        let numerator = u128::from(elapsed) * u128::from(self.sample.rate_hz());
        let index = numerator / ENGINE_RATE;
        if index >= u128::from(self.sample.frames()) {
            return None;
        }
        Some((
            index as u64,
            (numerator % ENGINE_RATE) as f64 / ENGINE_RATE as f64,
        ))
    }
    fn expired(&self, frame: u64) -> bool {
        frame >= self.onset && self.coordinate(frame).is_none()
    }
}

#[derive(Debug)]
pub(crate) struct KitRuntime {
    channels: u8,
    capacity: usize,
    samples: BTreeMap<String, Arc<KitSample>>,
    voices: Vec<Voice>,
}
impl KitRuntime {
    pub(crate) fn new(
        channels: u8,
        capacity: u32,
        samples: BTreeMap<String, Arc<KitSample>>,
    ) -> Result<Self, PlanError> {
        if !(1..=2).contains(&channels) || capacity == 0 {
            return Err(error(
                "E_RANGE",
                "kit requires mono/stereo channels and positive voices",
            ));
        }
        if capacity > PlanLimits::MAX_VOICES {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "kit capacity or sample count exceeds limit",
            ));
        }
        if samples.is_empty() {
            return Err(error("E_REFERENCE", "kit requires samples"));
        }
        let mut bytes = 0usize;
        let mut unique = BTreeSet::new();
        let mut key_bytes = 0usize;
        for (key, sample) in &samples {
            if key.len() > PlanLimits::MAX_STRING_BYTES {
                return Err(error("E_RESOURCE_LIMIT", "kit key exceeds length limit"));
            }
            if sample.channels() != channels {
                return Err(error("E_PORT_TYPE", "sample channels differ from kit"));
            }
            key_bytes = key_bytes
                .checked_add(key.len())
                .ok_or_else(|| error("E_RESOURCE_LIMIT", "key storage overflow"))?;
            if unique.insert(Arc::as_ptr(sample)) {
                bytes = bytes
                    .checked_add(sample.byte_len())
                    .ok_or_else(|| error("E_RESOURCE_LIMIT", "sample storage overflow"))?;
            }
        }
        if unique.len() > crate::bundle::MAX_BUNDLE_ASSETS
            || key_bytes > PlanLimits::MAX_TOTAL_STRING_BYTES
            || samples.len() > PlanLimits::MAX_EVENTS
            || bytes > crate::bundle::MAX_BUNDLE_ASSET_BYTES
        {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "kit sample storage exceeds limit",
            ));
        }
        let voices = Vec::new();
        Ok(Self {
            channels,
            capacity: capacity as usize,
            samples,
            voices,
        })
    }
    pub(crate) fn onset(
        &mut self,
        frame: u64,
        key: &str,
        velocity: f64,
        address: &str,
    ) -> Result<(), PlanError> {
        if !velocity.is_finite() || !(0.0..=1.0).contains(&velocity) {
            return Err(error("E_RANGE", "hit velocity must be finite in [0,1]"));
        }
        if address.is_empty() || address.len() > PlanLimits::MAX_STRING_BYTES {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "hit address is empty or too long",
            ));
        }
        let sample = self
            .samples
            .get(key)
            .ok_or_else(|| error("E_REFERENCE", "unknown kit key"))?
            .clone();
        self.voices.retain(|voice| !voice.expired(frame));
        if sample.frames() == 0 {
            return Ok(());
        }
        if self.voices.iter().any(|voice| voice.address == address) {
            return Err(error("E_DUPLICATE_ID", "duplicate active hit address"));
        }
        if self.voices.len() >= self.capacity {
            return Err(error("E_VOICE_LIMIT", "kit voice capacity exceeded"));
        }
        let position = self
            .voices
            .binary_search_by(|voice| voice.address.as_bytes().cmp(address.as_bytes()))
            .unwrap_or_else(|position| position);
        self.voices
            .try_reserve(1)
            .map_err(|_| error("E_RESOURCE_LIMIT", "cannot allocate kit voice"))?;
        let mut owned_address = String::new();
        owned_address
            .try_reserve_exact(address.len())
            .map_err(|_| error("E_RESOURCE_LIMIT", "cannot allocate hit address"))?;
        owned_address.push_str(address);
        self.voices.insert(
            position,
            Voice {
                sample,
                onset: frame,
                velocity,
                address: owned_address,
            },
        );
        Ok(())
    }
    pub(crate) fn render_frame(&mut self, frame: u64, level: f64) -> Result<[f64; 2], PlanError> {
        if !level.is_finite() || level < 0.0 {
            return Err(error("E_RANGE", "kit level must be finite and nonnegative"));
        }
        self.voices.retain(|voice| !voice.expired(frame));
        let mut output = [0.; 2];
        for voice in &self.voices {
            let Some((index, fraction)) = voice.coordinate(frame) else {
                continue;
            };
            for (channel, sum) in output.iter_mut().enumerate().take(self.channels as usize) {
                *sum += voice.sample.interpolate(index, fraction, channel) * voice.velocity * level;
                if !sum.is_finite() {
                    return Err(error("E_NONFINITE", "kit output is nonfinite"));
                }
            }
        }
        Ok(output)
    }
    pub(crate) fn reset(&mut self) {
        self.voices.clear();
    }
}
fn error(code: &str, message: &str) -> PlanError {
    PlanError {
        code: code.into(),
        path: "kit".into(),
        message: message.into(),
        span: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_asset::{AudioAsset, CORE_AUDIO_FORMAT};
    use crate::bundle::sha256_digest;

    fn sample(rate: u32, channels: u8, values: &[f32]) -> Arc<KitSample> {
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        Arc::new(
            KitSample::from_asset(&AudioAsset {
                id: "sample".into(),
                format: CORE_AUDIO_FORMAT.into(),
                rate_hz: rate,
                channels,
                frames: (values.len() / channels as usize) as u64,
                hash: sha256_digest(&bytes),
                bytes,
            })
            .unwrap(),
        )
    }
    fn runtime(rate: u32, channels: u8, values: &[f32], capacity: u32) -> KitRuntime {
        KitRuntime::new(
            channels,
            capacity,
            BTreeMap::from([("key".into(), sample(rate, channels, values))]),
        )
        .unwrap()
    }
    #[test]
    fn equal_rate_stereo_and_live_level_reset() {
        let mut kit = runtime(48000, 2, &[1., -2., 3., 4.], 1);
        kit.onset(5, "key", 0.5, "a").unwrap();
        assert_eq!(kit.render_frame(5, 2.).unwrap(), [1., -2.]);
        assert_eq!(kit.render_frame(6, 1.).unwrap(), [1.5, 2.]);
        assert_eq!(kit.render_frame(7, 1.).unwrap(), [0., 0.]);
        kit.reset();
        kit.onset(5, "key", 0.5, "a").unwrap();
        assert_eq!(kit.render_frame(5, 2.).unwrap(), [1., -2.]);
    }
    #[test]
    fn unequal_rates_interpolate_last_sample_to_zero() {
        let mut kit = runtime(24000, 1, &[2., 4.], 1);
        kit.onset(0, "key", 1., "a").unwrap();
        for (frame, expected) in [2., 3., 4., 2., 0.].into_iter().enumerate() {
            assert_eq!(kit.render_frame(frame as u64, 1.).unwrap(), [expected, 0.]);
        }
        let mut fast = runtime(72000, 1, &[2., 4., 8.], 1);
        fast.onset(0, "key", 1., "a").unwrap();
        assert_eq!(fast.render_frame(1, 1.).unwrap(), [6., 0.]);
        assert_eq!(fast.render_frame(2, 1.).unwrap(), [0., 0.]);
    }
    #[test]
    fn empty_silent_expiry_and_overflow() {
        let mut empty = runtime(48000, 1, &[], 1);
        empty.onset(0, "key", 1., "a").unwrap();
        empty.onset(0, "key", 1., "b").unwrap();
        assert!(empty.onset(0, "missing", 1., "c").is_err());
        let mut kit = runtime(24000, 1, &[1.], 1);
        kit.onset(0, "key", 0., "a").unwrap();
        assert_eq!(
            kit.onset(1, "key", 1., "b").unwrap_err().code,
            "E_VOICE_LIMIT"
        );
        kit.onset(2, "key", 1., "b").unwrap();
        assert_eq!(kit.render_frame(2, 1.).unwrap(), [1., 0.]);
    }
    #[test]
    fn exact_coordinates_at_large_frames_and_shared_empty_keys() {
        let start = u64::MAX - 4;
        let shared = sample(16000, 1, &[3., 6.]);
        let map = BTreeMap::from([("".into(), shared.clone()), ("alias".into(), shared)]);
        let mut kit = KitRuntime::new(1, 1, map).unwrap();
        kit.onset(start, "", 1., "a").unwrap();
        assert_eq!(kit.render_frame(start + 1, 1.).unwrap(), [4., 0.]);
        assert_eq!(kit.render_frame(start + 2, 1.).unwrap(), [5., 0.]);
        kit.reset();
        assert_eq!(kit.render_frame(start + 2, 1.).unwrap(), [0., 0.]);
        let mut fast = runtime(u32::MAX, 1, &[1.], 1);
        fast.onset(0, "key", 1., "a").unwrap();
        assert_eq!(fast.render_frame(u64::MAX, 1.).unwrap(), [0., 0.]);
    }
    #[test]
    fn sums_in_address_order_and_rejects_invalid_inputs() {
        let map = BTreeMap::from([
            ("large".into(), sample(48000, 1, &[1e20])),
            ("negative".into(), sample(48000, 1, &[-1e20])),
            ("small".into(), sample(48000, 1, &[1.])),
        ]);
        let mut kit = KitRuntime::new(1, 3, map.clone()).unwrap();
        kit.onset(0, "small", 1., "c").unwrap();
        kit.onset(0, "negative", 1., "b").unwrap();
        kit.onset(0, "large", 1., "a").unwrap();
        assert_eq!(kit.render_frame(0, 1.).unwrap(), [1., 0.]);
        for velocity in [-1., 1.1, f64::NAN, f64::INFINITY] {
            assert!(kit.onset(0, "small", velocity, "d").is_err());
        }
        for level in [-1., f64::NAN, f64::INFINITY, f64::MAX] {
            assert!(kit.render_frame(0, level).is_err());
        }
        assert!(KitRuntime::new(1, 0, map.clone()).is_err());
        assert!(KitRuntime::new(1, PlanLimits::MAX_VOICES + 1, map.clone()).is_err());
        assert!(KitRuntime::new(2, 1, map).is_err());
    }
}
