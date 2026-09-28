//! Strict source metadata for native audio take membership and comp selection.
//!
//! Metadata certifies existing rate clips. It never creates clips or changes
//! their playback; the ordinary compiler verifies the asset bytes and graph.

use std::collections::{BTreeMap, BTreeSet};

use num_traits::{One, ToPrimitive, Zero};

use crate::bundle::{normalize_file_reference, sha256_digest};
use crate::diagnostic::{DiagnosticCode, Diagnostics};
use crate::exact::{Rational, MAX_RATIONAL_BITS};
use crate::plan::PlanError;
use crate::syntax::{Document, Field, Object, Unit, Value, ValueKind};

pub const CAPABILITY: &str = "maac.takes/1";
pub const SCHEMA_BYTES: &[u8] = include_bytes!("../takes.schema.json");
pub const MAX_GROUPS: usize = 64;
pub const MAX_TAKES: usize = 64;
pub const MAX_REGIONS: usize = 256;

/// Validate the take extension and remove only its validated metadata objects.
/// Audio assets, clips, project requirements and all other source remain intact.
/// Descriptor paths use the bundle's package-root asset namespace.
pub fn prepare_document(
    document: &Document,
    assets: &BTreeMap<String, Vec<u8>>,
) -> Result<Document, Diagnostics> {
    prepare(document, assets).map_err(|failure| {
        let mut diagnostics = Diagnostics::new();
        let mut diagnostic = failure.diagnostic();
        diagnostic.code = match failure.code.as_str() {
            "E_ASSET" => DiagnosticCode::Asset,
            "E_HASH" => DiagnosticCode::Hash,
            "E_UNIT" => DiagnosticCode::Unit,
            _ => diagnostic.code,
        };
        diagnostics.push(diagnostic);
        diagnostics
    })
}

pub(crate) fn declared(document: &Document) -> bool {
    document.objects.values().any(|object| {
        (object.kind == "extension"
            && object.field("namespace").and_then(|f| f.value.as_string()) == Some(CAPABILITY))
            || (object.kind == "project" && requires(object, CAPABILITY))
    })
}

fn prepare(document: &Document, assets: &BTreeMap<String, Vec<u8>>) -> Result<Document, PlanError> {
    let extensions: Vec<_> = document
        .objects
        .values()
        .filter(|object| {
            object.kind == "extension"
                && object.field("namespace").and_then(|f| f.value.as_string()) == Some(CAPABILITY)
        })
        .collect();
    if extensions.is_empty() {
        if document
            .objects
            .values()
            .any(|object| object.kind == "project" && requires(object, CAPABILITY))
        {
            return Err(error(
                "E_CAPABILITY",
                "takes",
                "the required take capability needs one take extension",
            ));
        }
        return Ok(document.clone());
    }
    if extensions.len() != 1 {
        return Err(error(
            "E_CAPABILITY",
            "takes",
            "exactly one take extension is supported",
        ));
    }
    let extension = extensions[0];
    object_fields(
        extension,
        &["namespace", "schema", "render_affecting", "data"],
    )?;
    if field(&extension.fields, "render_affecting")?.kind != ValueKind::Boolean(true) {
        return Err(error(
            "E_CAPABILITY",
            "takes",
            "take selection must affect rendering",
        ));
    }
    let projects: Vec<_> = document
        .objects
        .values()
        .filter(|o| o.kind == "project")
        .collect();
    if projects.len() != 1 || !requires(projects[0], CAPABILITY) {
        return Err(error(
            "E_CAPABILITY",
            "project.requires",
            "takes need one project requiring maac.takes/1",
        ));
    }
    let descriptor = descriptor(document, extension, assets)?;
    let data = record(field(&extension.fields, "data")?)?;
    exact_fields(data, &["groups"], &[], "takes.data")?;
    let groups = record(field(data, "groups")?)?;
    bounded_count(groups.len(), MAX_GROUPS, "takes.groups")?;
    let mut take_count = 0usize;
    let mut region_count = 0usize;
    let mut clips = BTreeSet::new();
    for (group_id, group) in groups {
        identifier(group_id)?;
        let fields = record(&group.value)?;
        let path = format!("takes.groups.{group_id}");
        exact_fields(fields, &["origin", "takes", "regions"], &[], &path)?;
        let origin = seconds(field(fields, "origin")?)?;
        if origin < Rational::zero() {
            return Err(error(
                "E_RANGE",
                &path,
                "capture origin must be nonnegative",
            ));
        }
        let definitions = record(field(fields, "takes")?)?;
        bounded_count(definitions.len(), MAX_TAKES, &path)?;
        take_count += definitions.len();
        if take_count > MAX_TAKES {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "takes",
                "total take count exceeds 64",
            ));
        }
        let mut takes = BTreeMap::new();
        let mut member_assets = BTreeSet::new();
        let mut layout = None;
        for (take_id, definition) in definitions {
            identifier(take_id)?;
            let values = record(&definition.value)?;
            exact_fields(values, &["asset", "source_origin"], &[], &path)?;
            let asset = top_reference(field(values, "asset")?)?;
            if !member_assets.insert(asset) {
                return Err(error(
                    "E_REFERENCE",
                    &path,
                    "take members must name distinct assets",
                ));
            }
            let metadata = audio_metadata(document, asset)?;
            let source_origin = frame(field(values, "source_origin")?)?;
            if source_origin >= metadata.frames {
                return Err(error(
                    "E_RANGE",
                    &path,
                    "take source origin must precede asset end",
                ));
            }
            let current_layout = (metadata.rate, metadata.channels);
            if layout
                .replace(current_layout)
                .is_some_and(|old| old != current_layout)
            {
                return Err(error(
                    "E_ASSET",
                    &path,
                    "take members must share sample rate and channels",
                ));
            }
            takes.insert(
                take_id.as_str(),
                Take {
                    asset,
                    source_origin,
                    metadata,
                },
            );
        }
        let regions = record(field(fields, "regions")?)?;
        bounded_count(regions.len(), MAX_REGIONS, &path)?;
        region_count += regions.len();
        if region_count > MAX_REGIONS {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "takes",
                "total region count exceeds 256",
            ));
        }
        let mut ranges = Vec::with_capacity(regions.len());
        for (region_id, definition) in regions {
            identifier(region_id)?;
            let values = record(&definition.value)?;
            let path = format!("{path}.regions.{region_id}");
            exact_fields(values, &["take", "range", "clip"], &[], &path)?;
            let take_id = symbol(field(values, "take")?)?;
            let take = takes
                .get(take_id)
                .ok_or_else(|| error("E_REFERENCE", &path, "region must select a member take"))?;
            let (start, end) = frame_range(field(values, "range")?)?;
            let source_start = take.source_origin.checked_add(start).ok_or_else(|| {
                error("E_RANGE", &path, "selected take source start overflows u64")
            })?;
            let source_end = take
                .source_origin
                .checked_add(end)
                .ok_or_else(|| error("E_RANGE", &path, "selected take source end overflows u64"))?;
            if source_end > take.metadata.frames {
                return Err(error(
                    "E_RANGE",
                    &path,
                    "region exceeds selected take asset",
                ));
            }
            let clip_id = top_reference(field(values, "clip")?)?;
            if !clips.insert(clip_id) {
                return Err(error(
                    "E_REFERENCE",
                    &path,
                    "each comp region must own a distinct clip",
                ));
            }
            let clip = document
                .objects
                .get(clip_id)
                .filter(|object| object.kind == "audio")
                .ok_or_else(|| {
                    error(
                        "E_REFERENCE",
                        &path,
                        "region clip must name a top-level audio object",
                    )
                })?;
            validate_clip(clip, take, &origin, start, source_start, source_end, &path)?;
            ranges.push((start, end));
        }
        ranges.sort_unstable();
        if ranges.windows(2).any(|pair| pair[1].0 < pair[0].1) {
            return Err(error(
                "E_RANGE",
                &path,
                "comp regions must not overlap within a group",
            ));
        }
    }
    let mut prepared = document.clone();
    prepared.objects.remove(&extension.id);
    prepared.objects.remove(&descriptor.id);
    Ok(prepared)
}

fn requires(project: &Object, capability: &str) -> bool {
    project.field("requires").is_some_and(|field| {
        matches!(&field.value.kind, ValueKind::List(values) if values.iter().any(|value| value.as_string() == Some(capability)))
    })
}

fn descriptor<'a>(
    document: &'a Document,
    extension: &Object,
    assets: &BTreeMap<String, Vec<u8>>,
) -> Result<&'a Object, PlanError> {
    let id = top_reference(field(&extension.fields, "schema")?)?;
    let descriptor = document
        .objects
        .get(id)
        .filter(|object| object.kind == "asset")
        .ok_or_else(|| {
            error(
                "E_ASSET",
                "takes.schema",
                "schema must reference a descriptor asset",
            )
        })?;
    object_fields(descriptor, &["kind", "path", "hash"])?;
    if symbol(field(&descriptor.fields, "kind")?)? != "descriptor" {
        return Err(error(
            "E_ASSET",
            "takes.schema",
            "schema asset must have descriptor kind",
        ));
    }
    let path =
        normalize_file_reference("package.maac", string(field(&descriptor.fields, "path")?)?)
            .map_err(|_| {
                error(
                    "E_REFERENCE",
                    "takes.schema",
                    "schema path must remain inside the package root",
                )
            })?;
    let supplied = assets.get(&path).ok_or_else(|| {
        error(
            "E_ASSET",
            "takes.schema",
            "schema bytes are missing from the bundle",
        )
    })?;
    if string(field(&descriptor.fields, "hash")?)? != sha256_digest(SCHEMA_BYTES)
        || supplied.as_slice() != SCHEMA_BYTES
    {
        return Err(error(
            "E_HASH",
            "takes.schema",
            "schema pin and bytes must match the recognized take schema",
        ));
    }
    Ok(descriptor)
}

struct AudioMetadata {
    rate: u32,
    channels: u8,
    frames: u64,
}
struct Take<'a> {
    asset: &'a str,
    source_origin: u64,
    metadata: AudioMetadata,
}

fn audio_metadata(document: &Document, id: &str) -> Result<AudioMetadata, PlanError> {
    let asset = document
        .objects
        .get(id)
        .filter(|o| o.kind == "asset")
        .ok_or_else(|| {
            error(
                "E_REFERENCE",
                "takes.asset",
                "take asset must name a top-level audio asset",
            )
        })?;
    if symbol(field(&asset.fields, "kind")?)? != "audio"
        || string(field(&asset.fields, "format")?)? != crate::audio_asset::CORE_AUDIO_FORMAT
    {
        return Err(error(
            "E_ASSET",
            "takes.asset",
            "takes require native core PCM audio assets",
        ));
    }
    let rate = match &field(&asset.fields, "rate")?.kind {
        ValueKind::Quantity {
            value,
            unit: Unit::Hz,
        } => value.clone(),
        ValueKind::Quantity {
            value,
            unit: Unit::KHz,
        } => value * integer(1000),
        _ => {
            return Err(error(
                "E_UNIT",
                "takes.asset.rate",
                "asset rate requires Hz or kHz",
            ))
        }
    };
    let rate = rate
        .is_integer()
        .then(|| rate.to_integer().to_u32())
        .flatten()
        .filter(|rate| *rate > 0)
        .ok_or_else(|| {
            error(
                "E_RANGE",
                "takes.asset.rate",
                "asset rate must be a positive u32 integer",
            )
        })?;
    let channels = number_integer(field(&asset.fields, "channels")?)?;
    let channels = u8::try_from(channels)
        .ok()
        .filter(|n| matches!(n, 1 | 2))
        .ok_or_else(|| {
            error(
                "E_CAPABILITY",
                "takes.asset.channels",
                "takes support mono or stereo assets",
            )
        })?;
    Ok(AudioMetadata {
        rate,
        channels,
        frames: number_integer(field(&asset.fields, "frames")?)?,
    })
}

fn validate_clip(
    clip: &Object,
    take: &Take<'_>,
    origin: &Rational,
    start: u64,
    source_start: u64,
    source_end: u64,
    path: &str,
) -> Result<(), PlanError> {
    if top_reference(field(&clip.fields, "asset")?)? != take.asset {
        return Err(error(
            "E_REFERENCE",
            path,
            "comp clip asset differs from selected take",
        ));
    }
    if symbol(field(&clip.fields, "mode")?)? != "rate"
        || clip
            .field("speed")
            .is_some_and(|f| f.value.kind != ValueKind::Number(Rational::one()))
        || clip
            .field("reverse")
            .is_some_and(|f| f.value.kind != ValueKind::Boolean(false))
    {
        return Err(error(
            "E_CAPABILITY",
            path,
            "comp clips require rate mode, speed 1 and forward playback",
        ));
    }
    if frame_range(field(&clip.fields, "source")?)? != (source_start, source_end) {
        return Err(error(
            "E_RANGE",
            path,
            "comp clip source differs from selected take range",
        ));
    }
    let expected_at = origin + integer(start) / integer(u64::from(take.metadata.rate));
    if seconds(field(&clip.fields, "at")?)? != expected_at {
        return Err(error(
            "E_RANGE",
            path,
            "comp clip position differs from the shared capture origin",
        ));
    }
    Ok(())
}

fn integer(value: u64) -> Rational {
    Rational::from_integer(value.into())
}
fn bounded_count(count: usize, max: usize, path: &str) -> Result<(), PlanError> {
    if count == 0 || count > max {
        return Err(error(
            "E_RESOURCE_LIMIT",
            path,
            "take metadata map is empty or exceeds its count bound",
        ));
    }
    Ok(())
}
fn identifier(value: &str) -> Result<(), PlanError> {
    let mut bytes = value.bytes();
    if value.len() > 128
        || !bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(error(
            "E_REFERENCE",
            "takes.id",
            "identifier must be at most 128 ASCII identifier characters",
        ));
    }
    Ok(())
}
fn exact_fields(
    fields: &BTreeMap<String, Field>,
    required: &[&str],
    optional: &[&str],
    path: &str,
) -> Result<(), PlanError> {
    if fields
        .keys()
        .any(|name| !required.contains(&name.as_str()) && !optional.contains(&name.as_str()))
    {
        return Err(error(
            "E_UNKNOWN_FIELD",
            path,
            "unknown take metadata field",
        ));
    }
    if required.iter().any(|name| !fields.contains_key(*name)) {
        return Err(error(
            "E_REFERENCE",
            path,
            "missing required take metadata field",
        ));
    }
    Ok(())
}
fn object_fields(object: &Object, required: &[&str]) -> Result<(), PlanError> {
    if !object.children.is_empty() {
        return Err(error(
            "E_UNKNOWN_KIND",
            &object.id,
            "take metadata objects cannot have children",
        ));
    }
    exact_fields(&object.fields, required, &["label"], &object.id)?;
    if let Some(label) = object.field("label") {
        if string(&label.value)?.len() > 4096 {
            return Err(error(
                "E_RESOURCE_LIMIT",
                &object.id,
                "label exceeds 4096 bytes",
            ));
        }
    }
    Ok(())
}
fn field<'a>(fields: &'a BTreeMap<String, Field>, name: &str) -> Result<&'a Value, PlanError> {
    fields.get(name).map(|f| &f.value).ok_or_else(|| {
        error(
            "E_REFERENCE",
            "takes",
            &format!("missing required {name} field"),
        )
    })
}
fn record(value: &Value) -> Result<&BTreeMap<String, Field>, PlanError> {
    match &value.kind {
        ValueKind::Record(fields) => Ok(fields),
        _ => Err(error("E_RANGE", "takes", "expected record")),
    }
}
fn string(value: &Value) -> Result<&str, PlanError> {
    value
        .as_string()
        .ok_or_else(|| error("E_RANGE", "takes", "expected string"))
}
fn symbol(value: &Value) -> Result<&str, PlanError> {
    value
        .as_symbol()
        .ok_or_else(|| error("E_RANGE", "takes", "expected symbol"))
}
fn top_reference(value: &Value) -> Result<&str, PlanError> {
    let reference = value
        .reference()
        .filter(|r| r.path.len() == 1 && r.port.is_none())
        .ok_or_else(|| {
            error(
                "E_REFERENCE",
                "takes",
                "reference must name one top-level object without a port",
            )
        })?;
    Ok(&reference.path[0])
}
fn number_integer(value: &Value) -> Result<u64, PlanError> {
    match &value.kind {
        ValueKind::Number(number) if number.is_integer() => number.to_integer().to_u64(),
        _ => None,
    }
    .ok_or_else(|| error("E_RANGE", "takes", "expected dimensionless u64 integer"))
}
fn frame(value: &Value) -> Result<u64, PlanError> {
    match &value.kind {
        ValueKind::Quantity {
            value,
            unit: Unit::Frame,
        } if value.is_integer() => value.to_integer().to_u64(),
        _ => None,
    }
    .ok_or_else(|| error("E_UNIT", "takes", "expected nonnegative u64 integer frames"))
}
fn frame_range(value: &Value) -> Result<(u64, u64), PlanError> {
    let ValueKind::List(values) = &value.kind else {
        return Err(error("E_RANGE", "takes", "expected two frame endpoints"));
    };
    if values.len() != 2 {
        return Err(error("E_RANGE", "takes", "expected two frame endpoints"));
    }
    let (start, end) = (frame(&values[0])?, frame(&values[1])?);
    if start >= end {
        return Err(error(
            "E_RANGE",
            "takes",
            "frame range must be nonempty and increasing",
        ));
    }
    Ok((start, end))
}
fn seconds(value: &Value) -> Result<Rational, PlanError> {
    let seconds = match &value.kind {
        ValueKind::Quantity {
            value,
            unit: Unit::S,
        } => value.clone(),
        ValueKind::Quantity {
            value,
            unit: Unit::Ms,
        } => value / integer(1000),
        _ => {
            return Err(error(
                "E_UNIT",
                "takes",
                "physical position requires seconds or milliseconds",
            ))
        }
    };
    if seconds.numer().bits() > MAX_RATIONAL_BITS || seconds.denom().bits() > MAX_RATIONAL_BITS {
        return Err(error(
            "E_RESOURCE_LIMIT",
            "takes",
            "physical position exceeds rational precision bound",
        ));
    }
    Ok(seconds)
}
fn error(code: &str, path: &str, message: &str) -> PlanError {
    PlanError {
        code: code.into(),
        path: path.into(),
        message: message.into(),
        span: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> String {
        format!(
            r#"maac 1;
project p {{ score=[0q,1q]; rate=48000Hz; tempo=&t; meter=&m; output=&mix:out; requires=["maac.takes/1"]; }}
tempo t {{ points=[(0q,120bpm,step)]; }}
meter m {{ points=[(0q,4,4)]; }}
asset schema {{ kind=descriptor; path="takes.schema.json"; hash="{}"; }}
asset first {{ kind=audio; path="first.pcm"; hash="{}"; format="pcm_f32le_interleaved/1"; rate=48000Hz; channels=1; frames=8; }}
asset second {{ kind=audio; path="second.pcm"; hash="{}"; format="pcm_f32le_interleaved/1"; rate=48kHz; channels=1; frames=6; }}
audio early {{ asset=&first; at=0s; source=[1frame,3frame]; mode=rate; }}
audio late {{ asset=&second; at=1/24000s; source=[4frame,6frame]; mode=rate; gain=1/2; fade_in=0ms; }}
node mix {{ type="core.sum/1"; config={{channels=1;}}; }}
connect early_mix {{ from=&early:out; to=&mix:in; }}
connect late_mix {{ from=&late:out; to=&mix:in; }}
extension comp {{ namespace="maac.takes/1"; schema=&schema; render_affecting=true; data={{ groups={{ vocals={{ origin=0s; takes={{
    first_take={{asset=&first; source_origin=1frame;}};
    second_take={{asset=&second; source_origin=2frame;}};
}}; regions={{
    early_region={{take=first_take; range=[0frame,2frame]; clip=&early;}};
    late_region={{take=second_take; range=[2frame,4frame]; clip=&late;}};
}}; }}; }}; }}; }}
"#,
            sha256_digest(SCHEMA_BYTES),
            sha256_digest(&[0; 32]),
            sha256_digest(&[0; 24])
        )
    }
    fn assets() -> BTreeMap<String, Vec<u8>> {
        BTreeMap::from([
            ("takes.schema.json".into(), SCHEMA_BYTES.to_vec()),
            ("first.pcm".into(), vec![0; 32]),
            ("second.pcm".into(), vec![0; 24]),
        ])
    }
    fn prepared(source: &str) -> Result<Document, Diagnostics> {
        prepare_document(&crate::parse(source).unwrap(), &assets())
    }
    fn compile(source: String) -> Result<crate::PlanArtifact, Diagnostics> {
        let mut bundle = crate::SourceBundle::new("scores/main.maac", source);
        bundle.assets = assets();
        crate::compile_bundle_artifact(&bundle)
    }

    #[test]
    fn removes_only_validated_metadata_and_compiles_adjacent_selections() {
        let original = crate::parse(&source()).unwrap();
        let prepared = prepare_document(&original, &assets()).unwrap();
        assert_eq!(prepared.objects.len(), original.objects.len() - 2);
        for (id, object) in &prepared.objects {
            assert_eq!(Some(object), original.objects.get(id));
        }
        assert!(!prepared.objects.contains_key("comp"));
        assert!(!prepared.objects.contains_key("schema"));
        let artifact = compile(source()).unwrap();
        assert!(artifact.to_json().is_ok());
        let alternate = source()
            .replace(
                "take=first_take; range=[0frame,2frame]",
                "take=second_take; range=[0frame,2frame]",
            )
            .replace(
                "audio early { asset=&first;",
                "audio early { asset=&second;",
            )
            .replace("source=[1frame,3frame]", "source=[2frame,4frame]");
        assert!(compile(alternate).is_ok());
    }

    #[test]
    fn normalizes_physical_units_and_accepts_explicit_native_defaults() {
        let source = source()
            .replace("origin=0s", "origin=1ms")
            .replace("at=0s", "at=1/1000s")
            .replace("at=1/24000s", "at=25/24ms")
            .replace("mode=rate;", "mode=rate; speed=1.0; reverse=false;");
        assert!(compile(source).is_ok());
    }

    #[test]
    fn rejects_clip_selection_and_capture_mismatches() {
        for (old, new) in [
            ("take=first_take;", "take=missing;"),
            ("take=first_take;", "take=\"first_take\";"),
            ("take=first_take;", "take=second_take;"),
            ("source=[1frame,3frame]", "source=[0frame,2frame]"),
            ("at=1/24000s", "at=0s"),
            ("at=0s", "at=0q"),
            ("origin=0s", "origin=0q"),
            ("origin=0s", "origin=-1s"),
            ("mode=rate;", "mode=warp_rate;"),
            ("mode=rate;", "mode=rate; speed=2;"),
            ("mode=rate;", "mode=rate; reverse=true;"),
            ("clip=&early;", "clip=&missing;"),
            ("clip=&early;", "clip=&early:out;"),
            ("clip=&early;", "clip=&early.nested;"),
            ("clip=&late;", "clip=&early;"),
            ("range=[0frame,2frame]", "range=[2frame,2frame]"),
            ("range=[0frame,2frame]", "range=[0,2]"),
            ("source_origin=1frame", "source_origin=8frame"),
            ("source_origin=1frame", "source_origin=-1frame"),
            ("source_origin=1frame", "source_origin=1/2frame"),
            (
                "source_origin=1frame",
                "source_origin=18446744073709551616frame",
            ),
            (
                "asset=&first; source_origin=1frame",
                "asset=&second; source_origin=1frame",
            ),
            (
                "asset=&first; source_origin=1frame",
                "asset=&missing; source_origin=1frame",
            ),
            (
                "asset=&first; source_origin=1frame",
                "asset=&first:out; source_origin=1frame",
            ),
        ] {
            assert!(source().contains(old), "missing {old}");
            let changed = source().replacen(old, new, 1);
            assert!(prepared(&changed).is_err(), "accepted {new}");
            assert!(compile(changed).is_err(), "compiler accepted {new}");
        }
    }

    #[test]
    fn rejects_overlap_even_when_both_clips_match() {
        let source = source()
            .replace("at=1/24000s", "at=1/48000s")
            .replace("source=[4frame,6frame]", "source=[3frame,5frame]")
            .replace("range=[2frame,4frame]", "range=[1frame,3frame]");
        assert!(prepared(&source).is_err());
    }

    #[test]
    fn shorter_alternates_are_retained_until_selected_range_exceeds_them() {
        let source = source()
            .replace("audio late { asset=&second;", "audio late { asset=&first;")
            .replace("source=[4frame,6frame]", "source=[3frame,7frame]")
            .replace(
                "take=second_take; range=[2frame,4frame]",
                "take=first_take; range=[2frame,6frame]",
            );
        assert!(compile(source.clone()).is_ok());
        let selected_short = source
            .replace("audio late { asset=&first;", "audio late { asset=&second;")
            .replace("source=[3frame,7frame]", "source=[4frame,8frame]")
            .replace(
                "take=first_take; range=[2frame,6frame]",
                "take=second_take; range=[2frame,6frame]",
            );
        assert!(prepared(&selected_short).is_err());
    }

    #[test]
    fn selected_source_addition_cannot_wrap() {
        let source = source()
            .replace("frames=8", "frames=18446744073709551615")
            .replace(
                "source_origin=1frame",
                "source_origin=18446744073709551614frame",
            );
        let failure = prepare(&crate::parse(&source).unwrap(), &assets()).unwrap_err();
        assert_eq!(failure.code, "E_RANGE");
        assert!(failure.message.contains("overflows u64"));
    }

    #[test]
    fn rejects_mixed_formats_and_invalid_native_metadata() {
        for (old, new) in [
            ("rate=48kHz", "rate=44100Hz"),
            ("rate=48kHz", "rate=0Hz"),
            ("rate=48kHz", "rate=48000s"),
            ("rate=48kHz", "rate=1/2Hz"),
            ("rate=48kHz", "rate=4294967296Hz"),
            ("channels=1; frames=6", "channels=2; frames=6"),
            ("channels=1; frames=6", "channels=3; frames=6"),
            ("channels=1; frames=6", "channels=0; frames=6"),
            ("channels=1; frames=6", "channels=1; frames=0"),
            ("format=\"pcm_f32le_interleaved/1\"", "format=\"other/1\""),
        ] {
            assert!(
                prepared(&source().replacen(old, new, 1)).is_err(),
                "accepted {new}"
            );
        }
    }

    #[test]
    fn requires_exact_pinned_schema_and_one_required_extension() {
        let source = source();
        let document = crate::parse(&source).unwrap();
        assert!(prepare_document(&document, &BTreeMap::new()).is_err());
        let mut changed_assets = assets();
        changed_assets
            .get_mut("takes.schema.json")
            .unwrap()
            .push(b'\n');
        let diagnostics = prepare_document(&document, &changed_assets).unwrap_err();
        assert_eq!(
            diagnostics.iter().next().unwrap().code,
            DiagnosticCode::Hash
        );
        let unknown = source.replace(&sha256_digest(SCHEMA_BYTES), &sha256_digest(b"{}"));
        assert!(prepared(&unknown).is_err());
        for (old, new) in [
            ("requires=[\"maac.takes/1\"]", "requires=[]"),
            ("render_affecting=true", "render_affecting=false"),
            ("schema=&schema", "schema=&first"),
            ("schema=&schema", "schema=&schema:out"),
            ("kind=descriptor", "kind=blob"),
            (
                "path=\"takes.schema.json\"",
                "path=\"../takes.schema.json\"",
            ),
            ("origin=0s", "origin=0s; unexpected=1"),
            ("source_origin=1frame", "source_origin=1frame; unexpected=1"),
            ("clip=&early", "clip=&early; unexpected=1"),
        ] {
            assert!(
                prepared(&source.replacen(old, new, 1)).is_err(),
                "accepted {new}"
            );
        }
        let mut missing = document.clone();
        missing.objects.remove("comp");
        assert!(prepare_document(&missing, &assets()).is_err());
        let mut duplicate = document.clone();
        let mut extra = duplicate.objects["comp"].clone();
        extra.id = "duplicate".into();
        duplicate.objects.insert(extra.id.clone(), extra);
        assert!(prepare_document(&duplicate, &assets()).is_err());
    }

    #[test]
    fn count_bounds_apply_to_total_maps() {
        assert!(bounded_count(0, MAX_GROUPS, "test").is_err());
        assert!(bounded_count(MAX_GROUPS, MAX_GROUPS, "test").is_ok());
        assert!(bounded_count(MAX_GROUPS + 1, MAX_GROUPS, "test").is_err());
        let mut document = crate::parse(&source()).unwrap();
        let extension = document.objects.get_mut("comp").unwrap();
        let ValueKind::Record(data) = &mut extension.fields.get_mut("data").unwrap().value.kind
        else {
            panic!()
        };
        let ValueKind::Record(groups) = &mut data.get_mut("groups").unwrap().value.kind else {
            panic!()
        };
        let group = groups["vocals"].clone();
        // Two take entries per group exceed the global 64 bound even while
        // every group satisfies its own local count limit. Remove overlapping
        // ownership to allow validation to reach that aggregate check.
        for index in 1..=32 {
            let mut group = group.clone();
            let ValueKind::Record(fields) = &mut group.value.kind else {
                panic!()
            };
            let ValueKind::Record(regions) = &mut fields.get_mut("regions").unwrap().value.kind
            else {
                panic!()
            };
            let mut one = regions["early_region"].clone();
            let ValueKind::Record(region) = &mut one.value.kind else {
                panic!()
            };
            let ValueKind::Reference(reference) = &mut region.get_mut("clip").unwrap().value.kind
            else {
                panic!()
            };
            reference.path = vec![format!("copy{index}")];
            *regions = BTreeMap::from([("early_region".into(), one)]);
            groups.insert(format!("g{index:02}"), group);
        }
        for index in 1..=32 {
            let mut clip = document.objects["early"].clone();
            clip.id = format!("copy{index}");
            document.objects.insert(clip.id.clone(), clip);
        }
        let diagnostics = prepare_document(&document, &assets()).unwrap_err();
        assert_eq!(
            diagnostics.iter().next().unwrap().code,
            DiagnosticCode::ResourceLimit
        );
    }

    #[test]
    fn region_count_is_bounded_across_groups_before_second_group_validation() {
        let mut document = crate::parse(&source()).unwrap();
        let ValueKind::Number(frames) = &mut document
            .objects
            .get_mut("first")
            .unwrap()
            .fields
            .get_mut("frames")
            .unwrap()
            .value
            .kind
        else {
            panic!()
        };
        *frames = integer(512);
        let extension = document.objects.get_mut("comp").unwrap();
        let ValueKind::Record(data) = &mut extension.fields.get_mut("data").unwrap().value.kind
        else {
            panic!()
        };
        let ValueKind::Record(groups) = &mut data.get_mut("groups").unwrap().value.kind else {
            panic!()
        };
        let template = groups["vocals"].clone();
        let mut new_clips = Vec::new();
        groups.clear();
        for (group_id, count) in [("a", 129), ("b", 128)] {
            let mut group = template.clone();
            let ValueKind::Record(fields) = &mut group.value.kind else {
                panic!()
            };
            let ValueKind::Record(takes) = &mut fields.get_mut("takes").unwrap().value.kind else {
                panic!()
            };
            takes.remove("second_take");
            let ValueKind::Record(regions) = &mut fields.get_mut("regions").unwrap().value.kind
            else {
                panic!()
            };
            regions.clear();
            for index in 0..count {
                let clip_id = format!("{group_id}_{index}");
                let parsed = crate::parse(&format!("maac 1; audio {clip_id} {{ asset=&first; at={index}/48000s; source=[{}frame,{}frame]; mode=rate; }}",index+1,index+2)).unwrap();
                new_clips.push(parsed.objects[&clip_id].clone());
                let parsed = crate::parse(&format!("maac 1; extension e {{ data={{take=first_take;range=[{index}frame,{}frame];clip=&{clip_id};}}; }}",index+1)).unwrap();
                regions.insert(
                    format!("r{index}"),
                    parsed.objects["e"].fields["data"].clone(),
                );
            }
            groups.insert(group_id.into(), group);
        }
        for clip in new_clips {
            document.objects.insert(clip.id.clone(), clip);
        }
        let failure = prepare(&document, &assets()).unwrap_err();
        assert_eq!(failure.code, "E_RESOURCE_LIMIT");
        assert_eq!(failure.message, "total region count exceeds 256");
    }
}
