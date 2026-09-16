//! Crate-private V4 wire representation. Decoding is structural, not plan validation.

use crate::audio_asset::AudioAsset;
use crate::instrument_plan::InstrumentResources;
use crate::plan::{
    rational_map_serde, Connection, OutputSettings, Processor, Rational, Region, SourceMapping,
    TempoMap,
};
use crate::plan_v3::{AutomationV3, ResolvedEventV3};
use serde::{de, Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlanV4 {
    #[serde(deserialize_with = "version_four")]
    pub version: u32,
    pub output: OutputSettings,
    pub tempo: TempoMap,
    #[serde(default)]
    pub events: Vec<ResolvedEventV3>,
    #[serde(default)]
    pub nodes: Vec<NodeV4>,
    pub audio_assets: Vec<AudioAsset>,
    #[serde(default)]
    pub connections: Vec<Connection>,
    #[serde(default)]
    pub automation: Vec<AutomationV3>,
    #[serde(default)]
    pub regions: Vec<Region>,
    #[serde(default)]
    pub source_mappings: Vec<SourceMapping>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruments: Option<InstrumentResources>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub production: Option<crate::production_data::ProductionSettings>,
}

impl PlanV4 {
    #[allow(dead_code)]
    pub(crate) fn validate_with_limits(
        &self,
        limits: &crate::plan::PlanLimits,
    ) -> Result<(), crate::plan::PlanError> {
        if self.version != 4 {
            return Err(crate::plan::err(
                "E_VERSION",
                "version",
                "PlanV4 requires version 4",
            ));
        }
        self.view().validate_with_limits(limits)
    }

    pub(crate) fn view(&self) -> crate::plan::PlanView<'_> {
        crate::plan::PlanView {
            version: self.version,
            output: &self.output,
            tempo: &self.tempo,
            events: crate::plan::EventSlice::V3(&self.events),
            nodes: crate::plan::NodeSlice::V4(&self.nodes),
            audio_assets: Some(&self.audio_assets),
            connections: &self.connections,
            modulations: &[],
            automation: crate::plan::AutomationSlice::V3(&self.automation),
            regions: &self.regions,
            source_mappings: &self.source_mappings,
            instruments: self.instruments.as_ref(),
            production: self.production.as_ref(),
        }
    }

    /// Decode only the strict wire structure and version tag. This does not
    /// validate resource bounds, assets, timing, graph contracts, or renderability.
    pub(crate) fn decode_wire(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

fn version_four<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let version = u32::deserialize(deserializer)?;
    if version != 4 {
        return Err(de::Error::custom("PlanV4 requires version 4"));
    }
    Ok(version)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NodeV4 {
    pub id: String,
    pub processor: ProcessorV4,
    #[serde(default, with = "rational_map_serde")]
    pub params: BTreeMap<String, Rational>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ProcessorV4 {
    Core {
        processor: Processor,
    },
    Kit {
        channels: u8,
        voices: u32,
        samples: Vec<KitSampleRef>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KitSampleRef {
    pub key: String,
    pub asset: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn fixture() -> Value {
        let bytes: Vec<u8> = [0u32, 0x80000000, 1, 0x80000001, 0x40000000]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        json!({
            "version":4,
            "output":{"score_start_q":"0/1","score_end_q":"4/1","tail_seconds":"0/1","sample_rate_hz":48000,"channels":1,"total_frames":96000,"output":{"node":"kit","port":"out"}},
            "tempo":{"points":[{"q":"0/1","bpm":"120/1","shape":"step"}]},
            "nodes":[
                {"id":"sine","processor":{"kind":"core","processor":{"kind":"sine","voices":1}},"params":{"level":"1/2"}},
                {"id":"kit","processor":{"kind":"kit","channels":1,"voices":64,"samples":[{"key":"kick","asset":"sample"}]},"params":{"level":"1/1"}}
            ],
            "audio_assets":[{"id":"sample","format":"pcm_f32le_interleaved/1","rate_hz":44100,"channels":1,"frames":5,"hash":crate::bundle::sha256_digest(&bytes),"bytes":bytes}],
            "events":[{"address":"main/hit","source":{"object":"hit","path":["main","hit"]},"target":{"node":"kit","port":"events"},"kind":{"kind":"hit","key":"kick","velocity":"1/2"},"score_on_q":"0/1","onset_offset_seconds":"0/1","release_offset_seconds":"0/1","release_velocity":0.0,"on_frame":0}]
        })
    }
    fn decode(value: &Value) -> Result<PlanV4, serde_json::Error> {
        PlanV4::decode_wire(&serde_json::to_vec(value).unwrap())
    }
    #[test]
    fn core_kit_hit_and_exact_asset_bytes_roundtrip() {
        let p = decode(&fixture()).unwrap();
        assert!(matches!(p.nodes[0].processor, ProcessorV4::Core { .. }));
        assert!(matches!(p.nodes[1].processor, ProcessorV4::Kit { .. }));
        assert_eq!(p.events[0].off_frame, None);
        assert_eq!(p.events[0].score_off_q, None);
        let bits: Vec<_> = p.audio_assets[0]
            .decode()
            .unwrap()
            .iter()
            .map(|x| x.to_bits())
            .collect();
        assert_eq!(bits, [0, 0x80000000, 1, 0x80000001, 0x40000000]);
        let encoded = serde_json::to_vec(&p).unwrap();
        assert_eq!(PlanV4::decode_wire(&encoded).unwrap(), p);
    }
    #[test]
    fn rejects_wrong_version_missing_assets_and_malformed_fields() {
        for version in [0, 1, 2, 3, 5] {
            let mut value = fixture();
            value["version"] = json!(version);
            assert!(decode(&value).is_err());
        }
        let mut value = fixture();
        value.as_object_mut().unwrap().remove("audio_assets");
        assert!(decode(&value).is_err());
        for (pointer, replacement) in [
            ("/nodes/1/processor/channels", json!(256)),
            ("/nodes/1/processor/voices", json!(-1)),
            ("/nodes/1/processor/samples/0/key", json!(2)),
            ("/nodes/0/params/level", json!("1/0")),
            ("/audio_assets/0/bytes/0", json!(256)),
            ("/events/0/score_on_q", json!(0)),
        ] {
            let mut value = fixture();
            *value.pointer_mut(pointer).unwrap() = replacement;
            assert!(decode(&value).is_err(), "{pointer}");
        }
    }
    #[test]
    fn rejects_unknown_and_duplicate_fields_at_nested_boundaries() {
        for pointer in [
            "",
            "/output",
            "/tempo",
            "/tempo/points/0",
            "/nodes/0",
            "/nodes/0/processor",
            "/nodes/0/processor/processor",
            "/nodes/1/processor",
            "/nodes/1/processor/samples/0",
            "/audio_assets/0",
            "/events/0",
            "/events/0/source",
            "/events/0/target",
            "/events/0/kind",
        ] {
            let original = fixture();
            let object = original.pointer(pointer).unwrap();
            let mut value = original.clone();
            value
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("unexpected".into(), json!(0));
            assert!(decode(&value).is_err(), "unknown {pointer}");
            let serialized = serde_json::to_string(&original).unwrap();
            let nested = serde_json::to_string(object).unwrap();
            for (key, field) in object.as_object().unwrap() {
                let duplicate = format!(
                    "{{{}:{},{}",
                    serde_json::to_string(key).unwrap(),
                    field,
                    &nested[1..]
                );
                let malformed = serialized.replacen(&nested, &duplicate, 1);
                assert!(
                    PlanV4::decode_wire(malformed.as_bytes()).is_err(),
                    "duplicate {pointer}/{key}"
                );
            }
        }
        let text = serde_json::to_string(&fixture())
            .unwrap()
            .replace("\"level\":\"1/2\"", "\"level\":\"1/2\",\"level\":\"1/2\"");
        assert!(PlanV4::decode_wire(text.as_bytes()).is_err());
    }
    #[test]
    fn wire_decode_does_not_claim_semantic_validation() {
        let mut value = fixture();
        value["output"]["total_frames"] = json!(7);
        value["audio_assets"][0]["hash"] = json!("invalid");
        let plan = decode(&value).unwrap();
        assert_eq!(plan.output.total_frames, 7);
        assert!(plan.audio_assets[0].validate().is_err());
    }
}
