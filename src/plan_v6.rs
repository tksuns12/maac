//! Strict V6 wire decoding. Decoding alone does not establish renderability.

use crate::audio_asset::AudioAsset;
use crate::instrument_plan::InstrumentResources;
use crate::plan::{
    rational_map_serde, rational_serde, Connection, OutputSettings, Processor, Rational, Region,
    SourceMapping, TempoMap,
};
use crate::plan_v3::{AutomationV3, ResolvedEventV3};
use serde::{de, Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlanV6 {
    #[serde(deserialize_with = "version_six")]
    pub version: u32,
    pub output: OutputSettings,
    pub tempo: TempoMap,
    #[serde(default)]
    pub events: Vec<ResolvedEventV3>,
    #[serde(default)]
    pub nodes: Vec<NodeV6>,
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

use crate::plan_v4::KitSampleRef;
use crate::plan_v5::{AudioClip, AudioFadeShape, NodeV5, ProcessorV5};

impl PlanV6 {
    pub(crate) fn validate_with_limits(
        &self,
        limits: &crate::plan::PlanLimits,
    ) -> Result<(), crate::plan::PlanError> {
        if self.version != 6 {
            return Err(crate::plan::err(
                "E_VERSION",
                "version",
                "PlanV6 requires version 6",
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
            nodes: crate::plan::NodeSlice::V6(&self.nodes),
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

    /// Decode strict structure only. Empty or unordered warp anchors, invalid
    /// intervals and other semantic failures require independent plan validation.
    pub(crate) fn decode_wire(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}
fn version_six<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let version = u32::deserialize(deserializer)?;
    if version != 6 {
        return Err(de::Error::custom("PlanV6 requires version 6"));
    }
    Ok(version)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NodeV6 {
    pub id: String,
    pub processor: ProcessorV6,
    #[serde(default, with = "rational_map_serde")]
    pub params: BTreeMap<String, Rational>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ProcessorV6 {
    Core {
        processor: Processor,
    },
    Kit {
        channels: u8,
        voices: u32,
        samples: Vec<KitSampleRef>,
    },
    Audio {
        clip: Box<AudioClip>,
    },
    WarpRate {
        clip: Box<WarpClip>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WarpClip {
    pub asset: String,
    pub channels: u8,
    #[serde(with = "rational_serde")]
    pub at_q: Rational,
    pub source_start_frame: u64,
    pub source_end_frame: u64,
    pub warp: Vec<WarpAnchor>,
    #[serde(with = "rational_serde")]
    pub gain: Rational,
    #[serde(with = "rational_serde")]
    pub fade_in_seconds: Rational,
    #[serde(with = "rational_serde")]
    pub fade_out_seconds: Rational,
    pub fade_shape: AudioFadeShape,
    pub source: SourceMapping,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    pub start_frame: u64,
    pub end_frame: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WarpAnchor {
    #[serde(with = "rational_serde")]
    pub q: Rational,
    pub source_frame: u64,
}

impl From<NodeV5> for NodeV6 {
    fn from(node: NodeV5) -> Self {
        let processor = match node.processor {
            ProcessorV5::Core { processor } => ProcessorV6::Core { processor },
            ProcessorV5::Kit {
                channels,
                voices,
                samples,
            } => ProcessorV6::Kit {
                channels,
                voices,
                samples,
            },
            ProcessorV5::Audio { clip } => ProcessorV6::Audio { clip },
        };
        Self {
            id: node.id,
            processor,
            params: node.params,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    fn fixture() -> Value {
        let bytes: Vec<u8> = [0u32, 0x80000000, 1, 0x80000001]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        let mut value = json!({"version":6,
            "output":{"score_start_q":"0/1","score_end_q":"4/1","tail_seconds":"0/1","sample_rate_hz":48000,"channels":1,"total_frames":96000,"output":{"node":"clip","port":"out"}},
            "tempo":{"points":[{"q":"0/1","bpm":"120/1","shape":"step"}]},
            "nodes":[
                {"id":"sine","processor":{"kind":"core","processor":{"kind":"sine","voices":1}},"params":{"level":"1/2"}},
                {"id":"kit","processor":{"kind":"kit","channels":1,"voices":64,"samples":[{"key":"kick","asset":"sample"}]}},
                {"id":"clip","processor":{"kind":"audio","clip":{
                    "asset":"sample","channels":1,"at":{"kind":"score","q":"1/3"},
                    "source_start_frame":1,"source_end_frame":4,"speed":"3/2","reverse":true,"gain":"2/1",
                    "fade_in_seconds":"1/1000","fade_out_seconds":"1/500","fade_shape":"equal_power",
                    "source":{"object":"clip","path":["clip"]},"track":"group","start_frame":8000,"end_frame":8002
                }}}
            ],
            "audio_assets":[{"id":"sample","format":"pcm_f32le_interleaved/1","rate_hz":48000,"channels":1,"frames":4,"hash":crate::bundle::sha256_digest(&bytes),"bytes":bytes}]
        });
        value["nodes"].as_array_mut().unwrap().push(json!({"id":"warp","processor":{"kind":"warp_rate","clip":{
            "asset":"sample","channels":1,"at_q":"1/3","source_start_frame":0,"source_end_frame":4,
            "warp":[{"q":"0/1","source_frame":0},{"q":"1/2","source_frame":1},{"q":"2/1","source_frame":4}],
            "gain":"2/3","fade_in_seconds":"1/1000","fade_out_seconds":"1/500","fade_shape":"equal_power",
            "source":{"object":"warp","path":["warp"]},"track":"group","start_frame":8000,"end_frame":56000
        }}}));
        value
    }
    fn decode(value: &Value) -> Result<PlanV6, serde_json::Error> {
        PlanV6::decode_wire(&serde_json::to_vec(value).unwrap())
    }

    #[test]
    fn artifact_admits_and_roundtrips_mixed_v6() {
        let bytes = serde_json::to_vec(&fixture()).unwrap();
        let artifact = crate::PlanArtifact::from_json(&bytes).unwrap();
        assert_eq!(artifact.version(), 6);
        assert_eq!(artifact.audio_clip_count(), 2);
        let encoded = artifact.to_json().unwrap();
        assert_eq!(
            crate::PlanArtifact::from_json(&encoded)
                .unwrap()
                .to_json()
                .unwrap(),
            encoded
        );
    }

    fn load(value: &Value) -> Result<crate::PlanArtifact, crate::plan::PlanError> {
        crate::PlanArtifact::from_json(&serde_json::to_vec(value).unwrap())
    }
    #[test]
    fn retained_warp_rejects_recipes_frames_metadata_and_parameters() {
        for (pointer, bad) in [
            ("/version", json!(5)),
            ("/nodes/3/processor/clip/warp", json!([])),
            ("/nodes/3/processor/clip/warp/0/q", json!("1/1")),
            ("/nodes/3/processor/clip/warp/0/source_frame", json!(1)),
            ("/nodes/3/processor/clip/warp/1/q", json!("0/1")),
            ("/nodes/3/processor/clip/warp/1/source_frame", json!(0)),
            ("/nodes/3/processor/clip/warp/2/source_frame", json!(3)),
            ("/nodes/3/processor/clip/at_q", json!("4/1")),
            ("/nodes/3/processor/clip/start_frame", json!(8001)),
            ("/nodes/3/processor/clip/end_frame", json!(55999)),
            ("/nodes/3/processor/clip/source_end_frame", json!(5)),
            ("/nodes/3/processor/clip/channels", json!(2)),
            ("/nodes/3/processor/clip/asset", json!("missing")),
            ("/nodes/3/processor/clip/gain", json!("-1/1")),
            ("/nodes/3/processor/clip/fade_out_seconds", json!("-1/1")),
            ("/nodes/3/processor/clip/source/path", json!([])),
            ("/nodes/3/processor/clip/source/object", json!("invalid/id")),
            ("/nodes/3/processor/clip/track", json!("invalid/id")),
        ] {
            let mut v = fixture();
            *v.pointer_mut(pointer).unwrap() = bad;
            assert!(load(&v).is_err(), "{pointer}");
        }
        let mut v = fixture();
        v["nodes"][3]["params"] = json!({"gain":"1/1"});
        assert_eq!(load(&v).unwrap_err().code, "E_UNKNOWN_FIELD");
        let mut plan = decode(&fixture()).unwrap();
        let ProcessorV6::WarpRate { clip } = &mut plan.nodes[3].processor else {
            unreachable!()
        };
        clip.end_frame += 1;
        assert_eq!(
            crate::PlanArtifact::from_v6(plan)
                .to_json()
                .unwrap_err()
                .code,
            "E_INTERVAL"
        );
    }
    #[test]
    fn warp_output_only_ports_and_automation_are_checked() {
        let mut v = fixture();
        v["output"]["output"] = json!({"node":"warp","port":"out"});
        assert_eq!(load(&v).unwrap().output().channels, 1);
        for port in ["in", "events", "gain"] {
            let mut bad = v.clone();
            bad["output"]["output"]["port"] = json!(port);
            assert!(load(&bad).is_err());
        }
        v["nodes"].as_array_mut().unwrap().push(
            json!({"id":"mix","processor":{"kind":"core","processor":{"kind":"sum","channels":1}}}),
        );
        v["connections"] = json!([{"id":"route","from":{"node":"warp","port":"out"},"to":{"node":"mix","port":"in"}}]);
        assert!(load(&v).is_ok());
        v["connections"][0]["to"] = json!({"node":"warp","port":"in"});
        assert!(load(&v).is_err());
        let mut v = fixture();
        v["events"] = json!([{"address":"hit","source":{"object":"hit","path":["hit"]},"target":{"node":"warp","port":"events"},"kind":{"kind":"hit","key":"kick","velocity":"1/1"},"score_on_q":"0/1","onset_offset_seconds":"0/1","release_offset_seconds":"0/1","release_velocity":0.0,"on_frame":0}]);
        assert!(load(&v).is_err());
        let mut v = fixture();
        v["automation"] = json!([{"id":"a","target":{"node":"warp","port":"gain"},"clock":"score","at":{"kind":"score","q":"0/1"},"points":[{"position":"0/1","value":"1/1","shape":"step"}]}]);
        assert!(load(&v).is_err());
    }
    #[test]
    fn warp_anchor_and_metadata_limits_precede_hash_and_tempo() {
        use crate::plan::PlanLimits;
        let mut v = fixture();
        v["audio_assets"][0]["hash"] = json!("bad");
        v["tempo"]["points"][0]["bpm"] = json!("0/1");
        for limits in [
            PlanLimits {
                max_automation_points: 2,
                ..Default::default()
            },
            PlanLimits {
                max_work: 1,
                ..Default::default()
            },
            PlanLimits {
                max_string_bytes: 1,
                ..Default::default()
            },
            PlanLimits {
                max_rational_bits: 1,
                ..Default::default()
            },
        ] {
            assert_eq!(
                crate::PlanArtifact::from_json_with_limits(
                    &serde_json::to_vec(&v).unwrap(),
                    &limits
                )
                .unwrap_err()
                .code,
                "E_RESOURCE_LIMIT"
            );
        }
        v["nodes"][3]["processor"]["clip"]["warp"] =
            json!(vec![json!({"q":"0/1","source_frame":0}); 4097]);
        assert_eq!(load(&v).unwrap_err().code, "E_RESOURCE_LIMIT");
        let mut v = fixture();
        v["automation"] = json!([{"id":"level","target":{"node":"kit","port":"level"},"clock":"score","at":{"kind":"score","q":"0/1"},"points":[{"position":"0/1","value":"1/1","shape":"step"}]}]);
        let bytes = serde_json::to_vec(&v).unwrap();
        for (limit, accepted) in [(3, false), (4, true)] {
            assert_eq!(
                crate::PlanArtifact::from_json_with_limits(
                    &bytes,
                    &PlanLimits {
                        max_automation_points: limit,
                        ..Default::default()
                    }
                )
                .is_ok(),
                accepted
            );
        }
    }
    #[test]
    fn warp_execution_cost_includes_silent_unconnected_and_search_work() {
        use crate::plan::PlanLimits;
        let mut plan = decode(&fixture()).unwrap();
        let total = plan.view().execution_work(&PlanLimits::default()).unwrap();
        let warp = plan.nodes.pop().unwrap();
        let without = plan.view().execution_work(&PlanLimits::default()).unwrap();
        assert_eq!(total - without, 96000 * 5 + 48000 * (96 + 8 + 2));
        plan.nodes.push(warp);
        let ProcessorV6::WarpRate { clip } = &mut plan.nodes[3].processor else {
            unreachable!()
        };
        clip.gain = Rational::from_integer(0.into());
        assert_eq!(
            plan.view().execution_work(&PlanLimits::default()).unwrap(),
            total
        );
        assert_eq!(
            plan.validate_with_limits(&PlanLimits {
                max_execution_work: total - 1,
                ..Default::default()
            })
            .unwrap_err()
            .code,
            "E_RESOURCE_LIMIT"
        );
        plan.validate_with_limits(&PlanLimits {
            max_execution_work: total,
            ..Default::default()
        })
        .unwrap();
        // The same resources retain their cost even if the graph output is another node.
        assert_eq!(plan.output.output.node, "clip");
    }

    #[test]
    fn mixed_nodes_exact_assets_and_warp_configuration_roundtrip() {
        let original = fixture();
        let plan = decode(&original).unwrap();
        assert_eq!(plan.nodes.len(), 4);
        let encoded = serde_json::to_vec(&plan).unwrap();
        assert_eq!(PlanV6::decode_wire(&encoded).unwrap(), plan);
        let ProcessorV6::WarpRate { clip } = &plan.nodes[3].processor else {
            panic!("warp kind lost")
        };
        assert_eq!(clip.warp.len(), 3);
        assert_eq!(clip.warp[1].source_frame, 1);
        assert_eq!(clip.track.as_deref(), Some("group"));
        assert_eq!(clip.fade_shape, AudioFadeShape::EqualPower);
        assert_eq!(
            serde_json::to_value(&plan).unwrap()["audio_assets"],
            original["audio_assets"]
        );
        let mut without = original;
        without["nodes"][3]["processor"]["clip"]
            .as_object_mut()
            .unwrap()
            .remove("track");
        let decoded = decode(&without).unwrap();
        assert!(
            serde_json::to_value(decoded).unwrap()["nodes"][3]["processor"]["clip"]
                .get("track")
                .is_none()
        );
    }
    #[test]
    fn rejects_versions_missing_fields_and_wrong_types() {
        for version in [0, 1, 2, 3, 4, 5, 7] {
            let mut v = fixture();
            v["version"] = json!(version);
            assert!(decode(&v).is_err());
        }
        for key in ["version", "output", "tempo", "audio_assets"] {
            let mut v = fixture();
            v.as_object_mut().unwrap().remove(key);
            assert!(decode(&v).is_err(), "{key}");
        }
        let original = fixture();
        for key in original["nodes"][3]["processor"]["clip"]
            .as_object()
            .unwrap()
            .keys()
            .filter(|k| *k != "track")
        {
            let mut v = original.clone();
            v["nodes"][3]["processor"]["clip"]
                .as_object_mut()
                .unwrap()
                .remove(key);
            assert!(decode(&v).is_err(), "{key}");
        }
        for key in ["q", "source_frame"] {
            let mut v = fixture();
            v["nodes"][3]["processor"]["clip"]["warp"][0]
                .as_object_mut()
                .unwrap()
                .remove(key);
            assert!(decode(&v).is_err());
        }
        for (pointer, wrong) in [
            ("/nodes/3/processor/clip/at_q", json!(1)),
            ("/nodes/3/processor/clip/gain", json!("1/0")),
            ("/nodes/3/processor/clip/channels", json!(256)),
            ("/nodes/3/processor/clip/warp", json!({})),
            ("/nodes/3/processor/clip/warp/0/q", json!(false)),
            ("/nodes/3/processor/clip/warp/0/source_frame", json!(-1)),
            ("/nodes/3/processor/clip/end_frame", json!(0.5)),
            ("/nodes/3/processor/clip/track", json!(true)),
            ("/nodes/3/processor/clip/fade_shape", json!("custom")),
        ] {
            let mut v = fixture();
            *v.pointer_mut(pointer).unwrap() = wrong;
            assert!(decode(&v).is_err(), "{pointer}");
        }
    }
    #[test]
    fn rejects_unknown_duplicate_and_rate_only_fields() {
        for pointer in [
            "",
            "/nodes/3",
            "/nodes/3/processor",
            "/nodes/3/processor/clip",
            "/nodes/3/processor/clip/warp/0",
            "/nodes/3/processor/clip/source",
        ] {
            let original = fixture();
            let object = original.pointer(pointer).unwrap();
            let mut v = original.clone();
            v.pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("unknown".into(), json!(0));
            assert!(decode(&v).is_err());
            let serialized = serde_json::to_string(&original).unwrap();
            let nested = serde_json::to_string(object).unwrap();
            for (key, field) in object.as_object().unwrap() {
                let duplicate = format!(
                    "{{{}:{},{}",
                    serde_json::to_string(key).unwrap(),
                    field,
                    &nested[1..]
                );
                assert!(
                    PlanV6::decode_wire(serialized.replacen(&nested, &duplicate, 1).as_bytes())
                        .is_err(),
                    "{pointer}/{key}"
                );
            }
        }
        for (key, value) in [
            ("speed", json!("1/1")),
            ("reverse", json!(false)),
            ("processor", json!("module")),
        ] {
            let mut v = fixture();
            v["nodes"][3]["processor"]["clip"][key] = value;
            assert!(decode(&v).is_err(), "{key}");
        }
    }
    #[test]
    fn decoding_is_structural_and_v5_conversion_preserves_wire_bytes() {
        let mut v = fixture();
        v["nodes"][3]["processor"]["clip"]["warp"] = json!([]);
        v["nodes"][3]["processor"]["clip"]["gain"] = json!("-1/1");
        assert!(decode(&v).is_ok());
        for node in fixture()["nodes"].as_array().unwrap().iter().take(3) {
            let old: NodeV5 = serde_json::from_value(node.clone()).unwrap();
            let bytes = serde_json::to_vec(&old).unwrap();
            assert_eq!(serde_json::to_vec(&NodeV6::from(old)).unwrap(), bytes);
        }
    }
}
