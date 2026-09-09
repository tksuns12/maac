//! Strict named delivery settings and hash-pinned production descriptor resolution.

use std::collections::BTreeMap;

use num_traits::ToPrimitive;
use serde::{Deserialize, Deserializer, Serialize};

use crate::bundle::{normalize_file_reference, sha256_digest};
use crate::diagnostic::Diagnostics;
use crate::exact::{Rational, MAX_RATIONAL_BITS};
use crate::plan::{Plan, PlanError, PlanLimits, PortRef};
use crate::syntax::{Document, Field, Object, Unit, Value, ValueKind};

pub const CAPABILITY: &str = "maac.production/1";
pub const RESAMPLER: &str = "maac.src.kaiser/1";
pub const SCHEMA_BYTES: &[u8] = include_bytes!("../production.schema.json");
pub const MAX_DELIVERIES: usize = 64;
pub const MAX_TARGETS: usize = 256;
pub const MAX_TOTAL_TARGETS: usize = 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProductionSettings {
    pub schema_hash: String,
    pub extension_id: String,
    /// Absent only while preparing source; final plans require validated evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_identity: Option<crate::production_identity::ExecutionIdentity>,
    pub deliveries: BTreeMap<String, Delivery>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsWire {
    schema_hash: String,
    extension_id: String,
    #[serde(default)]
    execution_identity: Option<crate::production_identity::ExecutionIdentity>,
    #[serde(deserialize_with = "delivery_map")]
    deliveries: BTreeMap<String, Delivery>,
}

impl<'de> Deserialize<'de> for ProductionSettings {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = SettingsWire::deserialize(deserializer)?;
        let value = Self {
            schema_hash: wire.schema_hash,
            extension_id: wire.extension_id,
            execution_identity: wire.execution_identity,
            deliveries: wire.deliveries,
        };
        value
            .validate_structure()
            .map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delivery {
    pub rate: u32,
    pub resampler: String,
    #[serde(deserialize_with = "target_map")]
    pub targets: BTreeMap<String, Target>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub role: Role,
    pub output: PortRef,
    pub encoding: Encoding,
    pub dither: Dither,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<Limits>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Master,
    Stem,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Encoding {
    #[serde(rename = "wav_f32le")]
    Float32,
    #[serde(rename = "wav_pcm16le")]
    Pcm16,
    #[serde(rename = "wav_pcm24le")]
    Pcm24,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Dither {
    None,
    Tpdf { seed: u64 },
}

impl<'de> Deserialize<'de> for Dither {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // A struct variant is deliberate: serde unit variants otherwise ignore
        // extra fields even with deny_unknown_fields on the enum.
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            None {},
            Tpdf { seed: u64 },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::None {} => Self::None,
            Wire::Tpdf { seed } => Self::Tpdf { seed },
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrated_loudness: Option<LoudnessLimit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_peak: Option<PeakLimit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub true_peak: Option<PeakLimit>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum LimitUnit {
    #[serde(rename = "LUFS")]
    LuFs,
    #[serde(rename = "dBFS")]
    DbFs,
    #[serde(rename = "dBTP")]
    DbTp,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoudnessLimit {
    pub unit: LimitUnit,
    #[serde(
        default,
        with = "crate::plan::optional_rational_serde",
        skip_serializing_if = "Option::is_none"
    )]
    pub min: Option<Rational>,
    #[serde(
        default,
        with = "crate::plan::optional_rational_serde",
        skip_serializing_if = "Option::is_none"
    )]
    pub max: Option<Rational>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeakLimit {
    pub unit: LimitUnit,
    #[serde(with = "crate::plan::rational_serde")]
    pub max: Rational,
}

impl ProductionSettings {
    /// Validate identities, bounded definitions and target-local contracts.
    pub fn validate_structure(&self) -> Result<(), PlanError> {
        if self.schema_hash != sha256_digest(SCHEMA_BYTES) {
            return Err(error(
                "E_HASH",
                "production.schema_hash",
                "unknown production schema identity",
            ));
        }
        identifier(&self.extension_id)?;
        if let Some(identity) = &self.execution_identity {
            identity.validate().map_err(|failure| {
                error(
                    "E_HASH",
                    "production.execution_identity",
                    &failure.to_string(),
                )
            })?;
        }
        if self.deliveries.is_empty() || self.deliveries.len() > MAX_DELIVERIES {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "production.deliveries",
                "delivery count must be 1 through 64",
            ));
        }
        let mut total = 0usize;
        for (name, delivery) in &self.deliveries {
            identifier(name)?;
            if !matches!(delivery.rate, 44100 | 48000 | 96000) || delivery.resampler != RESAMPLER {
                return Err(error(
                    "E_CAPABILITY",
                    "production.deliveries",
                    "unsupported delivery rate or resampler",
                ));
            }
            if delivery.targets.is_empty() || delivery.targets.len() > MAX_TARGETS {
                return Err(error(
                    "E_RESOURCE_LIMIT",
                    "production.targets",
                    "target count must be 1 through 256",
                ));
            }
            total += delivery.targets.len();
            if total > MAX_TOTAL_TARGETS {
                return Err(error(
                    "E_RESOURCE_LIMIT",
                    "production.targets",
                    "total target count exceeds 1024",
                ));
            }
            let mut masters = 0;
            for (name, target) in &delivery.targets {
                identifier(name)?;
                PortRef::new(&target.output.node, &target.output.port)?;
                masters += usize::from(target.role == Role::Master);
                if target.encoding == Encoding::Float32 && target.dither != Dither::None {
                    return Err(error(
                        "E_CAPABILITY",
                        "production.dither",
                        "Float32 requires explicit none dither",
                    ));
                }
                if let Some(limits) = &target.limits {
                    limits.validate()?;
                }
            }
            if masters != 1 {
                return Err(error(
                    "E_RANGE",
                    "production.targets",
                    "each delivery requires exactly one master",
                ));
            }
        }
        Ok(())
    }

    /// Resolve every target against the same complete validated graph.
    pub fn validate(&self, plan: &Plan) -> Result<(), PlanError> {
        self.validate_with_limits(plan, &PlanLimits::default())
    }

    pub fn validate_with_limits(&self, plan: &Plan, limits: &PlanLimits) -> Result<(), PlanError> {
        self.resource_usage(limits)?;
        self.validate_structure()?;
        let identity = self.execution_identity.as_ref().ok_or_else(|| {
            error(
                "E_HASH",
                "production.execution_identity",
                "completed production plans require original-source identity evidence",
            )
        })?;
        self.validate_identity_data(identity)?;
        if plan.output.sample_rate_hz != 48000 {
            return Err(error(
                "E_CAPABILITY",
                "production",
                "production requires a 48000 Hz engine",
            ));
        }
        for delivery in self.deliveries.values() {
            for target in delivery.targets.values() {
                let channels = plan.audio_output_channels(&target.output)?;
                if !(1..=2).contains(&channels) {
                    return Err(error(
                        "E_PORT_TYPE",
                        "production.output",
                        "target layout must be mono or stereo",
                    ));
                }
                if target.role == Role::Master && target.output != plan.output.output {
                    return Err(error(
                        "E_REFERENCE",
                        "production.output",
                        "master must select project.output",
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Additional records and authored strings charged to the enclosing plan.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProductionUsage {
    pub objects: usize,
    pub string_bytes: usize,
}

impl ProductionSettings {
    /// Apply caller limits before identity parsing or plan serialization. The
    /// enclosing plan adds this usage to its existing aggregate counters.
    pub fn resource_usage(&self, limits: &PlanLimits) -> Result<ProductionUsage, PlanError> {
        let limits = limits.bounded();
        let failure = || {
            error("E_RESOURCE_LIMIT", "production", "production exceeds caller identifier, string, object, rational, or evidence byte allowance")
        };
        let mut usage = ProductionUsage {
            objects: 1,
            string_bytes: 0,
        };
        let add_string =
            |usage: &mut ProductionUsage, value: &str, identifier: bool| -> Result<(), PlanError> {
                if value.len() > limits.max_string_bytes
                    || (identifier && value.len() > limits.max_id_bytes)
                {
                    return Err(failure());
                }
                usage.string_bytes = usage
                    .string_bytes
                    .checked_add(value.len())
                    .ok_or_else(failure)?;
                Ok(())
            };
        add_string(&mut usage, &self.schema_hash, false)?;
        add_string(&mut usage, &self.extension_id, true)?;
        if let Some(identity) = &self.execution_identity {
            usage.objects += 1;
            for value in [
                &identity.algorithm,
                &identity.execution_hash,
                &identity.source_input_hash,
            ] {
                add_string(&mut usage, value, false)?;
            }
            let bytes = identity.normalized_source_json.len();
            if bytes > limits.max_json_bytes {
                return Err(failure());
            }
            usage.string_bytes = usage.string_bytes.checked_add(bytes).ok_or_else(failure)?;
        }
        for (id, delivery) in &self.deliveries {
            usage.objects = usage.objects.saturating_add(1);
            add_string(&mut usage, id, true)?;
            add_string(&mut usage, &delivery.resampler, false)?;
            for (id, target) in &delivery.targets {
                usage.objects = usage.objects.saturating_add(3); // target, output, dither
                for value in [id, &target.output.node, &target.output.port] {
                    add_string(&mut usage, value, true)?;
                }
                if let Some(limits_record) = &target.limits {
                    usage.objects = usage.objects.saturating_add(1);
                    let mut numbers = Vec::with_capacity(4);
                    if let Some(loudness) = &limits_record.integrated_loudness {
                        usage.objects = usage.objects.saturating_add(1);
                        numbers.extend(loudness.min.iter());
                        numbers.extend(loudness.max.iter());
                    }
                    for peak in [&limits_record.sample_peak, &limits_record.true_peak]
                        .into_iter()
                        .flatten()
                    {
                        usage.objects = usage.objects.saturating_add(1);
                        numbers.push(&peak.max);
                    }
                    for value in numbers {
                        usage.objects = usage.objects.saturating_add(1);
                        if value.numer().bits() > limits.max_rational_bits
                            || value.denom().bits() > limits.max_rational_bits
                        {
                            return Err(failure());
                        }
                    }
                }
            }
        }
        if usage.objects > limits.max_objects || usage.string_bytes > limits.max_total_string_bytes
        {
            return Err(failure());
        }
        Ok(usage)
    }
}

impl ProductionSettings {
    fn validate_identity_data(
        &self,
        identity: &crate::production_identity::ExecutionIdentity,
    ) -> Result<(), PlanError> {
        use serde_json::{json, Value as Json};
        let source: Json =
            serde_json::from_str(&identity.normalized_source_json).map_err(|_| {
                error(
                    "E_HASH",
                    "production.execution_identity",
                    "malformed normalized source evidence",
                )
            })?;
        let extension = &source["objects"][&self.extension_id];
        let fields = &extension["fields"];
        if extension["kind"] != "extension"
            || fields["namespace"] != json!({"t":"string","v":CAPABILITY})
            || fields["render_affecting"] != json!({"t":"boolean","v":true})
            || fields["data"] != tagged_delivery_data(&self.deliveries)
        {
            return Err(error(
                "E_HASH",
                "production.execution_identity",
                "delivery definitions differ from original-source identity evidence",
            ));
        }
        let schema = &fields["schema"];
        let path = schema["path"]
            .as_array()
            .filter(|path| path.len() == 1)
            .and_then(|path| path[0].as_str());
        let descriptor = path.map(|id| &source["objects"][id]);
        if schema["t"] != "ref"
            || !schema["port"].is_null()
            || !descriptor.is_some_and(|asset| {
                asset["kind"] == "asset"
                    && asset["fields"]["kind"] == json!({"t":"symbol","v":"descriptor"})
                    && asset["fields"]["hash"] == json!({"t":"string","v":self.schema_hash})
            })
        {
            return Err(error(
                "E_HASH",
                "production.execution_identity",
                "schema identity differs from original-source evidence",
            ));
        }
        Ok(())
    }
}

fn tagged_delivery_data(deliveries: &BTreeMap<String, Delivery>) -> serde_json::Value {
    use serde_json::{json, Map, Value as Json};
    fn record(fields: Map<String, Json>) -> Json {
        json!({"t":"record","fields":fields})
    }
    fn symbol(value: &str) -> Json {
        json!({"t":"symbol","v":value})
    }
    fn rational(value: &Rational) -> Json {
        json!({"t":"number","n":value.numer().to_string(),"d":value.denom().to_string()})
    }
    fn limit(unit: &str, minimum: Option<&Rational>, maximum: Option<&Rational>) -> Json {
        let mut fields = Map::from_iter([("unit".into(), symbol(unit))]);
        if let Some(value) = minimum {
            fields.insert("min".into(), rational(value));
        }
        if let Some(value) = maximum {
            fields.insert("max".into(), rational(value));
        }
        record(fields)
    }
    let mut definitions = Map::new();
    for (id, delivery) in deliveries {
        let mut targets = Map::new();
        for (name, target) in &delivery.targets {
            let encoding = match target.encoding {
                Encoding::Float32 => "wav_f32le",
                Encoding::Pcm16 => "wav_pcm16le",
                Encoding::Pcm24 => "wav_pcm24le",
            };
            let dither = match target.dither {
                Dither::None => record(Map::from_iter([("type".into(), symbol("none"))])),
                Dither::Tpdf { seed } => record(Map::from_iter([
                    ("type".into(), symbol("tpdf")),
                    (
                        "seed".into(),
                        rational(&Rational::from_integer(seed.into())),
                    ),
                ])),
            };
            let mut fields = Map::from_iter([
                (
                    "role".into(),
                    symbol(if target.role == Role::Master {
                        "master"
                    } else {
                        "stem"
                    }),
                ),
                (
                    "output".into(),
                    json!({"t":"ref","path":[target.output.node],"port":target.output.port}),
                ),
                ("encoding".into(), symbol(encoding)),
                ("dither".into(), dither),
            ]);
            if let Some(limits) = &target.limits {
                let mut values = Map::new();
                if let Some(value) = &limits.integrated_loudness {
                    values.insert(
                        "integrated_loudness".into(),
                        limit("LUFS", value.min.as_ref(), value.max.as_ref()),
                    );
                }
                if let Some(value) = &limits.sample_peak {
                    values.insert("sample_peak".into(), limit("dBFS", None, Some(&value.max)));
                }
                if let Some(value) = &limits.true_peak {
                    values.insert("true_peak".into(), limit("dBTP", None, Some(&value.max)));
                }
                fields.insert("limits".into(), record(values));
            }
            targets.insert(name.clone(), record(fields));
        }
        definitions.insert(
            id.clone(),
            record(Map::from_iter([
                (
                    "rate".into(),
                    json!({"t":"quantity","n":delivery.rate.to_string(),"d":"1","u":"Hz"}),
                ),
                (
                    "resampler".into(),
                    json!({"t":"string","v":delivery.resampler}),
                ),
                ("targets".into(), record(targets)),
            ])),
        );
    }
    record(Map::from_iter([("deliveries".into(), record(definitions))]))
}

impl Limits {
    fn validate(&self) -> Result<(), PlanError> {
        if let Some(limit) = &self.integrated_loudness {
            if limit.unit != LimitUnit::LuFs || (limit.min.is_none() && limit.max.is_none()) {
                return Err(error(
                    "E_UNIT",
                    "production.limits",
                    "loudness requires LUFS and at least one bound",
                ));
            }
            for bound in [&limit.min, &limit.max].into_iter().flatten() {
                finite_rational(bound)?;
            }
            if let (Some(minimum), Some(maximum)) = (&limit.min, &limit.max) {
                if minimum > maximum {
                    return Err(error(
                        "E_RANGE",
                        "production.limits",
                        "loudness minimum exceeds maximum",
                    ));
                }
            }
        }
        for (limit, expected) in [
            (&self.sample_peak, LimitUnit::DbFs),
            (&self.true_peak, LimitUnit::DbTp),
        ] {
            if let Some(limit) = limit {
                if limit.unit != expected {
                    return Err(error(
                        "E_UNIT",
                        "production.limits",
                        "peak limit has the wrong logarithmic unit",
                    ));
                }
                finite_rational(&limit.max)?;
            }
        }
        Ok(())
    }
}

/// Validate known production source data and its exact bundled descriptor bytes.
/// Only the validated extension and the descriptor it references are removed;
/// all other syntax reaches the ordinary semantic compiler unchanged. Port
/// layout resolution is finalized by `ProductionSettings::validate` after the
/// complete graph has been compiled.
pub fn prepare_document(
    document: &Document,
    assets: &BTreeMap<String, Vec<u8>>,
) -> Result<(Document, Option<ProductionSettings>), Diagnostics> {
    prepare(document, assets).map_err(|failure| {
        let mut diagnostics = Diagnostics::new();
        diagnostics.push(failure.diagnostic());
        diagnostics
    })
}

fn prepare(
    document: &Document,
    assets: &BTreeMap<String, Vec<u8>>,
) -> Result<(Document, Option<ProductionSettings>), PlanError> {
    let extensions: Vec<_> = document
        .objects
        .values()
        .filter(|object| object.kind == "extension")
        .collect();
    if extensions.is_empty() {
        return Ok((document.clone(), None));
    }
    if extensions.len() != 1 {
        return Err(error(
            "E_CAPABILITY",
            "production",
            "only one recognized production extension is supported",
        ));
    }
    let extension = extensions[0];
    object_fields(
        extension,
        &["namespace", "schema", "render_affecting", "data"],
    )?;
    if string(field(&extension.fields, "namespace")?)? != CAPABILITY {
        return Err(error(
            "E_CAPABILITY",
            "production.namespace",
            "unknown required extension",
        ));
    }
    if field(&extension.fields, "render_affecting")?.kind != ValueKind::Boolean(true) {
        return Err(error(
            "E_CAPABILITY",
            "production.render_affecting",
            "production must affect rendering",
        ));
    }
    let projects: Vec<_> = document
        .objects
        .values()
        .filter(|object| object.kind == "project")
        .collect();
    if projects.len() != 1 {
        return Err(error(
            "E_REFERENCE",
            "production",
            "production needs one project",
        ));
    }
    let project = projects[0];
    let requires = field(&project.fields, "requires")?;
    let enabled = match &requires.kind {
        ValueKind::List(values) => values
            .iter()
            .any(|value| value.as_string() == Some(CAPABILITY)),
        _ => false,
    };
    if !enabled {
        return Err(error(
            "E_CAPABILITY",
            "project.requires",
            "production capability is required",
        ));
    }
    let master = output(field(&project.fields, "output")?)?;
    let schema_ref = field(&extension.fields, "schema")?
        .reference()
        .ok_or_else(|| {
            error(
                "E_REFERENCE",
                "production.schema",
                "schema requires a descriptor reference",
            )
        })?;
    if schema_ref.path.len() != 1 || schema_ref.port.is_some() {
        return Err(error(
            "E_REFERENCE",
            "production.schema",
            "schema reference must name one top-level descriptor",
        ));
    }
    let descriptor = document.objects.get(&schema_ref.path[0]).ok_or_else(|| {
        error(
            "E_REFERENCE",
            "production.schema",
            "missing schema descriptor",
        )
    })?;
    if descriptor.kind != "asset" {
        return Err(error(
            "E_ASSET",
            "production.schema",
            "schema must reference an asset",
        ));
    }
    object_fields(descriptor, &["kind", "path", "hash"])?;
    if symbol(field(&descriptor.fields, "kind")?)? != "descriptor" {
        return Err(error(
            "E_ASSET",
            "production.schema",
            "schema asset must have descriptor kind",
        ));
    }
    let path =
        normalize_file_reference("package.maac", string(field(&descriptor.fields, "path")?)?)
            .map_err(|_| {
                error(
                    "E_REFERENCE",
                    "production.schema.path",
                    "schema path must remain inside the package root",
                )
            })?;
    let supplied = assets.get(&path).ok_or_else(|| {
        error(
            "E_ASSET",
            "production.schema",
            "schema asset bytes are missing from the bundle",
        )
    })?;
    let expected_hash = sha256_digest(SCHEMA_BYTES);
    if string(field(&descriptor.fields, "hash")?)? != expected_hash
        || supplied.as_slice() != SCHEMA_BYTES
        || sha256_digest(supplied) != expected_hash
    {
        return Err(error(
            "E_HASH",
            "production.schema",
            "schema pin and bytes must match the recognized self-contained schema",
        ));
    }
    let data = record(field(&extension.fields, "data")?)?;
    exact_fields(data, &["deliveries"], &[])?;
    let definitions = record(field(data, "deliveries")?)?;
    if definitions.is_empty() || definitions.len() > MAX_DELIVERIES {
        return Err(error(
            "E_RESOURCE_LIMIT",
            "production.deliveries",
            "delivery count must be 1 through 64",
        ));
    }
    let mut deliveries = BTreeMap::new();
    for (name, definition) in definitions {
        identifier(name)?;
        let values = record(&definition.value)?;
        exact_fields(values, &["rate", "resampler", "targets"], &[])?;
        let rate = match &field(values, "rate")?.kind {
            ValueKind::Quantity {
                value,
                unit: Unit::Hz,
            } => value.clone(),
            ValueKind::Quantity {
                value,
                unit: Unit::KHz,
            } => value * Rational::from_integer(1000.into()),
            _ => {
                return Err(error(
                    "E_UNIT",
                    "production.rate",
                    "delivery rate requires Hz or kHz",
                ))
            }
        };
        let rate = if rate.is_integer() {
            rate.to_integer().to_u32()
        } else {
            None
        }
        .ok_or_else(|| {
            error(
                "E_RANGE",
                "production.rate",
                "delivery rate must be an exact supported integer",
            )
        })?;
        let target_values = record(field(values, "targets")?)?;
        if target_values.is_empty() || target_values.len() > MAX_TARGETS {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "production.targets",
                "target count must be 1 through 256",
            ));
        }
        let mut targets = BTreeMap::new();
        for (target_id, target) in target_values {
            identifier(target_id)?;
            targets.insert(target_id.clone(), parse_target(&target.value)?);
        }
        deliveries.insert(
            name.clone(),
            Delivery {
                rate,
                resampler: string(field(values, "resampler")?)?.to_owned(),
                targets,
            },
        );
    }
    let settings = ProductionSettings {
        schema_hash: expected_hash,
        extension_id: extension.id.clone(),
        execution_identity: None,
        deliveries,
    };
    settings.validate_structure()?;
    for delivery in settings.deliveries.values() {
        for target in delivery.targets.values() {
            if target.role == Role::Master && target.output != master {
                return Err(error(
                    "E_REFERENCE",
                    "production.output",
                    "master must select project.output",
                ));
            }
            if document
                .objects
                .get(&target.output.node)
                .is_none_or(|object| object.kind != "node")
            {
                return Err(error(
                    "E_REFERENCE",
                    "production.output",
                    "target must reference an existing project node",
                ));
            }
        }
    }
    let mut prepared = document.clone();
    prepared.objects.remove(&extension.id);
    prepared.objects.remove(&descriptor.id);
    Ok((prepared, Some(settings)))
}

fn parse_target(value: &Value) -> Result<Target, PlanError> {
    let fields = record(value)?;
    exact_fields(
        fields,
        &["role", "output", "encoding", "dither"],
        &["limits"],
    )?;
    let role = match symbol(field(fields, "role")?)? {
        "master" => Role::Master,
        "stem" => Role::Stem,
        _ => return Err(error("E_RANGE", "production.role", "unknown target role")),
    };
    let encoding = match symbol(field(fields, "encoding")?)? {
        "wav_f32le" => Encoding::Float32,
        "wav_pcm16le" => Encoding::Pcm16,
        "wav_pcm24le" => Encoding::Pcm24,
        _ => {
            return Err(error(
                "E_CAPABILITY",
                "production.encoding",
                "unsupported encoding",
            ))
        }
    };
    let dither_fields = record(field(fields, "dither")?)?;
    let dither = match symbol(field(dither_fields, "type")?)? {
        "none" => {
            exact_fields(dither_fields, &["type"], &[])?;
            Dither::None
        }
        "tpdf" => {
            exact_fields(dither_fields, &["type", "seed"], &[])?;
            let value = number(field(dither_fields, "seed")?)?;
            let seed = if value.is_integer() {
                value.to_integer().to_u64()
            } else {
                None
            }
            .ok_or_else(|| {
                error(
                    "E_RANGE",
                    "production.dither.seed",
                    "seed must be an unsigned 64-bit integer",
                )
            })?;
            Dither::Tpdf { seed }
        }
        _ => {
            return Err(error(
                "E_CAPABILITY",
                "production.dither",
                "unsupported dither",
            ))
        }
    };
    let limits = fields
        .get("limits")
        .map(|field| parse_limits(&field.value))
        .transpose()?;
    Ok(Target {
        role,
        output: output(field(fields, "output")?)?,
        encoding,
        dither,
        limits,
    })
}

fn parse_limits(value: &Value) -> Result<Limits, PlanError> {
    let fields = record(value)?;
    exact_fields(
        fields,
        &[],
        &["integrated_loudness", "sample_peak", "true_peak"],
    )?;
    let integrated_loudness = fields
        .get("integrated_loudness")
        .map(|value| {
            let fields = record(&value.value)?;
            exact_fields(fields, &["unit"], &["min", "max"])?;
            Ok(LoudnessLimit {
                unit: parse_unit(field(fields, "unit")?)?,
                min: fields
                    .get("min")
                    .map(|value| number(&value.value).cloned())
                    .transpose()?,
                max: fields
                    .get("max")
                    .map(|value| number(&value.value).cloned())
                    .transpose()?,
            })
        })
        .transpose()?;
    let peak = |name: &str| -> Result<Option<PeakLimit>, PlanError> {
        fields
            .get(name)
            .map(|value| {
                let fields = record(&value.value)?;
                exact_fields(fields, &["unit", "max"], &[])?;
                Ok(PeakLimit {
                    unit: parse_unit(field(fields, "unit")?)?,
                    max: number(field(fields, "max")?)?.clone(),
                })
            })
            .transpose()
    };
    Ok(Limits {
        integrated_loudness,
        sample_peak: peak("sample_peak")?,
        true_peak: peak("true_peak")?,
    })
}

fn parse_unit(value: &Value) -> Result<LimitUnit, PlanError> {
    match symbol(value)? {
        "LUFS" => Ok(LimitUnit::LuFs),
        "dBFS" => Ok(LimitUnit::DbFs),
        "dBTP" => Ok(LimitUnit::DbTp),
        _ => Err(error(
            "E_UNIT",
            "production.limits",
            "unsupported logarithmic unit",
        )),
    }
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
            "production.id",
            "identifier must be at most 128 ASCII identifier characters",
        ));
    }
    Ok(())
}

fn finite_rational(value: &Rational) -> Result<(), PlanError> {
    if value.numer().bits() > MAX_RATIONAL_BITS || value.denom().bits() > MAX_RATIONAL_BITS {
        return Err(error(
            "E_RESOURCE_LIMIT",
            "production.limits",
            "rational precision exceeds the published bound",
        ));
    }
    if !value.to_f64().is_some_and(f64::is_finite) {
        return Err(error(
            "E_NONFINITE",
            "production.limits",
            "limit must convert to finite binary64",
        ));
    }
    Ok(())
}

fn object_fields(object: &Object, required: &[&str]) -> Result<(), PlanError> {
    if !object.children.is_empty() {
        return Err(error(
            "E_UNKNOWN_KIND",
            &object.id,
            "production objects cannot have children",
        ));
    }
    exact_fields(&object.fields, required, &["label"])?;
    if let Some(label) = object.fields.get("label") {
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
fn exact_fields(
    fields: &BTreeMap<String, Field>,
    required: &[&str],
    optional: &[&str],
) -> Result<(), PlanError> {
    if fields
        .keys()
        .any(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()))
    {
        return Err(error(
            "E_UNKNOWN_FIELD",
            "production",
            "unknown production field",
        ));
    }
    if required.iter().any(|key| !fields.contains_key(*key)) {
        return Err(error(
            "E_REFERENCE",
            "production",
            "missing required production field",
        ));
    }
    Ok(())
}
fn field<'a>(fields: &'a BTreeMap<String, Field>, name: &str) -> Result<&'a Value, PlanError> {
    fields
        .get(name)
        .map(|field| &field.value)
        .ok_or_else(|| error("E_REFERENCE", name, "missing required field"))
}
fn record(value: &Value) -> Result<&BTreeMap<String, Field>, PlanError> {
    if let ValueKind::Record(fields) = &value.kind {
        Ok(fields)
    } else {
        Err(error("E_RANGE", "production", "expected record"))
    }
}
fn string(value: &Value) -> Result<&str, PlanError> {
    value
        .as_string()
        .ok_or_else(|| error("E_RANGE", "production", "expected string"))
}
fn symbol(value: &Value) -> Result<&str, PlanError> {
    value
        .as_symbol()
        .ok_or_else(|| error("E_RANGE", "production", "expected symbol"))
}
fn number(value: &Value) -> Result<&Rational, PlanError> {
    if let ValueKind::Number(value) = &value.kind {
        Ok(value)
    } else {
        Err(error(
            "E_UNIT",
            "production",
            "expected dimensionless exact number",
        ))
    }
}
fn output(value: &Value) -> Result<PortRef, PlanError> {
    let reference = value.reference().ok_or_else(|| {
        error(
            "E_REFERENCE",
            "production.output",
            "expected output reference",
        )
    })?;
    if reference.path.len() != 1 {
        return Err(error(
            "E_REFERENCE",
            "production.output",
            "output must select a top-level node",
        ));
    }
    let port = reference.port.as_ref().ok_or_else(|| {
        error(
            "E_REFERENCE",
            "production.output",
            "output requires an explicit port",
        )
    })?;
    PortRef::new(&reference.path[0], port)
}
fn error(code: &str, path: &str, message: &str) -> PlanError {
    PlanError {
        code: code.into(),
        path: path.into(),
        message: message.into(),
        span: None,
    }
}

// Maps at the standalone boundary reject duplicate IDs instead of silently
// overwriting earlier definitions, and enforce allocation counts while reading.
fn bounded_map<'de, D, T>(deserializer: D, maximum: usize) -> Result<BTreeMap<String, T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Visitor<T> {
        maximum: usize,
        marker: std::marker::PhantomData<T>,
    }
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Visitor<T> {
        type Value = BTreeMap<String, T>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a bounded map of unique MaaC identifiers")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> Result<Self::Value, M::Error> {
            let mut result = BTreeMap::new();
            while let Some(key) = map.next_key::<String>()? {
                identifier(&key).map_err(serde::de::Error::custom)?;
                if result.contains_key(&key) {
                    return Err(serde::de::Error::custom("duplicate production identifier"));
                }
                if result.len() >= self.maximum {
                    return Err(serde::de::Error::custom(
                        "production map exceeds its resource bound",
                    ));
                }
                result.insert(key, map.next_value::<T>()?);
            }
            Ok(result)
        }
    }
    deserializer.deserialize_map(Visitor {
        maximum,
        marker: std::marker::PhantomData,
    })
}
fn delivery_map<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, Delivery>, D::Error> {
    bounded_map(deserializer, MAX_DELIVERIES)
}
fn target_map<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, Target>, D::Error> {
    bounded_map(deserializer, MAX_TARGETS)
}
