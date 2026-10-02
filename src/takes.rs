//! Strict source metadata for native audio take membership and comp selection.
//!
//! Metadata certifies existing rate clips. It never creates clips or changes
//! their playback; the ordinary compiler verifies the asset bytes and graph.

use std::collections::{BTreeMap, BTreeSet};

use num_traits::{One, ToPrimitive, Zero};

use crate::bundle::{normalize_file_reference, sha256_digest};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::exact::{Rational, MAX_RATIONAL_BITS};
use crate::located::{object_fields, Located, Location, Record};
use crate::syntax::{Document, Object, Unit, ValueKind};

pub const CAPABILITY: &str = "maac.takes/1";
pub const CAPABILITY_V2: &str = "maac.takes/2";
pub const SCHEMA_BYTES: &[u8] = include_bytes!("../takes.schema.json");
pub const SCHEMA_V2_BYTES: &[u8] = include_bytes!("../takes-v2.schema.json");
pub const MAX_LANES: usize = 16;
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
        diagnostics.push(failure);
        diagnostics
    })
}

const NOUN: &str = "take metadata";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Version {
    One,
    Two,
}

impl Version {
    fn from_capability(value: &str) -> Option<Self> {
        match value {
            CAPABILITY => Some(Self::One),
            CAPABILITY_V2 => Some(Self::Two),
            _ => None,
        }
    }

    fn capability(self) -> &'static str {
        match self {
            Self::One => CAPABILITY,
            Self::Two => CAPABILITY_V2,
        }
    }

    fn schema(self) -> &'static [u8] {
        match self {
            Self::One => SCHEMA_BYTES,
            Self::Two => SCHEMA_V2_BYTES,
        }
    }
}

pub(crate) fn is_capability(value: &str) -> bool {
    Version::from_capability(value).is_some()
}

pub(crate) fn declared(document: &Document) -> bool {
    document.objects.values().any(|object| {
        (object.kind == "extension"
            && object
                .field("namespace")
                .and_then(|f| f.value.as_string())
                .is_some_and(is_capability))
            || (object.kind == "project"
                && [CAPABILITY, CAPABILITY_V2]
                    .iter()
                    .any(|capability| requires(object, capability)))
    })
}

#[derive(Default)]
struct Counts {
    groups: usize,
    takes: usize,
    regions: usize,
}

fn prepare(
    document: &Document,
    assets: &BTreeMap<String, Vec<u8>>,
) -> Result<Document, Diagnostic> {
    let extensions: Vec<_> = document
        .objects
        .values()
        .filter_map(|object| {
            if object.kind != "extension" {
                return None;
            }
            let version = Version::from_capability(object.field("namespace")?.value.as_string()?)?;
            Some((object, version))
        })
        .collect();
    for version in [Version::One, Version::Two] {
        let mut matching = extensions
            .iter()
            .filter(|(_, candidate)| *candidate == version);
        let first = matching.next();
        if let Some((second, _)) = matching.next() {
            return Err(Location::object(second).error(
                "E_CAPABILITY",
                "at most one take extension per version is supported",
            ));
        }
        if first.is_none() {
            if let Some(entry) = document
                .objects
                .values()
                .filter(|object| object.kind == "project")
                .find_map(|project| required_entry(project, version.capability()))
            {
                return Err(entry.error(
                    "E_CAPABILITY",
                    "the required take capability needs one take extension",
                ));
            }
        }
    }
    if extensions.is_empty() {
        return Ok(document.clone());
    }
    let projects: Vec<_> = document
        .objects
        .values()
        .filter(|object| object.kind == "project")
        .collect();
    let mut counts = Counts::default();
    let mut clips = BTreeSet::new();
    let mut prepared = document.clone();
    for (extension, version) in extensions {
        let fields = object_fields(
            extension,
            &["namespace", "schema", "render_affecting", "data"],
            NOUN,
            NOUN,
        )?;
        let render_affecting = fields.get("render_affecting")?;
        if render_affecting.value.kind != ValueKind::Boolean(true) {
            return Err(
                render_affecting.error("E_CAPABILITY", "take selection must affect rendering")
            );
        }
        if projects.len() != 1 || !requires(projects[0], version.capability()) {
            return Err(fields.get("namespace")?.error(
                "E_CAPABILITY",
                "takes need one project requiring the matching take capability",
            ));
        }
        let descriptor = descriptor(document, &fields, assets, version)?;
        let data = fields.get("data")?.record()?;
        data.exact(&["groups"], &[])?;
        let groups_value = data.get("groups")?;
        let groups = groups_value.record()?;
        bounded_count(groups.len(), MAX_GROUPS, &groups_value)?;
        counts.groups += groups.len();
        if counts.groups > MAX_GROUPS {
            return Err(groups_value.error("E_RESOURCE_LIMIT", "total group count exceeds 64"));
        }
        for group in groups.entries() {
            identifier(group.name, &group.key)?;
            validate_group(document, &group.value, version, &mut counts, &mut clips)?;
        }
        prepared.objects.remove(&extension.id);
        prepared.objects.remove(&descriptor.id);
    }
    Ok(prepared)
}

/// The `project.requires` item naming `capability`, if any.
fn required_entry<'a>(project: &'a Object, capability: &str) -> Option<Located<'a>> {
    Record::object(project, "project")
        .optional("requires")?
        .items()?
        .into_iter()
        .find(|item| item.value.as_string() == Some(capability))
}

fn validate_group<'a>(
    document: &'a Document,
    group: &Located<'a>,
    version: Version,
    counts: &mut Counts,
    clips: &mut BTreeSet<&'a str>,
) -> Result<(), Diagnostic> {
    let fields = group.record()?;
    fields.exact(&["origin", "takes", "regions"], &[])?;
    let origin_value = fields.get("origin")?;
    let origin = seconds(&origin_value)?;
    if origin < Rational::zero() {
        return Err(origin_value.error("E_RANGE", "capture origin must be nonnegative"));
    }
    let takes_value = fields.get("takes")?;
    let definitions = takes_value.record()?;
    bounded_count(definitions.len(), MAX_TAKES, &takes_value)?;
    counts.takes += definitions.len();
    if counts.takes > MAX_TAKES {
        return Err(takes_value.error("E_RESOURCE_LIMIT", "total take count exceeds 64"));
    }
    let mut takes: BTreeMap<&str, BTreeMap<&str, Take<'_>>> = BTreeMap::new();
    let mut member_assets = BTreeSet::new();
    let mut rate = None;
    let mut channels = BTreeMap::new();
    for take in definitions.entries() {
        identifier(take.name, &take.key)?;
        let values = take.value.record()?;
        let lanes = match version {
            Version::One => vec![("", values)],
            Version::Two => {
                values.exact(&["lanes"], &[])?;
                let lanes_value = values.get("lanes")?;
                let lanes = lanes_value.record()?;
                bounded_count(lanes.len(), MAX_LANES, &lanes_value)?;
                lanes
                    .entries()
                    .map(|lane| {
                        identifier(lane.name, &lane.key)?;
                        Ok((lane.name, lane.value.record()?))
                    })
                    .collect::<Result<Vec<_>, Diagnostic>>()?
            }
        };
        if let Some(first) = takes.values().next() {
            if !first.keys().copied().eq(lanes.iter().map(|(id, _)| *id)) {
                return Err(take.value.error(
                    "E_REFERENCE",
                    "every take must contain the same microphone lanes",
                ));
            }
        }
        let mut members = BTreeMap::new();
        for (lane_id, values) in lanes {
            values.exact(&["asset", "source_origin"], &[])?;
            let asset_value = values.get("asset")?;
            let asset = top_reference(&asset_value)?;
            if !member_assets.insert(asset) {
                return Err(
                    asset_value.error("E_REFERENCE", "take members must name distinct assets")
                );
            }
            let metadata = audio_metadata(document, asset, &asset_value)?;
            let source_value = values.get("source_origin")?;
            let source_origin = frame(&source_value)?;
            if source_origin >= metadata.frames {
                return Err(
                    source_value.error("E_RANGE", "take source origin must precede asset end")
                );
            }
            if rate
                .replace(metadata.rate)
                .is_some_and(|old| old != metadata.rate)
                || channels
                    .insert(lane_id, metadata.channels)
                    .is_some_and(|old| old != metadata.channels)
            {
                return Err(asset_value.error("E_ASSET", "take files must share a sample rate and each lane must retain its channel count"));
            }
            members.insert(
                lane_id,
                Take {
                    asset,
                    source_origin,
                    metadata,
                },
            );
        }
        takes.insert(take.name, members);
    }
    let regions_value = fields.get("regions")?;
    let regions = regions_value.record()?;
    bounded_count(regions.len(), MAX_REGIONS, &regions_value)?;
    counts.regions += regions.len();
    if counts.regions > MAX_REGIONS {
        return Err(regions_value.error("E_RESOURCE_LIMIT", "total region count exceeds 256"));
    }
    let mut ranges = Vec::with_capacity(regions.len());
    for region in regions.entries() {
        identifier(region.name, &region.key)?;
        let values = region.value.record()?;
        let clip_field = match version {
            Version::One => "clip",
            Version::Two => "clips",
        };
        values.exact(&["take", "range", clip_field], &[])?;
        let take_value = values.get("take")?;
        let selected = takes
            .get(take_value.symbol()?)
            .ok_or_else(|| take_value.error("E_REFERENCE", "region must select a member take"))?;
        let range_value = values.get("range")?;
        let (start, end) = frame_range(&range_value)?;
        let clips_value = values.get(clip_field)?;
        let bindings = match version {
            Version::One => vec![("", clips_value.clone())],
            Version::Two => {
                let bindings = clips_value.record()?;
                bounded_count(bindings.len(), MAX_LANES, &clips_value)?;
                bindings
                    .entries()
                    .map(|binding| (binding.name, binding.value))
                    .collect()
            }
        };
        if !selected
            .keys()
            .copied()
            .eq(bindings.iter().map(|(id, _)| *id))
        {
            return Err(clips_value.error(
                "E_REFERENCE",
                "region clips must contain every microphone lane exactly once",
            ));
        }
        for (lane_id, reference) in bindings {
            let take = &selected[lane_id];
            let source_start = take.source_origin.checked_add(start).ok_or_else(|| {
                range_value.error("E_RANGE", "selected take source start overflows u64")
            })?;
            let source_end = take.source_origin.checked_add(end).ok_or_else(|| {
                range_value.error("E_RANGE", "selected take source end overflows u64")
            })?;
            if source_end > take.metadata.frames {
                return Err(range_value.error("E_RANGE", "region exceeds selected take asset"));
            }
            let clip_id = top_reference(&reference)?;
            if !clips.insert(clip_id) {
                return Err(
                    reference.error("E_REFERENCE", "each comp region must own a distinct clip")
                );
            }
            let clip = document
                .objects
                .get(clip_id)
                .filter(|object| object.kind == "audio")
                .ok_or_else(|| {
                    reference.error(
                        "E_REFERENCE",
                        "region clip must name a top-level audio object",
                    )
                })?;
            validate_clip(clip, take, &origin, start, source_start, source_end)?;
        }
        ranges.push((start, end, range_value));
    }
    ranges.sort_by_key(|(start, end, _)| (*start, *end));
    if let Some(pair) = ranges.windows(2).find(|pair| pair[1].0 < pair[0].1) {
        return Err(pair[1]
            .2
            .error("E_RANGE", "comp regions must not overlap within a group"));
    }
    Ok(())
}

fn requires(project: &Object, capability: &str) -> bool {
    project.field("requires").is_some_and(|field| {
        matches!(&field.value.kind, ValueKind::List(values) if values.iter().any(|value| value.as_string() == Some(capability)))
    })
}

fn descriptor<'a>(
    document: &'a Document,
    extension: &Record<'_>,
    assets: &BTreeMap<String, Vec<u8>>,
    version: Version,
) -> Result<&'a Object, Diagnostic> {
    let schema = extension.get("schema")?;
    let id = top_reference(&schema)?;
    let descriptor = document
        .objects
        .get(id)
        .filter(|object| object.kind == "asset")
        .ok_or_else(|| schema.error("E_ASSET", "schema must reference a descriptor asset"))?;
    let fields = object_fields(descriptor, &["kind", "path", "hash"], NOUN, NOUN)?;
    let kind = fields.get("kind")?;
    if kind.symbol()? != "descriptor" {
        return Err(kind.error("E_ASSET", "schema asset must have descriptor kind"));
    }
    let path_value = fields.get("path")?;
    let path = normalize_file_reference("package.maac", path_value.string()?).map_err(|_| {
        path_value.error(
            "E_REFERENCE",
            "schema path must remain inside the package root",
        )
    })?;
    let supplied = assets
        .get(&path)
        .ok_or_else(|| path_value.error("E_ASSET", "schema bytes are missing from the bundle"))?;
    let hash = fields.get("hash")?;
    if hash.string()? != sha256_digest(version.schema()) || supplied.as_slice() != version.schema()
    {
        return Err(hash.error(
            "E_HASH",
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

/// The audio asset `id` that a take member names at `reference`.
fn audio_metadata(
    document: &Document,
    id: &str,
    reference: &Located<'_>,
) -> Result<AudioMetadata, Diagnostic> {
    let asset = document
        .objects
        .get(id)
        .filter(|o| o.kind == "asset")
        .ok_or_else(|| {
            reference.error(
                "E_REFERENCE",
                "take asset must name a top-level audio asset",
            )
        })?;
    let fields = Record::object(asset, NOUN);
    let kind = fields.get("kind")?;
    if kind.symbol()? != "audio" {
        return Err(kind.error("E_ASSET", "takes require native core PCM audio assets"));
    }
    let format = fields.get("format")?;
    if format.string()? != crate::audio_asset::CORE_AUDIO_FORMAT {
        return Err(format.error("E_ASSET", "takes require native core PCM audio assets"));
    }
    let rate_value = fields.get("rate")?;
    let rate = match &rate_value.value.kind {
        ValueKind::Quantity {
            value,
            unit: Unit::Hz,
        } => value.clone(),
        ValueKind::Quantity {
            value,
            unit: Unit::KHz,
        } => value * integer(1000),
        _ => return Err(rate_value.error("E_UNIT", "asset rate requires Hz or kHz")),
    };
    let rate = rate
        .is_integer()
        .then(|| rate.to_integer().to_u32())
        .flatten()
        .filter(|rate| *rate > 0)
        .ok_or_else(|| rate_value.error("E_RANGE", "asset rate must be a positive u32 integer"))?;
    let channels_value = fields.get("channels")?;
    let channels = u8::try_from(number_integer(&channels_value)?)
        .ok()
        .filter(|n| matches!(n, 1 | 2))
        .ok_or_else(|| {
            channels_value.error("E_CAPABILITY", "takes support mono or stereo assets")
        })?;
    Ok(AudioMetadata {
        rate,
        channels,
        frames: number_integer(&fields.get("frames")?)?,
    })
}

fn validate_clip(
    clip: &Object,
    take: &Take<'_>,
    origin: &Rational,
    start: u64,
    source_start: u64,
    source_end: u64,
) -> Result<(), Diagnostic> {
    let fields = Record::object(clip, NOUN);
    let asset = fields.get("asset")?;
    if top_reference(&asset)? != take.asset {
        return Err(asset.error("E_REFERENCE", "comp clip asset differs from selected take"));
    }
    let playback = "comp clips require rate mode, speed 1 and forward playback";
    let mode = fields.get("mode")?;
    if mode.symbol()? != "rate" {
        return Err(mode.error("E_CAPABILITY", playback));
    }
    if let Some(speed) = fields
        .optional("speed")
        .filter(|f| f.value.kind != ValueKind::Number(Rational::one()))
    {
        return Err(speed.error("E_CAPABILITY", playback));
    }
    if let Some(reverse) = fields
        .optional("reverse")
        .filter(|f| f.value.kind != ValueKind::Boolean(false))
    {
        return Err(reverse.error("E_CAPABILITY", playback));
    }
    let source = fields.get("source")?;
    if frame_range(&source)? != (source_start, source_end) {
        return Err(source.error(
            "E_RANGE",
            "comp clip source differs from selected take range",
        ));
    }
    let at = fields.get("at")?;
    let expected_at = origin + integer(start) / integer(u64::from(take.metadata.rate));
    if seconds(&at)? != expected_at {
        return Err(at.error(
            "E_RANGE",
            "comp clip position differs from the shared capture origin",
        ));
    }
    Ok(())
}

fn integer(value: u64) -> Rational {
    Rational::from_integer(value.into())
}
fn bounded_count(count: usize, max: usize, at: &Located<'_>) -> Result<(), Diagnostic> {
    if count == 0 || count > max {
        return Err(at.error(
            "E_RESOURCE_LIMIT",
            "take metadata map is empty or exceeds its count bound",
        ));
    }
    Ok(())
}
fn identifier(value: &str, key: &Location) -> Result<(), Diagnostic> {
    let mut bytes = value.bytes();
    if value.len() > 128
        || !bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(key.error(
            "E_REFERENCE",
            "identifier must be at most 128 ASCII identifier characters",
        ));
    }
    Ok(())
}
fn top_reference<'a>(value: &Located<'a>) -> Result<&'a str, Diagnostic> {
    let reference = value
        .value
        .reference()
        .filter(|r| r.path.len() == 1 && r.port.is_none())
        .ok_or_else(|| {
            value.error(
                "E_REFERENCE",
                "reference must name one top-level object without a port",
            )
        })?;
    Ok(&reference.path[0])
}
fn number_integer(value: &Located<'_>) -> Result<u64, Diagnostic> {
    match &value.value.kind {
        ValueKind::Number(number) if number.is_integer() => number.to_integer().to_u64(),
        _ => None,
    }
    .ok_or_else(|| value.error("E_RANGE", "expected dimensionless u64 integer"))
}
fn frame(value: &Located<'_>) -> Result<u64, Diagnostic> {
    match &value.value.kind {
        ValueKind::Quantity {
            value,
            unit: Unit::Frame,
        } if value.is_integer() => value.to_integer().to_u64(),
        _ => None,
    }
    .ok_or_else(|| value.error("E_UNIT", "expected nonnegative u64 integer frames"))
}
fn frame_range(value: &Located<'_>) -> Result<(u64, u64), Diagnostic> {
    let items = value
        .items()
        .filter(|items| items.len() == 2)
        .ok_or_else(|| value.error("E_RANGE", "expected two frame endpoints"))?;
    let (start, end) = (frame(&items[0])?, frame(&items[1])?);
    if start >= end {
        return Err(value.error("E_RANGE", "frame range must be nonempty and increasing"));
    }
    Ok((start, end))
}
fn seconds(value: &Located<'_>) -> Result<Rational, Diagnostic> {
    let seconds = match &value.value.kind {
        ValueKind::Quantity {
            value,
            unit: Unit::S,
        } => value.clone(),
        ValueKind::Quantity {
            value,
            unit: Unit::Ms,
        } => value / integer(1000),
        _ => {
            return Err(value.error(
                "E_UNIT",
                "physical position requires seconds or milliseconds",
            ))
        }
    };
    if seconds.numer().bits() > MAX_RATIONAL_BITS || seconds.denom().bits() > MAX_RATIONAL_BITS {
        return Err(value.error(
            "E_RESOURCE_LIMIT",
            "physical position exceeds rational precision bound",
        ));
    }
    Ok(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::DiagnosticCode;

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
        assert_eq!(failure.code.as_str(), "E_RANGE");
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
        let document = crate::parse(&source()).unwrap();
        let data = Record::object(&document.objects["comp"], NOUN)
            .get("data")
            .unwrap();
        assert!(bounded_count(0, MAX_GROUPS, &data).is_err());
        assert!(bounded_count(MAX_GROUPS, MAX_GROUPS, &data).is_ok());
        assert!(bounded_count(MAX_GROUPS + 1, MAX_GROUPS, &data).is_err());
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
        assert_eq!(failure.code.as_str(), "E_RESOURCE_LIMIT");
        assert_eq!(failure.message, "total region count exceeds 256");
    }

    fn grouped_source() -> String {
        source()
            .replace(CAPABILITY, CAPABILITY_V2)
            .replace("takes.schema.json", "takes-v2.schema.json")
            .replace(&sha256_digest(SCHEMA_BYTES), &sha256_digest(SCHEMA_V2_BYTES))
            .replace("first_take={asset=&first; source_origin=1frame;}", "first_take={lanes={close={asset=&first; source_origin=1frame;};room={asset=&third; source_origin=2frame;};};}")
            .replace("second_take={asset=&second; source_origin=2frame;}", "second_take={lanes={close={asset=&second; source_origin=2frame;};room={asset=&fourth; source_origin=3frame;};};}")
            .replace("clip=&early", "clips={close=&early;room=&early_room;}")
            .replace("clip=&late", "clips={close=&late;room=&late_room;}")
            + &format!(r#"
asset third {{ kind=audio; path="room.pcm"; hash="{hash}"; format="pcm_f32le_interleaved/1"; rate=48000Hz; channels=2; frames=8; }}
asset fourth {{ kind=audio; path="room.pcm"; hash="{hash}"; format="pcm_f32le_interleaved/1"; rate=48000Hz; channels=2; frames=8; }}
audio early_room {{ asset=&third; at=0s; source=[2frame,4frame]; mode=rate; }}
audio late_room {{ asset=&fourth; at=1/24000s; source=[5frame,7frame]; mode=rate; }}
"#, hash=sha256_digest(&[0;64]))
    }

    fn grouped_assets() -> BTreeMap<String, Vec<u8>> {
        let mut assets = assets();
        assets.insert("takes-v2.schema.json".into(), SCHEMA_V2_BYTES.to_vec());
        assets.insert("room.pcm".into(), vec![0; 64]);
        assets
    }

    fn grouped_prepare(source: &str) -> Result<Document, Diagnostic> {
        prepare(&crate::parse(source).unwrap(), &grouped_assets())
    }

    #[test]
    fn grouped_takes_accept_mixed_lane_layouts_and_preserve_explicit_clips() {
        let source = grouped_source();
        let document = crate::parse(&source).unwrap();
        let prepared = grouped_prepare(&source).unwrap();
        assert_eq!(prepared.objects.len(), document.objects.len() - 2);
        for (id, object) in &prepared.objects {
            assert_eq!(Some(object), document.objects.get(id));
        }
        let mut bundle = crate::SourceBundle::new("nested/main.maac", source);
        bundle.assets = grouped_assets();
        assert!(crate::compile_bundle_artifact(&bundle).is_ok());
    }

    #[test]
    fn grouped_takes_require_complete_lanes_and_exact_selected_alignment() {
        for (old, new) in [
            ("room={asset=&fourth; source_origin=3frame;};", ""),
            ("room={asset=&fourth;", "other={asset=&fourth;"),
            ("room=&early_room;", ""),
            ("room=&early_room;", "room=&early_room;extra=&early_room;"),
            ("room=&early_room;", "room=&early;"),
            ("room=&early_room;", "room=&missing;"),
            ("room=&early_room;", "room=&early_room:out;"),
            (
                "asset=&fourth; source_origin=3frame",
                "asset=&third; source_origin=3frame",
            ),
            (
                "audio early_room { asset=&third;",
                "audio early_room { asset=&fourth;",
            ),
            ("source=[2frame,4frame]", "source=[3frame,5frame]"),
            (
                "audio early_room { asset=&third; at=0s;",
                "audio early_room { asset=&third; at=1ms;",
            ),
            ("source_origin=3frame", "source_origin=8frame"),
            ("source_origin=3frame", "source_origin=5frame"),
            ("clips={close=&early;room=&early_room;}", "clip=&early"),
            ("source_origin=3frame", "source_origin=3frame;unknown=1"),
        ] {
            let source = grouped_source();
            assert!(source.contains(old), "missing {old}");
            assert!(
                grouped_prepare(&source.replacen(old, new, 1)).is_err(),
                "accepted {new}"
            );
        }
    }

    #[test]
    fn grouped_takes_reject_layout_changes_and_missing_version_contracts() {
        let source = grouped_source();
        for changed in [
            source.replacen("rate=48kHz", "rate=44100Hz", 1),
            source.replacen("channels=2; frames=8", "channels=1; frames=8", 1),
            source.replace("requires=[\"maac.takes/2\"]", "requires=[]"),
            source.replace("namespace=\"maac.takes/2\"", "namespace=\"maac.takes/1\""),
            source.replace(
                &sha256_digest(SCHEMA_V2_BYTES),
                &sha256_digest(SCHEMA_BYTES),
            ),
            source.replace("render_affecting=true", "render_affecting=false"),
        ] {
            assert!(grouped_prepare(&changed).is_err());
        }
        let mut document = crate::parse(&source).unwrap();
        document.objects.remove("comp");
        assert_eq!(
            prepare(&document, &grouped_assets())
                .unwrap_err()
                .code
                .as_str(),
            "E_CAPABILITY"
        );
        let mut document = crate::parse(&source).unwrap();
        let mut duplicate = document.objects["comp"].clone();
        duplicate.id = "duplicate".into();
        document.objects.insert(duplicate.id.clone(), duplicate);
        assert_eq!(
            prepare(&document, &grouped_assets())
                .unwrap_err()
                .code
                .as_str(),
            "E_CAPABILITY"
        );
    }

    #[test]
    fn grouped_lane_count_and_source_addition_are_bounded() {
        let extra_lanes = (0..17)
            .map(|i| format!("l{i}={{asset=&third;source_origin=0frame;}};"))
            .collect::<String>();
        let source = grouped_source().replace("close={asset=&first; source_origin=1frame;};room={asset=&third; source_origin=2frame;};", &extra_lanes);
        assert_eq!(
            grouped_prepare(&source).unwrap_err().code.as_str(),
            "E_RESOURCE_LIMIT"
        );
        let source = grouped_source()
            .replace("frames=8", "frames=18446744073709551615")
            .replace(
                "source_origin=2frame;};};}",
                "source_origin=18446744073709551614frame;};};}",
            );
        let failure = grouped_prepare(&source).unwrap_err();
        assert_eq!(failure.code.as_str(), "E_RANGE");
        assert!(failure.message.contains("overflows u64"));
    }

    #[test]
    fn versions_share_clip_ownership_and_aggregate_counts() {
        let mut grouped = crate::parse(&grouped_source()).unwrap();
        let original = crate::parse(&source()).unwrap();
        // The v1 extension deliberately points at the same clips as v2.
        let mut extension = original.objects["comp"].clone();
        extension.id = "v1_comp".into();
        let mut schema = original.objects["schema"].clone();
        schema.id = "v1_schema".into();
        let ValueKind::Reference(reference) =
            &mut extension.fields.get_mut("schema").unwrap().value.kind
        else {
            panic!()
        };
        reference.path = vec![schema.id.clone()];
        grouped.objects.insert(schema.id.clone(), schema);
        grouped.objects.insert(extension.id.clone(), extension);
        let ValueKind::List(requirements) = &mut grouped
            .objects
            .get_mut("p")
            .unwrap()
            .fields
            .get_mut("requires")
            .unwrap()
            .value
            .kind
        else {
            panic!()
        };
        let original_requires = &original.objects["p"].fields["requires"].value;
        let ValueKind::List(v1) = &original_requires.kind else {
            panic!()
        };
        requirements.extend(v1.clone());
        let failure = prepare(&grouped, &grouped_assets()).unwrap_err();
        assert_eq!(failure.code.as_str(), "E_REFERENCE");
        assert!(failure.message.contains("distinct clip"));

        // Simulate previous groups/versions filling each shared counter. The
        // next valid group's addition must fail before any clips are accepted.
        let document = crate::parse(&grouped_source()).unwrap();
        let group = Record::object(&document.objects["comp"], NOUN)
            .get("data")
            .and_then(|data| data.record())
            .and_then(|data| data.get("groups"))
            .and_then(|groups| groups.record())
            .and_then(|groups| groups.get("vocals"))
            .unwrap();
        for mut counts in [
            Counts {
                takes: MAX_TAKES,
                ..Counts::default()
            },
            Counts {
                regions: MAX_REGIONS,
                ..Counts::default()
            },
        ] {
            let failure = validate_group(
                &document,
                &group,
                Version::Two,
                &mut counts,
                &mut BTreeSet::new(),
            )
            .unwrap_err();
            assert_eq!(failure.code.as_str(), "E_RESOURCE_LIMIT");
        }
    }
}
