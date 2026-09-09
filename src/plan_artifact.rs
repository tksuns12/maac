//! Opaque, independently validated standalone plan artifacts.

use crate::plan::{err, OutputSettings, PlanError, PlanLimits, PlanView};
use crate::plan_v3::VersionedPlan;
use crate::plan_v4::PlanV4;
use serde::Deserialize;

/// A self-contained plan with an opaque versioned representation.
/// Public loading and encoding independently validate the artifact.
/// Use [`Self::to_json`] to encode; generic serde serialization is deliberately
/// unavailable so converting a mutable legacy plan cannot bypass validation.
///
/// ```compile_fail,E0277
/// fn encode_without_validation(artifact: &maac::PlanArtifact) {
///     let _ = serde_json::to_vec(artifact);
/// }
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct PlanArtifact {
    inner: ArtifactVersion,
}
#[derive(Clone, Debug, PartialEq)]
enum ArtifactVersion {
    Existing(VersionedPlan),
    V4(PlanV4),
    V5(crate::plan_v5::PlanV5),
    V6(crate::plan_v6::PlanV6),
}
impl PlanArtifact {
    pub fn from_json(bytes: &[u8]) -> Result<Self, PlanError> {
        Self::from_json_with_limits(bytes, &PlanLimits::default())
    }
    pub fn from_json_with_limits(bytes: &[u8], limits: &PlanLimits) -> Result<Self, PlanError> {
        let limits = limits.bounded();
        check_bytes(bytes, &limits)?;
        #[derive(Deserialize)]
        struct Version {
            version: u32,
        }
        let version: Version = serde_json::from_slice(bytes).map_err(json_error)?;
        let inner = match version.version {
            1..=3 => {
                ArtifactVersion::Existing(VersionedPlan::from_json_with_limits(bytes, &limits)?)
            }
            6 => {
                let plan = crate::plan_v6::PlanV6::decode_wire(bytes).map_err(json_error)?;
                plan.validate_with_limits(&limits)?;
                ArtifactVersion::V6(plan)
            }
            5 => {
                let plan = crate::plan_v5::PlanV5::decode_wire(bytes).map_err(json_error)?;
                plan.validate_with_limits(&limits)?;
                ArtifactVersion::V5(plan)
            }
            4 => {
                let plan = PlanV4::decode_wire(bytes).map_err(json_error)?;
                plan.validate_with_limits(&limits)?;
                ArtifactVersion::V4(plan)
            }
            _ => {
                return Err(err(
                    "E_VERSION",
                    "version",
                    "unsupported performance-plan version",
                ))
            }
        };
        Ok(Self { inner })
    }
    pub fn to_json(&self) -> Result<Vec<u8>, PlanError> {
        self.to_json_with_limits(&PlanLimits::default())
    }
    pub fn to_json_with_limits(&self, limits: &PlanLimits) -> Result<Vec<u8>, PlanError> {
        match &self.inner {
            ArtifactVersion::Existing(plan) => plan.to_json_with_limits(limits),
            ArtifactVersion::V4(plan) => {
                let limits = limits.bounded();
                self.validate_with_limits(&limits)?;
                let bytes = serde_json::to_vec(plan).map_err(json_error)?;
                check_bytes(&bytes, &limits)?;
                Ok(bytes)
            }
            ArtifactVersion::V6(plan) => {
                let limits = limits.bounded();
                self.validate_with_limits(&limits)?;
                let bytes = serde_json::to_vec(plan).map_err(json_error)?;
                check_bytes(&bytes, &limits)?;
                Ok(bytes)
            }
            ArtifactVersion::V5(plan) => {
                let limits = limits.bounded();
                self.validate_with_limits(&limits)?;
                let bytes = serde_json::to_vec(plan).map_err(json_error)?;
                check_bytes(&bytes, &limits)?;
                Ok(bytes)
            }
        }
    }
    pub fn validate(&self) -> Result<(), PlanError> {
        self.validate_with_limits(&PlanLimits::default())
    }
    pub fn validate_with_limits(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        match &self.inner {
            ArtifactVersion::Existing(VersionedPlan::Legacy(plan)) => {
                plan.validate_with_limits(limits)
            }
            ArtifactVersion::Existing(VersionedPlan::V3(plan)) => plan.validate_with_limits(limits),
            ArtifactVersion::V4(plan) => plan.validate_with_limits(limits),
            ArtifactVersion::V5(plan) => plan.validate_with_limits(limits),
            ArtifactVersion::V6(plan) => plan.validate_with_limits(limits),
        }
    }
    pub fn version(&self) -> u32 {
        self.view().version
    }
    pub fn output(&self) -> &OutputSettings {
        self.view().output
    }
    pub fn audio_clip_count(&self) -> usize {
        self.view()
            .nodes
            .iter()
            .filter(|node| {
                matches!(
                    node.processor,
                    crate::plan::ProcessorView::Audio(_) | crate::plan::ProcessorView::WarpRate(_)
                )
            })
            .count()
    }
    pub(crate) fn from_v6(plan: crate::plan_v6::PlanV6) -> Self {
        Self {
            inner: ArtifactVersion::V6(plan),
        }
    }
    pub(crate) fn from_v5(plan: crate::plan_v5::PlanV5) -> Self {
        Self {
            inner: ArtifactVersion::V5(plan),
        }
    }
    pub fn event_count(&self) -> usize {
        self.view().events().len()
    }
    pub(crate) fn view(&self) -> PlanView<'_> {
        match &self.inner {
            ArtifactVersion::Existing(VersionedPlan::Legacy(plan)) => plan.view(),
            ArtifactVersion::Existing(VersionedPlan::V3(plan)) => plan.view(),
            ArtifactVersion::V4(plan) => plan.view(),
            ArtifactVersion::V5(plan) => plan.view(),
            ArtifactVersion::V6(plan) => plan.view(),
        }
    }
    #[cfg(test)]
    pub(crate) fn access_v4(&self) -> Option<&PlanV4> {
        match &self.inner {
            ArtifactVersion::V4(plan) => Some(plan),
            _ => None,
        }
    }
    /// Wrap a V4 plan after the internal caller has validated it. Compilation
    /// must reuse its existing TimingContext before calling this constructor.
    pub(crate) fn from_v4(plan: PlanV4) -> Self {
        Self {
            inner: ArtifactVersion::V4(plan),
        }
    }
}
impl From<VersionedPlan> for PlanArtifact {
    fn from(plan: VersionedPlan) -> Self {
        Self {
            inner: ArtifactVersion::Existing(plan),
        }
    }
}
fn check_bytes(bytes: &[u8], limits: &PlanLimits) -> Result<(), PlanError> {
    if bytes.len() > limits.max_json_bytes {
        return Err(err(
            "E_RESOURCE_LIMIT",
            "json",
            "plan JSON exceeds byte allowance",
        ));
    }
    Ok(())
}
fn json_error(error: serde_json::Error) -> PlanError {
    let message = error.to_string();
    err(crate::plan::serde_error_code(&message), "json", message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    fn wire() -> Value {
        let bytes = vec![0, 0, 0, 128, 1, 0, 0, 0];
        json!({"version":4,"output":{"score_start_q":"0/1","score_end_q":"4/1","tail_seconds":"1/1","sample_rate_hz":48000,"channels":1,"total_frames":144000,"output":{"node":"kit","port":"out"}},"tempo":{"points":[{"q":"0/1","bpm":"120/1","shape":"step"}]},"nodes":[{"id":"kit","processor":{"kind":"kit","channels":1,"voices":2,"samples":[{"key":"","asset":"sample"}]},"params":{"level":"1/1"}}],"audio_assets":[{"id":"sample","format":"pcm_f32le_interleaved/1","rate_hz":24000,"channels":1,"frames":2,"hash":crate::bundle::sha256_digest(&bytes),"bytes":bytes}],"events":[{"address":"main/hit","source":{"object":"hit","path":["main","hit"]},"target":{"node":"kit","port":"events"},"kind":{"kind":"hit","key":"","velocity":"1/1"},"score_on_q":"0/1","onset_offset_seconds":"0/1","release_offset_seconds":"0/1","release_velocity":0.0,"on_frame":0}]})
    }
    fn load(v: &Value) -> Result<PlanArtifact, crate::plan::PlanError> {
        PlanArtifact::from_json(&serde_json::to_vec(v).unwrap())
    }
    #[test]
    fn embedded_hit_roundtrips_step_ramp_and_negative_origin() {
        for (start, end, tempo, frames) in [
            (
                "0/1",
                "4/1",
                json!([{"q":"0/1","bpm":"120/1","shape":"step"}]),
                144000,
            ),
            (
                "0/1",
                "4/1",
                json!([{"q":"0/1","bpm":"120/1","shape":"linear"},{"q":"4/1","bpm":"240/1","shape":"step"}]),
                114543,
            ),
            (
                "-4/1",
                "0/1",
                json!([{"q":"-4/1","bpm":"120/1","shape":"step"}]),
                144000,
            ),
        ] {
            let mut v = wire();
            v["output"]["score_start_q"] = json!(start);
            v["output"]["score_end_q"] = json!(end);
            v["output"]["total_frames"] = json!(frames);
            v["tempo"]["points"] = tempo;
            v["events"][0]["score_on_q"] = json!(start);
            let p = load(&v).unwrap();
            assert_eq!(p.version(), 4);
            assert_eq!(p.event_count(), 1);
            assert_eq!(p.output().total_frames, frames);
            let encoded = p.to_json().unwrap();
            assert_eq!(
                PlanArtifact::from_json(&encoded)
                    .unwrap()
                    .to_json()
                    .unwrap(),
                encoded
            );
        }
    }
    #[test]
    fn rejects_invalid_assets_keys_receivers_and_hit_off_fields() {
        for (pointer, value) in [
            ("/events/0/kind/key", json!("missing")),
            ("/events/0/target/port", json!("out")),
            ("/events/0/kind/velocity", json!("2/1")),
            ("/events/0/release_offset_seconds", json!("1/1")),
            ("/events/0/release_velocity", json!(0.5)),
            ("/events/0/on_frame", json!(1)),
            ("/audio_assets/0/frames", json!(3)),
            ("/audio_assets/0/hash", json!("bad")),
            ("/audio_assets/0/channels", json!(2)),
            ("/audio_assets/0/format", json!("wav")),
            ("/nodes/0/params/level", json!("-1/1")),
        ] {
            let mut v = wire();
            *v.pointer_mut(pointer).unwrap() = value;
            assert!(load(&v).is_err(), "{pointer}");
        }
        for (name, value) in [("off_frame", json!(1)), ("score_off_q", json!("1/1"))] {
            let mut v = wire();
            v["events"][0][name] = value;
            assert!(load(&v).is_err());
        }
        let mut v = wire();
        let sample = v["nodes"][0]["processor"]["samples"][0].clone();
        v["nodes"][0]["processor"]["samples"]
            .as_array_mut()
            .unwrap()
            .push(sample);
        assert!(load(&v).is_err());
        let mut v = wire();
        v["nodes"][0]["processor"] = json!({"kind":"core","processor":{"kind":"sine","voices":2}});
        assert!(load(&v).is_err());
        let mut v = wire();
        v["events"][0]["kind"] = json!({"kind":"note","pitch_hz":440.0,"velocity":"1/1"});
        assert!(load(&v).is_err());
        let mut v = wire();
        let a = v["audio_assets"][0].clone();
        v["audio_assets"].as_array_mut().unwrap().push(a);
        assert!(load(&v).is_err());
    }
    #[test]
    fn rejects_hit_quantizing_into_tail_and_preserves_zero_assets() {
        let mut v = wire();
        v["events"][0]["score_on_q"] = json!("383999/96000");
        v["events"][0]["on_frame"] = json!(96000);
        assert_eq!(load(&v).unwrap_err().code, "E_INTERVAL");
        let mut v = wire();
        v["audio_assets"][0]["frames"] = json!(0);
        v["audio_assets"][0]["bytes"] = json!([]);
        v["audio_assets"][0]["hash"] = json!(crate::bundle::sha256_digest(&[]));
        load(&v).unwrap();
    }
    #[test]
    fn bounds_static_assets_before_timing_and_charges_silent_playback() {
        let bytes = serde_json::to_vec(&wire()).unwrap();
        let mut limits = crate::plan::PlanLimits {
            max_execution_work: 720047,
            ..Default::default()
        };
        assert!(PlanArtifact::from_json_with_limits(&bytes, &limits).is_err());
        limits.max_execution_work = 720048;
        PlanArtifact::from_json_with_limits(&bytes, &limits).unwrap();
        let mut silent = wire();
        silent["events"][0]["kind"]["velocity"] = json!("0/1");
        limits.max_execution_work = 720047;
        assert!(PlanArtifact::from_json_with_limits(
            &serde_json::to_vec(&silent).unwrap(),
            &limits
        )
        .is_err());
        let mut v = wire();
        v["tempo"]["points"][0]["bpm"] = json!("0/1");
        let limits = crate::plan::PlanLimits {
            max_objects: 1,
            ..Default::default()
        };
        assert_eq!(
            PlanArtifact::from_json_with_limits(&serde_json::to_vec(&v).unwrap(), &limits)
                .unwrap_err()
                .code,
            "E_RESOURCE_LIMIT"
        );
        let limits = crate::plan::PlanLimits {
            max_json_bytes: 1,
            ..Default::default()
        };
        assert_eq!(
            PlanArtifact::from_json_with_limits(&bytes, &limits)
                .unwrap_err()
                .code,
            "E_RESOURCE_LIMIT"
        );
    }
    #[test]
    fn validates_kit_automation_channels_and_all_raw_asset_metadata() {
        let mut v = wire();
        v["automation"] = json!([{"id":"level","target":{"node":"kit","port":"level"},"clock":"score","at":{"kind":"score","q":"0/1"},"points":[{"position":"0/1","value":"0/1","shape":"linear"},{"position":"1/1","value":"2/1","shape":"step"}]}]);
        load(&v).unwrap();
        for (pointer, value) in [
            ("/automation/0/target/port", json!("gain")),
            ("/automation/0/points/0/value", json!("-1/1")),
            (
                "/automation/0/at",
                json!({"kind":"seconds","seconds":"0/1"}),
            ),
        ] {
            let mut bad = v.clone();
            *bad.pointer_mut(pointer).unwrap() = value;
            assert!(load(&bad).is_err(), "{pointer}");
        }
        for (pointer, value) in [
            ("/audio_assets/0/rate_hz", json!(0)),
            ("/nodes/0/processor/voices", json!(0)),
            ("/nodes/0/processor/channels", json!(3)),
            ("/nodes/0/processor/samples", json!([])),
        ] {
            let mut bad = wire();
            *bad.pointer_mut(pointer).unwrap() = value;
            assert!(load(&bad).is_err(), "{pointer}");
        }
        let mut bad = wire();
        let bytes: Vec<u8> = [f32::NAN.to_bits(), 0]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        bad["audio_assets"][0]["bytes"] = json!(bytes);
        bad["audio_assets"][0]["hash"] = json!(crate::bundle::sha256_digest(&bytes));
        assert_eq!(load(&bad).unwrap_err().code, "E_ASSET");
    }
    #[test]
    fn caller_asset_string_work_and_voice_limits_precede_clock_work() {
        let mut v = wire();
        v["tempo"]["points"][0]["bpm"] = json!("0/1");
        v["audio_assets"][0]["hash"] = json!("bad");
        for mut limits in [
            crate::plan::PlanLimits {
                max_work: 1,
                ..Default::default()
            },
            crate::plan::PlanLimits {
                max_string_bytes: 1,
                ..Default::default()
            },
            crate::plan::PlanLimits {
                max_total_string_bytes: 1,
                ..Default::default()
            },
        ] {
            assert_eq!(
                PlanArtifact::from_json_with_limits(&serde_json::to_vec(&v).unwrap(), &limits)
                    .unwrap_err()
                    .code,
                "E_RESOURCE_LIMIT"
            );
            limits.max_json_bytes = 1;
            assert_eq!(
                PlanArtifact::from_json_with_limits(&serde_json::to_vec(&v).unwrap(), &limits)
                    .unwrap_err()
                    .code,
                "E_RESOURCE_LIMIT"
            );
        }
        let bytes = serde_json::to_vec(&wire()).unwrap();
        for limits in [
            crate::plan::PlanLimits {
                max_voices: 1,
                ..Default::default()
            },
            crate::plan::PlanLimits {
                max_voice_work: 1,
                ..Default::default()
            },
        ] {
            assert_eq!(
                PlanArtifact::from_json_with_limits(&bytes, &limits)
                    .unwrap_err()
                    .code,
                "E_RESOURCE_LIMIT"
            );
        }
        let mut v = wire();
        v["tempo"]["points"][0]["bpm"] = json!("0/1");
        v["audio_assets"] = Value::Array(vec![
            v["audio_assets"][0].clone();
            crate::bundle::MAX_BUNDLE_ASSETS + 1
        ]);
        assert_eq!(load(&v).unwrap_err().code, "E_RESOURCE_LIMIT");
    }
    #[test]
    fn mixed_core_kit_graph_preserves_connections_and_default_level() {
        let mut v = wire();
        v["nodes"][0]["params"] = json!({});
        v["nodes"].as_array_mut().unwrap().push(json!({"id":"gain","processor":{"kind":"core","processor":{"kind":"gain","channels":1}},"params":{"gain":"1/1"}}));
        v["connections"] =
            json!([{"id":"c","from":{"node":"kit","port":"out"},"to":{"node":"gain","port":"in"}}]);
        v["output"]["output"] = json!({"node":"gain","port":"out"});
        let p = load(&v).unwrap();
        assert_eq!(
            p.view()
                .resolved_node_params(p.view().nodes.get(0).unwrap())
                .unwrap()["level"],
            crate::parse_rational("1/1").unwrap()
        );
        let encoded = p.to_json().unwrap();
        let decoded: Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded["nodes"][1]["processor"]["kind"], "core");
        assert_eq!(
            p.access_v4().unwrap().audio_assets[0].bytes,
            vec![0, 0, 0, 128, 1, 0, 0, 0]
        );
    }
    #[test]
    fn raw_asset_limits_and_hash_validation_precede_invalid_tempo() {
        let mut v = wire();
        v["tempo"]["points"][0]["bpm"] = json!("0/1");
        v["audio_assets"][0]["hash"] = json!("bad");
        assert_eq!(load(&v).unwrap_err().code, "E_HASH");
        let mut p = PlanV4::decode_wire(&serde_json::to_vec(&v).unwrap()).unwrap();
        p.audio_assets[0].bytes = vec![0; crate::bundle::MAX_BUNDLE_FILE_BYTES + 1];
        assert_eq!(
            PlanArtifact::from_v4(p).validate().unwrap_err().code,
            "E_RESOURCE_LIMIT"
        );
        let mut p = PlanV4::decode_wire(&serde_json::to_vec(&v).unwrap()).unwrap();
        p.audio_assets[0].bytes = vec![0; crate::bundle::MAX_BUNDLE_FILE_BYTES];
        p.audio_assets = vec![p.audio_assets[0].clone(); 5];
        assert_eq!(
            PlanArtifact::from_v4(p).validate().unwrap_err().code,
            "E_RESOURCE_LIMIT"
        );
    }
    #[test]
    fn audio_ids_use_maac_grammar_and_caller_limit_before_hash() {
        for id in ["bad/id", "échantillon", "1sample"] {
            let mut v = wire();
            v["audio_assets"][0]["id"] = json!(id);
            v["nodes"][0]["processor"]["samples"][0]["asset"] = json!(id);
            v["audio_assets"][0]["hash"] = json!("bad");
            assert_eq!(load(&v).unwrap_err().code, "E_RANGE", "{id}");
        }
        let mut v = wire();
        v["audio_assets"][0]["hash"] = json!("bad");
        let limits = PlanLimits {
            max_id_bytes: 5,
            ..PlanLimits::default()
        };
        assert_eq!(
            PlanArtifact::from_json_with_limits(&serde_json::to_vec(&v).unwrap(), &limits)
                .unwrap_err()
                .code,
            "E_RESOURCE_LIMIT"
        );
    }

    #[test]
    fn audio_ids_share_the_global_namespace_before_hash() {
        for kind in ["node", "connection", "lane", "region", "asset"] {
            let mut v = wire();
            match kind {
                "node" => {
                    v["audio_assets"][0]["id"] = json!("kit");
                    v["nodes"][0]["processor"]["samples"][0]["asset"] = json!("kit");
                }
                "connection" => {
                    v["connections"] = json!([{"id":"sample","from":{"node":"kit","port":"out"},"to":{"node":"kit","port":"events"}}])
                }
                "lane" => {
                    v["automation"] = json!([{"id":"sample","target":{"node":"kit","port":"level"},"clock":"score","at":{"kind":"score","q":"0/1"},"points":[{"position":"0/1","value":"1/1","shape":"step"}]}])
                }
                "region" => v["regions"] = json!([{"id":"sample","start_q":"0/1","end_q":"1/1"}]),
                "asset" => {
                    let a = v["audio_assets"][0].clone();
                    v["audio_assets"].as_array_mut().unwrap().push(a);
                }
                _ => unreachable!(),
            }
            v["audio_assets"][0]["hash"] = json!("bad");
            assert_eq!(load(&v).unwrap_err().code, "E_DUPLICATE_ID", "{kind}");
        }
    }

    #[test]
    fn hit_unused_release_requires_positive_zero_but_pcm_preserves_negative_zero() {
        let mut v = wire();
        v["events"][0]["release_velocity"] = json!(-0.0);
        assert_eq!(load(&v).unwrap_err().code, "E_INTERVAL");
        v["events"][0]["release_velocity"] = json!(0.0);
        let p = load(&v).unwrap();
        assert_eq!(
            p.access_v4().unwrap().audio_assets[0].decode().unwrap()[0].to_bits(),
            (-0.0_f32).to_bits()
        );
    }
    #[test]
    fn legacy_artifact_bytes_and_type_version_guards_are_preserved() {
        let mut legacy = wire();
        legacy["version"] = json!(1);
        legacy.as_object_mut().unwrap().remove("audio_assets");
        legacy["events"] = json!([]);
        legacy["nodes"][0]["processor"] = json!({"kind":"sine","voices":2});
        let p = crate::plan::Plan::from_json(&serde_json::to_vec(&legacy).unwrap()).unwrap();
        let bytes = p.to_json().unwrap();
        assert_eq!(
            PlanArtifact::from_json(&bytes).unwrap().to_json().unwrap(),
            bytes
        );
        let mut v3 = legacy.clone();
        v3["version"] = json!(3);
        let p3 = crate::plan_v3::PlanV3::from_json(&serde_json::to_vec(&v3).unwrap()).unwrap();
        let bytes3 = p3.to_json().unwrap();
        assert_eq!(
            PlanArtifact::from_json(&bytes3).unwrap().to_json().unwrap(),
            bytes3
        );
        let mut forged = p3.clone();
        forged.output.total_frames = 0;
        let forged = PlanArtifact::from(VersionedPlan::V3(forged));
        assert_eq!(forged.to_json().unwrap_err().code, "E_INTERVAL");
        let mut p3 = p3;
        p3.version = 4;
        assert_eq!(p3.validate().unwrap_err().code, "E_VERSION");
        let converted = PlanArtifact::from(VersionedPlan::V3(p3));
        assert_eq!(converted.to_json().unwrap_err().code, "E_VERSION");
        assert_eq!(
            converted
                .to_json_with_limits(&PlanLimits::default())
                .unwrap_err()
                .code,
            "E_VERSION"
        );
        let mut p = p;
        p.version = 4;
        assert_eq!(p.validate().unwrap_err().code, "E_VERSION");
        let converted = PlanArtifact::from(VersionedPlan::Legacy(p));
        assert_eq!(converted.to_json().unwrap_err().code, "E_VERSION");
        assert_eq!(
            converted
                .to_json_with_limits(&PlanLimits::default())
                .unwrap_err()
                .code,
            "E_VERSION"
        );
        for version in [0, 7] {
            let mut v = wire();
            v["version"] = json!(version);
            assert_eq!(load(&v).unwrap_err().code, "E_VERSION");
        }
    }
}
