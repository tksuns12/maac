//! One-click offline Freeze: render any AU/CLAP plugin-hosted node or master
//! chain to a pinned `pcm_f32le_interleaved/1` asset and wire it atomically
//! into the `.maac` source via `maac::editing`, so pure MaaC CLI, CI, and AI
//! agents render the exact audio bit-for-bit without needing the plugin.

use std::path::Path;

use maac::bundle::{sha256_digest, SourceBundle};
use maac::dsp::{DspEngine, HostBlockContext, HostProcessor, RenderSpan};
use maac::editing::{Operation, SourceDocument, Transaction};
use maac::plan::{PlanLimits, PortRef};
use maac::PlanArtifact;
use serde_json::{json, Value};

use super::rack_file::RackFile;
use super::{HostRack, PluginDescriptor, SlotTarget};
use crate::project::{build_edit_context, EditedBundle};

/// Safe raw-PCM ceiling so JSON-serialized `audio_assets[].bytes` (~3.6x bytes)
/// plus the rest of a full song's `PlanArtifact` JSON stays safely under
/// `PlanLimits::MAX_JSON_BYTES` (4 MiB). (~2.5s stereo / ~5s mono @ 48 kHz f32.)
const MAX_INLINE_ASSET_BYTES: usize = 950_000;
const FREEZE_CHUNK_FRAMES: u64 = 512;

#[derive(Clone)]
pub struct FreezeOutcome {
    pub edited: EditedBundle,
    pub pcm_rel_path: String,
    pub frozen_node_id: String,
    pub frames: usize,
    pub bytes: usize,
    pub summary: String,
}

impl std::fmt::Debug for FreezeOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FreezeOutcome")
            .field("pcm_rel_path", &self.pcm_rel_path)
            .field("frozen_node_id", &self.frozen_node_id)
            .field("frames", &self.frames)
            .field("bytes", &self.bytes)
            .field("summary", &self.summary)
            .finish()
    }
}

fn sanitize_ident(raw: &str) -> String {
    let mut out: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() || out.as_bytes()[0].is_ascii_digit() {
        out.insert(0, 'n');
    }
    out
}

fn update_production_master_deliveries(
    objects: &serde_json::Map<String, Value>,
    audio_id: &str,
    ops: &mut Vec<Operation>,
) {
    let new_ref = json!({ "t": "ref", "path": [audio_id], "port": "out" });
    for (obj_id, obj_val) in objects {
        if obj_val.get("kind").and_then(|k| k.as_str()) != Some("extension") {
            continue;
        }
        let ns = obj_val
            .pointer("/fields/namespace/v")
            .and_then(|v| v.as_str());
        if ns != Some("maac.production/1") {
            continue;
        }
        let Some(deliveries) = obj_val
            .pointer("/fields/data/fields/deliveries/fields")
            .and_then(|v| v.as_object())
        else {
            continue;
        };
        for (deliv_name, deliv_val) in deliveries {
            let Some(targets) = deliv_val
                .pointer("/fields/targets/fields")
                .and_then(|v| v.as_object())
            else {
                continue;
            };
            for (tgt_name, tgt_val) in targets {
                let role = tgt_val.pointer("/fields/role/v").and_then(|v| v.as_str());
                if role == Some("master") {
                    ops.push(Operation::Set {
                        object: vec![obj_id.clone()],
                        field: vec![
                            "data".into(),
                            "deliveries".into(),
                            deliv_name.clone(),
                            "targets".into(),
                            tgt_name.clone(),
                            "output".into(),
                        ],
                        value: new_ref.clone(),
                        expect: None,
                        expect_absent: false,
                    });
                }
            }
        }
    }
}

fn unique_id(objects: &serde_json::Map<String, Value>, base: &str) -> String {
    if !objects.contains_key(base) {
        return base.to_owned();
    }
    for n in 2..1000 {
        let cand = format!("{base}_{n}");
        if !objects.contains_key(&cand) {
            return cand;
        }
    }
    format!("{base}_frozen")
}

/// Offline-render a plugin-hosted target (`TrackInstrument`, `NodeInsert`, or
/// `MasterInsert`) into `frozen/<name>.pcm` under `project_root` and atomically
/// wire it into `bundle`'s entry `.maac` document via Protocol 2 editing.
#[allow(clippy::too_many_arguments)]
pub fn freeze_target_to_maac(
    bundle: &SourceBundle,
    plan: &PlanArtifact,
    limits: &PlanLimits,
    project_root: &Path,
    rack_file: &RackFile,
    catalog: &[PluginDescriptor],
    target: &SlotTarget,
    span_opt: Option<RenderSpan>,
) -> Result<FreezeOutcome, String> {
    let output = plan.output();
    let rate = output.sample_rate_hz;
    let total = output.total_frames;
    let span = span_opt
        .map(|s| RenderSpan {
            start: s.start.min(total.saturating_sub(1)),
            end: s.end.min(total),
        })
        .unwrap_or(RenderSpan {
            start: 0,
            end: total,
        });
    if span.start >= span.end {
        return Err("freeze span is empty".into());
    }

    // Build an isolated offline HostRack containing only slots relevant to `target`.
    let filtered = RackFile {
        version: rack_file.version,
        slots: rack_file
            .slots
            .iter()
            .filter(|s| {
                !s.bypassed
                    && match target {
                        SlotTarget::TrackInstrument(node) | SlotTarget::NodeInsert(node) => {
                            matches!(&s.target, SlotTarget::TrackInstrument(n) | SlotTarget::NodeInsert(n) if n == node)
                        }
                        SlotTarget::MasterInsert => true,
                    }
            })
            .cloned()
            .collect(),
    };
    if filtered.slots.is_empty() {
        return Err("no active (non-bypassed) plugins on target to freeze".into());
    }

    let mut offline_rack = HostRack::new(f64::from(rate), FREEZE_CHUNK_FRAMES as usize);
    let warnings = offline_rack.load_rack_file(&filtered, catalog);
    if !warnings.is_empty() && offline_rack.is_empty() {
        return Err(warnings.join("; "));
    }
    offline_rack.reset(f64::from(rate), FREEZE_CHUNK_FRAMES as usize);

    let frames = (span.end - span.start) as usize;
    let (channels, rendered_f64): (usize, Vec<f64>) = match target {
        SlotTarget::MasterInsert => {
            let ch = usize::from(output.channels);
            let mut engine = DspEngine::new_artifact_with_limits(plan, limits)
                .map_err(|err| format!("freeze engine init failed: {err}"))?;
            let mut out = Vec::with_capacity(frames * ch);
            let mut pos = span.start;
            while pos < span.end {
                let end = (pos + FREEZE_CHUNK_FRAMES).min(span.end);
                let chunk = RenderSpan { start: pos, end };
                let cb = |frame: &[f64]| {
                    out.extend_from_slice(frame);
                    Ok(())
                };
                if span.start > 0 && pos == span.start {
                    engine
                        .render_preview_chunk_with_host(chunk, &mut offline_rack, cb)
                        .map_err(|err| format!("freeze render failed: {err}"))?;
                } else if span.start > 0 {
                    engine
                        .render_preview_chunk_with_host(chunk, &mut offline_rack, cb)
                        .map_err(|err| format!("freeze render failed: {err}"))?;
                } else {
                    engine
                        .render_chunk_with_host(chunk, &mut offline_rack, cb)
                        .map_err(|err| format!("freeze render failed: {err}"))?;
                }
                pos = end;
            }
            (ch, out)
        }
        SlotTarget::TrackInstrument(node_id) | SlotTarget::NodeInsert(node_id) => {
            let port = PortRef {
                node: node_id.clone(),
                port: "out".into(),
            };
            // Determine channel width & render base node output if not overridden by an instrument.
            let mut engine = DspEngine::new_artifact_with_limits(plan, limits)
                .map_err(|err| format!("freeze engine init failed: {err}"))?;
            let has_inst = offline_rack.has_instrument_override(node_id);
            let mut ch = usize::from(output.channels);
            let mut buf: Vec<f64> = Vec::new();
            if has_inst {
                // Gather sample-accurate events for `node_id` across `span` and render the hosted instrument.
                let all_events = plan
                    .query_events_score_window_with_limits(
                        &output.score_start_q,
                        &output.score_end_q,
                        limits,
                    )
                    .map_err(|err| format!("failed to query events: {}", err.message))?;
                // Probe node channel width via a 1-frame port capture.
                engine
                    .render_ports_span(
                        std::slice::from_ref(&port),
                        RenderSpan {
                            start: span.start,
                            end: (span.start + 1).min(span.end),
                        },
                        |outs| {
                            ch = outs[0].len().max(1);
                            Ok(())
                        },
                    )
                    .map_err(|err| format!("freeze port probe failed: {err}"))?;

                use maac::dsp::HostNoteEvent;
                use maac::plan::EventKind;
                use num_traits::ToPrimitive;

                let mut raw_evs: Vec<(u64, bool, u8, f32, f64)> = Vec::new();
                for ev in &all_events {
                    if ev.target.node != *node_id {
                        continue;
                    }
                    if let EventKind::Note {
                        pitch_hz, velocity, ..
                    } = &ev.kind
                    {
                        let midi = if *pitch_hz > 0.0 {
                            (69.0 + 12.0 * (pitch_hz / 440.0).log2())
                                .round()
                                .clamp(0.0, 127.0) as u8
                        } else {
                            60
                        };
                        let vel = velocity.to_f32().unwrap_or(0.7).clamp(0.0, 1.0);
                        if ev.on_frame >= span.start && ev.on_frame < span.end {
                            raw_evs.push((ev.on_frame, true, midi, vel, *pitch_hz));
                        }
                        if let Some(off) = ev.off_frame {
                            if off >= span.start && off < span.end {
                                raw_evs.push((off, false, midi, 0.0, *pitch_hz));
                            }
                        }
                    }
                }
                buf.resize(frames * ch, 0.0);
                let mut pos = span.start;
                while pos < span.end {
                    let end = (pos + FREEZE_CHUNK_FRAMES).min(span.end);
                    let fcount = (end - pos) as usize;
                    let mut block_evs: Vec<HostNoteEvent> = raw_evs
                        .iter()
                        .filter(|(f, ..)| *f >= pos && *f < end)
                        .map(|&(f, is_on, midi, vel, hz)| HostNoteEvent {
                            frame_offset: (f - pos) as usize,
                            is_note_on: is_on,
                            midi_note: midi,
                            velocity: vel,
                            pitch_hz: hz,
                        })
                        .collect();
                    block_evs.sort_by_key(|e| (e.frame_offset, e.is_note_on));
                    let ctx = HostBlockContext {
                        start_frame: pos,
                        frames: fcount,
                        sample_rate_hz: f64::from(rate),
                        tempo_bpm: 120.0,
                    };
                    let start_idx = (pos - span.start) as usize * ch;
                    let slice = &mut buf[start_idx..start_idx + fcount * ch];
                    offline_rack.process_instrument(node_id, ctx, &block_evs, ch, slice);
                    if offline_rack.has_node_insert(node_id) {
                        offline_rack.process_node_insert(node_id, ctx, ch, slice);
                    }
                    pos = end;
                }
            } else {
                // Capture built-in MaaC node output, then run the node's insert effects offline.
                engine
                    .render_ports_span(std::slice::from_ref(&port), span, |outs| {
                        ch = outs[0].len().max(1);
                        buf.extend_from_slice(&outs[0]);
                        Ok(())
                    })
                    .map_err(|err| format!("freeze node capture failed: {err}"))?;
                let mut pos = span.start;
                while pos < span.end {
                    let end = (pos + FREEZE_CHUNK_FRAMES).min(span.end);
                    let fcount = (end - pos) as usize;
                    let ctx = HostBlockContext {
                        start_frame: pos,
                        frames: fcount,
                        sample_rate_hz: f64::from(rate),
                        tempo_bpm: 120.0,
                    };
                    let start_idx = (pos - span.start) as usize * ch;
                    let slice = &mut buf[start_idx..start_idx + fcount * ch];
                    offline_rack.process_node_insert(node_id, ctx, ch, slice);
                    pos = end;
                }
            }
            (ch, buf)
        }
    };

    // Encode as exact `pcm_f32le_interleaved/1`.
    let mut pcm_bytes = Vec::with_capacity(rendered_f64.len() * 4);
    for &sample in &rendered_f64 {
        let finite = if sample.is_finite() {
            sample as f32
        } else {
            0.0
        };
        pcm_bytes.extend_from_slice(&finite.to_le_bytes());
    }
    if pcm_bytes.len() > MAX_INLINE_ASSET_BYTES {
        let mib = pcm_bytes.len() as f64 / (1024.0 * 1024.0);
        let max_s = MAX_INLINE_ASSET_BYTES as f64 / (f64::from(rate) * channels as f64 * 4.0);
        return Err(format!(
            "Frozen stem is {mib:.2} MiB (exceeds MaaC's ~0.9 MiB inline PCM budget inside the 4 MiB Plan JSON limit ≈ {max_s:.1}s at {rate} Hz {channels}ch). Set a shorter Loop range to freeze inline, or use Export WAV."
        ));
    }

    let digest = sha256_digest(&pcm_bytes);
    let short_hash = digest
        .strip_prefix("sha256:")
        .unwrap_or(&digest)
        .chars()
        .take(8)
        .collect::<String>();
    let target_label = match target {
        SlotTarget::TrackInstrument(n) | SlotTarget::NodeInsert(n) => sanitize_ident(n),
        SlotTarget::MasterInsert => "master".into(),
    };
    let rel_path = format!("frozen/{target_label}_{short_hash}.pcm");
    let full_path = project_root.join(&rel_path);
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }
    std::fs::write(&full_path, &pcm_bytes)
        .map_err(|err| format!("failed to write {}: {err}", full_path.display()))?;

    // Atomically edit the `.maac` entry document via Protocol 2 (`SourceDocument` + `BundleEditContext`).
    let mut staged_bundle = bundle.clone();
    staged_bundle
        .assets
        .insert(rel_path.clone(), pcm_bytes.clone());

    let entry_src = staged_bundle
        .sources
        .get(&staged_bundle.entry)
        .ok_or_else(|| "missing entry source in bundle".to_string())?;
    let mut doc =
        SourceDocument::parse(entry_src).map_err(|err| format!("{}: {}", err.code, err.message))?;
    let objects = doc
        .authored()
        .tree()
        .get("objects")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "entry document has no objects".to_string())?
        .clone();

    let asset_id = unique_id(&objects, &format!("frozen_{target_label}_pcm"));
    let audio_id = unique_id(&objects, &format!("frozen_{target_label}"));

    let asset_obj = json!({
        "kind": "asset",
        "fields": {
            "kind": { "t": "symbol", "v": "audio" },
            "path": { "t": "string", "v": rel_path },
            "hash": { "t": "string", "v": digest },
            "format": { "t": "string", "v": "pcm_f32le_interleaved/1" },
            "rate": { "t": "quantity", "n": rate.to_string(), "d": "1", "u": "Hz" },
            "channels": { "t": "number", "n": channels.to_string(), "d": "1" },
            "frames": { "t": "number", "n": frames.to_string(), "d": "1" }
        },
        "children": {}
    });

    let at_val = if span.start == 0 {
        json!({ "t": "quantity", "n": "0", "d": "1", "u": "s" })
    } else {
        json!({
            "t": "quantity",
            "n": span.start.to_string(),
            "d": rate.to_string(),
            "u": "s"
        })
    };
    let audio_obj = json!({
        "kind": "audio",
        "fields": {
            "asset": { "t": "ref", "path": [asset_id], "port": null },
            "at": at_val,
            "source": {
                "t": "list",
                "items": [
                    { "t": "quantity", "n": "0", "d": "1", "u": "frame" },
                    { "t": "quantity", "n": frames.to_string(), "d": "1", "u": "frame" }
                ]
            },
            "mode": { "t": "symbol", "v": "rate" }
        },
        "children": {}
    });

    let mut ops = vec![
        Operation::InsertObject {
            parent: Vec::new(),
            id: asset_id.clone(),
            object_value: asset_obj,
        },
        Operation::InsertObject {
            parent: Vec::new(),
            id: audio_id.clone(),
            object_value: audio_obj,
        },
    ];

    match target {
        SlotTarget::TrackInstrument(node_id) | SlotTarget::NodeInsert(node_id) => {
            // Rewire every connection sourcing from `&<node_id>:out` to `&<audio_id>:out`.
            for (obj_id, obj_val) in &objects {
                if obj_val.get("kind").and_then(|k| k.as_str()) == Some("connect") {
                    let from_node = obj_val
                        .pointer("/fields/from/path/0")
                        .and_then(|v| v.as_str());
                    let from_port = obj_val
                        .pointer("/fields/from/port")
                        .and_then(|v| v.as_str());
                    if from_node == Some(node_id.as_str()) && from_port == Some("out") {
                        ops.push(Operation::Set {
                            object: vec![obj_id.clone()],
                            field: vec!["from".into()],
                            value: json!({ "t": "ref", "path": [audio_id], "port": "out" }),
                            expect: None,
                            expect_absent: false,
                        });
                    }
                } else if obj_val.get("kind").and_then(|k| k.as_str()) == Some("project") {
                    let out_node = obj_val
                        .pointer("/fields/output/path/0")
                        .and_then(|v| v.as_str());
                    if out_node == Some(node_id.as_str()) {
                        ops.push(Operation::Set {
                            object: vec![obj_id.clone()],
                            field: vec!["output".into()],
                            value: json!({ "t": "ref", "path": [audio_id], "port": "out" }),
                            expect: None,
                            expect_absent: false,
                        });
                        update_production_master_deliveries(&objects, &audio_id, &mut ops);
                    }
                }
            }
        }
        SlotTarget::MasterInsert => {
            // Point `project.output` (and any `maac.production/1` master delivery target) to `&<audio_id>:out`.
            for (obj_id, obj_val) in &objects {
                if obj_val.get("kind").and_then(|k| k.as_str()) == Some("project") {
                    ops.push(Operation::Set {
                        object: vec![obj_id.clone()],
                        field: vec!["output".into()],
                        value: json!({ "t": "ref", "path": [audio_id], "port": "out" }),
                        expect: None,
                        expect_absent: false,
                    });
                }
            }
            update_production_master_deliveries(&objects, &audio_id, &mut ops);
        }
    }

    let tx = Transaction::new(doc.revision().to_owned(), ops)
        .map_err(|err| format!("{}: {}", err.code, err.message))?;
    let context = build_edit_context(&staged_bundle, &doc, limits, None)?;
    doc.apply(&tx, &context)
        .map_err(|err| format!("{}: {}", err.code, err.message))?;
    let compiled_plan = context.take_compiled_artifact(doc.authored());

    staged_bundle
        .sources
        .insert(staged_bundle.entry.clone(), doc.source().to_owned());

    let secs = frames as f64 / f64::from(rate);
    let kib = pcm_bytes.len() / 1024;
    let summary = format!("Froze {target_label} ({secs:.1}s, {kib} KiB) → {rel_path}");
    Ok(FreezeOutcome {
        edited: EditedBundle {
            bundle: staged_bundle,
            compiled_plan,
        },
        pcm_rel_path: rel_path,
        frozen_node_id: audio_id,
        frames,
        bytes: pcm_bytes.len(),
        summary,
    })
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::plugins::scan_installed_plugins;
    use crate::project::load_and_compile;

    #[test]
    fn freezes_au_delay_on_hello_into_valid_pure_maac_bundle() {
        let entry = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/hello.maac");
        let data = load_and_compile(&entry).expect("compile hello");
        let catalog = scan_installed_plugins();
        let delay = catalog
            .iter()
            .find(|p| p.name.contains("AUDelay"))
            .expect("Apple AUDelay");

        let mut rack = HostRack::new(f64::from(data.sample_rate_hz), 512);
        rack.add_slot(SlotTarget::NodeInsert("piano".into()), delay)
            .expect("add AUDelay to piano");
        let rack_file = rack.to_rack_file();

        let tmp_dir =
            std::env::temp_dir().join(format!("maac-studio-freeze-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp_dir);
        std::fs::create_dir_all(&tmp_dir).unwrap();

        // Freeze 2 seconds (96,000 frames stereo f32 = 768 KiB < 4 MiB).
        let outcome = freeze_target_to_maac(
            &data.bundle,
            &data.plan,
            &data.limits,
            &tmp_dir,
            &rack_file,
            &catalog,
            &SlotTarget::NodeInsert("piano".into()),
            Some(RenderSpan {
                start: 0,
                end: 96_000,
            }),
        )
        .expect("freeze piano + AUDelay to pure .maac");

        assert!(outcome.edited.compiled_plan.is_some());
        assert!(tmp_dir.join(&outcome.pcm_rel_path).is_file());
        assert!(outcome.frames == 96_000);
        // Verify pure MaaC compiler compiles the frozen bundle independently without any plugin host!
        let pure_plan = maac::compiler::compile_bundle_artifact_with_limits(
            &outcome.edited.bundle,
            &data.limits,
        )
        .expect("pure MaaC must compile frozen bundle without plugins");
        let mut engine = DspEngine::new_artifact_with_limits(&pure_plan, &data.limits).unwrap();
        let mut peak = 0.0_f64;
        engine
            .render_span(
                RenderSpan {
                    start: 0,
                    end: 4_096,
                },
                |frame| {
                    for &s in frame {
                        peak = peak.max(s.abs());
                    }
                    Ok(())
                },
            )
            .unwrap();
        assert!(peak > 1e-4, "expected audible frozen piano stem");
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
}
