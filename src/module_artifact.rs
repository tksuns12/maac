//! Opaque, validated source artifacts for reusable musical libraries.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::bundle::{
    sha256_digest, validate_hash_pin, ResolvedBundle, ResolvedSourceSnapshot, SourceBundle,
    MAX_BUNDLE_ASSETS, MAX_BUNDLE_SOURCES,
};
use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics};
use crate::library::LibrarySet;

pub const MODULE_ARTIFACT_FORMAT: &str = "maac.module-source";
pub const MODULE_ARTIFACT_VERSION: u32 = 1;
pub const MAX_MODULE_ARTIFACT_JSON_BYTES: usize = 72 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModuleArtifactLimits {
    max_json_bytes: usize,
}

impl ModuleArtifactLimits {
    pub const fn new(max_json_bytes: usize) -> Self {
        Self { max_json_bytes }
    }

    pub const fn max_json_bytes(self) -> usize {
        self.max_json_bytes
    }

    fn bounded(self) -> Self {
        Self::new(if self.max_json_bytes < MAX_MODULE_ARTIFACT_JSON_BYTES {
            self.max_json_bytes
        } else {
            MAX_MODULE_ARTIFACT_JSON_BYTES
        })
    }
}

impl Default for ModuleArtifactLimits {
    fn default() -> Self {
        Self::new(MAX_MODULE_ARTIFACT_JSON_BYTES)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModuleExport {
    kind: String,
    name: String,
}

impl ModuleExport {
    pub fn kind(&self) -> &str {
        &self.kind
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A self-contained, canonical source package for one reusable musical library.
/// Its representation is opaque so every construction and decode path performs
/// closure, pin, resource, semantic, and compiler-level validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModuleArtifact {
    wire: ModuleWire,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ModuleWire {
    format: String,
    version: u32,
    entry: String,
    sources: Vec<ModuleSource>,
    assets: Vec<ModuleAsset>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
struct ModuleSource {
    path: String,
    hash: String,
    builtin: bool,
    text: String,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
struct ModuleAsset {
    path: String,
    hash: String,
    bytes: String,
}

impl ModuleArtifact {
    pub fn from_source_bundle(bundle: &SourceBundle) -> Result<Self, Diagnostics> {
        let resolved = bundle.resolve()?;
        validate_library_root(&resolved)?;
        LibrarySet::resolve(&resolved)?;
        let sources = bundle
            .snapshot_resolved_sources(&resolved)?
            .into_iter()
            .map(|(path, source)| ModuleSource {
                path,
                hash: source.hash,
                builtin: source.builtin,
                text: source.source,
            })
            .collect();
        let assets = resolved
            .assets
            .iter()
            .map(|(path, bytes)| ModuleAsset {
                path: path.clone(),
                hash: sha256_digest(bytes),
                bytes: encode_hex(bytes),
            })
            .collect();
        let artifact = Self {
            wire: ModuleWire {
                format: MODULE_ARTIFACT_FORMAT.into(),
                version: MODULE_ARTIFACT_VERSION,
                entry: resolved.entry,
                sources,
                assets,
            },
        };
        artifact.validate()?;
        Ok(artifact)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, Diagnostics> {
        Self::from_json_with_limits(bytes, &ModuleArtifactLimits::default())
    }

    pub fn from_json_with_limits(
        bytes: &[u8],
        limits: &ModuleArtifactLimits,
    ) -> Result<Self, Diagnostics> {
        check_json_bytes(bytes, limits.bounded())?;
        let mut wire: ModuleWire = serde_json::from_slice(bytes).map_err(json_error)?;
        wire.sources
            .sort_by(|left, right| left.path.cmp(&right.path));
        wire.assets
            .sort_by(|left, right| left.path.cmp(&right.path));
        for asset in &mut wire.assets {
            asset.bytes.make_ascii_lowercase();
        }
        let artifact = Self { wire };
        artifact.validate()?;
        Ok(artifact)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, Diagnostics> {
        self.to_json_with_limits(&ModuleArtifactLimits::default())
    }

    pub fn to_json_with_limits(
        &self,
        limits: &ModuleArtifactLimits,
    ) -> Result<Vec<u8>, Diagnostics> {
        self.validate()?;
        let bytes = serde_json::to_vec(&self.wire).map_err(json_error)?;
        check_json_bytes(&bytes, limits.bounded())?;
        Ok(bytes)
    }

    pub fn validate(&self) -> Result<(), Diagnostics> {
        validate_wire(&self.wire).map(|_| ())
    }

    pub fn digest(&self) -> Result<String, Diagnostics> {
        self.to_json().map(|bytes| sha256_digest(&bytes))
    }

    pub fn exports(&self) -> Vec<ModuleExport> {
        let Some(source) = self
            .wire
            .sources
            .iter()
            .find(|source| source.path == self.wire.entry)
        else {
            return Vec::new();
        };
        let Ok(document) = crate::syntax::parse(&source.text) else {
            return Vec::new();
        };
        document
            .objects
            .values()
            .filter(|object| matches!(object.kind.as_str(), "pattern" | "curve" | "tuning"))
            .map(|object| ModuleExport {
                kind: object.kind.clone(),
                name: object.id.clone(),
            })
            .collect()
    }

    pub fn to_source_bundle(&self) -> Result<SourceBundle, Diagnostics> {
        let bundle = validate_wire(&self.wire)?;
        Ok(bundle)
    }
}

fn validate_wire(wire: &ModuleWire) -> Result<SourceBundle, Diagnostics> {
    if wire.format != MODULE_ARTIFACT_FORMAT {
        return Err(error(
            DiagnosticCode::Version,
            format!("module format must be `{MODULE_ARTIFACT_FORMAT}`"),
        ));
    }
    if wire.version != MODULE_ARTIFACT_VERSION {
        return Err(error(
            DiagnosticCode::Version,
            format!("unsupported module artifact version {}", wire.version),
        ));
    }
    if wire.sources.len() > MAX_BUNDLE_SOURCES {
        return Err(error(
            DiagnosticCode::ResourceLimit,
            format!("module contains more than {MAX_BUNDLE_SOURCES} sources"),
        ));
    }
    if wire.assets.len() > MAX_BUNDLE_ASSETS {
        return Err(error(
            DiagnosticCode::ResourceLimit,
            format!("module contains more than {MAX_BUNDLE_ASSETS} assets"),
        ));
    }

    let mut all_sources = BTreeMap::new();
    let mut local_sources = BTreeMap::new();
    for source in &wire.sources {
        if all_sources.contains_key(&source.path) {
            return Err(error(
                DiagnosticCode::DuplicateId,
                format!("duplicate module source `{}`", source.path),
            ));
        }
        validate_hash_pin(&source.hash, "module source hash")?;
        let actual = sha256_digest(source.text.as_bytes());
        if actual != source.hash {
            return Err(error(
                DiagnosticCode::Hash,
                format!(
                    "module source `{}` expected `{}`, but its text has `{actual}`",
                    source.path, source.hash
                ),
            ));
        }
        if source.builtin {
            let builtin = crate::stdlib::lookup_path(&source.path).ok_or_else(|| {
                error(
                    DiagnosticCode::Reference,
                    format!("unknown built-in module source `{}`", source.path),
                )
            })?;
            if builtin.source != source.text {
                return Err(error(
                    DiagnosticCode::Hash,
                    format!("built-in module source `{}` has altered bytes", source.path),
                ));
            }
        } else {
            local_sources.insert(source.path.clone(), source.text.clone());
        }
        all_sources.insert(
            source.path.clone(),
            ResolvedSourceSnapshot {
                hash: source.hash.clone(),
                source: source.text.clone(),
                builtin: source.builtin,
            },
        );
    }

    let mut assets = BTreeMap::new();
    for asset in &wire.assets {
        if assets.contains_key(&asset.path) {
            return Err(error(
                DiagnosticCode::DuplicateId,
                format!("duplicate module asset `{}`", asset.path),
            ));
        }
        validate_hash_pin(&asset.hash, "module asset hash")?;
        let bytes = decode_hex(&asset.bytes)?;
        let actual = sha256_digest(&bytes);
        if actual != asset.hash {
            return Err(error(
                DiagnosticCode::Hash,
                format!(
                    "module asset `{}` expected `{}`, but its bytes have `{actual}`",
                    asset.path, asset.hash
                ),
            ));
        }
        assets.insert(asset.path.clone(), bytes);
    }

    let bundle = SourceBundle {
        entry: wire.entry.clone(),
        sources: local_sources,
        assets,
    };
    let resolved = bundle.resolve()?;
    validate_library_root(&resolved)?;
    LibrarySet::resolve(&resolved)?;
    let resolved_sources = bundle.snapshot_resolved_sources(&resolved)?;
    if resolved_sources != all_sources {
        let expected: BTreeSet<_> = resolved_sources.keys().cloned().collect();
        let packaged: BTreeSet<_> = all_sources.keys().cloned().collect();
        return Err(error(
            DiagnosticCode::Reference,
            format!("module source closure mismatch; expected {expected:?}, packaged {packaged:?}"),
        ));
    }
    if resolved.assets != bundle.assets {
        let expected: BTreeSet<_> = resolved.assets.keys().cloned().collect();
        let packaged: BTreeSet<_> = bundle.assets.keys().cloned().collect();
        return Err(error(
            DiagnosticCode::Asset,
            format!("module asset closure mismatch; expected {expected:?}, packaged {packaged:?}"),
        ));
    }
    Ok(bundle)
}

fn validate_library_root(resolved: &ResolvedBundle) -> Result<(), Diagnostics> {
    let document = resolved.documents.get(&resolved.entry).ok_or_else(|| {
        error(
            DiagnosticCode::Reference,
            format!("module entry source `{}` is missing", resolved.entry),
        )
    })?;
    if !document
        .objects
        .values()
        .any(|object| object.kind == "library")
        || document
            .objects
            .values()
            .any(|object| object.kind == "project")
    {
        return Err(error(
            DiagnosticCode::Conflict,
            "module entry must be a reusable library, not a composition",
        ));
    }
    if !document
        .objects
        .values()
        .any(|object| matches!(object.kind.as_str(), "pattern" | "curve" | "tuning"))
    {
        return Err(error(
            DiagnosticCode::Conflict,
            "module entry must declare at least one pattern, curve, or tuning export",
        ));
    }
    Ok(())
}

fn check_json_bytes(bytes: &[u8], limits: ModuleArtifactLimits) -> Result<(), Diagnostics> {
    if bytes.len() > limits.max_json_bytes() {
        Err(error(
            DiagnosticCode::ResourceLimit,
            format!(
                "module JSON exceeds {} byte allowance",
                limits.max_json_bytes()
            ),
        ))
    } else {
        Ok(())
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        result.push(DIGITS[(byte >> 4) as usize] as char);
        result.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    result
}

fn decode_hex(value: &str) -> Result<Vec<u8>, Diagnostics> {
    if !value.len().is_multiple_of(2) {
        return Err(error(
            DiagnosticCode::Syntax,
            "module asset bytes must contain hexadecimal byte pairs",
        ));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_digit(pair[0])?;
            let low = hex_digit(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_digit(byte: u8) -> Result<u8, Diagnostics> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(error(
            DiagnosticCode::Syntax,
            "module asset bytes contain non-hexadecimal characters",
        )),
    }
}

fn json_error(error_value: serde_json::Error) -> Diagnostics {
    let message = error_value.to_string();
    let code = if message.contains("duplicate field") {
        DiagnosticCode::DuplicateField
    } else if message.contains("unknown field") {
        DiagnosticCode::UnknownField
    } else {
        DiagnosticCode::Syntax
    };
    error(code, format!("invalid module JSON: {message}"))
}

fn error(code: DiagnosticCode, message: impl Into<String>) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(Diagnostic::error(code, message, None));
    diagnostics
}
