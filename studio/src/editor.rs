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
    pub at_q: (i64, i64),
    pub dur_q: (i64, i64),
    pub velocity: Option<f32>,
    pub pattern_length_q: Option<(i64, i64)>,
}

#[derive(Clone, Debug)]
pub enum NoteEdit {
    SetPitchMidi(i32),
    ShiftAtQuarter16ths(i64),
    ShiftDurQuarter16ths(i64),
    SetVelocity(f32),
    Duplicate,
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
    if kind != "note" && kind != "chord" {
        return None;
    }
    let fields = obj.get("fields")?.as_object()?;
    let at_q = fields.get("at").and_then(read_quantity_q).unwrap_or((0, 1));
    let dur_q = fields
        .get("dur")
        .and_then(read_quantity_q)
        .unwrap_or((1, 1));
    let pitch_symbol = fields
        .get("pitch")
        .and_then(|v| (v.get("t")?.as_str()? == "symbol").then(|| v.get("v")?.as_str()))
        .flatten()
        .map(str::to_owned);
    let pitch_midi = pitch_symbol.as_deref().and_then(pitch_symbol_to_midi);
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
}
