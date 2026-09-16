//! Strict V7 wire decoding. Decoding alone does not establish renderability.

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
pub(crate) struct PlanV7 {
    #[serde(deserialize_with = "version_seven")]
    pub version: u32,
    pub output: OutputSettings,
    pub tempo: TempoMap,
    #[serde(default)]
    pub events: Vec<ResolvedEventV3>,
    #[serde(default)]
    pub nodes: Vec<NodeV7>,
    pub audio_assets: Vec<AudioAsset>,
    #[serde(default)]
    pub connections: Vec<Connection>,
    pub modulations: Vec<ModulationV7>,
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
use crate::plan_v5::AudioClip;
use crate::plan_v6::{NodeV6, ProcessorV6};

impl PlanV7 {
    #[allow(dead_code)]
    pub(crate) fn validate_with_limits(
        &self,
        limits: &crate::plan::PlanLimits,
    ) -> Result<(), crate::plan::PlanError> {
        if self.version != 7 {
            return Err(crate::plan::err(
                "E_VERSION",
                "version",
                "PlanV7 requires version 7",
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
            nodes: crate::plan::NodeSlice::V7(&self.nodes),
            audio_assets: Some(&self.audio_assets),
            connections: &self.connections,
            modulations: &self.modulations,
            automation: crate::plan::AutomationSlice::V3(&self.automation),
            regions: &self.regions,
            source_mappings: &self.source_mappings,
            instruments: self.instruments.as_ref(),
            production: self.production.as_ref(),
        }
    }

    /// Decode strict structure only; semantic admission remains independent.
    pub(crate) fn decode_wire(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}
fn version_seven<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let version = u32::deserialize(deserializer)?;
    if version != 7 {
        return Err(de::Error::custom("PlanV7 requires version 7"));
    }
    Ok(version)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NodeV7 {
    pub id: String,
    pub processor: ProcessorV7,
    #[serde(default, with = "rational_map_serde")]
    pub params: BTreeMap<String, Rational>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ProcessorV7 {
    Lfo {
        config: LfoConfig,
    },
    Constant {},
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

use crate::plan_v6::WarpClip;

impl From<NodeV6> for NodeV7 {
    fn from(node: NodeV6) -> Self {
        let processor = match node.processor {
            ProcessorV6::Core { processor } => ProcessorV7::Core { processor },
            ProcessorV6::Kit {
                channels,
                voices,
                samples,
            } => ProcessorV7::Kit {
                channels,
                voices,
                samples,
            },
            ProcessorV6::Audio { clip } => ProcessorV7::Audio { clip },
            ProcessorV6::WarpRate { clip } => ProcessorV7::WarpRate { clip },
        };
        Self {
            id: node.id,
            processor,
            params: node.params,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LfoConfig {
    pub clock: ControlClock,
    #[serde(with = "rational_serde")]
    pub period: Rational,
    pub wave: LfoWave,
    #[serde(with = "rational_serde")]
    pub phase: Rational,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ControlClock {
    Score,
    Seconds,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LfoWave {
    Sine,
    Triangle,
    Saw,
    Square,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModulationV7 {
    pub id: String,
    pub from: crate::plan::PortRef,
    pub target: crate::plan::PortRef,
    #[serde(with = "rational_serde")]
    pub amount: Rational,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    fn fixture() -> Value {
        json!({"version":7,"output":{"score_start_q":"0/1","score_end_q":"1/1","tail_seconds":"0/1","sample_rate_hz":48000,"channels":1,"total_frames":24000,"output":{"node":"sound","port":"out"}},"tempo":{"points":[{"q":"0/1","bpm":"120/1","shape":"step"}]},"audio_assets":[],"nodes":[{"id":"sound","processor":{"kind":"core","processor":{"kind":"sine","voices":1}}},{"id":"motion","processor":{"kind":"lfo","config":{"clock":"score","period":"2/1","wave":"sine","phase":"1/4"}}},{"id":"offset","processor":{"kind":"constant"},"params":{"value":"-1/1"}}],"modulations":[{"id":"m","from":{"node":"motion","port":"out"},"target":{"node":"offset","port":"value"},"amount":"-2/1"},{"id":"n","from":{"node":"offset","port":"out"},"target":{"node":"sound","port":"level"},"amount":"1/4"}]})
    }
    fn load(v: &Value) -> Result<crate::PlanArtifact, crate::plan::PlanError> {
        crate::PlanArtifact::from_json(&serde_json::to_vec(v).unwrap())
    }
    #[test]
    fn public_roundtrip_preserves_controls_and_validates_encoding() {
        let artifact = load(&fixture()).unwrap();
        let bytes = artifact.to_json().unwrap();
        assert_eq!(
            crate::PlanArtifact::from_json(&bytes)
                .unwrap()
                .to_json()
                .unwrap(),
            bytes
        );
        assert_eq!(artifact.event_count(), 0);
        assert_eq!(artifact.audio_clip_count(), 0);
        let mut plan = PlanV7::decode_wire(&bytes).unwrap();
        plan.modulations[0].from.port = "in".into();
        assert_eq!(
            crate::PlanArtifact::from_v7(plan)
                .to_json()
                .unwrap_err()
                .code,
            "E_PORT_TYPE"
        );
        let mut v = fixture();
        v["nodes"].as_array_mut().unwrap().reverse();
        v["modulations"].as_array_mut().unwrap().reverse();
        load(&v).unwrap();
    }
    #[test]
    fn hostile_controls_ports_targets_cycles_and_quantities_fail() {
        for (pointer, value, code) in [
            ("/nodes/1/processor/config/phase", json!("1/1"), "E_RANGE"),
            ("/nodes/1/processor/config/phase", json!("-1/4"), "E_RANGE"),
            ("/nodes/1/processor/config/period", json!("0/1"), "E_RANGE"),
            ("/nodes/1/processor/config/period", json!("-2/1"), "E_RANGE"),
            ("/modulations/0/from/node", json!("missing"), "E_REFERENCE"),
            ("/modulations/0/from/node", json!("sound"), "E_PORT_TYPE"),
            ("/modulations/0/from/port", json!("events"), "E_PORT_TYPE"),
            ("/modulations/0/target/port", json!("out"), "E_REFERENCE"),
            (
                "/modulations/0/from/node",
                json!("offset"),
                "E_ALGEBRAIC_LOOP",
            ),
            ("/modulations/0/id", json!("sound"), "E_DUPLICATE_ID"),
            ("/output/output/node", json!("motion"), "E_PORT_TYPE"),
        ] {
            let mut v = fixture();
            *v.pointer_mut(pointer).unwrap() = value;
            assert_eq!(load(&v).unwrap_err().code, code, "{pointer}");
        }
        let mut v = fixture();
        v["nodes"][1]["processor"] = json!({"kind":"constant"});
        v["modulations"][1]["target"] = json!({"node":"motion","port":"value"});
        assert_eq!(load(&v).unwrap_err().code, "E_ALGEBRAIC_LOOP");
        let mut v = fixture();
        v["nodes"][1]["params"] = json!({"value":"0/1"});
        assert_eq!(load(&v).unwrap_err().code, "E_UNKNOWN_FIELD");
        let mut v = fixture();
        v["connections"] = json!([{"id":"c","from":{"node":"motion","port":"out"},"to":{"node":"sound","port":"events"}}]);
        assert_eq!(load(&v).unwrap_err().code, "E_PORT_TYPE");
    }
    #[test]
    fn strict_wire_rejects_missing_unknown_duplicate_and_unit_strings() {
        for pointer in [
            "",
            "/nodes/1/processor",
            "/nodes/1/processor/config",
            "/modulations/0",
            "/modulations/0/from",
        ] {
            let mut v = fixture();
            v.pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("unexpected".into(), json!(0));
            assert!(load(&v).is_err(), "{pointer}");
        }
        for (pointer, key) in [
            ("", "modulations"),
            ("/nodes/1/processor/config", "period"),
            ("/nodes/1/processor/config", "phase"),
            ("/nodes/1/processor/config", "wave"),
            ("/modulations/0", "amount"),
        ] {
            let mut v = fixture();
            v.pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(key);
            assert!(load(&v).is_err(), "{key}");
        }
        for (pointer, value) in [
            ("/nodes/1/processor/config/clock", json!("q")),
            ("/nodes/1/processor/config/wave", json!("noise")),
            ("/modulations/0/amount", json!("2Hz")),
            ("/nodes/2/params/value", json!("NaN")),
        ] {
            let mut v = fixture();
            *v.pointer_mut(pointer).unwrap() = value;
            assert!(load(&v).is_err());
        }
        let bytes = serde_json::to_string(&fixture()).unwrap().replace(
            "\"amount\":\"-2/1\"",
            "\"amount\":\"-2/1\",\"amount\":\"1/1\"",
        );
        assert!(crate::PlanArtifact::from_json(bytes.as_bytes()).is_err());
        for version in [0, 1, 2, 3, 4, 5, 6, 8] {
            let mut v = fixture();
            v["version"] = json!(version);
            assert!(load(&v).is_err());
        }
    }
    #[test]
    fn constants_automate_and_control_resource_limits_are_independent() {
        let mut v = fixture();
        v["nodes"][2]["params"] = json!({});
        v["automation"] = json!([{"id":"a","target":{"node":"offset","port":"value"},"clock":"score","at":{"kind":"score","q":"0/1"},"points":[{"position":"0/1","value":"-3/1","shape":"step"}]}]);
        let artifact = load(&v).unwrap();
        assert_eq!(
            artifact
                .view()
                .resolved_node_params(artifact.view().nodes.get(2).unwrap())
                .unwrap()["value"],
            Rational::from_integer(0.into())
        );
        let bytes = serde_json::to_vec(&v).unwrap();
        for limits in [
            crate::plan::PlanLimits {
                max_connections: 1,
                ..Default::default()
            },
            crate::plan::PlanLimits {
                max_rational_bits: 1,
                ..Default::default()
            },
            crate::plan::PlanLimits {
                max_work: 1,
                ..Default::default()
            },
            crate::plan::PlanLimits {
                max_execution_work: 1,
                ..Default::default()
            },
        ] {
            assert_eq!(
                crate::PlanArtifact::from_json_with_limits(&bytes, &limits)
                    .unwrap_err()
                    .code,
                "E_RESOURCE_LIMIT"
            );
        }
        assert_eq!(
            artifact
                .view()
                .audio_output_channels(&crate::plan::PortRef {
                    node: "offset".into(),
                    port: "out".into()
                })
                .unwrap_err()
                .code,
            "E_PORT_TYPE"
        );
    }
    #[test]
    fn disconnected_controls_and_edges_pay_exact_execution_work() {
        let mut plan = PlanV7::decode_wire(&serde_json::to_vec(&fixture()).unwrap()).unwrap();
        let limits = crate::plan::PlanLimits::default();
        let total = plan.view().execution_work(&limits).unwrap();
        let edge = plan.modulations.pop().unwrap();
        assert_eq!(
            total - plan.view().execution_work(&limits).unwrap(),
            24000 * 8
        );
        plan.modulations.push(edge);
        plan.nodes[2]
            .params
            .insert("value".into(), Rational::from_integer(0.into()));
        assert_eq!(plan.view().execution_work(&limits).unwrap(), total);
        plan.modulations.clear();
        let disconnected = plan.view().execution_work(&limits).unwrap();
        plan.nodes.pop();
        assert_eq!(
            disconnected - plan.view().execution_work(&limits).unwrap(),
            24000 * 8
        );
        plan.validate_with_limits(&crate::plan::PlanLimits {
            max_execution_work: plan.view().execution_work(&limits).unwrap() - 1,
            ..limits
        })
        .unwrap_err();
    }
    #[test]
    fn aggregate_lfo_preparation_and_nonfinite_amounts_are_rejected() {
        let mut v = fixture();
        v["modulations"] = json!([]);
        v["nodes"].as_array_mut().unwrap().truncate(2);
        let bytes = serde_json::to_vec(&v).unwrap();
        let limits = crate::plan::PlanLimits {
            max_work: 1000,
            ..Default::default()
        };
        crate::PlanArtifact::from_json_with_limits(&bytes, &limits).unwrap();
        for i in 0..3 {
            let mut node = v["nodes"][1].clone();
            node["id"] = json!(format!("extra{i}"));
            v["nodes"].as_array_mut().unwrap().push(node);
        }
        assert_eq!(
            crate::PlanArtifact::from_json_with_limits(&serde_json::to_vec(&v).unwrap(), &limits)
                .unwrap_err()
                .code,
            "E_RESOURCE_LIMIT"
        );
        let mut v = fixture();
        v["modulations"][0]["amount"] = json!(format!("{}/1", num_bigint::BigInt::from(1) << 2000));
        assert_eq!(load(&v).unwrap_err().code, "E_NONFINITE");
    }
}
