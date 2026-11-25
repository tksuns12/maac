//! Automation lane extraction and transactional `curve` editing via
//! `maac::editing::SourceDocument` + `BundleEditContext::checking_compile`.

use maac::bundle::SourceBundle;
use maac::editing::{Operation, SourceDocument, Transaction};
use maac::parse_rational;
use maac::plan::{PlanLimits, TempoMap};
use maac::PlanArtifact;
use num_traits::ToPrimitive;
use serde_json::{json, Value};

use crate::mixer::{MixerNodeInfo, ParamUnit};
use crate::project::{build_edit_context, EditedBundle, TrackInfo};

#[derive(Clone, Debug)]
pub struct AutomationPointView {
    pub index: usize,
    pub frame: u64,
    pub pos_raw: f64,
    pub value: f32,
    pub shape: String,
}

#[derive(Clone, Debug)]
pub struct AutomationLane {
    pub id: String,
    pub curve_id: Option<String>,
    pub target_node: String,
    pub target_param: String,
    pub track_idx: Option<usize>,
    pub clock_is_seconds: bool,
    pub anchor_raw: f64,
    pub min_val: f32,
    pub max_val: f32,
    pub unit: ParamUnit,
    pub editable_in_entry: bool,
    pub points: Vec<AutomationPointView>,
}

/// Extract all resolved automation lanes from `plan_json` and match them to
/// their editable `curve` objects in the entry source document.
pub fn extract_automations(
    bundle: &SourceBundle,
    plan_json: &Value,
    tracks: &[TrackInfo],
    mixer_nodes: &[MixerNodeInfo],
    sample_rate_hz: u32,
    total_frames: u64,
) -> Vec<AutomationLane> {
    let Some(raw_autos) = plan_json.get("automation").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    if raw_autos.is_empty() {
        return Vec::new();
    }

    let authored_tree = bundle
        .sources
        .get(&bundle.entry)
        .and_then(|src| SourceDocument::parse(src).ok())
        .map(|doc| doc.authored().tree().clone());

    let checked_tempo = plan_json
        .get("tempo")
        .cloned()
        .and_then(|v| serde_json::from_value::<TempoMap>(v).ok())
        .and_then(|tm| tm.checked().ok());

    let score_start_q_str = plan_json
        .pointer("/output/score_start_q")
        .and_then(|v| v.as_str())
        .unwrap_or("0/1");
    let score_start_q =
        parse_rational(score_start_q_str).unwrap_or_else(|_| parse_rational("0").unwrap());
    let origin_s = checked_tempo
        .as_ref()
        .and_then(|t| t.seconds_at(&score_start_q).ok())
        .and_then(|s| s.to_f64())
        .unwrap_or(0.0);

    let q_to_frame = |q_f: f64| -> u64 {
        if let Some(tempo) = &checked_tempo {
            let scaled = (q_f * 16.0).round() as i64;
            let q_str = format!("{scaled}/16");
            if let Ok(q_rat) = parse_rational(&q_str) {
                if let Some(secs) = tempo.seconds_at(&q_rat).ok().and_then(|s| s.to_f64()) {
                    let rel = (secs - origin_s).max(0.0);
                    return ((rel * sample_rate_hz as f64).round() as u64).min(total_frames);
                }
            }
        }
        ((q_f * 0.5 * sample_rate_hz as f64).round() as u64).min(total_frames)
    };

    let mut out = Vec::new();
    for raw in raw_autos {
        let Some(id) = raw.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let target_node = raw
            .pointer("/target/node")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let target_param = raw
            .pointer("/target/port")
            .or_else(|| raw.pointer("/target/parameter"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let clock_is_seconds = raw
            .get("clock")
            .and_then(|v| v.as_str())
            .is_some_and(|c| c == "seconds");

        // `at` can be either a rational string (legacy) or `{"score": "..."}` / `{"seconds": "..."}`
        let anchor_raw = parse_anchor_f64(raw.get("at")).unwrap_or(0.0);

        let curve_id = authored_tree
            .as_ref()
            .and_then(|t| t.pointer(&format!("/objects/{id}/fields/curve/path/0")))
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let editable_in_entry = curve_id.as_deref().is_some_and(|cid| {
            authored_tree
                .as_ref()
                .and_then(|t| t.pointer(&format!("/objects/{cid}/kind")))
                .and_then(|v| v.as_str())
                == Some("curve")
        });

        let Some(raw_points) = raw.get("points").and_then(|v| v.as_array()) else {
            continue;
        };

        let mut points = Vec::new();
        let mut p_min = f32::INFINITY;
        let mut p_max = f32::NEG_INFINITY;
        for (idx, pt) in raw_points.iter().enumerate() {
            let pos = pt
                .get("position")
                .and_then(parse_rational_json_f64)
                .unwrap_or(0.0);
            let value = pt
                .get("value")
                .and_then(parse_rational_json_f64)
                .unwrap_or(0.0) as f32;
            let shape = pt
                .get("shape")
                .and_then(|v| v.as_str())
                .unwrap_or("linear")
                .to_owned();
            let abs_pos = anchor_raw + pos;
            let frame = if clock_is_seconds {
                ((abs_pos.max(0.0) * sample_rate_hz as f64).round() as u64).min(total_frames)
            } else {
                q_to_frame(abs_pos)
            };
            p_min = p_min.min(value);
            p_max = p_max.max(value);
            points.push(AutomationPointView {
                index: idx,
                frame,
                pos_raw: pos,
                value,
                shape,
            });
        }
        if points.is_empty() {
            continue;
        }

        // Derive nice bounds & unit from the target mixer parameter if known.
        let param_meta = mixer_nodes
            .iter()
            .find(|n| n.id == target_node)
            .and_then(|n| n.params.iter().find(|p| p.name == target_param));

        let (min_val, max_val, unit) = match param_meta {
            Some(p) => (p.min.min(p_min), p.max.max(p_max), p.unit),
            None => {
                let lo = if p_min.is_finite() {
                    p_min.min(0.0)
                } else {
                    0.0
                };
                let hi = if p_max.is_finite() {
                    (p_max * 1.25).max(lo + 1.0)
                } else {
                    1.0
                };
                (lo, hi, ParamUnit::Number)
            }
        };

        let track_idx = tracks.iter().position(|t| t.name == target_node);

        out.push(AutomationLane {
            id: id.to_owned(),
            curve_id,
            target_node,
            target_param,
            track_idx,
            clock_is_seconds,
            anchor_raw,
            min_val,
            max_val,
            unit,
            editable_in_entry,
            points,
        });
    }
    out
}

/// Update the value and/or interpolation shape of one control point in `curve <curve_id>`.
pub fn apply_curve_point_edit(
    bundle: &SourceBundle,
    curve_id: &str,
    point_index: usize,
    new_value: Option<f32>,
    new_shape: Option<&str>,
    limits: &PlanLimits,
    base_plan: Option<&PlanArtifact>,
) -> Result<(EditedBundle, String), String> {
    let entry_src = bundle
        .sources
        .get(&bundle.entry)
        .ok_or_else(|| "missing entry source in bundle".to_string())?;
    let mut doc =
        SourceDocument::parse(entry_src).map_err(|err| format!("{}: {}", err.code, err.message))?;

    let mut points_val = doc
        .authored()
        .tree()
        .pointer(&format!("/objects/{curve_id}/fields/points"))
        .cloned()
        .ok_or_else(|| format!("curve `{curve_id}` is not editable in entry document"))?;

    let items = points_val
        .get_mut("items")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| "curve.points is not a list".to_string())?;
    let tuple = items
        .get_mut(point_index)
        .and_then(|v| v.get_mut("items"))
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| format!("point index {point_index} out of range"))?;
    if tuple.len() < 3 {
        return Err("curve point tuple must have 3 elements".into());
    }

    if let Some(val) = new_value {
        let existing_unit = tuple[1]
            .get("t")
            .and_then(|t| t.as_str())
            .filter(|&t| t == "quantity")
            .and_then(|_| tuple[1].get("u").and_then(|u| u.as_str()))
            .map(str::to_owned);
        tuple[1] = encode_curve_value(val, existing_unit.as_deref());
    }
    if let Some(shape) = new_shape {
        tuple[2] = json!({ "t": "symbol", "v": shape });
    }

    let op = Operation::Set {
        object: vec![curve_id.to_owned()],
        field: vec!["points".into()],
        value: points_val,
        expect: None,
        expect_absent: false,
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
    let summary = match (new_value, new_shape) {
        (Some(v), _) => format!("{curve_id}[{point_index}] → {v:.2}"),
        (None, Some(s)) => format!("{curve_id}[{point_index}] shape → {s}"),
        (None, None) => format!("Updated {curve_id}[{point_index}]"),
    };
    Ok((
        EditedBundle {
            bundle: updated,
            compiled_plan,
        },
        summary,
    ))
}

/// Insert a new control point into `curve <curve_id>` in sorted time order.
pub fn apply_curve_point_add(
    bundle: &SourceBundle,
    curve_id: &str,
    pos_raw: f64,
    value: f32,
    clock_is_seconds: bool,
    limits: &PlanLimits,
    base_plan: Option<&PlanArtifact>,
) -> Result<(EditedBundle, usize, String), String> {
    let entry_src = bundle
        .sources
        .get(&bundle.entry)
        .ok_or_else(|| "missing entry source in bundle".to_string())?;
    let mut doc =
        SourceDocument::parse(entry_src).map_err(|err| format!("{}: {}", err.code, err.message))?;

    let mut points_val = doc
        .authored()
        .tree()
        .pointer(&format!("/objects/{curve_id}/fields/points"))
        .cloned()
        .ok_or_else(|| format!("curve `{curve_id}` is not editable in entry document"))?;

    let items = points_val
        .get_mut("items")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| "curve.points is not a list".to_string())?;

    // Inherit unit & default shape from existing points when available.
    let existing_unit = items
        .first()
        .and_then(|p| p.pointer("/items/1"))
        .and_then(|v| (v.get("t")?.as_str()? == "quantity").then(|| v.get("u")?.as_str()))
        .flatten()
        .map(str::to_owned);
    let default_shape = items
        .first()
        .and_then(|p| p.pointer("/items/2/v"))
        .and_then(|v| v.as_str())
        .unwrap_or("linear")
        .to_owned();

    let (pos_json, snapped_pos) = if clock_is_seconds {
        let tenths = (pos_raw.max(0.0) * 10.0).round() as i64;
        let (n, d) = reduce_frac(tenths, 10);
        (
            json!({ "t": "quantity", "n": n.to_string(), "d": d.to_string(), "u": "s" }),
            tenths as f64 / 10.0,
        )
    } else {
        let q4 = (pos_raw.max(0.0) * 4.0).round() as i64;
        let (n, d) = reduce_frac(q4, 4);
        (
            json!({ "t": "quantity", "n": n.to_string(), "d": d.to_string(), "u": "q" }),
            q4 as f64 / 4.0,
        )
    };

    // Find sorted insert position and avoid colliding with an existing timestamp.
    let mut insert_idx = items.len();
    for (idx, pt) in items.iter().enumerate() {
        if let Some(p_pos) = pt.pointer("/items/0").and_then(read_tagged_pos_f64) {
            if (p_pos - snapped_pos).abs() < 1e-4 {
                return Err("a control point already exists at that time".into());
            }
            if snapped_pos < p_pos {
                insert_idx = idx;
                break;
            }
        }
    }

    let new_tuple = json!({
        "t": "tuple",
        "items": [
            pos_json,
            encode_curve_value(value, existing_unit.as_deref()),
            { "t": "symbol", "v": default_shape }
        ]
    });
    items.insert(insert_idx, new_tuple);

    let op = Operation::Set {
        object: vec![curve_id.to_owned()],
        field: vec!["points".into()],
        value: points_val,
        expect: None,
        expect_absent: false,
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
    let unit_lbl = if clock_is_seconds { "s" } else { "q" };
    Ok((
        EditedBundle {
            bundle: updated,
            compiled_plan,
        },
        insert_idx,
        format!("Added point on {curve_id} @ {snapped_pos:.2}{unit_lbl}"),
    ))
}

/// Remove one control point from `curve <curve_id>` (retaining at least one point).
pub fn apply_curve_point_delete(
    bundle: &SourceBundle,
    curve_id: &str,
    point_index: usize,
    limits: &PlanLimits,
    base_plan: Option<&PlanArtifact>,
) -> Result<(EditedBundle, String), String> {
    let entry_src = bundle
        .sources
        .get(&bundle.entry)
        .ok_or_else(|| "missing entry source in bundle".to_string())?;
    let mut doc =
        SourceDocument::parse(entry_src).map_err(|err| format!("{}: {}", err.code, err.message))?;

    let mut points_val = doc
        .authored()
        .tree()
        .pointer(&format!("/objects/{curve_id}/fields/points"))
        .cloned()
        .ok_or_else(|| format!("curve `{curve_id}` is not editable in entry document"))?;

    let items = points_val
        .get_mut("items")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| "curve.points is not a list".to_string())?;
    if items.len() <= 1 {
        return Err("curve must keep at least 1 control point".into());
    }
    if point_index >= items.len() {
        return Err("point index out of range".into());
    }
    items.remove(point_index);

    let op = Operation::Set {
        object: vec![curve_id.to_owned()],
        field: vec!["points".into()],
        value: points_val,
        expect: None,
        expect_absent: false,
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
        format!("Deleted {curve_id}[{point_index}]"),
    ))
}

fn encode_curve_value(val: f32, unit: Option<&str>) -> Value {
    match unit {
        Some("Hz") => {
            let hz = (val as f64).round().max(1.0) as i64;
            json!({ "t": "quantity", "n": hz.to_string(), "d": "1", "u": "Hz" })
        }
        Some("dB") => {
            let scaled = (val as f64 * 10.0).round() as i64;
            let (n, d) = reduce_frac(scaled, 10);
            json!({ "t": "quantity", "n": n.to_string(), "d": d.to_string(), "u": "dB" })
        }
        Some("ms") => {
            let ms = (val as f64 * 1000.0).round().max(1.0) as i64;
            json!({ "t": "quantity", "n": ms.to_string(), "d": "1", "u": "ms" })
        }
        Some(u) => {
            let scaled = (val as f64 * 100.0).round() as i64;
            let (n, d) = reduce_frac(scaled, 100);
            json!({ "t": "quantity", "n": n.to_string(), "d": d.to_string(), "u": u })
        }
        None => {
            let scaled = (val as f64 * 100.0).round() as i64;
            let (n, d) = reduce_frac(scaled, 100);
            json!({ "t": "number", "n": n.to_string(), "d": d.to_string() })
        }
    }
}

fn parse_anchor_f64(val: Option<&Value>) -> Option<f64> {
    let v = val?;
    if let Some(f) = parse_rational_json_f64(v) {
        return Some(f);
    }
    if let Some(obj) = v.as_object() {
        for key in ["score", "seconds", "value", "q", "s"] {
            if let Some(inner) = obj.get(key) {
                if let Some(f) = parse_rational_json_f64(inner) {
                    return Some(f);
                }
            }
        }
    }
    None
}

fn parse_rational_json_f64(val: &Value) -> Option<f64> {
    if let Some(s) = val.as_str() {
        if let Some((n, d)) = s.split_once('/') {
            let num: f64 = n.parse().ok()?;
            let den: f64 = d.parse().ok()?;
            return (den != 0.0).then_some(num / den);
        }
        return s.parse::<f64>().ok();
    }
    val.as_f64()
}

fn read_tagged_pos_f64(val: &Value) -> Option<f64> {
    let n: f64 = val.get("n")?.as_str()?.parse().ok()?;
    let d: f64 = val.get("d")?.as_str()?.parse().ok()?;
    (d > 0.0).then_some(n / d)
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
    fn extracts_and_edits_lead_drive_automation_in_ferry() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/ferry/ferry.maac");
        let data = load_and_compile(&path).expect("compile ferry");
        assert!(!data.automations.is_empty(), "expected automation in ferry");
        let lane = &data.automations[0];
        assert_eq!(lane.target_node, "lead");
        assert_eq!(lane.target_param, "drive");
        assert_eq!(lane.curve_id.as_deref(), Some("lead_drive"));
        assert!(lane.editable_in_entry);
        assert_eq!(lane.points.len(), 5);

        // Edit point 1 (chorus1 drive: 12 -> 8.5, shape -> linear)
        let (b1, _) = apply_curve_point_edit(
            &data.bundle,
            "lead_drive",
            1,
            Some(8.5),
            Some("linear"),
            &data.limits,
            Some(&data.plan),
        )
        .expect("edit lead_drive point 1");
        assert!(b1.compiled_plan.is_some());
        let d1 = load_from_edited(
            data.entry_path.clone(),
            data.project_root.clone(),
            b1.clone(),
        )
        .expect("recompile edited curve");
        assert!((d1.automations[0].points[1].value - 8.5).abs() < 0.05);
        assert_eq!(d1.automations[0].points[1].shape, "linear");

        // Add a new point at 64q with value 10.0, then delete it cleanly.
        let (b2, idx, _) = apply_curve_point_add(
            &b1.bundle,
            "lead_drive",
            64.0,
            10.0,
            false,
            &data.limits,
            Some(&d1.plan),
        )
        .expect("add point on lead_drive");
        assert!(b2.compiled_plan.is_some());
        let d2 = load_from_edited(
            data.entry_path.clone(),
            data.project_root.clone(),
            b2.clone(),
        )
        .expect("recompile after add point");
        assert_eq!(d2.automations[0].points.len(), 6);

        let (b3, _) =
            apply_curve_point_delete(&b2.bundle, "lead_drive", idx, &data.limits, Some(&d2.plan))
                .expect("delete added point");
        assert!(b3.compiled_plan.is_some());
        let d3 = load_from_edited(data.entry_path, data.project_root, b3)
            .expect("recompile after delete point");
        assert_eq!(d3.automations[0].points.len(), 5);
    }
}
