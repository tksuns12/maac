//! Pure-Rust SFZ (`.sfz`) sample instrument parser.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hound::SampleFormat;

use super::{sanitize_maac_ident, ImportedInstrument, ImportedSample, ImportedZone};

#[derive(Clone, Default)]
struct SfzState {
    default_path: String,
    opcodes: BTreeMap<String, String>,
}

fn parse_key_token(tok: &str) -> Option<u8> {
    let t = tok.trim();
    if let Ok(n) = t.parse::<u8>() {
        return (n <= 127).then_some(n);
    }
    let bytes = t.as_bytes();
    if bytes.len() < 2 {
        return None;
    }
    let base: i32 = match bytes[0].to_ascii_uppercase() {
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
    let mut acc = 0i32;
    if idx < bytes.len() {
        match bytes[idx] {
            b'#' => {
                acc = 1;
                idx += 1;
            }
            b'b' | b'B' => {
                acc = -1;
                idx += 1;
            }
            _ => {}
        }
    }
    let octave: i32 = t[idx..].parse().ok()?;
    let midi = (octave + 1) * 12 + base + acc;
    (0..=127).contains(&midi).then_some(midi as u8)
}

fn load_wav_f32(path: &Path) -> Result<(u32, u8, Vec<f32>), String> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|err| format!("failed to open WAV {}: {err}", path.display()))?;
    let spec = reader.spec();
    let ch = spec.channels;
    if ch != 1 && ch != 2 {
        return Err(format!(
            "WAV {} has {} channels (only mono/stereo supported)",
            path.display(),
            ch
        ));
    }
    let samples: Vec<f32> = match spec.sample_format {
        SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("WAV decode error: {e}"))?,
        SampleFormat::Int => {
            let scale = (1_i64 << (spec.bits_per_sample.saturating_sub(1))) as f32;
            reader
                .samples::<i32>()
                .map(|r| r.map(|v| v as f32 / scale))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("WAV decode error: {e}"))?
        }
    };
    Ok((spec.sample_rate.max(8_000), ch as u8, samples))
}

fn expand_includes(path: &Path, depth: usize) -> Result<String, String> {
    if depth > 8 {
        return Err("SFZ #include recursion limit exceeded".into());
    }
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.split("//").next().unwrap_or("").trim();
        if let Some(rest) = trimmed.strip_prefix("#include") {
            let inc = rest.trim().trim_matches('"').trim_matches('\'');
            let inc_path = dir.join(inc.replace('\\', "/"));
            out.push_str(&expand_includes(&inc_path, depth + 1)?);
            out.push('\n');
        } else {
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    Ok(out)
}

/// Tokenize an SFZ section header/opcode stream while preserving spaces inside `sample=...`.
fn tokenize_sfz(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for line in text.lines() {
        let mut rest = line.trim();
        while !rest.is_empty() {
            if rest.starts_with('<') {
                if let Some(end) = rest.find('>') {
                    tokens.push(rest[..=end].to_owned());
                    rest = rest[end + 1..].trim_start();
                    continue;
                }
            }
            // Handle `sample=path with spaces.wav` extending to next opcode (`word=`) or `<header>` or EOL.
            if let Some(after_sample) = rest
                .strip_prefix("sample=")
                .or_else(|| rest.strip_prefix("default_path="))
            {
                let key = if rest.starts_with("sample=") {
                    "sample="
                } else {
                    "default_path="
                };
                // Find next whitespace followed by `<` or an `ident=` token.
                let mut split_at = after_sample.len();
                for (idx, _) in after_sample.match_indices(char::is_whitespace) {
                    let tail = after_sample[idx..].trim_start();
                    if tail.starts_with('<') {
                        split_at = idx;
                        break;
                    }
                    if let Some(eq) = tail.find('=') {
                        let cand = &tail[..eq];
                        if !cand.is_empty()
                            && cand.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                        {
                            split_at = idx;
                            break;
                        }
                    }
                }
                tokens.push(format!("{key}{}", after_sample[..split_at].trim()));
                rest = after_sample[split_at..].trim_start();
                continue;
            }
            let next_ws = rest.find(char::is_whitespace).unwrap_or(rest.len());
            tokens.push(rest[..next_ws].to_owned());
            rest = rest[next_ws..].trim_start();
        }
    }
    tokens
}

pub fn parse_sfz(path: &Path) -> Result<ImportedInstrument, String> {
    let text = expand_includes(path, 0)?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tokens = tokenize_sfz(&text);

    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Section {
        None,
        Control,
        Global,
        Group,
        Region,
    }

    let mut section = Section::None;
    let mut control = SfzState::default();
    let mut global = SfzState::default();
    let mut group = SfzState::default();
    let mut current_region: Option<BTreeMap<String, String>> = None;
    let mut regions: Vec<BTreeMap<String, String>> = Vec::new();

    let flush_region = |cur: &mut Option<BTreeMap<String, String>>,
                        out: &mut Vec<BTreeMap<String, String>>| {
        if let Some(reg) = cur.take() {
            if reg.contains_key("sample") {
                out.push(reg);
            }
        }
    };

    for tok in tokens {
        let lower = tok.to_ascii_lowercase();
        match lower.as_str() {
            "<control>" => {
                flush_region(&mut current_region, &mut regions);
                section = Section::Control;
                continue;
            }
            "<global>" => {
                flush_region(&mut current_region, &mut regions);
                global.opcodes.clear();
                group.opcodes.clear();
                section = Section::Global;
                continue;
            }
            "<group>" => {
                flush_region(&mut current_region, &mut regions);
                group.opcodes = global.opcodes.clone();
                section = Section::Group;
                continue;
            }
            "<region>" => {
                flush_region(&mut current_region, &mut regions);
                let mut base = global.opcodes.clone();
                for (k, v) in &group.opcodes {
                    base.insert(k.clone(), v.clone());
                }
                current_region = Some(base);
                section = Section::Region;
                continue;
            }
            _ => {}
        }
        if let Some((k, v)) = tok.split_once('=') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim().to_owned();
            match section {
                Section::Control => {
                    if key == "default_path" {
                        control.default_path = val.replace('\\', "/");
                    }
                }
                Section::Global => {
                    global.opcodes.insert(key, val);
                }
                Section::Group => {
                    group.opcodes.insert(key, val);
                }
                Section::Region => {
                    if let Some(reg) = current_region.as_mut() {
                        reg.insert(key, val);
                    }
                }
                Section::None => {}
            }
        }
    }
    flush_region(&mut current_region, &mut regions);

    if regions.is_empty() {
        return Err(format!(
            "SFZ {} contains no <region> with sample=",
            path.display()
        ));
    }

    type SampleCacheKey = (PathBuf, u8, Option<(u32, u32)>);
    let mut samples_out: Vec<ImportedSample> = Vec::new();
    let mut sample_cache: BTreeMap<SampleCacheKey, usize> = BTreeMap::new();
    let mut zones_out: Vec<ImportedZone> = Vec::new();
    let mut warnings = Vec::new();
    let mut has_stereo = false;
    let mut attack_s = 0.005_f32;
    let mut decay_s = 0.25_f32;
    let mut sustain = 0.85_f32;
    let mut release_s = 0.25_f32;

    for reg in regions {
        let Some(sample_rel) = reg.get("sample") else {
            continue;
        };
        let normalized_rel = sample_rel.replace('\\', "/");
        let wav_path = if control.default_path.is_empty() {
            base_dir.join(&normalized_rel)
        } else {
            base_dir.join(&control.default_path).join(&normalized_rel)
        };
        if !wav_path.is_file() {
            warnings.push(format!("Skipped missing sample: {}", wav_path.display()));
            continue;
        }
        let key_single = reg.get("key").and_then(|s| parse_key_token(s));
        let low_key = reg
            .get("lokey")
            .and_then(|s| parse_key_token(s))
            .or(key_single)
            .unwrap_or(0);
        let high_key = reg
            .get("hikey")
            .and_then(|s| parse_key_token(s))
            .or(key_single)
            .unwrap_or(127);
        let root_midi = reg
            .get("pitch_keycenter")
            .and_then(|s| parse_key_token(s))
            .or(key_single)
            .unwrap_or(low_key);
        let low_vel = reg
            .get("lovel")
            .and_then(|s| s.parse::<u8>().ok())
            .unwrap_or(0)
            .min(127);
        let high_vel = reg
            .get("hivel")
            .and_then(|s| s.parse::<u8>().ok())
            .unwrap_or(127)
            .min(127);
        if low_key > high_key || low_vel > high_vel {
            continue;
        }

        if let Some(v) = reg.get("ampeg_attack").and_then(|s| s.parse::<f32>().ok()) {
            attack_s = v.clamp(0.001, 10.0);
        }
        if let Some(v) = reg.get("ampeg_decay").and_then(|s| s.parse::<f32>().ok()) {
            decay_s = v.clamp(0.001, 10.0);
        }
        if let Some(v) = reg.get("ampeg_sustain").and_then(|s| s.parse::<f32>().ok()) {
            sustain = (v / 100.0).clamp(0.0, 1.0);
        }
        if let Some(v) = reg.get("ampeg_release").and_then(|s| s.parse::<f32>().ok()) {
            release_s = v.clamp(0.005, 10.0);
        }

        let looped = reg
            .get("loop_mode")
            .is_some_and(|m| m == "loop_continuous" || m == "loop_sustain");
        let loop_range = if looped {
            let ls = reg.get("loop_start").and_then(|s| s.parse::<u32>().ok());
            let le = reg.get("loop_end").and_then(|s| s.parse::<u32>().ok());
            match (ls, le) {
                (Some(a), Some(b)) if b > a + 1 => Some((a, b)),
                _ => None,
            }
        } else {
            None
        };

        let cache_key = (wav_path.clone(), root_midi, loop_range);
        let sample_index = if let Some(&idx) = sample_cache.get(&cache_key) {
            idx
        } else {
            let (rate_hz, channels, samples) = load_wav_f32(&wav_path)?;
            if channels == 2 {
                has_stereo = true;
            }
            let total_frames = (samples.len() / usize::from(channels)) as u32;
            let clamped_loop = loop_range.and_then(|(a, b)| {
                let b2 = b.min(total_frames);
                (b2 > a + 1).then_some((a, b2))
            });
            let idx = samples_out.len();
            let stem = wav_path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("smp_{idx}"));
            samples_out.push(ImportedSample {
                id: format!("s{}_{}", idx + 1, sanitize_maac_ident(&stem)),
                rate_hz,
                channels,
                root_midi,
                loop_range: clamped_loop,
                samples,
            });
            sample_cache.insert(cache_key, idx);
            idx
        };

        zones_out.push(ImportedZone {
            sample_index,
            low_key,
            high_key,
            low_vel,
            high_vel,
        });
    }

    if zones_out.is_empty() || samples_out.is_empty() {
        return Err(format!(
            "SFZ {} produced no playable sample zones",
            path.display()
        ));
    }

    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sfz_instrument".into());

    Ok(ImportedInstrument {
        slug: sanitize_maac_ident(&name),
        name,
        channels: if has_stereo { 2 } else { 1 },
        attack_s,
        decay_s,
        sustain,
        release_s,
        samples: samples_out,
        zones: zones_out,
        warnings,
    })
}
