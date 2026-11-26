//! Sidecar rack file persistence (`<song>.maac-rack.json`) so external AU/CLAP
//! plugin chains and preset states survive Studio restarts while keeping `.maac`
//! sources 100% pure and portable.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{PluginFormat, SlotTarget};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RackFile {
    pub version: u32,
    pub slots: Vec<RackSlotRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RackSlotRecord {
    pub target: SlotTarget,
    pub plugin_id: String,
    pub plugin_name: String,
    pub format: PluginFormat,
    #[serde(default)]
    pub bypassed: bool,
    #[serde(default)]
    pub params: BTreeMap<u32, f32>,
    #[serde(default)]
    pub state_b64: Option<String>,
}

/// Return the sidecar rack path next to a `.maac` entry file:
/// `examples/ferry/ferry.maac` -> `examples/ferry/ferry.maac-rack.json`.
pub fn rack_path_for(entry_path: &Path) -> PathBuf {
    let mut name = entry_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project.maac".into());
    name.push_str("-rack.json");
    entry_path.with_file_name(name)
}

pub fn load_rack_file(entry_path: &Path) -> Result<Option<RackFile>, String> {
    let path = rack_path_for(entry_path);
    if !path.exists() {
        return Ok(None);
    }
    let bytes =
        std::fs::read(&path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let parsed: RackFile = serde_json::from_slice(&bytes)
        .map_err(|err| format!("failed to parse {}: {err}", path.display()))?;
    Ok(Some(parsed))
}

pub fn save_rack_file(entry_path: &Path, rack: &RackFile) -> Result<(), String> {
    let path = rack_path_for(entry_path);
    if rack.slots.is_empty() {
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
        return Ok(());
    }
    let json = serde_json::to_string_pretty(rack)
        .map_err(|err| format!("failed to serialize rack: {err}"))?;
    std::fs::write(&path, json)
        .map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    Ok(())
}

// Minimal self-contained RFC 4648 base64 codec (no extra crate dependency).

const B64_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(B64_CHARS[((n >> 18) & 63) as usize] as char);
        out.push(B64_CHARS[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(B64_CHARS[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(B64_CHARS[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

pub fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let mut vals = Vec::with_capacity(input.len());
    let mut pad = 0usize;
    for byte in input.bytes() {
        if byte.is_ascii_whitespace() {
            continue;
        }
        if byte == b'=' {
            pad += 1;
            vals.push(0u8);
            continue;
        }
        let v = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err("invalid base64 character".into()),
        };
        vals.push(v);
    }
    if !vals.len().is_multiple_of(4) {
        return Err("invalid base64 length".into());
    }
    let mut out = Vec::with_capacity((vals.len() / 4) * 3);
    for chunk in vals.as_chunks::<4>().0 {
        let n = ((chunk[0] as u32) << 18)
            | ((chunk[1] as u32) << 12)
            | ((chunk[2] as u32) << 6)
            | (chunk[3] as u32);
        out.push(((n >> 16) & 0xFF) as u8);
        out.push(((n >> 8) & 0xFF) as u8);
        out.push((n & 0xFF) as u8);
    }
    if pad > 0 && out.len() >= pad {
        out.truncate(out.len() - pad);
    }
    Ok(out)
}
