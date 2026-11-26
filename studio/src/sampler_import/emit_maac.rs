//! Compile an `ImportedInstrument` (from `.sf2` or `.sfz`) into a portable,
//! validated MaaC library (`imported/<slug>/<slug>.maac` + pinned WAV samples)
//! using `sample` declarations and `synth.sample/1`.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;

use maac::bundle::{sha256_digest, SourceBundle};
use maac::compiler::check_bundle_artifact_with_limits;
use maac::editing::{Operation, SourceDocument, Transaction};
use maac::plan::PlanLimits;
use maac::PlanArtifact;
use serde_json::{json, Value};

use super::{ImportSummary, ImportedInstrument, ImportedSample, ImportedZone};
use crate::project::{build_edit_context, EditedBundle};

/// MaaC `synth.sample/1` limits (`docs/instruments.md`).
const MAX_MAAC_SAMPLES: usize = 64;
const MAX_MAAC_ZONES: usize = 128;
/// Shared embedded WAV value budget per composition (`262,144` scalar values).
const EMBEDDED_SAMPLE_VALUE_BUDGET: usize = 262_144;

fn encode_float32_wav(samples: &[f32], rate_hz: u32, channels: u8) -> Result<Vec<u8>, String> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(
            &mut cursor,
            hound::WavSpec {
                channels: u16::from(channels),
                sample_rate: rate_hz,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .map_err(|err| format!("WAV encoder error: {err}"))?;
        for &s in samples {
            writer
                .write_sample(if s.is_finite() { s } else { 0.0 })
                .map_err(|err| format!("WAV write error: {err}"))?;
        }
        writer
            .finalize()
            .map_err(|err| format!("WAV finalize error: {err}"))?;
    }
    Ok(cursor.into_inner())
}

fn normalize_channels(sample: &mut ImportedSample, target_ch: u8) {
    if sample.channels == target_ch {
        return;
    }
    if sample.channels == 1 && target_ch == 2 {
        let mut stereo = Vec::with_capacity(sample.samples.len() * 2);
        for &s in &sample.samples {
            stereo.push(s);
            stereo.push(s);
        }
        sample.samples = stereo;
        sample.channels = 2;
    } else if sample.channels == 2 && target_ch == 1 {
        let mut mono = Vec::with_capacity(sample.samples.len() / 2);
        for pair in sample.samples.as_chunks::<2>().0 {
            mono.push(0.5 * (pair[0] + pair[1]));
        }
        sample.samples = mono;
        sample.channels = 1;
    }
}

/// Downsample by integer factor `k` (keeping loop points scaled) when needed to
/// stay cleanly within MaaC's 262,144-value embedded WAV budget.
fn decimate_sample(sample: &mut ImportedSample, factor: usize) {
    if factor <= 1 {
        return;
    }
    let ch = usize::from(sample.channels.max(1));
    let frames = sample.frames();
    let new_frames = frames.div_ceil(factor).max(2);
    let mut out = Vec::with_capacity(new_frames * ch);
    for f in (0..frames).step_by(factor) {
        for c in 0..ch {
            out.push(sample.samples[f * ch + c]);
        }
    }
    sample.samples = out;
    sample.rate_hz = (sample.rate_hz / factor as u32).max(8_000);
    if let Some((a, b)) = sample.loop_range {
        let a2 = a / factor as u32;
        let b2 = (b / factor as u32).min(sample.frames() as u32);
        sample.loop_range = (b2 > a2 + 1).then_some((a2, b2));
    }
}

fn format_frac_seconds(secs: f32) -> String {
    let ms = (secs * 1000.0).round().max(1.0) as i64;
    format!("{ms}ms")
}

fn format_frac_unit(val: f32) -> String {
    let num = (val.clamp(0.0, 1.0) * 100.0).round() as i64;
    format!("{num}/100")
}

pub type EmittedLibrary = (ImportSummary, String, BTreeMap<String, Vec<u8>>);

/// Emit `imported/<slug>/<slug>.maac` + pinned `.wav` files under `project_root`,
/// validate with `maac::compiler`, and return `(ImportSummary, library_source, assets)`.
pub fn emit_maac_library(
    mut inst: ImportedInstrument,
    project_root: &Path,
    limits: &PlanLimits,
) -> Result<EmittedLibrary, String> {
    let original_zones = inst.zones.len();
    let mut warnings = std::mem::take(&mut inst.warnings);

    // 1. Thin zones if exceeding MaaC's 128-zone or 64-sample limit.
    if inst.zones.len() > MAX_MAAC_ZONES {
        let step = inst.zones.len().div_ceil(MAX_MAAC_ZONES);
        let thinned: Vec<ImportedZone> = inst.zones.iter().step_by(step).cloned().collect();
        warnings.push(format!(
            "Thinned {} zones to {} to fit MaaC's 128-zone limit",
            inst.zones.len(),
            thinned.len()
        ));
        inst.zones = thinned;
    }

    // Collect referenced samples (capping at 64).
    let mut used_old_to_new: BTreeMap<usize, usize> = BTreeMap::new();
    let mut kept_samples: Vec<ImportedSample> = Vec::new();
    let mut kept_zones: Vec<ImportedZone> = Vec::new();
    for z in &inst.zones {
        let new_idx = if let Some(&idx) = used_old_to_new.get(&z.sample_index) {
            idx
        } else if kept_samples.len() < MAX_MAAC_SAMPLES {
            let idx = kept_samples.len();
            kept_samples.push(inst.samples[z.sample_index].clone());
            used_old_to_new.insert(z.sample_index, idx);
            idx
        } else {
            // Map to nearest already-kept sample by root pitch.
            let target_root = inst.samples[z.sample_index].root_midi;
            kept_samples
                .iter()
                .enumerate()
                .min_by_key(|(_, s)| s.root_midi.abs_diff(target_root))
                .map(|(i, _)| i)
                .unwrap_or(0)
        };
        kept_zones.push(ImportedZone {
            sample_index: new_idx,
            ..*z
        });
    }
    if kept_samples.len() < inst.samples.len() {
        warnings.push(format!(
            "Consolidated {} source samples into {} samples (64-sample cap)",
            inst.samples.len(),
            kept_samples.len()
        ));
    }

    // Normalize all samples to the instrument's channel count (prefer mono if budget is tight).
    let raw_stereo_values: usize = kept_samples.iter().map(|s| s.frames() * 2).sum();
    let target_ch = if inst.channels == 2 && raw_stereo_values <= EMBEDDED_SAMPLE_VALUE_BUDGET {
        2u8
    } else {
        if inst.channels == 2 {
            warnings.push("Mixed stereo samples to mono to fit inline sample budget".into());
        }
        1u8
    };
    for s in &mut kept_samples {
        normalize_channels(s, target_ch);
    }

    // Ensure total scalar values fit cleanly within MaaC's 262,144 embedded sample budget.
    let mut total_values: usize = kept_samples.iter().map(|s| s.samples.len()).sum();
    if total_values > EMBEDDED_SAMPLE_VALUE_BUDGET {
        let factor = total_values.div_ceil(EMBEDDED_SAMPLE_VALUE_BUDGET).min(4);
        for s in &mut kept_samples {
            decimate_sample(s, factor);
        }
        total_values = kept_samples.iter().map(|s| s.samples.len()).sum();
        if total_values > EMBEDDED_SAMPLE_VALUE_BUDGET {
            // Trim trailing silence/tail proportionally per sample to guarantee budget compliance.
            let max_per_sample = (EMBEDDED_SAMPLE_VALUE_BUDGET / kept_samples.len().max(1))
                .max(usize::from(target_ch) * 64);
            let max_frames = max_per_sample / usize::from(target_ch);
            for s in &mut kept_samples {
                if s.frames() > max_frames {
                    s.samples.truncate(max_frames * usize::from(target_ch));
                    if let Some((a, b)) = s.loop_range {
                        let b2 = b.min(max_frames as u32);
                        s.loop_range = (b2 > a + 1).then_some((a, b2));
                    }
                }
            }
            total_values = kept_samples.iter().map(|s| s.samples.len()).sum();
        }
        warnings.push(format!(
            "Compacted samples to {total_values} values to fit MaaC's inline 262,144-value budget"
        ));
    }

    // Ensure full 0..127 key coverage so no score note hits `E_RANGE` on an uncovered key!
    if !kept_zones.is_empty() {
        let min_lo = kept_zones.iter().map(|z| z.low_key).min().unwrap_or(0);
        let max_hi = kept_zones.iter().map(|z| z.high_key).max().unwrap_or(127);
        if min_lo > 0 {
            for z in &mut kept_zones {
                if z.low_key == min_lo {
                    z.low_key = 0;
                }
            }
        }
        if max_hi < 127 {
            for z in &mut kept_zones {
                if z.high_key == max_hi {
                    z.high_key = 127;
                }
            }
        }
    }

    let slug = inst.slug.clone();
    let lib_dir_rel = format!("imported/{slug}");
    let lib_rel_path = format!("{lib_dir_rel}/{slug}.maac");
    let full_lib_dir = project_root.join(&lib_dir_rel).join("samples");
    std::fs::create_dir_all(&full_lib_dir)
        .map_err(|err| format!("failed to create {}: {err}", full_lib_dir.display()))?;

    let mut bundle_assets: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut sample_decls = String::new();
    for s in &kept_samples {
        let wav = encode_float32_wav(&s.samples, s.rate_hz, s.channels)?;
        let hash = sha256_digest(&wav);
        let rel_wav_from_lib = format!("samples/{}.wav", s.id);
        let rel_wav_from_root = format!("{lib_dir_rel}/{rel_wav_from_lib}");
        std::fs::write(project_root.join(&rel_wav_from_root), &wav)
            .map_err(|err| format!("failed to write {rel_wav_from_root}: {err}"))?;
        bundle_assets.insert(rel_wav_from_root, wav);

        let loop_clause = match s.loop_range {
            Some((a, b)) => format!(" loop = [{a}frame, {b}frame];"),
            None => String::new(),
        };
        sample_decls.push_str(&format!(
            "sample {} {{ path = \"{rel_wav_from_lib}\"; hash = \"{hash}\"; root = key({});{loop_clause} }}\n",
            s.id, s.root_midi
        ));
    }

    let mut zone_entries = Vec::with_capacity(kept_zones.len());
    for z in &kept_zones {
        let sid = &kept_samples[z.sample_index].id;
        let vel_clause = if z.low_vel > 0 || z.high_vel < 127 {
            let v0 = format_frac_unit(f32::from(z.low_vel) / 127.0);
            let v1 = if z.high_vel >= 127 {
                "1".to_owned()
            } else {
                format_frac_unit((f32::from(z.high_vel) + 1.0).min(127.0) / 127.0)
            };
            format!(" velocity = [{v0}, {v1}];")
        } else {
            String::new()
        };
        zone_entries.push(format!(
            "      {{ sample = &{sid}; low = key({}); high = key({});{vel_clause} }}",
            z.low_key, z.high_key
        ));
    }

    let ch_clause = if target_ch == 2 { " channels = 2;" } else { "" };
    let release_str = format_frac_seconds(inst.release_s);
    // Always output stereo (`channels = 2`) so imported SF2/SFZ instruments drop
    // cleanly into any stereo MaaC mix bus (like `std/studio` & `std/acoustic`)
    // without doubling mono WAV sample budgets: mono voices pan through a shared
    // `synth.pan/1` stage with an exposed `pan` control.
    let (pan_control, shared_stage) = if target_ch == 1 {
        (
            "           control pan { target = &stage.stereo.params.pan; default = 0; }\n",
            "           shared stage {\n\
             \x20            channels = 2;\n\
             \x20            output = &stereo:out;\n\
             \x20            node stereo { type = \"synth.pan/1\"; params = { pan = 0; }; }\n\
             \x20            connect input_stereo { from = &input:out; to = &stereo:in; }\n\
             \x20          }\n",
        )
    } else {
        ("", "")
    };
    let lib_source = format!(
        "maac 1;\n\
         // Imported from \"{}\" by MaaC Studio\n\
         library {slug}_lib {{ version = \"1.0.0\"; }}\n\n\
         {sample_decls}\n\
         instrument {slug} {{\n\
           channels = 2;\n\
           control level {{ target = &v.smp.params.level; default = 8/10; }}\n\
           control release {{ target = &v.amp.params.release; default = {release_str}; }}\n\
         {pan_control}\
           voice v {{\n\
             channels = {target_ch};\n\
             amplitude = &amp;\n\
             output = &smp:out;\n\
             node amp {{ type = \"synth.adsr/1\"; params = {{ attack = {}; decay = {}; sustain = {}; release = {release_str}; }}; }}\n\
             node smp {{ type = \"synth.sample/1\"; config = {{{ch_clause} zones = [\n\
         {}\n\
             ]; }}; params = {{ level = 8/10; }}; }}\n\
           }}\n\
         {shared_stage}\
         }}\n",
        inst.name.replace('"', "'"),
        format_frac_seconds(inst.attack_s),
        format_frac_seconds(inst.decay_s),
        format_frac_unit(inst.sustain),
        zone_entries.join(",\n")
    );

    let full_lib_path = project_root.join(&lib_rel_path);
    std::fs::write(&full_lib_path, &lib_source)
        .map_err(|err| format!("failed to write {}: {err}", full_lib_path.display()))?;

    // Validate the emitted MaaC library independently with `maac::compiler`.
    let test_bundle = SourceBundle {
        entry: lib_rel_path.clone(),
        sources: BTreeMap::from([(lib_rel_path.clone(), lib_source.clone())]),
        assets: bundle_assets.clone(),
    };
    check_bundle_artifact_with_limits(&test_bundle, limits).map_err(|diags| {
        diags
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;

    Ok((
        ImportSummary {
            library_rel_path: lib_rel_path,
            alias: slug.clone(),
            instrument_name: slug,
            emitted_samples: kept_samples.len(),
            emitted_zones: kept_zones.len(),
            original_zones,
            total_values,
            warnings,
        },
        lib_source,
        bundle_assets,
    ))
}

/// Atomically import an emitted MaaC library into an existing project bundle
/// and optionally reassign `target_track_node`'s `instrument` field to it.
pub fn wire_imported_library_into_bundle(
    bundle: &SourceBundle,
    summary: &ImportSummary,
    lib_source: String,
    lib_assets: BTreeMap<String, Vec<u8>>,
    target_track_node: Option<&str>,
    limits: &PlanLimits,
    base_plan: Option<&PlanArtifact>,
) -> Result<(EditedBundle, String), String> {
    let mut staged = bundle.clone();
    let lib_hash = sha256_digest(lib_source.as_bytes());
    staged
        .sources
        .insert(summary.library_rel_path.clone(), lib_source);
    for (k, v) in lib_assets {
        staged.assets.insert(k, v);
    }

    let entry_src = staged
        .sources
        .get(&staged.entry)
        .ok_or_else(|| "missing entry source".to_string())?;
    let mut doc =
        SourceDocument::parse(entry_src).map_err(|err| format!("{}: {}", err.code, err.message))?;
    let objects = doc
        .authored()
        .tree()
        .get("objects")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "missing objects".to_string())?;

    let mut alias = summary.alias.clone();
    if objects.contains_key(&alias) {
        for n in 2..100 {
            let cand = format!("{}_{n}", summary.alias);
            if !objects.contains_key(&cand) {
                alias = cand;
                break;
            }
        }
    }

    let import_obj = json!({
        "kind": "import",
        "fields": {
            "path": { "t": "string", "v": summary.library_rel_path },
            "hash": { "t": "string", "v": lib_hash }
        },
        "children": {}
    });

    let mut ops = vec![Operation::InsertObject {
        parent: Vec::new(),
        id: alias.clone(),
        object_value: import_obj,
    }];

    let mut action_desc = format!("Imported {}", summary.library_rel_path);
    if let Some(node_id) = target_track_node {
        if objects
            .get(node_id)
            .and_then(|o| o.get("kind"))
            .and_then(Value::as_str)
            == Some("node")
        {
            ops.push(Operation::Set {
                object: vec![node_id.to_owned()],
                field: vec!["instrument".into()],
                value: json!({ "t": "ref", "path": [alias, summary.instrument_name], "port": null }),
                expect: None,
                expect_absent: false,
            });
            // Clear incompatible custom params that the imported sample instrument does not declare.
            if objects
                .get(node_id)
                .and_then(|o| o.pointer("/fields/params"))
                .is_some()
            {
                ops.push(Operation::Unset {
                    object: vec![node_id.to_owned()],
                    field: vec!["params".into()],
                    expect: None,
                });
            }
            action_desc = format!(
                "Assigned imported `{}` to `{node_id}`",
                summary.instrument_name
            );
        }
    }

    let tx = Transaction::new(doc.revision().to_owned(), ops)
        .map_err(|err| format!("{}: {}", err.code, err.message))?;
    let context = build_edit_context(&staged, &doc, limits, base_plan)?;
    doc.apply(&tx, &context)
        .map_err(|err| format!("{}: {}", err.code, err.message))?;
    let compiled_plan = context.take_compiled_artifact(doc.authored());

    staged
        .sources
        .insert(staged.entry.clone(), doc.source().to_owned());

    Ok((
        EditedBundle {
            bundle: staged,
            compiled_plan,
        },
        action_desc,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sampler_import::{sf2, sfz};

    fn make_sine_i16(frames: usize, freq: f64, rate: f64) -> Vec<i16> {
        (0..frames)
            .map(|i| ((std::f64::consts::TAU * freq * i as f64 / rate).sin() * 20000.0) as i16)
            .collect()
    }

    fn write_chunk(buf: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
        buf.extend_from_slice(id);
        buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
        buf.extend_from_slice(body);
        if body.len() % 2 == 1 {
            buf.push(0);
        }
    }

    fn write_list(buf: &mut Vec<u8>, list_type: &[u8; 4], body: &[u8]) {
        let mut inner = Vec::with_capacity(4 + body.len());
        inner.extend_from_slice(list_type);
        inner.extend_from_slice(body);
        write_chunk(buf, b"LIST", &inner);
    }

    fn build_synthetic_sf2() -> Vec<u8> {
        let pcm = make_sine_i16(2400, 440.0, 24_000.0);
        let mut smpl_bytes = Vec::with_capacity(pcm.len() * 2);
        for s in &pcm {
            smpl_bytes.extend_from_slice(&s.to_le_bytes());
        }

        let mut sdta = Vec::new();
        write_chunk(&mut sdta, b"smpl", &smpl_bytes);

        let mut phdr = Vec::new();
        let mut rec = [0u8; 38];
        rec[..10].copy_from_slice(b"Warm Piano");
        phdr.extend_from_slice(&rec);
        let mut eop = [0u8; 38];
        eop[..3].copy_from_slice(b"EOP");
        eop[24..26].copy_from_slice(&1u16.to_le_bytes());
        phdr.extend_from_slice(&eop);

        let mut pbag = Vec::new();
        pbag.extend_from_slice(&0u16.to_le_bytes());
        pbag.extend_from_slice(&0u16.to_le_bytes());
        pbag.extend_from_slice(&1u16.to_le_bytes());
        pbag.extend_from_slice(&0u16.to_le_bytes());

        let mut pgen = Vec::new();
        pgen.extend_from_slice(&41u16.to_le_bytes()); // instrument = 0
        pgen.extend_from_slice(&0u16.to_le_bytes());

        let mut inst = Vec::new();
        let mut irec = [0u8; 22];
        irec[..5].copy_from_slice(b"Piano");
        inst.extend_from_slice(&irec);
        let mut eoi = [0u8; 22];
        eoi[..3].copy_from_slice(b"EOI");
        eoi[20..22].copy_from_slice(&1u16.to_le_bytes());
        inst.extend_from_slice(&eoi);

        let mut ibag = Vec::new();
        ibag.extend_from_slice(&0u16.to_le_bytes());
        ibag.extend_from_slice(&0u16.to_le_bytes());
        ibag.extend_from_slice(&3u16.to_le_bytes());
        ibag.extend_from_slice(&0u16.to_le_bytes());

        let mut igen = Vec::new();
        igen.extend_from_slice(&43u16.to_le_bytes()); // keyRange 0..127
        igen.extend_from_slice(&(127u16 << 8).to_le_bytes());
        igen.extend_from_slice(&54u16.to_le_bytes()); // sampleModes = loop
        igen.extend_from_slice(&1u16.to_le_bytes());
        igen.extend_from_slice(&53u16.to_le_bytes()); // sampleID = 0
        igen.extend_from_slice(&0u16.to_le_bytes());

        let mut shdr = Vec::new();
        let mut srec = [0u8; 46];
        srec[..6].copy_from_slice(b"A4Tone");
        srec[20..24].copy_from_slice(&0u32.to_le_bytes());
        srec[24..28].copy_from_slice(&(pcm.len() as u32).to_le_bytes());
        srec[28..32].copy_from_slice(&200u32.to_le_bytes());
        srec[32..36].copy_from_slice(&1800u32.to_le_bytes());
        srec[36..40].copy_from_slice(&24_000u32.to_le_bytes());
        srec[40] = 69; // A4
        srec[44..46].copy_from_slice(&1u16.to_le_bytes()); // mono
        shdr.extend_from_slice(&srec);
        let mut eos = [0u8; 46];
        eos[..3].copy_from_slice(b"EOS");
        shdr.extend_from_slice(&eos);

        let mut pdta = Vec::new();
        write_chunk(&mut pdta, b"phdr", &phdr);
        write_chunk(&mut pdta, b"pbag", &pbag);
        write_chunk(&mut pdta, b"pgen", &pgen);
        write_chunk(&mut pdta, b"inst", &inst);
        write_chunk(&mut pdta, b"ibag", &ibag);
        write_chunk(&mut pdta, b"igen", &igen);
        write_chunk(&mut pdta, b"shdr", &shdr);

        let mut body = Vec::new();
        body.extend_from_slice(b"sfbk");
        write_list(&mut body, b"sdta", &sdta);
        write_list(&mut body, b"pdta", &pdta);

        let mut riff = Vec::new();
        riff.extend_from_slice(b"RIFF");
        riff.extend_from_slice(&(body.len() as u32).to_le_bytes());
        riff.extend_from_slice(&body);
        riff
    }

    #[test]
    fn imports_synthetic_sf2_and_sfz_into_playable_maac_libraries() {
        let tmp = std::env::temp_dir().join(format!("maac-sampler-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // 1. Test SF2 roundtrip -> pure MaaC compile & render.
        let sf2_path = tmp.join("warm.sf2");
        std::fs::write(&sf2_path, build_synthetic_sf2()).unwrap();
        let presets = sf2::list_sf2_presets(&sf2_path).expect("list sf2 presets");
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].name, "Warm Piano");

        let parsed_sf2 = sf2::parse_sf2_preset(&sf2_path, 0, 0).expect("parse sf2 preset");
        let limits = PlanLimits::song();
        let (summary, lib_src, lib_assets) =
            emit_maac_library(parsed_sf2, &tmp, &limits).expect("emit SF2 MaaC library");
        assert_eq!(summary.emitted_samples, 1);
        assert_eq!(summary.emitted_zones, 1);

        // Compile a song using the imported library and render audible audio through pure DspEngine!
        let lib_hash = sha256_digest(lib_src.as_bytes());
        let song_src = format!(
            "maac 1;\n\
             project p {{ score = [0q, 2q]; rate = 48000Hz; tempo = &c; meter = &m; output = &k:out; }}\n\
             tempo c {{ points = [(0q, 120bpm, step)]; }}\n\
             meter m {{ points = [(0q, 4, 4)]; }}\n\
             import wp {{ path = \"{}\"; hash = \"{lib_hash}\"; }}\n\
             node k {{ instrument = &wp.{}; }}\n\
             pattern pat {{ length = 2q; note n1 {{ at = 0q; dur = 1q; pitch = C4; velocity = 8/10; }} }}\n\
             track t {{ target = &k:events; }}\n\
             place pl {{ pattern = &pat; track = &t; at = 0q; }}\n",
            summary.library_rel_path, summary.instrument_name
        );
        let mut sources = BTreeMap::new();
        sources.insert("main.maac".into(), song_src);
        sources.insert(summary.library_rel_path.clone(), lib_src);
        let bundle = SourceBundle {
            entry: "main.maac".into(),
            sources,
            assets: lib_assets,
        };
        let plan =
            maac::compiler::compile_bundle_artifact_with_limits(&bundle, &limits).expect("compile");
        let mut engine = maac::dsp::DspEngine::new_artifact_with_limits(&plan, &limits).unwrap();
        let mut peak = 0.0_f64;
        engine
            .render_span(
                maac::dsp::RenderSpan {
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
        assert!(peak > 1e-3, "expected audible SF2 render, got peak={peak}");

        // 2. Test SFZ roundtrip.
        let wav_bytes = encode_float32_wav(
            &make_sine_i16(2400, 440.0, 24_000.0)
                .iter()
                .map(|&v| f32::from(v) / 32768.0)
                .collect::<Vec<_>>(),
            24_000,
            1,
        )
        .unwrap();
        std::fs::write(tmp.join("bell.wav"), wav_bytes).unwrap();
        let sfz_path = tmp.join("bell.sfz");
        std::fs::write(
            &sfz_path,
            "<global>\nampeg_attack=0.01\nampeg_release=0.3\n<region>\nsample=bell.wav\nlokey=48 hikey=84 pitch_keycenter=69\n",
        )
        .unwrap();
        let parsed_sfz = sfz::parse_sfz(&sfz_path).expect("parse sfz");
        let (sfz_sum, _, _) =
            emit_maac_library(parsed_sfz, &tmp, &limits).expect("emit SFZ MaaC library");
        assert_eq!(sfz_sum.emitted_samples, 1);
        assert_eq!(sfz_sum.emitted_zones, 1);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
