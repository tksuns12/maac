//! Live mixer & node/FX parameter inspection and transactional editing via
//! `maac::editing::SourceDocument` + `BundleEditContext::checking_compile`.

use std::collections::{BTreeMap, HashSet};

use maac::bundle::SourceBundle;
use maac::editing::{Operation, SourceDocument, Transaction};
use maac::graph::{GraphUnit, ParameterSpec};
use maac::instrument_plan::InstrumentResources;
use maac::plan::PlanLimits;
use maac::PlanArtifact;
use num_traits::ToPrimitive;
use serde_json::{json, Value};

use crate::project::{build_edit_context, EditedBundle, TrackInfo};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeCategory {
    Instrument,
    Effect,
    MixBus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParamUnit {
    Number,
    Decibels,
    Hertz,
    Seconds,
    Quarter,
}

#[derive(Clone, Debug)]
pub struct NodeParamInfo {
    pub name: String,
    pub unit: ParamUnit,
    pub value: f32,
    #[cfg_attr(not(test), allow(dead_code))]
    pub default: f32,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub is_authored: bool,
    pub is_automated: bool,
    pub logarithmic: bool,
}

impl NodeParamInfo {
    /// Convert a real parameter value into a normalized `0.0..=1.0` slider position.
    pub fn to_slider_pos(&self, val: f32) -> f32 {
        let clamped = val.clamp(self.min, self.max);
        if self.logarithmic && self.min > 0.0 && self.max > self.min {
            let lo = self.min.ln();
            let hi = self.max.ln();
            ((clamped.ln() - lo) / (hi - lo)).clamp(0.0, 1.0)
        } else {
            let span = (self.max - self.min).max(1e-6);
            ((clamped - self.min) / span).clamp(0.0, 1.0)
        }
    }

    /// Convert a normalized `0.0..=1.0` slider position back into a real value.
    pub fn value_at_slider_pos(&self, pos: f32) -> f32 {
        let t = pos.clamp(0.0, 1.0);
        let raw = if self.logarithmic && self.min > 0.0 && self.max > self.min {
            let lo = self.min.ln();
            let hi = self.max.ln();
            (lo + t * (hi - lo)).exp()
        } else {
            self.min + t * (self.max - self.min)
        };
        if self.step > 0.0 && !self.logarithmic {
            let snapped = (raw / self.step).round() * self.step;
            snapped.clamp(self.min, self.max)
        } else {
            raw.clamp(self.min, self.max)
        }
    }

    /// Format a value cleanly for display on mixer strips and the inspector.
    pub fn format_value(&self, val: f32) -> String {
        format_param_value(self.unit, val)
    }
}

#[derive(Clone, Debug)]
pub struct MixerNodeInfo {
    pub id: String,
    pub processor_label: String,
    pub category: NodeCategory,
    pub track_idx: Option<usize>,
    pub editable_in_entry: bool,
    pub params: Vec<NodeParamInfo>,
}

/// Extract all parameter-bearing nodes from a compiled plan JSON + entry source.
pub fn extract_mixer_nodes(
    bundle: &SourceBundle,
    plan_json: &Value,
    tracks: &[TrackInfo],
    sample_rate_hz: u32,
) -> Vec<MixerNodeInfo> {
    let authored_tree = bundle
        .sources
        .get(&bundle.entry)
        .and_then(|src| SourceDocument::parse(src).ok())
        .map(|doc| doc.authored().tree().clone());

    let resources: Option<InstrumentResources> = plan_json
        .get("instruments")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok());

    let mut automated: HashSet<(String, String)> = HashSet::new();
    if let Some(autos) = plan_json.get("automation").and_then(|v| v.as_array()) {
        for a in autos {
            let node = a
                .pointer("/target/node")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let param = a
                .pointer("/target/port")
                .or_else(|| a.pointer("/target/parameter"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !node.is_empty() && !param.is_empty() {
                automated.insert((node.to_owned(), param.to_owned()));
            }
        }
    }

    let Some(nodes) = plan_json.get("nodes").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for node in nodes {
        let Some(id) = node.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(outer_proc) = node.get("processor").and_then(|v| v.as_object()) else {
            continue;
        };
        let Some(proc_obj) = (if outer_proc.get("kind").and_then(|v| v.as_str()) == Some("core") {
            outer_proc.get("processor").and_then(|v| v.as_object())
        } else {
            Some(outer_proc)
        }) else {
            continue;
        };
        let Some(kind) = proc_obj.get("kind").and_then(|v| v.as_str()) else {
            continue;
        };

        let resolved_params = parse_resolved_params(node.get("params"));
        let authored_node = authored_tree
            .as_ref()
            .and_then(|t| t.get("objects"))
            .and_then(|o| o.get(id))
            .filter(|o| o.get("kind").and_then(|k| k.as_str()) == Some("node"));
        let editable_in_entry = authored_node.is_some();
        let authored_param_keys: HashSet<String> = authored_node
            .and_then(|n| n.pointer("/fields/params/fields"))
            .and_then(|v| v.as_object())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();

        let track_idx = tracks.iter().position(|t| t.name == id);
        let (category, processor_label, mut params) = build_node_params(
            id,
            kind,
            proc_obj,
            &resolved_params,
            &authored_param_keys,
            &automated,
            resources.as_ref(),
            sample_rate_hz,
        );

        if params.is_empty() {
            continue;
        }
        params.sort_by_key(|p| (param_priority(&p.name), p.name.clone()));

        out.push(MixerNodeInfo {
            id: id.to_owned(),
            processor_label,
            category,
            track_idx,
            editable_in_entry,
            params,
        });
    }

    // Order mixer strips logically: Instruments first, then MixBus / Effects.
    out.sort_by_key(|n| match n.category {
        NodeCategory::Instrument => (0u8, n.track_idx.unwrap_or(usize::MAX)),
        NodeCategory::MixBus => (1u8, 0),
        NodeCategory::Effect => (2u8, 0),
    });
    out
}

#[allow(clippy::too_many_arguments)]
fn build_node_params(
    node_id: &str,
    kind: &str,
    proc_obj: &serde_json::Map<String, Value>,
    resolved: &BTreeMap<String, f32>,
    authored_keys: &HashSet<String>,
    automated: &HashSet<(String, String)>,
    resources: Option<&InstrumentResources>,
    sample_rate_hz: u32,
) -> (NodeCategory, String, Vec<NodeParamInfo>) {
    let mut params = Vec::new();
    let nyquist = (sample_rate_hz as f32 * 0.5 - 10.0).max(1000.0);

    match kind {
        "instrument" => {
            let prog_id = proc_obj
                .get("program")
                .and_then(|v| v.as_str())
                .unwrap_or("instrument");
            let mut label = short_program_label(prog_id);
            if let Some(program) =
                resources.and_then(|r| r.programs.iter().find(|p| p.id == prog_id))
            {
                if !program.source.object.is_empty() {
                    label = program.source.object.clone();
                }
                for (name, control) in &program.controls {
                    let Some(spec) = program.control_spec(name) else {
                        continue;
                    };
                    let default = control.default.to_f32().unwrap_or(0.0);
                    let value = resolved.get(name).copied().unwrap_or(default);
                    params.push(param_from_spec(
                        name,
                        &spec,
                        value,
                        default,
                        authored_keys.contains(name),
                        automated.contains(&(node_id.to_owned(), name.clone())),
                        nyquist,
                    ));
                }
            }
            (NodeCategory::Instrument, label, params)
        }
        "kit_instrument" => {
            let kit_id = proc_obj
                .get("kit")
                .and_then(|v| v.as_str())
                .unwrap_or("kit");
            let mut label = short_program_label(kit_id);
            if let Some(res) = resources {
                if let Some(kit) = res.kits.iter().find(|k| k.id == kit_id) {
                    if !kit.source.object.is_empty() {
                        label = kit.source.object.clone();
                    }
                    for (name, control) in &kit.controls {
                        let Some(spec) = kit.control_spec(&res.programs, name) else {
                            continue;
                        };
                        let default = control.default.to_f32().unwrap_or(0.0);
                        let value = resolved.get(name).copied().unwrap_or(default);
                        params.push(param_from_spec(
                            name,
                            &spec,
                            value,
                            default,
                            authored_keys.contains(name),
                            automated.contains(&(node_id.to_owned(), name.clone())),
                            nyquist,
                        ));
                    }
                }
            }
            (NodeCategory::Instrument, label, params)
        }
        "effect" => {
            let eff_id = proc_obj
                .get("effect")
                .and_then(|v| v.as_str())
                .unwrap_or("effect");
            let mut label = short_program_label(eff_id);
            if let Some(effect) = resources.and_then(|r| r.effects.iter().find(|e| e.id == eff_id))
            {
                if !effect.source.object.is_empty() {
                    label = effect.source.object.clone();
                }
                for (name, control) in &effect.controls {
                    let Some(spec) = effect.control_spec(name) else {
                        continue;
                    };
                    let default = control.default.to_f32().unwrap_or(0.0);
                    let value = resolved.get(name).copied().unwrap_or(default);
                    params.push(param_from_spec(
                        name,
                        &spec,
                        value,
                        default,
                        authored_keys.contains(name),
                        automated.contains(&(node_id.to_owned(), name.clone())),
                        nyquist,
                    ));
                }
            }
            (NodeCategory::Effect, label, params)
        }
        "fx.reverb/1" => {
            push_builtin(
                &mut params,
                node_id,
                "mix",
                ParamUnit::Number,
                resolved,
                0.2,
                0.0,
                1.0,
                0.02,
                false,
                authored_keys,
                automated,
            );
            push_builtin(
                &mut params,
                node_id,
                "decay",
                ParamUnit::Seconds,
                resolved,
                1.5,
                0.1,
                12.0,
                0.05,
                true,
                authored_keys,
                automated,
            );
            (NodeCategory::Effect, "fx.reverb/1".into(), params)
        }
        "fx.compressor/1" => {
            push_builtin(
                &mut params,
                node_id,
                "threshold",
                ParamUnit::Decibels,
                resolved,
                -18.0,
                -48.0,
                0.0,
                0.5,
                false,
                authored_keys,
                automated,
            );
            push_builtin(
                &mut params,
                node_id,
                "ratio",
                ParamUnit::Number,
                resolved,
                4.0,
                1.0,
                20.0,
                0.1,
                false,
                authored_keys,
                automated,
            );
            push_builtin(
                &mut params,
                node_id,
                "knee",
                ParamUnit::Decibels,
                resolved,
                6.0,
                0.0,
                24.0,
                0.5,
                false,
                authored_keys,
                automated,
            );
            push_builtin(
                &mut params,
                node_id,
                "attack",
                ParamUnit::Seconds,
                resolved,
                0.01,
                0.001,
                0.25,
                0.001,
                true,
                authored_keys,
                automated,
            );
            push_builtin(
                &mut params,
                node_id,
                "release",
                ParamUnit::Seconds,
                resolved,
                0.1,
                0.01,
                1.5,
                0.005,
                true,
                authored_keys,
                automated,
            );
            push_builtin(
                &mut params,
                node_id,
                "makeup",
                ParamUnit::Decibels,
                resolved,
                0.0,
                -12.0,
                24.0,
                0.5,
                false,
                authored_keys,
                automated,
            );
            (NodeCategory::Effect, "fx.compressor/1".into(), params)
        }
        "fx.limiter/1" => {
            push_builtin(
                &mut params,
                node_id,
                "gain",
                ParamUnit::Decibels,
                resolved,
                0.0,
                0.0,
                18.0,
                0.2,
                false,
                authored_keys,
                automated,
            );
            push_builtin(
                &mut params,
                node_id,
                "ceiling",
                ParamUnit::Decibels,
                resolved,
                -1.0,
                -12.0,
                0.0,
                0.1,
                false,
                authored_keys,
                automated,
            );
            push_builtin(
                &mut params,
                node_id,
                "release",
                ParamUnit::Seconds,
                resolved,
                0.1,
                0.005,
                1.0,
                0.005,
                true,
                authored_keys,
                automated,
            );
            (NodeCategory::Effect, "fx.limiter/1".into(), params)
        }
        "fx.eq/1" => {
            let mode = proc_obj
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("peak");
            push_builtin(
                &mut params,
                node_id,
                "frequency",
                ParamUnit::Hertz,
                resolved,
                1000.0,
                20.0,
                nyquist.min(20_000.0),
                1.0,
                true,
                authored_keys,
                automated,
            );
            if resolved.contains_key("gain") || matches!(mode, "peak" | "low_shelf" | "high_shelf")
            {
                push_builtin(
                    &mut params,
                    node_id,
                    "gain",
                    ParamUnit::Decibels,
                    resolved,
                    0.0,
                    -24.0,
                    24.0,
                    0.5,
                    false,
                    authored_keys,
                    automated,
                );
            }
            push_builtin(
                &mut params,
                node_id,
                "q",
                ParamUnit::Number,
                resolved,
                1.0,
                0.1,
                18.0,
                0.1,
                true,
                authored_keys,
                automated,
            );
            (NodeCategory::Effect, format!("fx.eq ({mode})"), params)
        }
        "gain" => {
            let cur = resolved.get("gain").copied().unwrap_or(1.0);
            let hi = (cur * 1.6).ceil().max(2.0);
            push_builtin(
                &mut params,
                node_id,
                "gain",
                ParamUnit::Number,
                resolved,
                1.0,
                0.0,
                hi,
                0.02,
                false,
                authored_keys,
                automated,
            );
            (NodeCategory::MixBus, "core.gain/1".into(), params)
        }
        "fader" => {
            push_builtin(
                &mut params,
                node_id,
                "level",
                ParamUnit::Decibels,
                resolved,
                0.0,
                -48.0,
                12.0,
                0.5,
                false,
                authored_keys,
                automated,
            );
            (NodeCategory::MixBus, "core.fader/1".into(), params)
        }
        "pan" => {
            push_builtin(
                &mut params,
                node_id,
                "pan",
                ParamUnit::Number,
                resolved,
                0.0,
                -1.0,
                1.0,
                0.05,
                false,
                authored_keys,
                automated,
            );
            (NodeCategory::MixBus, "core.pan/1".into(), params)
        }
        "one_pole" | "onepole" => {
            push_builtin(
                &mut params,
                node_id,
                "cutoff",
                ParamUnit::Hertz,
                resolved,
                1000.0,
                20.0,
                nyquist.min(20_000.0),
                1.0,
                true,
                authored_keys,
                automated,
            );
            (NodeCategory::Effect, "core.onepole/1".into(), params)
        }
        "sine" => {
            push_builtin(
                &mut params,
                node_id,
                "level",
                ParamUnit::Number,
                resolved,
                0.2,
                0.0,
                2.0,
                0.02,
                false,
                authored_keys,
                automated,
            );
            push_builtin(
                &mut params,
                node_id,
                "attack",
                ParamUnit::Seconds,
                resolved,
                0.005,
                0.001,
                2.0,
                0.005,
                true,
                authored_keys,
                automated,
            );
            push_builtin(
                &mut params,
                node_id,
                "release",
                ParamUnit::Seconds,
                resolved,
                0.08,
                0.005,
                4.0,
                0.01,
                true,
                authored_keys,
                automated,
            );
            (NodeCategory::Instrument, "core.sine/1".into(), params)
        }
        _ => (NodeCategory::MixBus, kind.to_owned(), params),
    }
}

#[allow(clippy::too_many_arguments)]
fn push_builtin(
    out: &mut Vec<NodeParamInfo>,
    node_id: &str,
    name: &str,
    unit: ParamUnit,
    resolved: &BTreeMap<String, f32>,
    default: f32,
    min: f32,
    max: f32,
    step: f32,
    logarithmic: bool,
    authored_keys: &HashSet<String>,
    automated: &HashSet<(String, String)>,
) {
    let value = resolved.get(name).copied().unwrap_or(default);
    let eff_min = min.min(value);
    let eff_max = max.max(value);
    out.push(NodeParamInfo {
        name: name.to_owned(),
        unit,
        value,
        default,
        min: eff_min,
        max: eff_max,
        step,
        is_authored: authored_keys.contains(name),
        is_automated: automated.contains(&(node_id.to_owned(), name.to_owned())),
        logarithmic,
    });
}

fn param_from_spec(
    name: &str,
    spec: &ParameterSpec,
    value: f32,
    default: f32,
    is_authored: bool,
    is_automated: bool,
    nyquist: f32,
) -> NodeParamInfo {
    let unit = match spec.unit {
        GraphUnit::Dimensionless => ParamUnit::Number,
        GraphUnit::Seconds => ParamUnit::Seconds,
        GraphUnit::Hertz => ParamUnit::Hertz,
        GraphUnit::Quarter => ParamUnit::Quarter,
    };
    let raw_min = spec.min.to_f32().unwrap_or(0.0);
    let raw_max = spec.max.to_f32().unwrap_or(1.0);
    let legal_min = if spec.min_open {
        (raw_min + 1e-3).max(if raw_min >= 0.0 { 1e-3 } else { raw_min + 1e-3 })
    } else {
        raw_min
    };
    let legal_max = if spec.max_open {
        (raw_max - 1e-3).min(nyquist)
    } else {
        raw_max
    };

    let (min, max, step, logarithmic) = match unit {
        ParamUnit::Hertz => {
            let lo = legal_min.max(20.0).min(value.max(20.0));
            let hi = legal_max
                .min(nyquist)
                .min(20_000.0)
                .max(value)
                .max(lo + 10.0);
            (lo, hi, 1.0, true)
        }
        ParamUnit::Seconds => {
            let lo = legal_min.max(0.001).min(value.max(0.001));
            let hi = legal_max
                .min(6.0_f32.max(value * 1.5).max(default * 1.5))
                .max(lo + 0.01);
            (lo, hi, 0.005, true)
        }
        ParamUnit::Quarter => {
            let hi = legal_max
                .min(8.0_f32.max(value * 1.5))
                .max(legal_min + 0.25);
            (legal_min, hi, 0.25, false)
        }
        ParamUnit::Decibels => {
            let lo = legal_min.max(-48.0).min(value);
            let hi = legal_max.min(24.0).max(value).max(lo + 1.0);
            (lo, hi, 0.5, false)
        }
        ParamUnit::Number => {
            if name.contains("pan") || name == "balance" {
                let lo = legal_min.max(-1.0).min(value);
                let hi = legal_max.min(1.0).max(value);
                (lo, hi, 0.05, false)
            } else if legal_max <= 1.01 && legal_min >= -1.01 {
                (legal_min, legal_max.max(legal_min + 0.01), 0.02, false)
            } else if name.contains("drive") {
                let hi = legal_max
                    .min(16.0_f32.max((value * 1.5).ceil()))
                    .max(legal_min + 1.0);
                (legal_min, hi, 0.25, false)
            } else {
                let cap = if name.contains("level") || name.contains("gain") {
                    6.0_f32
                        .max((value * 1.5).ceil())
                        .max((default * 2.0).ceil())
                } else {
                    10.0_f32
                        .max((value * 1.5).ceil())
                        .max((default * 2.0).ceil())
                };
                let hi = legal_max.min(cap).max(value).max(legal_min + 0.1);
                (legal_min, hi, 0.05, false)
            }
        }
    };

    NodeParamInfo {
        name: name.to_owned(),
        unit,
        value,
        default,
        min,
        max,
        step,
        is_authored,
        is_automated,
        logarithmic,
    }
}

/// Apply a transactional parameter edit on `node <node_id>` in the entry source
/// of `bundle`, validated atomically via `BundleEditContext::checking_compile`.
pub fn apply_node_param_edit(
    bundle: &SourceBundle,
    node_id: &str,
    param_name: &str,
    unit: ParamUnit,
    value: f32,
    limits: &PlanLimits,
    base_plan: Option<&PlanArtifact>,
) -> Result<(EditedBundle, String), String> {
    let entry_src = bundle
        .sources
        .get(&bundle.entry)
        .ok_or_else(|| "missing entry source in bundle".to_string())?;
    let mut doc =
        SourceDocument::parse(entry_src).map_err(|err| format!("{}: {}", err.code, err.message))?;

    let node_obj = doc
        .authored()
        .tree()
        .get("objects")
        .and_then(|o| o.get(node_id))
        .ok_or_else(|| format!("node `{node_id}` is not editable in entry document"))?;
    let has_params_record = node_obj
        .pointer("/fields/params")
        .and_then(|v| v.get("t"))
        .and_then(|t| t.as_str())
        == Some("record");

    let wire_val = param_value_json(unit, value);
    let op = if has_params_record {
        Operation::Set {
            object: vec![node_id.to_owned()],
            field: vec!["params".into(), param_name.to_owned()],
            value: wire_val,
            expect: None,
            expect_absent: false,
        }
    } else {
        Operation::Set {
            object: vec![node_id.to_owned()],
            field: vec!["params".into()],
            value: json!({
                "t": "record",
                "fields": {
                    param_name: wire_val
                }
            }),
            expect: None,
            expect_absent: false,
        }
    };

    let tx = Transaction::new(doc.revision().to_owned(), vec![op])
        .map_err(|err| format!("{}: {}", err.code, err.message))?;
    let context = build_edit_context(bundle, &doc, limits, base_plan)?;
    doc.apply(&tx, &context)
        .map_err(|err| format!("{}: {}", err.code, err.message))?;
    let compiled_plan = context.take_compiled_artifact(doc.authored());

    let mut updated = bundle.clone();
    updated
        .sources
        .insert(updated.entry.clone(), doc.source().to_owned());
    let summary = format!(
        "{node_id}.{param_name} → {}",
        format_param_value(unit, value)
    );
    Ok((
        EditedBundle {
            bundle: updated,
            compiled_plan,
        },
        summary,
    ))
}

/// Remove an authored override from `node.<node_id>.params.<param_name>` so it
/// returns cleanly to its declared default.
pub fn apply_node_param_reset(
    bundle: &SourceBundle,
    node_id: &str,
    param_name: &str,
    limits: &PlanLimits,
    base_plan: Option<&PlanArtifact>,
) -> Result<(EditedBundle, String), String> {
    let entry_src = bundle
        .sources
        .get(&bundle.entry)
        .ok_or_else(|| "missing entry source in bundle".to_string())?;
    let mut doc =
        SourceDocument::parse(entry_src).map_err(|err| format!("{}: {}", err.code, err.message))?;

    let op = Operation::Unset {
        object: vec![node_id.to_owned()],
        field: vec!["params".into(), param_name.to_owned()],
        expect: None,
    };
    let tx = Transaction::new(doc.revision().to_owned(), vec![op])
        .map_err(|err| format!("{}: {}", err.code, err.message))?;
    let context = build_edit_context(bundle, &doc, limits, base_plan)?;
    doc.apply(&tx, &context)
        .map_err(|err| format!("{}: {}", err.code, err.message))?;
    let compiled_plan = context.take_compiled_artifact(doc.authored());

    let mut updated = bundle.clone();
    updated
        .sources
        .insert(updated.entry.clone(), doc.source().to_owned());
    Ok((
        EditedBundle {
            bundle: updated,
            compiled_plan,
        },
        format!("Reset {node_id}.{param_name}"),
    ))
}

pub fn format_param_value(unit: ParamUnit, val: f32) -> String {
    match unit {
        ParamUnit::Number => format!("{val:.2}"),
        ParamUnit::Decibels => format!("{val:+.1} dB"),
        ParamUnit::Hertz => {
            if val >= 1000.0 {
                format!("{:.2} kHz", val / 1000.0)
            } else {
                format!("{:.0} Hz", val.round())
            }
        }
        ParamUnit::Seconds => {
            if val < 1.0 {
                format!("{:.0} ms", (val * 1000.0).round())
            } else {
                format!("{val:.2} s")
            }
        }
        ParamUnit::Quarter => format!("{val:.2} q"),
    }
}

fn param_value_json(unit: ParamUnit, val: f32) -> Value {
    match unit {
        ParamUnit::Number => {
            let scaled = (val as f64 * 100.0).round() as i64;
            let (n, d) = reduce_frac(scaled, 100);
            json!({ "t": "number", "n": n.to_string(), "d": d.to_string() })
        }
        ParamUnit::Decibels => {
            let scaled = (val as f64 * 10.0).round() as i64;
            let (n, d) = reduce_frac(scaled, 10);
            json!({ "t": "quantity", "n": n.to_string(), "d": d.to_string(), "u": "dB" })
        }
        ParamUnit::Hertz => {
            let hz = (val as f64).round().max(1.0) as i64;
            json!({ "t": "quantity", "n": hz.to_string(), "d": "1", "u": "Hz" })
        }
        ParamUnit::Seconds => {
            if val < 1.0 {
                let ms = (val as f64 * 1000.0).round().max(1.0) as i64;
                json!({ "t": "quantity", "n": ms.to_string(), "d": "1", "u": "ms" })
            } else {
                let scaled = (val as f64 * 100.0).round().max(1.0) as i64;
                let (n, d) = reduce_frac(scaled, 100);
                json!({ "t": "quantity", "n": n.to_string(), "d": d.to_string(), "u": "s" })
            }
        }
        ParamUnit::Quarter => {
            let scaled = (val as f64 * 4.0).round().max(1.0) as i64;
            let (n, d) = reduce_frac(scaled, 4);
            json!({ "t": "quantity", "n": n.to_string(), "d": d.to_string(), "u": "q" })
        }
    }
}

fn parse_resolved_params(raw: Option<&Value>) -> BTreeMap<String, f32> {
    let mut map = BTreeMap::new();
    let Some(obj) = raw.and_then(|v| v.as_object()) else {
        return map;
    };
    for (k, v) in obj {
        if let Some(s) = v.as_str() {
            if let Some(f) = parse_rational_str(s) {
                map.insert(k.clone(), f);
            }
        } else if let Some(f) = v.as_f64() {
            map.insert(k.clone(), f as f32);
        }
    }
    map
}

fn parse_rational_str(s: &str) -> Option<f32> {
    if let Some((n, d)) = s.split_once('/') {
        let num: f64 = n.parse().ok()?;
        let den: f64 = d.parse().ok()?;
        (den != 0.0).then_some((num / den) as f32)
    } else {
        s.parse::<f64>().ok().map(|v| v as f32)
    }
}

fn short_program_label(id: &str) -> String {
    id.rsplit(['/', ':', '.'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(id)
        .to_owned()
}

fn param_priority(name: &str) -> u8 {
    match name {
        "level" => 0,
        "gain" => 1,
        "pan" => 2,
        "mix" => 3,
        "threshold" => 4,
        "ceiling" => 5,
        "ratio" => 6,
        "drive" => 7,
        "brightness" | "tone" | "cutoff" | "frequency" => 8,
        "decay" => 9,
        "attack" => 10,
        "release" => 11,
        "makeup" => 12,
        _ => 50,
    }
}

fn gcd_i64(mut a: i64, mut b: i64) -> i64 {
    a = a.abs();
    b = b.abs();
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a.max(1)
}

fn reduce_frac(n: i64, d: i64) -> (i64, i64) {
    let g = gcd_i64(n, d);
    (n / g, d / g)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{load_and_compile, load_from_edited};
    use std::path::Path;

    #[test]
    fn extracts_and_edits_ferry_mixer_and_fx_nodes() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/ferry/ferry.maac");
        let data = load_and_compile(&path).expect("compile ferry");

        assert!(
            data.mixer_nodes.len() >= 10,
            "expected instruments + fx/mix nodes in ferry, got {}",
            data.mixer_nodes.len()
        );
        let lead = data
            .mixer_nodes
            .iter()
            .find(|n| n.id == "lead")
            .expect("lead node");
        assert_eq!(lead.category, NodeCategory::Instrument);
        let drive = lead
            .params
            .iter()
            .find(|p| p.name == "drive")
            .expect("lead drive");
        assert!(drive.is_automated, "lead.drive should be flagged automated");

        let hall = data
            .mixer_nodes
            .iter()
            .find(|n| n.id == "hall")
            .expect("hall reverb node");
        assert_eq!(hall.category, NodeCategory::Effect);
        assert!(hall.params.iter().any(|p| p.name == "mix"));
        assert!(hall.params.iter().any(|p| p.name == "decay"));

        // Edit reverb mix, compressor threshold, and instrument level atomically.
        let (b1, _) = apply_node_param_edit(
            &data.bundle,
            "hall",
            "mix",
            ParamUnit::Number,
            0.55,
            &data.limits,
            Some(&data.plan),
        )
        .expect("edit hall.mix");
        assert!(b1.compiled_plan.is_some());
        assert!(b1.bundle.sources[&b1.bundle.entry].contains("Morning Ferry"));

        let (b2, _) = apply_node_param_edit(
            &b1.bundle,
            "glue",
            "threshold",
            ParamUnit::Decibels,
            -14.5,
            &data.limits,
            b1.compiled_plan.as_ref(),
        )
        .expect("edit glue.threshold");
        assert!(b2.compiled_plan.is_some());

        let (b3, _) = apply_node_param_edit(
            &b2.bundle,
            "picking",
            "brightness",
            ParamUnit::Hertz,
            4200.0,
            &data.limits,
            b2.compiled_plan.as_ref(),
        )
        .expect("edit picking.brightness");
        assert!(b3.compiled_plan.is_some());

        let recompiled = load_from_edited(
            data.entry_path.clone(),
            data.project_root.clone(),
            b3.clone(),
        )
        .expect("recompile edited ferry");
        let hall2 = recompiled
            .mixer_nodes
            .iter()
            .find(|n| n.id == "hall")
            .unwrap();
        let mix2 = hall2.params.iter().find(|p| p.name == "mix").unwrap();
        assert!((mix2.value - 0.55).abs() < 0.02);

        // Reset picking.brightness back to default.
        let (b4, _) = apply_node_param_reset(
            &b3.bundle,
            "picking",
            "brightness",
            &data.limits,
            Some(&recompiled.plan),
        )
        .expect("reset picking.brightness");
        assert!(b4.compiled_plan.is_some());
        let recompiled2 = load_from_edited(data.entry_path.clone(), data.project_root.clone(), b4)
            .expect("recompile after reset");
        let picking2 = recompiled2
            .mixer_nodes
            .iter()
            .find(|n| n.id == "picking")
            .unwrap();
        let bright2 = picking2
            .params
            .iter()
            .find(|p| p.name == "brightness")
            .unwrap();
        assert!(!bright2.is_authored);
    }

    #[test]
    fn creates_params_record_when_node_had_none_in_hello() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/hello.maac");
        let data = load_and_compile(&path).expect("compile hello");
        let piano = data
            .mixer_nodes
            .iter()
            .find(|n| n.id == "piano")
            .expect("piano node");
        let first_param = piano.params.first().expect("piano control");
        let (edited, _) = apply_node_param_edit(
            &data.bundle,
            "piano",
            &first_param.name,
            first_param.unit,
            first_param.default,
            &data.limits,
            Some(&data.plan),
        )
        .expect("set param on node without existing params record");
        assert!(edited.compiled_plan.is_some());
        load_from_edited(data.entry_path, data.project_root, edited)
            .expect("recompile hello after adding params");
    }
}
