//! Transactional `.maac` note editor powered by `maac::editing::SourceDocument`
//! and `maac::editing::BundleEditContext::checking_compile`.
//!
//! Every edit is validated atomically against the full bundle and compiler
//! before committing; untouched source comments and formatting are preserved
//! byte-for-byte.

use std::collections::BTreeSet;

use maac::bundle::SourceBundle;
use maac::editing::{BundleEditContext, Operation, SourceDocument, Transaction};
use maac::plan::PlanLimits;
use serde_json::{json, Value};

const PITCH_NAMES: [&str; 12] = [
    "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
];

#[derive(Clone, Debug)]
pub struct NoteSourceInfo {
    pub path: Vec<String>,
    pub kind: String,
    pub pitch_symbol: Option<String>,
    pub pitch_midi: Option<i32>,
    pub hit_key: Option<String>,
    pub at_q: (i64, i64),
    pub dur_q: (i64, i64),
    pub velocity: Option<f32>,
    pub pattern_length_q: Option<(i64, i64)>,
}

#[derive(Clone, Debug)]
pub enum NoteEdit {
    SetPitchMidi(i32),
    SetHitKey(String),
    ShiftAtQuarter16ths(i64),
    ShiftDurQuarter16ths(i64),
    SetVelocity(f32),
    Duplicate,
    Delete,
}

#[derive(Clone, Debug)]
pub enum BatchNoteEdit {
    ShiftPitchSemitones(i32),
    ShiftAtQuarter16ths(i64),
    ShiftDurQuarter16ths(i64),
    SetVelocity(f32),
    Delete,
}

/// Convert a MIDI note number (`0..=127`, where 60 = `C4`, 69 = `A4`) into a
/// MaaC pitch symbol such as `"C4"` or `"F#5"`.
pub fn midi_to_pitch_symbol(midi: i32) -> String {
    let clamped = midi.clamp(12, 119);
    let pc = clamped.rem_euclid(12) as usize;
    let octave = (clamped / 12) - 1;
    format!("{}{octave}", PITCH_NAMES[pc])
}

/// Parse a MaaC pitch symbol (`C4`, `F#3`, `Bb2`, etc.) into a MIDI note number.
pub fn pitch_symbol_to_midi(symbol: &str) -> Option<i32> {
    let bytes = symbol.as_bytes();
    if bytes.len() < 2 {
        return None;
    }
    let base = match bytes[0] {
        b'C' => 0,
        b'D' => 2,
        b'E' => 4,
        b'F' => 5,
        b'G' => 7,
        b'A' => 9,
        b'B' => 11,
        _ => return None,
    };
    let mut idx = 1usize;
    let mut accidental = 0i32;
    while idx < bytes.len() {
        match bytes[idx] {
            b'#' => {
                accidental += 1;
                idx += 1;
            }
            b'b' => {
                accidental -= 1;
                idx += 1;
            }
            _ => break,
        }
    }
    let octave: i32 = symbol[idx..].parse().ok()?;
    Some((octave + 1) * 12 + base + accidental)
}

/// Inspect an authored note/chord at `source_path` inside the entry document of
/// `bundle`. Returns `None` if the object is not in the entry document (e.g.
/// inside an imported library) or is not a `note`/`chord`.
pub fn inspect_note_in_bundle(
    bundle: &SourceBundle,
    source_path: &[String],
) -> Option<NoteSourceInfo> {
    if source_path.is_empty() {
        return None;
    }
    let entry_src = bundle.sources.get(&bundle.entry)?;
    let doc = SourceDocument::parse(entry_src).ok()?;
    let tree = doc.authored().tree();
    let obj = lookup_object(tree, source_path)?;
    let kind = obj.get("kind")?.as_str()?.to_owned();
    if kind != "note" && kind != "chord" && kind != "hit" {
        return None;
    }
    let fields = obj.get("fields")?.as_object()?;
    let at_q = fields.get("at").and_then(read_quantity_q).unwrap_or((0, 1));
    let dur_q = fields
        .get("dur")
        .and_then(read_quantity_q)
        .unwrap_or((1, 2));
    let pitch_symbol = fields
        .get("pitch")
        .and_then(|v| (v.get("t")?.as_str()? == "symbol").then(|| v.get("v")?.as_str()))
        .flatten()
        .map(str::to_owned);
    let pitch_midi = pitch_symbol.as_deref().and_then(pitch_symbol_to_midi);
    let hit_key = fields
        .get("key")
        .and_then(|v| (v.get("t")?.as_str()? == "string").then(|| v.get("v")?.as_str()))
        .flatten()
        .map(str::to_owned);
    let velocity = fields.get("velocity").and_then(read_number_f32);

    let pattern_length_q = if source_path.len() >= 2 {
        lookup_object(tree, &source_path[..source_path.len() - 1])
            .and_then(|parent| parent.pointer("/fields/length"))
            .and_then(read_quantity_q)
    } else {
        None
    };

    Some(NoteSourceInfo {
        path: source_path.to_vec(),
        kind,
        pitch_symbol,
        pitch_midi,
        hit_key,
        at_q,
        dur_q,
        velocity,
        pattern_length_q,
    })
}

/// Apply one transactional `NoteEdit` to the entry source of `bundle`,
/// validating through `BundleEditContext::new(&bundle)?.checking_compile(limits)`.
/// Returns the updated `SourceBundle` and a short human-readable summary.
pub fn apply_note_edit(
    bundle: &SourceBundle,
    source_path: &[String],
    edit: NoteEdit,
    limits: &PlanLimits,
) -> Result<(SourceBundle, String), String> {
    let entry_src = bundle
        .sources
        .get(&bundle.entry)
        .ok_or_else(|| "missing entry source in bundle".to_string())?;
    let mut doc =
        SourceDocument::parse(entry_src).map_err(|err| format!("{}: {}", err.code, err.message))?;
    let info = inspect_note_in_bundle(bundle, source_path)
        .ok_or_else(|| "selected note lives outside the editable entry document".to_string())?;

    let (ops, summary) = build_operations(doc.authored().tree(), &info, edit)?;
    let tx = Transaction::new(doc.revision().to_owned(), ops)
        .map_err(|err| format!("{}: {}", err.code, err.message))?;
    let context = BundleEditContext::new(bundle)
        .map_err(|err| format!("{}: {}", err.code, err.message))?
        .checking_compile(*limits);
    doc.apply(&tx, &context)
        .map_err(|err| format!("{}: {}", err.code, err.message))?;

    let mut updated = bundle.clone();
    updated
        .sources
        .insert(updated.entry.clone(), doc.source().to_owned());
    Ok((updated, summary))
}

/// Apply one `BatchNoteEdit` across many unique note `source_paths` in a single
/// atomic Protocol 2 transaction (so one Undo reverts the entire batch).
pub fn apply_batch_note_edit(
    bundle: &SourceBundle,
    source_paths: &[Vec<String>],
    edit: BatchNoteEdit,
    limits: &PlanLimits,
) -> Result<(SourceBundle, String), String> {
    let entry_src = bundle
        .sources
        .get(&bundle.entry)
        .ok_or_else(|| "missing entry source in bundle".to_string())?;
    let mut doc =
        SourceDocument::parse(entry_src).map_err(|err| format!("{}: {}", err.code, err.message))?;

    let mut seen: BTreeSet<Vec<String>> = BTreeSet::new();
    let mut ops: Vec<Operation> = Vec::new();
    let mut touched = 0usize;

    for sp in source_paths {
        if sp.is_empty() || !seen.insert(sp.clone()) {
            continue;
        }
        let Some(info) = inspect_note_in_bundle(bundle, sp) else {
            continue;
        };
        let single_edit = match &edit {
            BatchNoteEdit::ShiftPitchSemitones(delta) => {
                let Some(midi) = info.pitch_midi else {
                    continue;
                };
                NoteEdit::SetPitchMidi(midi + *delta)
            }
            BatchNoteEdit::ShiftAtQuarter16ths(d) => NoteEdit::ShiftAtQuarter16ths(*d),
            BatchNoteEdit::ShiftDurQuarter16ths(d) => NoteEdit::ShiftDurQuarter16ths(*d),
            BatchNoteEdit::SetVelocity(v) => NoteEdit::SetVelocity(*v),
            BatchNoteEdit::Delete => NoteEdit::Delete,
        };
        let (mut note_ops, _) = build_operations(doc.authored().tree(), &info, single_edit)?;
        ops.append(&mut note_ops);
        touched += 1;
    }

    if ops.is_empty() {
        return Err("no editable notes in selection".into());
    }

    let tx = Transaction::new(doc.revision().to_owned(), ops)
        .map_err(|err| format!("{}: {}", err.code, err.message))?;
    let context = BundleEditContext::new(bundle)
        .map_err(|err| format!("{}: {}", err.code, err.message))?
        .checking_compile(*limits);
    doc.apply(&tx, &context)
        .map_err(|err| format!("{}: {}", err.code, err.message))?;

    let summary = match edit {
        BatchNoteEdit::ShiftPitchSemitones(d) => format!("Transposed {touched} notes ({d:+})"),
        BatchNoteEdit::ShiftAtQuarter16ths(d) => format!("Shifted {touched} notes ({d:+}/4q)"),
        BatchNoteEdit::ShiftDurQuarter16ths(d) => format!("Resized {touched} notes ({d:+}/4q)"),
        BatchNoteEdit::SetVelocity(v) => format!("Set velocity {:.2} on {touched} notes", v),
        BatchNoteEdit::Delete => format!("Deleted {touched} notes"),
    };

    let mut updated = bundle.clone();
    updated
        .sources
        .insert(updated.entry.clone(), doc.source().to_owned());
    Ok((updated, summary))
}

/// Split a shared placement into an independent pattern using Protocol 2's
/// `BundleEditContext::prepare_materialize_instance`, returning the updated
/// bundle and the remapped event address for `event_address`.
pub fn materialize_note_placement(
    bundle: &SourceBundle,
    event_address: &str,
    limits: &PlanLimits,
) -> Result<(SourceBundle, Option<String>, String), String> {
    let placement_id = event_address
        .split('/')
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "cannot determine placement ID from note address".to_string())?;

    let entry_src = bundle
        .sources
        .get(&bundle.entry)
        .ok_or_else(|| "missing entry source in bundle".to_string())?;
    let mut doc =
        SourceDocument::parse(entry_src).map_err(|err| format!("{}: {}", err.code, err.message))?;

    let objects = doc
        .authored()
        .tree()
        .get("objects")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "missing objects map".to_string())?;
    if objects
        .get(placement_id)
        .and_then(|o| o.get("kind"))
        .and_then(|k| k.as_str())
        != Some("place")
    {
        return Err(format!(
            "placement `{placement_id}` is not editable in the entry document"
        ));
    }

    let mut suffix = 1usize;
    let new_pat_id = loop {
        let candidate = if suffix == 1 {
            format!("{placement_id}_unique")
        } else {
            format!("{placement_id}_unique_{suffix}")
        };
        if !objects.contains_key(&candidate) {
            break candidate;
        }
        suffix += 1;
    };

    let context = BundleEditContext::new(bundle)
        .map_err(|err| format!("{}: {}", err.code, err.message))?
        .checking_compile(*limits);
    let plan = context
        .prepare_materialize_instance(doc.authored(), placement_id, &new_pat_id)
        .map_err(|err| format!("{}: {}", err.code, err.message))?;
    doc.apply(&plan.transaction, &context)
        .map_err(|err| format!("{}: {}", err.code, err.message))?;

    let remapped_addr = plan
        .mappings
        .iter()
        .find(|m| m.old_event_address == event_address)
        .map(|m| m.new_event_address.clone());

    let mut updated = bundle.clone();
    updated
        .sources
        .insert(updated.entry.clone(), doc.source().to_owned());
    let summary = format!("Made `{placement_id}` unique → `{new_pat_id}`");
    Ok((updated, remapped_addr, summary))
}

/// Insert a new `note` directly into the entry document's leaf `pattern`
/// covering `(track_name, score_q)`, compensating for any `transpose` on
/// enclosing `place`/`use` statements so the inserted note sounds at `midi_pitch`.
pub fn add_note_at_score_pos(
    bundle: &SourceBundle,
    track_name: &str,
    score_q: f64,
    midi_pitch: i32,
    limits: &PlanLimits,
) -> Result<(SourceBundle, Vec<String>, String), String> {
    let entry_src = bundle
        .sources
        .get(&bundle.entry)
        .ok_or_else(|| "missing entry source in bundle".to_string())?;
    let mut doc =
        SourceDocument::parse(entry_src).map_err(|err| format!("{}: {}", err.code, err.message))?;

    let tree = doc.authored().tree().clone();
    let objects = tree
        .get("objects")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "missing objects map".to_string())?;

    // Find all `track` object IDs targeting `node <track_name>`.
    let track_ids: BTreeSet<String> = objects
        .iter()
        .filter_map(|(id, obj)| {
            if obj.get("kind")?.as_str()? != "track" {
                return None;
            }
            let target_node = obj
                .pointer("/fields/target/path/0")
                .and_then(|v| v.as_str())?;
            (target_node == track_name).then(|| id.clone())
        })
        .collect();

    if track_ids.is_empty() {
        return Err(format!(
            "no editable track targeting `{track_name}` in entry document"
        ));
    }

    // Find a `place` on one of those tracks covering `score_q` (or fallback to nearest place).
    let mut chosen: Option<(String, f64, i32)> = None;
    let mut nearest: Option<(f64, String, f64, i32)> = None;

    for obj in objects.values() {
        if obj.get("kind").and_then(|k| k.as_str()) != Some("place") {
            continue;
        }
        let Some(trk_id) = obj.pointer("/fields/track/path/0").and_then(|v| v.as_str()) else {
            continue;
        };
        if !track_ids.contains(trk_id) {
            continue;
        }
        let Some(pat_id) = obj
            .pointer("/fields/pattern/path/0")
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        let Some(pat_len) = pattern_length_f64(objects, pat_id) else {
            continue;
        };
        let place_at = obj
            .pointer("/fields/at")
            .and_then(read_quantity_q)
            .map(frac_to_f64)
            .unwrap_or(0.0);
        let count = obj
            .pointer("/fields/count")
            .and_then(read_number_f32)
            .map(|c| c.round().max(1.0) as i64)
            .unwrap_or(1);
        let place_transpose = obj
            .pointer("/fields/transpose")
            .and_then(read_transpose_semitones)
            .unwrap_or(0);
        let total_len = pat_len * (count as f64);
        if score_q >= place_at - 0.01 && score_q < place_at + total_len - 0.01 {
            let local_q = (score_q - place_at).rem_euclid(pat_len);
            chosen = Some((pat_id.to_owned(), local_q, place_transpose));
            break;
        }
        let dist = if score_q < place_at {
            place_at - score_q
        } else {
            score_q - (place_at + total_len)
        };
        if nearest.as_ref().is_none_or(|n| dist < n.0) {
            let clamped_local = (score_q - place_at).clamp(0.0, (pat_len - 0.25).max(0.0));
            nearest = Some((dist, pat_id.to_owned(), clamped_local, place_transpose));
        }
    }

    let (mut pat_id, mut local_q, mut transpose_semis) = chosen
        .or_else(|| nearest.map(|(_, p, q, t)| (p, q, t)))
        .ok_or_else(|| format!("no editable placement found for track `{track_name}`"))?;

    // Descend through nested `use` children to find the concrete leaf `pattern`.
    for _ in 0..8 {
        let Some(pat_obj) = objects.get(&pat_id) else {
            break;
        };
        let Some(children) = pat_obj.get("children").and_then(|v| v.as_object()) else {
            break;
        };
        let matching_use = children.values().find_map(|child| {
            if child.get("kind").and_then(|k| k.as_str()) != Some("use") {
                return None;
            }
            let target_pat = child
                .pointer("/fields/pattern/path/0")
                .and_then(|v| v.as_str())?;
            let child_len = pattern_length_f64(objects, target_pat)?;
            let use_at = child
                .pointer("/fields/at")
                .and_then(read_quantity_q)
                .map(frac_to_f64)
                .unwrap_or(0.0);
            let count = child
                .pointer("/fields/count")
                .and_then(read_number_f32)
                .map(|c| c.round().max(1.0) as i64)
                .unwrap_or(1);
            let total = child_len * (count as f64);
            if local_q >= use_at - 0.01 && local_q < use_at + total - 0.01 {
                let inner_q = (local_q - use_at).rem_euclid(child_len);
                let use_tr = child
                    .pointer("/fields/transpose")
                    .and_then(read_transpose_semitones)
                    .unwrap_or(0);
                Some((target_pat.to_owned(), inner_q, use_tr))
            } else {
                None
            }
        });
        if let Some((next_pat, next_q, next_tr)) = matching_use {
            pat_id = next_pat;
            local_q = next_q;
            transpose_semis += next_tr;
        } else {
            break;
        }
    }

    let leaf_pat = objects
        .get(&pat_id)
        .filter(|o| o.get("kind").and_then(|k| k.as_str()) == Some("pattern"))
        .ok_or_else(|| format!("target pattern `{pat_id}` is not editable in entry document"))?;
    let pat_len_q = leaf_pat
        .pointer("/fields/length")
        .and_then(read_quantity_q)
        .unwrap_or((4, 1));
    let pat_len_f = frac_to_f64(pat_len_q);

    // Quantize local onset to nearest 1/4q (16th note), clamped inside pattern length.
    let q16 = (local_q * 4.0)
        .round()
        .clamp(0.0, ((pat_len_f * 4.0) - 1.0).max(0.0)) as i64;
    let dur_16 = if (pat_len_f * 4.0) as i64 - q16 >= 2 {
        2
    } else {
        1
    };

    let authored_midi = (midi_pitch - transpose_semis).clamp(12, 119);
    let pitch_sym = midi_to_pitch_symbol(authored_midi);

    let existing: BTreeSet<String> = leaf_pat
        .get("children")
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let mut suffix = 1usize;
    let new_id = loop {
        let candidate = format!("add_{suffix}");
        if !existing.contains(&candidate) {
            break candidate;
        }
        suffix += 1;
    };

    let note_obj = json!({
        "kind": "note",
        "fields": {
            "at": quantity_q_json(q16, 4),
            "dur": quantity_q_json(dur_16, 4),
            "pitch": { "t": "symbol", "v": pitch_sym },
            "velocity": { "t": "number", "n": "7", "d": "10" }
        },
        "children": {}
    });

    let op = Operation::InsertObject {
        parent: vec![pat_id.clone()],
        id: new_id.clone(),
        object_value: note_obj,
    };
    let tx = Transaction::new(doc.revision().to_owned(), vec![op])
        .map_err(|err| format!("{}: {}", err.code, err.message))?;
    let context = BundleEditContext::new(bundle)
        .map_err(|err| format!("{}: {}", err.code, err.message))?
        .checking_compile(*limits);
    doc.apply(&tx, &context)
        .map_err(|err| format!("{}: {}", err.code, err.message))?;

    let mut updated = bundle.clone();
    updated
        .sources
        .insert(updated.entry.clone(), doc.source().to_owned());
    let new_path = vec![pat_id.clone(), new_id.clone()];
    let summary = format!("Added {pitch_sym} in {pat_id} @ {}", format_q(q16, 4));
    Ok((updated, new_path, summary))
}

/// Insert a new drum `hit` into the entry document's leaf `pattern` covering
/// `(track_name, score_q)`.
pub fn add_hit_at_score_pos(
    bundle: &SourceBundle,
    track_name: &str,
    score_q: f64,
    key: &str,
    velocity: f32,
    limits: &PlanLimits,
) -> Result<(SourceBundle, Vec<String>, String), String> {
    let entry_src = bundle
        .sources
        .get(&bundle.entry)
        .ok_or_else(|| "missing entry source in bundle".to_string())?;
    let mut doc =
        SourceDocument::parse(entry_src).map_err(|err| format!("{}: {}", err.code, err.message))?;

    let tree = doc.authored().tree().clone();
    let objects = tree
        .get("objects")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "missing objects map".to_string())?;

    let track_ids: BTreeSet<String> = objects
        .iter()
        .filter_map(|(id, obj)| {
            if obj.get("kind")?.as_str()? != "track" {
                return None;
            }
            let target_node = obj
                .pointer("/fields/target/path/0")
                .and_then(|v| v.as_str())?;
            (target_node == track_name).then(|| id.clone())
        })
        .collect();
    if track_ids.is_empty() {
        return Err(format!(
            "no editable track targeting `{track_name}` in entry document"
        ));
    }

    let mut chosen: Option<(String, f64)> = None;
    let mut nearest: Option<(f64, String, f64)> = None;
    for obj in objects.values() {
        if obj.get("kind").and_then(|k| k.as_str()) != Some("place") {
            continue;
        }
        let Some(trk_id) = obj.pointer("/fields/track/path/0").and_then(|v| v.as_str()) else {
            continue;
        };
        if !track_ids.contains(trk_id) {
            continue;
        }
        let Some(pat_id) = obj
            .pointer("/fields/pattern/path/0")
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        let Some(pat_len) = pattern_length_f64(objects, pat_id) else {
            continue;
        };
        let place_at = obj
            .pointer("/fields/at")
            .and_then(read_quantity_q)
            .map(frac_to_f64)
            .unwrap_or(0.0);
        let count = obj
            .pointer("/fields/count")
            .and_then(read_number_f32)
            .map(|c| c.round().max(1.0) as i64)
            .unwrap_or(1);
        let total_len = pat_len * (count as f64);
        if score_q >= place_at - 0.01 && score_q < place_at + total_len - 0.01 {
            chosen = Some((pat_id.to_owned(), (score_q - place_at).rem_euclid(pat_len)));
            break;
        }
        let dist = if score_q < place_at {
            place_at - score_q
        } else {
            score_q - (place_at + total_len)
        };
        if nearest.as_ref().is_none_or(|n| dist < n.0) {
            let clamped = (score_q - place_at).clamp(0.0, (pat_len - 0.25).max(0.0));
            nearest = Some((dist, pat_id.to_owned(), clamped));
        }
    }

    let (mut pat_id, mut local_q) = chosen
        .or_else(|| nearest.map(|(_, p, q)| (p, q)))
        .ok_or_else(|| format!("no editable placement found for drum track `{track_name}`"))?;

    // Descend through nested `use` children to find a leaf pattern.
    for _ in 0..8 {
        let Some(pat_obj) = objects.get(&pat_id) else {
            break;
        };
        let Some(children) = pat_obj.get("children").and_then(|v| v.as_object()) else {
            break;
        };
        let matching_use = children.values().find_map(|child| {
            if child.get("kind").and_then(|k| k.as_str()) != Some("use") {
                return None;
            }
            let target_pat = child
                .pointer("/fields/pattern/path/0")
                .and_then(|v| v.as_str())?;
            let child_len = pattern_length_f64(objects, target_pat)?;
            let use_at = child
                .pointer("/fields/at")
                .and_then(read_quantity_q)
                .map(frac_to_f64)
                .unwrap_or(0.0);
            let count = child
                .pointer("/fields/count")
                .and_then(read_number_f32)
                .map(|c| c.round().max(1.0) as i64)
                .unwrap_or(1);
            let total = child_len * (count as f64);
            if local_q >= use_at - 0.01 && local_q < use_at + total - 0.01 {
                Some((
                    target_pat.to_owned(),
                    (local_q - use_at).rem_euclid(child_len),
                ))
            } else {
                None
            }
        });
        if let Some((next_pat, next_q)) = matching_use {
            pat_id = next_pat;
            local_q = next_q;
        } else {
            break;
        }
    }

    let leaf_pat = objects
        .get(&pat_id)
        .filter(|o| o.get("kind").and_then(|k| k.as_str()) == Some("pattern"))
        .ok_or_else(|| format!("target pattern `{pat_id}` is not editable in entry document"))?;
    let pat_len_q = leaf_pat
        .pointer("/fields/length")
        .and_then(read_quantity_q)
        .unwrap_or((4, 1));
    let pat_len_f = frac_to_f64(pat_len_q);
    let q16 = (local_q * 4.0)
        .round()
        .clamp(0.0, ((pat_len_f * 4.0) - 1.0).max(0.0)) as i64;

    let pct = (velocity.clamp(0.05, 1.0) * 100.0).round() as i64;
    let (vn, vd) = reduce_frac(pct, 100);

    let existing: BTreeSet<String> = leaf_pat
        .get("children")
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let mut suffix = 1usize;
    let new_id = loop {
        let candidate = format!("hit_{suffix}");
        if !existing.contains(&candidate) {
            break candidate;
        }
        suffix += 1;
    };

    let hit_obj = json!({
        "kind": "hit",
        "fields": {
            "at": quantity_q_json(q16, 4),
            "key": { "t": "string", "v": key },
            "velocity": { "t": "number", "n": vn.to_string(), "d": vd.to_string() }
        },
        "children": {}
    });

    let op = Operation::InsertObject {
        parent: vec![pat_id.clone()],
        id: new_id.clone(),
        object_value: hit_obj,
    };
    let tx = Transaction::new(doc.revision().to_owned(), vec![op])
        .map_err(|err| format!("{}: {}", err.code, err.message))?;
    let context = BundleEditContext::new(bundle)
        .map_err(|err| format!("{}: {}", err.code, err.message))?
        .checking_compile(*limits);
    doc.apply(&tx, &context)
        .map_err(|err| format!("{}: {}", err.code, err.message))?;

    let mut updated = bundle.clone();
    updated
        .sources
        .insert(updated.entry.clone(), doc.source().to_owned());
    let new_path = vec![pat_id.clone(), new_id];
    let summary = format!("Added hit `{key}` in {pat_id} @ {}", format_q(q16, 4));
    Ok((updated, new_path, summary))
}

fn pattern_length_f64(objects: &serde_json::Map<String, Value>, pat_id: &str) -> Option<f64> {
    let pat = objects.get(pat_id)?;
    if pat.get("kind")?.as_str()? != "pattern" {
        return None;
    }
    let len = pat.pointer("/fields/length").and_then(read_quantity_q)?;
    let f = frac_to_f64(len);
    (f > 0.0).then_some(f)
}

fn frac_to_f64(q: (i64, i64)) -> f64 {
    q.0 as f64 / q.1.max(1) as f64
}

fn read_transpose_semitones(value: &Value) -> Option<i32> {
    if value.get("t")?.as_str()? != "quantity" {
        return None;
    }
    let unit = value.get("u")?.as_str()?;
    let n: f64 = value.get("n")?.as_str()?.parse().ok()?;
    let d: f64 = value.get("d")?.as_str()?.parse().ok()?;
    if d <= 0.0 {
        return None;
    }
    let v = n / d;
    match unit {
        "ct" => Some((v / 100.0).round() as i32),
        "st" => Some(v.round() as i32),
        _ => None,
    }
}

fn build_operations(
    tree: &Value,
    info: &NoteSourceInfo,
    edit: NoteEdit,
) -> Result<(Vec<Operation>, String), String> {
    match edit {
        NoteEdit::SetPitchMidi(midi) => {
            if info.kind != "note" {
                return Err("pitch editing is supported for single `note` objects".into());
            }
            let symbol = midi_to_pitch_symbol(midi);
            let op = Operation::Set {
                object: info.path.clone(),
                field: vec!["pitch".into()],
                value: json!({ "t": "symbol", "v": symbol }),
                expect: None,
                expect_absent: false,
            };
            Ok((vec![op], format!("Pitch → {symbol}")))
        }
        NoteEdit::SetHitKey(key) => {
            if info.kind != "hit" {
                return Err("kit piece key editing is supported for `hit` objects".into());
            }
            let op = Operation::Set {
                object: info.path.clone(),
                field: vec!["key".into()],
                value: json!({ "t": "string", "v": key }),
                expect: None,
                expect_absent: false,
            };
            Ok((vec![op], format!("Hit key → {key}")))
        }
        NoteEdit::ShiftAtQuarter16ths(delta_16ths) => {
            let (mut num, mut den) = add_frac(info.at_q, (delta_16ths, 4));
            if num < 0 {
                num = 0;
                den = 1;
            }
            if let Some(len) = info.pattern_length_q {
                // Keep onset strictly before pattern length (at least 1/4q room if possible).
                let max_at = add_frac(len, (-1, 4));
                if max_at.0 >= 0 && cmp_frac((num, den), max_at) > 0 {
                    num = max_at.0;
                    den = max_at.1;
                }
            }
            let op = Operation::Set {
                object: info.path.clone(),
                field: vec!["at".into()],
                value: quantity_q_json(num, den),
                expect: None,
                expect_absent: false,
            };
            Ok((vec![op], format!("At → {}", format_q(num, den))))
        }
        NoteEdit::ShiftDurQuarter16ths(delta_16ths) => {
            let (mut num, mut den) = add_frac(info.dur_q, (delta_16ths, 4));
            if cmp_frac((num, den), (1, 4)) < 0 {
                num = 1;
                den = 4;
            }
            let op = Operation::Set {
                object: info.path.clone(),
                field: vec!["dur".into()],
                value: quantity_q_json(num, den),
                expect: None,
                expect_absent: false,
            };
            Ok((vec![op], format!("Dur → {}", format_q(num, den))))
        }
        NoteEdit::SetVelocity(vel) => {
            let pct = (vel.clamp(0.05, 1.0) * 100.0).round() as i64;
            let (n, d) = reduce_frac(pct, 100);
            let op = Operation::Set {
                object: info.path.clone(),
                field: vec!["velocity".into()],
                value: json!({ "t": "number", "n": n.to_string(), "d": d.to_string() }),
                expect: None,
                expect_absent: false,
            };
            Ok((vec![op], format!("Velocity → {:.2}", pct as f32 / 100.0)))
        }
        NoteEdit::Delete => {
            let op = Operation::DeleteObject {
                object: info.path.clone(),
                expect_object: None,
            };
            Ok((vec![op], format!("Deleted {}", info.path.join("/"))))
        }
        NoteEdit::Duplicate => {
            if info.path.len() < 2 {
                return Err("cannot duplicate top-level object".into());
            }
            let parent_path = info.path[..info.path.len() - 1].to_vec();
            let parent = lookup_object(tree, &parent_path)
                .ok_or_else(|| "parent pattern not found".to_string())?;
            let mut copy = lookup_object(tree, &info.path)
                .ok_or_else(|| "selected object not found".to_string())?
                .clone();

            // Place the duplicate right after the note (or +1/2q), clamped inside the pattern.
            let mut new_at = add_frac(info.at_q, info.dur_q);
            if let Some(len) = info.pattern_length_q {
                if cmp_frac(new_at, len) >= 0 {
                    new_at = add_frac(info.at_q, (1, 2));
                }
                if cmp_frac(new_at, len) >= 0 {
                    new_at = info.at_q;
                }
            }
            if let Some(fields) = copy.get_mut("fields").and_then(|v| v.as_object_mut()) {
                fields.insert("at".into(), quantity_q_json(new_at.0, new_at.1));
            }
            let existing: BTreeSet<String> = parent
                .get("children")
                .and_then(|v| v.as_object())
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            let base_id = info.path.last().map(String::as_str).unwrap_or("n");
            let mut suffix = 2usize;
            let new_id = loop {
                let candidate = format!("{base_id}_{suffix}");
                if !existing.contains(&candidate) {
                    break candidate;
                }
                suffix += 1;
            };
            let op = Operation::InsertObject {
                parent: parent_path,
                id: new_id.clone(),
                object_value: copy,
            };
            Ok((vec![op], format!("Duplicated as {new_id}")))
        }
    }
}

fn lookup_object<'a>(tree: &'a Value, path: &[String]) -> Option<&'a Value> {
    let mut current = tree.get("objects")?.get(&path[0])?;
    for segment in &path[1..] {
        current = current.get("children")?.get(segment)?;
    }
    Some(current)
}

fn read_quantity_q(value: &Value) -> Option<(i64, i64)> {
    if value.get("t")?.as_str()? != "quantity" || value.get("u")?.as_str()? != "q" {
        return None;
    }
    let n: i64 = value.get("n")?.as_str()?.parse().ok()?;
    let d: i64 = value.get("d")?.as_str()?.parse().ok()?;
    (d > 0).then(|| reduce_frac(n, d))
}

fn read_number_f32(value: &Value) -> Option<f32> {
    if value.get("t")?.as_str()? != "number" {
        return None;
    }
    let n: f64 = value.get("n")?.as_str()?.parse().ok()?;
    let d: f64 = value.get("d")?.as_str()?.parse().ok()?;
    (d > 0.0).then_some((n / d) as f32)
}

fn quantity_q_json(num: i64, den: i64) -> Value {
    let (n, d) = reduce_frac(num, den);
    json!({
        "t": "quantity",
        "n": n.to_string(),
        "d": d.to_string(),
        "u": "q",
    })
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

fn add_frac(a: (i64, i64), b: (i64, i64)) -> (i64, i64) {
    let num =
        a.0.saturating_mul(b.1)
            .saturating_add(b.0.saturating_mul(a.1));
    let den = a.1.saturating_mul(b.1).max(1);
    reduce_frac(num, den)
}

fn cmp_frac(a: (i64, i64), b: (i64, i64)) -> i8 {
    let left = (a.0 as i128) * (b.1 as i128);
    let right = (b.0 as i128) * (a.1 as i128);
    match left.cmp(&right) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

pub fn format_q(num: i64, den: i64) -> String {
    let (n, d) = reduce_frac(num, den);
    if d == 1 {
        format!("{n}q")
    } else {
        format!("{n}/{d}q")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{load_and_compile, load_from_bundle};
    use std::path::Path;

    #[test]
    fn pitch_symbol_round_trips() {
        for midi in [36, 48, 60, 61, 69, 72, 83] {
            let sym = midi_to_pitch_symbol(midi);
            assert_eq!(pitch_symbol_to_midi(&sym), Some(midi), "sym={sym}");
        }
    }

    #[test]
    fn edits_note_in_ferry_preserving_comments_and_recompiling() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/ferry/ferry.maac");
        let data = load_and_compile(&path).expect("compile ferry");
        let first_note = data
            .notes
            .iter()
            .find(|n| {
                inspect_note_in_bundle(&data.bundle, &n.source_path)
                    .is_some_and(|i| i.kind == "note" && i.pitch_midi.is_some())
            })
            .expect("find editable note");
        let info = inspect_note_in_bundle(&data.bundle, &first_note.source_path).unwrap();
        let orig_midi = info.pitch_midi.unwrap();

        // 1. Transpose +2 semitones
        let (edited_bundle, _) = apply_note_edit(
            &data.bundle,
            &first_note.source_path,
            NoteEdit::SetPitchMidi(orig_midi + 2),
            &data.limits,
        )
        .expect("transpose note");
        let edited_src = &edited_bundle.sources[&edited_bundle.entry];
        assert!(
            edited_src.contains("Morning Ferry"),
            "top comment must be preserved"
        );
        let recompiled = load_from_bundle(
            data.entry_path.clone(),
            data.project_root.clone(),
            edited_bundle.clone(),
        )
        .expect("recompile edited ferry");
        let updated_info =
            inspect_note_in_bundle(&recompiled.bundle, &first_note.source_path).unwrap();
        assert_eq!(updated_info.pitch_midi, Some(orig_midi + 2));

        // 2. Adjust velocity
        let (vel_bundle, _) = apply_note_edit(
            &recompiled.bundle,
            &first_note.source_path,
            NoteEdit::SetVelocity(0.95),
            &data.limits,
        )
        .expect("set velocity");
        let vel_info = inspect_note_in_bundle(&vel_bundle, &first_note.source_path).unwrap();
        assert!((vel_info.velocity.unwrap() - 0.95).abs() < 0.01);

        // 3. Duplicate and delete
        let (dup_bundle, _) = apply_note_edit(
            &vel_bundle,
            &first_note.source_path,
            NoteEdit::Duplicate,
            &data.limits,
        )
        .expect("duplicate note");
        let dup_data = load_from_bundle(
            data.entry_path.clone(),
            data.project_root.clone(),
            dup_bundle.clone(),
        )
        .expect("recompile duplicated ferry");
        assert!(dup_data.notes.len() > data.notes.len());

        let (del_bundle, _) = apply_note_edit(
            &dup_bundle,
            &first_note.source_path,
            NoteEdit::Delete,
            &data.limits,
        )
        .expect("delete note");
        assert!(inspect_note_in_bundle(&del_bundle, &first_note.source_path).is_none());
    }

    #[test]
    fn materializes_shared_chorus_placement_keeping_note_identity() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/ferry/ferry.maac");
        let data = load_and_compile(&path).expect("compile ferry");
        let shared_note = data
            .notes
            .iter()
            .find(|n| {
                n.shared_occurrences > 1
                    && n.source_path.first().map(String::as_str) == Some("melody_chorus")
            })
            .expect("find shared melody_chorus note");
        assert_eq!(shared_note.shared_occurrences, 2);

        let (mat_bundle, remapped, _) =
            materialize_note_placement(&data.bundle, &shared_note.address, &data.limits)
                .expect("materialize melody_chorus placement");
        let new_addr = remapped.expect("remapped event address");
        let recompiled = load_from_bundle(data.entry_path, data.project_root, mat_bundle)
            .expect("recompile materialized ferry");
        let split_note = recompiled
            .notes
            .iter()
            .find(|n| n.address == new_addr)
            .expect("find remapped note in recompiled plan");
        assert_eq!(
            split_note.shared_occurrences, 1,
            "split placement note should now be unique"
        );
    }

    #[test]
    fn batch_transposes_multiple_notes_atomically() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/ferry/ferry.maac");
        let data = load_and_compile(&path).expect("compile ferry");
        let paths: Vec<Vec<String>> = data
            .notes
            .iter()
            .filter(|n| n.source_path.first().map(String::as_str) == Some("melody_chorus"))
            .take(3)
            .map(|n| n.source_path.clone())
            .collect();
        let orig_midis: Vec<i32> = paths
            .iter()
            .map(|p| {
                inspect_note_in_bundle(&data.bundle, p)
                    .unwrap()
                    .pitch_midi
                    .unwrap()
            })
            .collect();

        let (edited, _) = apply_batch_note_edit(
            &data.bundle,
            &paths,
            BatchNoteEdit::ShiftPitchSemitones(2),
            &data.limits,
        )
        .expect("batch transpose 3 notes");
        for (p, orig) in paths.iter().zip(orig_midis.iter()) {
            let after = inspect_note_in_bundle(&edited, p)
                .unwrap()
                .pitch_midi
                .unwrap();
            assert_eq!(after, orig + 2);
        }
    }

    #[test]
    fn adds_note_at_score_position_in_ferry() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/ferry/ferry.maac");
        let data = load_and_compile(&path).expect("compile ferry");
        let (edited, new_path, _) =
            add_note_at_score_pos(&data.bundle, "lead", 50.0, 76, &data.limits)
                .expect("add note to lead in chorus1");
        let info = inspect_note_in_bundle(&edited, &new_path).expect("inspect newly added note");
        assert_eq!(info.pitch_midi, Some(76));
        let recompiled = load_from_bundle(data.entry_path, data.project_root, edited)
            .expect("recompile ferry with added note");
        assert!(recompiled.notes.len() > data.notes.len());
    }

    #[test]
    fn edits_and_adds_drum_hits_in_ferry() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/ferry/ferry.maac");
        let data = load_and_compile(&path).expect("compile ferry");
        let hit = data.hits.first().expect("at least one drum hit in ferry");
        let info = inspect_note_in_bundle(&data.bundle, &hit.source_path)
            .expect("inspect drum hit in ferry");
        assert_eq!(info.kind, "hit");
        assert!(info.hit_key.is_some());

        let (b1, _) = apply_note_edit(
            &data.bundle,
            &hit.source_path,
            NoteEdit::SetVelocity(0.92),
            &data.limits,
        )
        .expect("set drum hit velocity");
        let info1 = inspect_note_in_bundle(&b1, &hit.source_path).unwrap();
        assert!((info1.velocity.unwrap() - 0.92).abs() < 0.02);

        let (b2, new_path, _) =
            add_hit_at_score_pos(&b1, "drums", 18.0, "kick", 0.85, &data.limits)
                .expect("add drum hit in ferry");
        let added = inspect_note_in_bundle(&b2, &new_path).expect("inspect added hit");
        assert_eq!(added.kind, "hit");
        assert_eq!(added.hit_key.as_deref(), Some("kick"));
        let recompiled = load_from_bundle(data.entry_path, data.project_root, b2)
            .expect("recompile ferry with added hit");
        assert!(recompiled.hits.len() > data.hits.len());
    }
}
