//! Exact core PCM assets, validated independently of filesystem state.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::bundle::{
    sha256_digest, MAX_BUNDLE_ASSETS, MAX_BUNDLE_ASSET_BYTES, MAX_BUNDLE_FILE_BYTES,
};
use crate::plan::PlanError;

pub(crate) const CORE_AUDIO_FORMAT: &str = "pcm_f32le_interleaved/1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AudioAsset {
    pub id: String,
    pub format: String,
    pub rate_hz: u32,
    pub channels: u8,
    pub frames: u64,
    pub hash: String,
    pub bytes: Vec<u8>,
}

impl AudioAsset {
    pub(crate) fn validate(&self) -> Result<(), PlanError> {
        let fail = |code, message| error(code, &self.id, message);
        if self.bytes.len() > MAX_BUNDLE_FILE_BYTES {
            return Err(fail(
                "E_RESOURCE_LIMIT",
                "audio asset exceeds per-file byte limit",
            ));
        }
        crate::plan::validate_identifier(&self.id, "audio_assets.id")?;
        if self.format != CORE_AUDIO_FORMAT {
            return Err(fail("E_ASSET", "unsupported core audio format"));
        }
        if self.rate_hz == 0
            || !(1..=crate::plan::PlanLimits::MAX_CHANNELS).contains(&self.channels)
        {
            return Err(fail(
                "E_ASSET",
                "audio asset requires positive rate and supported channels",
            ));
        }
        let expected = self
            .frames
            .checked_mul(u64::from(self.channels))
            .and_then(|count| count.checked_mul(4))
            .ok_or_else(|| fail("E_RESOURCE_LIMIT", "audio asset byte length overflow"))?;
        if expected != self.bytes.len() as u64 {
            return Err(fail(
                "E_ASSET",
                "audio asset byte length differs from metadata",
            ));
        }
        if self.hash != sha256_digest(&self.bytes) {
            return Err(fail(
                "E_HASH",
                "audio asset hash differs from exact source bytes",
            ));
        }
        for bytes in self.bytes.chunks_exact(4) {
            if !f32::from_le_bytes(bytes.try_into().unwrap()).is_finite() {
                return Err(fail("E_ASSET", "audio asset sample must be finite"));
            }
        }
        Ok(())
    }

    pub(crate) fn decode(&self) -> Result<Arc<[f32]>, PlanError> {
        self.validate()?;
        let mut samples = Vec::new();
        samples
            .try_reserve_exact(self.bytes.len() / 4)
            .map_err(|_| {
                error(
                    "E_RESOURCE_LIMIT",
                    &self.id,
                    "cannot allocate decoded audio samples",
                )
            })?;
        samples.extend(
            self.bytes
                .chunks_exact(4)
                .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap())),
        );
        Ok(samples.into())
    }
}

pub(crate) fn validate_assets(assets: &[AudioAsset]) -> Result<(), PlanError> {
    let total_bytes = assets
        .iter()
        .try_fold(0usize, |total, asset| total.checked_add(asset.bytes.len()));
    if assets.len() > MAX_BUNDLE_ASSETS
        || total_bytes.is_none_or(|total| total > MAX_BUNDLE_ASSET_BYTES)
    {
        return Err(error(
            "E_RESOURCE_LIMIT",
            "audio_assets",
            "audio asset count or aggregate bytes exceed limit",
        ));
    }
    let mut ids = BTreeSet::new();
    for asset in assets {
        if !ids.insert(asset.id.as_str()) {
            return Err(error(
                "E_DUPLICATE_ID",
                &asset.id,
                "duplicate audio asset ID",
            ));
        }
        asset.validate()?;
    }
    Ok(())
}

fn error(code: &str, id: &str, message: &str) -> PlanError {
    PlanError {
        code: code.into(),
        path: format!("audio_assets.{id}"),
        message: message.into(),
        span: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(bits: &[u32]) -> AudioAsset {
        let bytes: Vec<u8> = bits.iter().flat_map(|bits| bits.to_le_bytes()).collect();
        AudioAsset {
            id: "sample".into(),
            format: CORE_AUDIO_FORMAT.into(),
            rate_hz: 48000,
            channels: 1,
            frames: bits.len() as u64,
            hash: sha256_digest(&bytes),
            bytes,
        }
    }

    #[test]
    fn preserves_exact_finite_source_bits_and_empty_assets() {
        let bits = [
            0,
            0x80000000,
            1,
            0x80000001,
            2f32.to_bits(),
            (-3f32).to_bits(),
        ];
        let source = asset(&bits);
        assert_eq!(
            source
                .decode()
                .unwrap()
                .iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>(),
            bits
        );
        assert!(asset(&[]).decode().unwrap().is_empty());
    }

    #[test]
    fn rejects_nonfinite_samples() {
        for bits in [
            f32::NAN.to_bits(),
            f32::INFINITY.to_bits(),
            f32::NEG_INFINITY.to_bits(),
        ] {
            assert_eq!(asset(&[bits]).decode().unwrap_err().code, "E_ASSET");
        }
    }

    #[test]
    fn rejects_invalid_metadata_identity_and_length_before_decoding() {
        let original = asset(&[0]);
        for field in ["format", "rate", "channels", "frames", "hash", "overflow"] {
            let mut source = original.clone();
            match field {
                "format" => source.format = "wav".into(),
                "rate" => source.rate_hz = 0,
                "channels" => source.channels = 3,
                "frames" => source.frames = 2,
                "hash" => source.hash = sha256_digest(b"different"),
                "overflow" => source.frames = u64::MAX,
                _ => unreachable!(),
            }
            assert!(source.decode().is_err(), "{field}");
        }
        let mut source = original;
        source.channels = 0;
        assert!(source.validate().is_err());
    }

    #[test]
    fn rejects_oversized_and_duplicate_resource_sets() {
        let mut oversized = asset(&[]);
        oversized.bytes = vec![0; MAX_BUNDLE_FILE_BYTES + 4];
        assert_eq!(oversized.validate().unwrap_err().code, "E_RESOURCE_LIMIT");
        assert!(validate_assets(&vec![asset(&[]); MAX_BUNDLE_ASSETS + 1]).is_err());
        assert!(validate_assets(&[asset(&[]), asset(&[])]).is_err());
        let assets: Vec<_> = (0..5)
            .map(|index| {
                let mut source = asset(&[]);
                source.id = format!("sample{index}");
                source.bytes = vec![0; MAX_BUNDLE_FILE_BYTES];
                source.frames = (source.bytes.len() / 4) as u64;
                source.hash = sha256_digest(&source.bytes);
                source
            })
            .collect();
        assert_eq!(
            validate_assets(&assets).unwrap_err().code,
            "E_RESOURCE_LIMIT"
        );
    }
}
