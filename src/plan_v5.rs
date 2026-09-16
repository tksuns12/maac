//! Strict V5 wire decoding. Decoding alone does not establish renderability.

use crate::audio_asset::AudioAsset;
use crate::instrument_plan::InstrumentResources;
use crate::plan::{
    rational_map_serde, rational_serde, Connection, OutputSettings, Processor, Rational, Region,
    SourceMapping, TempoMap,
};
use crate::plan_v3::{AutomationAnchor, AutomationV3, ResolvedEventV3};
use serde::{de, Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlanV5 {
    #[serde(deserialize_with = "version_five")]
    pub version: u32,
    pub output: OutputSettings,
    pub tempo: TempoMap,
    #[serde(default)]
    pub events: Vec<ResolvedEventV3>,
    #[serde(default)]
    pub nodes: Vec<NodeV5>,
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

use crate::plan_v4::{KitSampleRef, NodeV4, ProcessorV4};

impl PlanV5 {
    #[allow(dead_code)]
    pub(crate) fn validate_with_limits(
        &self,
        limits: &crate::plan::PlanLimits,
    ) -> Result<(), crate::plan::PlanError> {
        if self.version != 5 {
            return Err(crate::plan::err(
                "E_VERSION",
                "version",
                "PlanV5 requires version 5",
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
            nodes: crate::plan::NodeSlice::V5(&self.nodes),
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

    /// Decode strict structure only; resource, graph and timing validation is separate.
    pub(crate) fn decode_wire(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}
fn version_five<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let version = u32::deserialize(deserializer)?;
    if version != 5 {
        return Err(de::Error::custom("PlanV5 requires version 5"));
    }
    Ok(version)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NodeV5 {
    pub id: String,
    pub processor: ProcessorV5,
    #[serde(default, with = "rational_map_serde")]
    pub params: BTreeMap<String, Rational>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ProcessorV5 {
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
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AudioClip {
    pub asset: String,
    pub channels: u8,
    pub at: AutomationAnchor,
    pub source_start_frame: u64,
    pub source_end_frame: u64,
    #[serde(with = "rational_serde")]
    pub speed: Rational,
    pub reverse: bool,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AudioFadeShape {
    Linear,
    EqualPower,
}

impl From<NodeV4> for NodeV5 {
    fn from(node: NodeV4) -> Self {
        let processor = match node.processor {
            ProcessorV4::Core { processor } => ProcessorV5::Core { processor },
            ProcessorV4::Kit {
                channels,
                voices,
                samples,
            } => ProcessorV5::Kit {
                channels,
                voices,
                samples,
            },
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
        json!({"version":5,
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
        })
    }
    fn decode(value: &Value) -> Result<PlanV5, serde_json::Error> {
        PlanV5::decode_wire(&serde_json::to_vec(value).unwrap())
    }
    #[test]
    fn validated_artifact_roundtrip() {
        let value = fixture();
        let artifact =
            crate::PlanArtifact::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(artifact.version(), 5);
        assert_eq!(artifact.audio_clip_count(), 1);
        assert_eq!(artifact.event_count(), 0);
        assert_eq!(
            crate::PlanArtifact::from_json(&artifact.to_json().unwrap()).unwrap(),
            artifact
        );
    }
    fn load(value: &Value) -> Result<crate::PlanArtifact, crate::plan::PlanError> {
        crate::PlanArtifact::from_json(&serde_json::to_vec(value).unwrap())
    }
    #[test]
    fn rejects_hostile_clip_records_and_graph_targets() {
        for (pointer, wrong) in [
            ("/nodes/2/processor/clip/asset", json!("missing")),
            (
                "/nodes/2/processor/clip/at",
                json!({"kind":"score","q":"4/1"}),
            ),
            (
                "/nodes/2/processor/clip/at",
                json!({"kind":"seconds","seconds":"-1/1"}),
            ),
            ("/nodes/2/processor/clip/channels", json!(2)),
            ("/nodes/2/processor/clip/channels", json!(0)),
            ("/nodes/2/processor/clip/source_start_frame", json!(4)),
            ("/nodes/2/processor/clip/source_end_frame", json!(5)),
            ("/nodes/2/processor/clip/speed", json!("0/1")),
            ("/nodes/2/processor/clip/gain", json!("-1/1")),
            ("/nodes/2/processor/clip/fade_in_seconds", json!("-1/1")),
            ("/nodes/2/processor/clip/fade_out_seconds", json!("-1/1")),
            ("/nodes/2/processor/clip/start_frame", json!(8001)),
            ("/nodes/2/processor/clip/end_frame", json!(7999)),
            ("/nodes/2/processor/clip/end_frame", json!(u64::MAX)),
            ("/nodes/2/processor/clip/source/path", json!([])),
            ("/nodes/2/processor/clip/source/object", json!("")),
            ("/nodes/2/processor/clip/source/path", json!(["bad/id"])),
            ("/nodes/2/processor/clip/track", json!("bad/id")),
            ("/output/output/port", json!("events")),
            ("/audio_assets/0/id", json!("clip")),
        ] {
            let mut value = fixture();
            *value.pointer_mut(pointer).unwrap() = wrong;
            assert!(load(&value).is_err(), "{pointer}");
        }
        let mut value = fixture();
        value["nodes"][2]["params"] = json!({"gain":"1/1"});
        assert!(load(&value).is_err());
        value = fixture();
        value["nodes"][2]["processor"]["clip"]["source"]["span"] = json!({"start":2,"end":1});
        assert!(load(&value).is_err());
        value = fixture();
        value["connections"] = json!([{"id":"route","from":{"node":"clip","port":"out"},"to":{"node":"kit","port":"events"}}]);
        assert!(load(&value).is_err());
        value["connections"][0]["to"] = json!({"node":"clip","port":"out"});
        assert!(load(&value).is_err());
        value = fixture();
        value["automation"] = json!([{"id":"lane","target":{"node":"clip","port":"gain"},"clock":"score","at":{"kind":"score","q":"0/1"},"points":[{"position":"0/1","value":"1/1","shape":"step"}]}]);
        assert!(decode(&value).is_ok());
        assert!(load(&value).is_err());
        value = fixture();
        value["events"] = json!([{"address":"hit","source":{"object":"hit","path":["hit"]},"target":{"node":"clip","port":"events"},"kind":{"kind":"hit","key":"kick","velocity":"1/1"},"score_on_q":"0/1","onset_offset_seconds":"0/1","release_offset_seconds":"0/1","release_velocity":0.0,"on_frame":0}]);
        assert!(decode(&value).is_ok());
        assert!(load(&value).is_err());
        value = fixture();
        value["version"] = json!(4);
        assert!(load(&value).is_err());
    }
    #[test]
    fn clip_resource_budgets_precede_asset_hash_and_work_is_checked() {
        use crate::plan::PlanLimits;
        let mut plan = decode(&fixture()).unwrap();
        assert_eq!(
            plan.view().execution_work(&PlanLimits::default()).unwrap(),
            960080
        );
        let mut limits = PlanLimits {
            max_execution_work: 960080,
            ..PlanLimits::default()
        };
        assert!(plan.validate_with_limits(&limits).is_ok());
        limits.max_execution_work -= 1;
        assert_eq!(
            plan.validate_with_limits(&limits).unwrap_err().code,
            "E_RESOURCE_LIMIT"
        );
        let mut lower = 0;
        let mut upper = PlanLimits::MAX_WORK;
        while lower < upper {
            let mid = lower + (upper - lower) / 2;
            let limits = PlanLimits {
                max_work: mid,
                ..PlanLimits::default()
            };
            if plan.validate_with_limits(&limits).is_ok() {
                upper = mid;
            } else {
                lower = mid + 1;
            }
        }
        assert!(plan
            .validate_with_limits(&PlanLimits {
                max_work: lower,
                ..PlanLimits::default()
            })
            .is_ok());
        assert_eq!(
            plan.validate_with_limits(&PlanLimits {
                max_work: lower - 1,
                ..PlanLimits::default()
            })
            .unwrap_err()
            .code,
            "E_RESOURCE_LIMIT"
        );
        plan.audio_assets[0].bytes[0] = 1;
        let ProcessorV5::Audio { clip } = &mut plan.nodes[2].processor else {
            unreachable!()
        };
        clip.track = Some("long_group_name".into());
        assert_eq!(
            plan.validate_with_limits(&PlanLimits {
                max_id_bytes: 6,
                ..PlanLimits::default()
            })
            .unwrap_err()
            .code,
            "E_RESOURCE_LIMIT"
        );
        let ProcessorV5::Audio { clip } = &mut plan.nodes[2].processor else {
            unreachable!()
        };
        clip.speed = Rational::new(1.into(), 65536.into());
        assert_eq!(
            plan.validate_with_limits(&PlanLimits {
                max_rational_bits: 12,
                ..PlanLimits::default()
            })
            .unwrap_err()
            .code,
            "E_RESOURCE_LIMIT"
        );
        let ProcessorV5::Audio { clip } = &mut plan.nodes[2].processor else {
            unreachable!()
        };
        clip.start_frame = 10;
        clip.end_frame = 1;
        assert!(plan.view().execution_work(&PlanLimits::default()).is_err());
    }
    #[test]
    fn empty_and_tail_first_sample_intervals_validate() {
        for (seconds, speed, start, end) in [
            ("383999/192000", "20/1", 96000, 96000),
            ("383999/192000", "1/1", 96000, 96003),
        ] {
            let mut value = fixture();
            value["output"]["tail_seconds"] = json!("1/1");
            value["output"]["total_frames"] = json!(144000);
            let clip = &mut value["nodes"][2]["processor"]["clip"];
            clip["at"] = json!({"kind":"seconds","seconds":seconds});
            clip["speed"] = json!(speed);
            clip["start_frame"] = json!(start);
            clip["end_frame"] = json!(end);
            assert!(load(&value).is_ok(), "{}", load(&value).unwrap_err());
        }
    }
    #[test]
    fn stereo_and_string_budgets_include_clip_metadata() {
        let mut plan = decode(&fixture()).unwrap();
        plan.output.channels = 2;
        plan.audio_assets[0].channels = 2;
        plan.audio_assets[0].bytes = plan.audio_assets[0].bytes.repeat(2);
        plan.audio_assets[0].hash = crate::bundle::sha256_digest(&plan.audio_assets[0].bytes);
        let ProcessorV5::Kit { channels, .. } = &mut plan.nodes[1].processor else {
            unreachable!()
        };
        *channels = 2;
        let ProcessorV5::Audio { clip } = &mut plan.nodes[2].processor else {
            unreachable!()
        };
        clip.channels = 2;
        assert!(plan
            .validate_with_limits(&crate::plan::PlanLimits::default())
            .is_ok());
        let before = plan
            .view()
            .execution_work(&crate::plan::PlanLimits::default())
            .unwrap();
        let ProcessorV5::Audio { clip } = &mut plan.nodes[2].processor else {
            unreachable!()
        };
        clip.gain = Rational::from_integer(0.into());
        clip.track = Some("a".repeat(100));
        assert_eq!(
            plan.view()
                .execution_work(&crate::plan::PlanLimits::default())
                .unwrap(),
            before
        );
        let limits = crate::plan::PlanLimits {
            max_string_bytes: 80,
            ..crate::plan::PlanLimits::default()
        };
        assert_eq!(
            plan.validate_with_limits(&limits).unwrap_err().code,
            "E_RESOURCE_LIMIT"
        );
    }
    #[test]
    fn encoding_revalidates_and_clip_routes_are_typed() {
        let mut value = fixture();
        value["nodes"].as_array_mut().unwrap().push(
            json!({"id":"mix","processor":{"kind":"core","processor":{"kind":"sum","channels":1}}}),
        );
        value["connections"] = json!([{"id":"route","from":{"node":"clip","port":"out"},"to":{"node":"mix","port":"in"}}]);
        value["output"]["output"] = json!({"node":"mix","port":"out"});
        assert!(load(&value).is_ok());
        let mut plan = decode(&value).unwrap();
        let ProcessorV5::Audio { clip } = &mut plan.nodes[2].processor else {
            unreachable!()
        };
        clip.end_frame = 0;
        assert!(crate::PlanArtifact::from_v5(plan).to_json().is_err());
        let mut plan = decode(&fixture()).unwrap();
        plan.version = 4;
        assert_eq!(
            plan.validate_with_limits(&crate::plan::PlanLimits::default())
                .unwrap_err()
                .code,
            "E_VERSION"
        );
        assert!(plan
            .view()
            .validate_with_limits(&crate::plan::PlanLimits::default())
            .is_err());
        let mut plan = decode(&fixture()).unwrap();
        let ProcessorV5::Audio { clip } = &mut plan.nodes[2].processor else {
            unreachable!()
        };
        clip.gain = Rational::new(1.into(), num_bigint::BigInt::from(1u8) << 4000);
        assert_eq!(
            plan.validate_with_limits(&crate::plan::PlanLimits::default())
                .unwrap_err()
                .code,
            "E_NONFINITE"
        );
    }
    #[test]
    fn mixed_nodes_preserve_bytes_anchors_fades_and_grouping() {
        for (anchor, shape, track) in [
            (
                json!({"kind":"score","q":"1/3"}),
                "equal_power",
                Some("group"),
            ),
            (json!({"kind":"seconds","seconds":"1/7"}), "linear", None),
        ] {
            let mut value = fixture();
            let clip = &mut value["nodes"][2]["processor"]["clip"];
            clip["at"] = anchor;
            clip["fade_shape"] = json!(shape);
            if track.is_none() {
                clip.as_object_mut().unwrap().remove("track");
            }
            let plan = decode(&value).unwrap();
            let encoded = serde_json::to_vec(&plan).unwrap();
            assert_eq!(PlanV5::decode_wire(&encoded).unwrap(), plan);
            assert_eq!(
                plan.audio_assets[0].bytes,
                [0u32, 0x80000000, 1, 0x80000001]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>()
            );
            let ProcessorV5::Audio { clip } = &plan.nodes[2].processor else {
                panic!("audio kind lost")
            };
            assert!(clip.reverse);
            assert_eq!(clip.track.as_deref(), track);
            if track.is_none() {
                assert!(
                    serde_json::to_value(&plan).unwrap()["nodes"][2]["processor"]["clip"]
                        .get("track")
                        .is_none()
                );
            }
        }
    }
    #[test]
    fn rejects_versions_missing_fields_and_wrong_types() {
        for version in [0, 1, 2, 3, 4, 6] {
            let mut v = fixture();
            v["version"] = json!(version);
            assert!(decode(&v).is_err());
        }
        for field in ["version", "output", "tempo", "audio_assets"] {
            let mut v = fixture();
            v.as_object_mut().unwrap().remove(field);
            assert!(decode(&v).is_err(), "{field}");
        }
        let original = fixture();
        for field in original["nodes"][2]["processor"]["clip"]
            .as_object()
            .unwrap()
            .keys()
            .filter(|k| *k != "track")
        {
            let mut v = original.clone();
            v["nodes"][2]["processor"]["clip"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(decode(&v).is_err(), "{field}");
        }
        for (field, wrong) in [
            ("speed", json!(1)),
            ("gain", json!("1/0")),
            ("reverse", json!(1)),
            ("channels", json!(256)),
            ("source_start_frame", json!(-1)),
            ("end_frame", json!(0.5)),
            ("fade_shape", json!("exponential")),
            ("track", json!(2)),
        ] {
            let mut v = fixture();
            v["nodes"][2]["processor"]["clip"][field] = wrong;
            assert!(decode(&v).is_err(), "{field}");
        }
    }
    #[test]
    fn rejects_unknown_and_duplicate_fields_at_every_new_boundary() {
        for pointer in [
            "",
            "/nodes/0",
            "/nodes/0/processor",
            "/nodes/1/processor",
            "/nodes/2/processor",
            "/nodes/2/processor/clip",
            "/nodes/2/processor/clip/at",
            "/nodes/2/processor/clip/source",
        ] {
            let original = fixture();
            let object = original.pointer(pointer).unwrap();
            let mut value = original.clone();
            value
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("unknown".into(), json!(0));
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
                assert!(
                    PlanV5::decode_wire(serialized.replacen(&nested, &duplicate, 1).as_bytes())
                        .is_err(),
                    "duplicate {pointer}/{key}"
                );
            }
        }
    }
    #[test]
    fn decode_is_structural_and_v4_node_conversion_is_lossless() {
        let mut value = fixture();
        value["nodes"][2]["processor"]["clip"]["speed"] = json!("0/1");
        value["nodes"][2]["processor"]["clip"]["end_frame"] = json!(0);
        assert!(decode(&value).is_ok());
        for node in fixture()["nodes"].as_array().unwrap().iter().take(2) {
            let old: crate::plan_v4::NodeV4 = serde_json::from_value(node.clone()).unwrap();
            let before = serde_json::to_value(&old).unwrap();
            let new = NodeV5::from(old);
            assert_eq!(serde_json::to_value(new).unwrap(), before);
        }
    }
}
