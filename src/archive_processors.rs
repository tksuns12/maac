//! Bounded, span-free native processor evidence for opt-in archive snapshots.
//!
//! This describes the compiled MaaC processor context at a reset origin. It
//! does not describe hosted plugins or capture live processor state.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

use num_bigint::BigInt;
use serde::{Deserialize, Serialize};

use crate::bundle::{sha256_digest, validate_hash_pin};
use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics};
use crate::disk_media::DiskMediaPlan;
use crate::exact::{parse_rational, Rational};
use crate::graph::{GraphProcessor, InstrumentProgram};
use crate::library::WavetableSource;
use crate::plan::{PlanView, Processor, ProcessorView, SourceMapping};
use crate::plan_v4::KitSampleRef;
use crate::plan_v5::AudioClip;
use crate::plan_v6::WarpClip;
use crate::plan_v7::LfoConfig;
use crate::production_identity::canonical_json_bytes;

pub(crate) const MAX_NATIVE_PROCESSOR_CONTEXT_BYTES: usize = 4 * 1024 * 1024;
const FORMAT: &str = "maac.archive-processors";
const VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub(crate) struct NativeProcessorContext {
    record: ContextRecord,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextRecord {
    format: String,
    version: u32,
    sample_rate_hz: u32,
    reset_origin_q: String,
    latency_compensation: String,
    nodes: Vec<NodeRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeRecord {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<SourceRef>,
    processor_id: String,
    processor: NativeProcessor,
    initial_params: BTreeMap<String, String>,
    latency_frames: u64,
    state_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    instrument: Option<InstrumentBinding>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceRef {
    object: String,
    path: Vec<String>,
}

impl From<&SourceMapping> for SourceRef {
    fn from(value: &SourceMapping) -> Self {
        Self {
            object: value.object.clone(),
            path: value.path.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum NativeProcessor {
    Core {
        processor: Processor,
    },
    Lfo {
        config: LfoConfig,
    },
    Constant {},
    Audio {
        clip: Box<AudioClip>,
    },
    WarpRate {
        clip: Box<WarpClip>,
    },
    Kit {
        channels: u8,
        voices: u32,
        samples: Vec<KitSampleRef>,
    },
}

impl NativeProcessor {
    fn identity(&self) -> &'static str {
        match self {
            Self::Core { processor } => processor.kind(),
            Self::Lfo { .. } => "lfo",
            Self::Constant {} => "constant",
            Self::Audio { .. } => "audio",
            Self::WarpRate { .. } => "warp_rate",
            Self::Kit { .. } => "kit",
        }
    }

    fn technical_latency_frames(&self) -> u64 {
        match self {
            Self::Core { processor } => processor.technical_latency_frames(),
            _ => 0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstrumentBinding {
    program: InstrumentProgram,
    wavetables: Vec<WavetableBinding>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WavetableBinding {
    source: WavetableSource,
    cycle_length: u32,
    sample_count: usize,
}

impl NativeProcessorContext {
    pub(crate) fn capture(plan: &DiskMediaPlan) -> Result<Self, Diagnostics> {
        let view = plan.view();
        let mut nodes = Vec::with_capacity(view.nodes.len());
        let mut remaining = MAX_NATIVE_PROCESSOR_CONTEXT_BYTES;
        for node in view.nodes {
            let source = view
                .source_mappings
                .iter()
                .find(|mapping| mapping.object == *node.id)
                .map(SourceRef::from)
                .or_else(|| match node.processor {
                    ProcessorView::Audio(clip) => Some(SourceRef::from(&clip.source)),
                    ProcessorView::WarpRate(clip) => Some(SourceRef::from(&clip.source)),
                    _ => None,
                });
            let processor = match node.processor {
                ProcessorView::Core(processor) => NativeProcessor::Core {
                    processor: processor.clone(),
                },
                ProcessorView::Lfo(config) => NativeProcessor::Lfo {
                    config: config.clone(),
                },
                ProcessorView::Constant => NativeProcessor::Constant {},
                ProcessorView::Audio(clip) => {
                    let mut clip = clip.clone();
                    clip.source.span = None;
                    NativeProcessor::Audio {
                        clip: Box::new(clip),
                    }
                }
                ProcessorView::WarpRate(clip) => {
                    let mut clip = clip.clone();
                    clip.source.span = None;
                    NativeProcessor::WarpRate {
                        clip: Box::new(clip),
                    }
                }
                ProcessorView::Kit {
                    channels,
                    voices,
                    samples,
                } => NativeProcessor::Kit {
                    channels,
                    voices,
                    samples: samples.to_vec(),
                },
            };
            let mut params = defaults_for(node.processor);
            params.extend(view.resolved_node_params(node).map_err(|e| {
                fail(
                    DiagnosticCode::Range,
                    format!("cannot resolve processor parameters: {e}"),
                )
            })?);
            let instrument = match node.processor {
                ProcessorView::Core(Processor::Instrument { program, .. }) => {
                    Some(bind_instrument(&view, program)?)
                }
                _ => None,
            };
            let record = NodeRecord {
                id: node.id.clone(),
                source,
                processor_id: processor.identity().into(),
                latency_frames: processor.technical_latency_frames(),
                processor,
                initial_params: params
                    .iter()
                    .map(|(name, value)| (name.clone(), exact(value)))
                    .collect(),
                state_mode: "reset".into(),
                instrument,
            };
            let size = bounded_json_size(&record, remaining)?;
            remaining = remaining.checked_sub(size + 1).ok_or_else(|| {
                fail(
                    DiagnosticCode::ResourceLimit,
                    "processor context exceeds 4 MiB",
                )
            })?;
            nodes.push(record);
        }
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        Self::from_record(ContextRecord {
            format: FORMAT.into(),
            version: VERSION,
            sample_rate_hz: view.output.sample_rate_hz,
            reset_origin_q: exact(view.output.score_origin_q()),
            latency_compensation: "none".into(),
            nodes,
        })
    }

    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, Diagnostics> {
        if bytes.len() > MAX_NATIVE_PROCESSOR_CONTEXT_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "processor context exceeds 4 MiB",
            ));
        }
        let record: ContextRecord = serde_json::from_slice(bytes).map_err(|e| {
            fail(
                DiagnosticCode::Syntax,
                format!("invalid processor context: {e}"),
            )
        })?;
        validate_record(&record)?;
        if encode(&record)? != bytes {
            return Err(fail(
                DiagnosticCode::Syntax,
                "processor context is not canonical JSON",
            ));
        }
        Ok(Self {
            record,
            bytes: bytes.to_vec(),
        })
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn digest(&self) -> String {
        sha256_digest(&self.bytes)
    }

    pub(crate) fn bytes_len(&self) -> usize {
        self.bytes.len()
    }

    /// Canonical comparison projection for the exact initial values authorized
    /// by a selective node freeze. The projection is comparison data, not an
    /// independently loadable processor record.
    pub(crate) fn projected_for_mask(
        &self,
        mask: &BTreeMap<String, &'static str>,
    ) -> Result<Vec<u8>, Diagnostics> {
        let mut record = self.record.clone();
        for (id, parameter) in mask {
            let node = record
                .nodes
                .iter_mut()
                .find(|node| &node.id == id)
                .ok_or_else(|| {
                    fail(
                        DiagnosticCode::Reference,
                        format!("masked node `{id}` is missing"),
                    )
                })?;
            let approved = matches!(
                (&node.processor, *parameter),
                (
                    NativeProcessor::Core {
                        processor: Processor::Gain { .. }
                    },
                    "gain"
                ) | (
                    NativeProcessor::Core {
                        processor: Processor::Pan
                    },
                    "pan"
                )
            );
            if !approved || !node.initial_params.contains_key(*parameter) {
                return Err(fail(
                    DiagnosticCode::Capability,
                    format!("node `{id}` has no approved `{parameter}` initial value"),
                ));
            }
            node.initial_params
                .insert((*parameter).into(), "<masked>".into());
        }
        encode(&record)
    }

    fn from_record(record: ContextRecord) -> Result<Self, Diagnostics> {
        validate_record(&record)?;
        let bytes = encode(&record)?;
        if bytes.len() > MAX_NATIVE_PROCESSOR_CONTEXT_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "processor context exceeds 4 MiB",
            ));
        }
        Ok(Self { record, bytes })
    }
}

fn bind_instrument(view: &PlanView<'_>, id: &str) -> Result<InstrumentBinding, Diagnostics> {
    let resources = view.instruments.ok_or_else(|| {
        fail(
            DiagnosticCode::Reference,
            format!("instrument `{id}` has no resources"),
        )
    })?;
    let mut program = view.instrument_program(id).cloned().ok_or_else(|| {
        fail(
            DiagnosticCode::Reference,
            format!("instrument program `{id}` is missing"),
        )
    })?;
    program.source.span = None;
    let table_ids: BTreeSet<&str> = program
        .voice
        .nodes
        .iter()
        .chain(program.shared.iter().flat_map(|graph| graph.nodes.iter()))
        .filter_map(|node| match &node.processor {
            GraphProcessor::Wavetable { table } => Some(table.as_str()),
            _ => None,
        })
        .collect();
    let mut wavetables = Vec::new();
    for id in table_ids {
        let table = resources
            .wavetables
            .iter()
            .find(|table| table.id == id)
            .ok_or_else(|| {
                fail(
                    DiagnosticCode::Reference,
                    format!("wavetable `{id}` is missing"),
                )
            })?;
        let source = resources
            .wavetable_sources
            .iter()
            .find(|source| source.table == id)
            .ok_or_else(|| {
                fail(
                    DiagnosticCode::Reference,
                    format!("wavetable source `{id}` is missing"),
                )
            })?;
        wavetables.push(WavetableBinding {
            source: source.clone(),
            cycle_length: table.cycle_length,
            sample_count: table.samples.len(),
        });
    }
    Ok(InstrumentBinding {
        program,
        wavetables,
    })
}

fn defaults_for(processor: ProcessorView<'_>) -> BTreeMap<String, Rational> {
    let fraction = |n: i64, d: i64| Rational::new(BigInt::from(n), BigInt::from(d));
    match processor {
        ProcessorView::Core(Processor::Sine { .. }) => BTreeMap::from([
            ("attack".into(), fraction(1, 200)),
            ("release".into(), fraction(2, 25)),
            ("level".into(), fraction(1, 5)),
        ]),
        ProcessorView::Core(Processor::OnePole { .. }) => {
            BTreeMap::from([("cutoff".into(), fraction(1000, 1))])
        }
        ProcessorView::Core(Processor::Gain { .. }) => {
            BTreeMap::from([("gain".into(), fraction(1, 1))])
        }
        ProcessorView::Core(Processor::Fader { .. }) => {
            BTreeMap::from([("level".into(), fraction(0, 1))])
        }
        ProcessorView::Core(Processor::Pan) => BTreeMap::from([("pan".into(), fraction(0, 1))]),
        ProcessorView::Core(
            processor @ (Processor::Eq { .. }
            | Processor::Compressor { .. }
            | Processor::Reverb { .. }),
        ) => crate::plan::production_defaults(processor),
        ProcessorView::Kit { .. } => BTreeMap::from([("level".into(), fraction(1, 1))]),
        ProcessorView::Constant => BTreeMap::from([("value".into(), fraction(0, 1))]),
        _ => BTreeMap::new(),
    }
}

fn validate_record(record: &ContextRecord) -> Result<(), Diagnostics> {
    if record.format != FORMAT || record.version != VERSION {
        return Err(fail(
            DiagnosticCode::Version,
            "unsupported processor context format",
        ));
    }
    if record.sample_rate_hz == 0 || record.latency_compensation != "none" {
        return Err(fail(
            DiagnosticCode::Range,
            "invalid processor timing context",
        ));
    }
    validate_exact(&record.reset_origin_q)?;
    let mut previous: Option<&str> = None;
    for node in &record.nodes {
        if node.id.is_empty() || previous.is_some_and(|id| id >= node.id.as_str()) {
            return Err(fail(
                DiagnosticCode::DuplicateId,
                "processor nodes are not in unique ID order",
            ));
        }
        previous = Some(&node.id);
        if node.processor_id != node.processor.identity()
            || node.latency_frames != node.processor.technical_latency_frames()
            || node.state_mode != "reset"
        {
            return Err(fail(
                DiagnosticCode::Range,
                format!("invalid processor metadata on `{}`", node.id),
            ));
        }
        if node
            .source
            .as_ref()
            .is_some_and(|source| source.object.is_empty() || source.path.is_empty())
        {
            return Err(fail(
                DiagnosticCode::Reference,
                format!("invalid source on `{}`", node.id),
            ));
        }
        for value in node.initial_params.values() {
            validate_exact(value)?;
        }
        let required = match &node.processor {
            NativeProcessor::Core { processor } => defaults_for(ProcessorView::Core(processor)),
            NativeProcessor::Kit { .. } => defaults_for(ProcessorView::Kit {
                channels: 1,
                voices: 1,
                samples: &[],
            }),
            NativeProcessor::Constant {} => defaults_for(ProcessorView::Constant),
            _ => BTreeMap::new(),
        };
        if required
            .keys()
            .any(|name| !node.initial_params.contains_key(name))
        {
            return Err(fail(
                DiagnosticCode::Reference,
                format!("processor `{}` lacks an initial parameter", node.id),
            ));
        }
        let valid_parameters = |name: &String| match &node.processor {
            NativeProcessor::Core {
                processor: Processor::Instrument { .. },
            } => node
                .instrument
                .as_ref()
                .is_some_and(|binding| binding.program.controls.contains_key(name)),
            NativeProcessor::Core { processor } => processor.parameter_allowed(name),
            NativeProcessor::Kit { .. } => name == "level",
            NativeProcessor::Constant {} => name == "value",
            _ => false,
        };
        if node
            .initial_params
            .keys()
            .any(|name| !valid_parameters(name))
        {
            return Err(fail(
                DiagnosticCode::UnknownField,
                format!("processor `{}` has an unknown initial parameter", node.id),
            ));
        }
        match (&node.processor, &node.instrument) {
            (
                NativeProcessor::Core {
                    processor: Processor::Instrument { program, .. },
                },
                Some(binding),
            ) if binding.program.id == *program => {
                binding.program.validate().map_err(|e| {
                    fail(
                        DiagnosticCode::Range,
                        format!("invalid bound instrument: {e}"),
                    )
                })?;
                if binding.program.source.span.is_some()
                    || binding
                        .program
                        .controls
                        .keys()
                        .any(|name| !node.initial_params.contains_key(name))
                {
                    return Err(fail(
                        DiagnosticCode::Range,
                        "instrument binding has a source span or missing control",
                    ));
                }
                let mut ids = BTreeSet::new();
                for table in &binding.wavetables {
                    if !ids.insert(&table.source.table)
                        || table.source.table.is_empty()
                        || table.source.file.is_empty()
                        || table.source.object.is_empty()
                        || table.source.path.is_empty()
                        || !(8..=2_048).contains(&table.cycle_length)
                        || !table.cycle_length.is_power_of_two()
                        || table.sample_count == 0
                        || table.sample_count > crate::wavetable::MAX_WAVETABLE_SAMPLES
                        || table.sample_count % table.cycle_length as usize != 0
                    {
                        return Err(fail(DiagnosticCode::Range, "invalid wavetable binding"));
                    }
                    validate_hash_pin(&table.source.hash, "wavetable hash")?;
                }
                let required_tables: BTreeSet<&str> = binding
                    .program
                    .voice
                    .nodes
                    .iter()
                    .chain(
                        binding
                            .program
                            .shared
                            .iter()
                            .flat_map(|graph| graph.nodes.iter()),
                    )
                    .filter_map(|graph_node| match &graph_node.processor {
                        GraphProcessor::Wavetable { table } => Some(table.as_str()),
                        _ => None,
                    })
                    .collect();
                if required_tables != ids.into_iter().map(String::as_str).collect() {
                    return Err(fail(
                        DiagnosticCode::Reference,
                        "instrument wavetable binding is incomplete",
                    ));
                }
            }
            (
                NativeProcessor::Core {
                    processor: Processor::Instrument { .. },
                },
                _,
            ) => {
                return Err(fail(
                    DiagnosticCode::Reference,
                    "missing instrument binding",
                ));
            }
            (_, Some(_)) => {
                return Err(fail(
                    DiagnosticCode::Capability,
                    "unexpected instrument binding",
                ))
            }
            _ => {}
        }
        if let NativeProcessor::Audio { clip } = &node.processor {
            if clip.source.span.is_some() {
                return Err(fail(
                    DiagnosticCode::Range,
                    "processor context contains a source span",
                ));
            }
        }
        if let NativeProcessor::WarpRate { clip } = &node.processor {
            if clip.source.span.is_some() {
                return Err(fail(
                    DiagnosticCode::Range,
                    "processor context contains a source span",
                ));
            }
        }
    }
    Ok(())
}

fn validate_exact(value: &str) -> Result<(), Diagnostics> {
    let parsed = parse_rational(value).map_err(|_| {
        fail(
            DiagnosticCode::Syntax,
            "processor context has invalid rational",
        )
    })?;
    if exact(&parsed) != value {
        return Err(fail(
            DiagnosticCode::Syntax,
            "processor context has noncanonical rational",
        ));
    }
    Ok(())
}

fn exact(value: &Rational) -> String {
    format!("{}/{}", value.numer(), value.denom())
}

fn encode(record: &ContextRecord) -> Result<Vec<u8>, Diagnostics> {
    let value = serde_json::to_value(record).map_err(|e| {
        fail(
            DiagnosticCode::Syntax,
            format!("cannot encode processor context: {e}"),
        )
    })?;
    let bytes = canonical_json_bytes(&value).map_err(|e| {
        fail(
            DiagnosticCode::ResourceLimit,
            format!("cannot canonicalize processor context: {e}"),
        )
    })?;
    if bytes.len() > MAX_NATIVE_PROCESSOR_CONTEXT_BYTES {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "processor context exceeds 4 MiB",
        ));
    }
    Ok(bytes)
}

fn bounded_json_size(value: &impl Serialize, limit: usize) -> Result<usize, Diagnostics> {
    struct Counter {
        size: usize,
        limit: usize,
    }

    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let next = self
                .size
                .checked_add(bytes.len())
                .filter(|size| *size <= self.limit);
            match next {
                Some(size) => {
                    self.size = size;
                    Ok(bytes.len())
                }
                None => Err(io::Error::other("processor context size limit")),
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let mut counter = Counter { size: 0, limit };
    serde_json::to_writer(&mut counter, value).map_err(|_| {
        fail(
            DiagnosticCode::ResourceLimit,
            "processor context exceeds 4 MiB",
        )
    })?;
    Ok(counter.size)
}

fn fail(code: DiagnosticCode, message: impl Into<String>) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(Diagnostic::error(code, message, None));
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::disk_media::DiskMediaProject;
    use crate::plan::PlanLimits;

    fn capture(source: &str) -> NativeProcessorContext {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("main.maac");
        fs::write(&entry, source).unwrap();
        let project = DiskMediaProject::load(&entry, temp.path()).unwrap();
        let plan = project.build_with_limits(&PlanLimits::default()).unwrap();
        NativeProcessorContext::capture(&plan).unwrap()
    }

    const GRAPH: &str = r#"maac 1;
project p { score=[0q,1/10q]; tail=0s; rate=48000Hz; tempo=&t; meter=&m; output=&gain:out; }
tempo t { points=[(0q,120bpm,step)]; }
meter m { points=[(0q,4,4)]; }
node sound { type="core.sine/1"; }
node delay { type="core.delay/1"; config={channels=1;frames=16;}; }
node gain { type="core.gain/1"; config={channels=1;}; params={gain=3/4;}; }
connect a { from=&sound:out; to=&delay:in; }
connect b { from=&delay:out; to=&gain:in; }
"#;

    #[test]
    fn canonical_context_captures_defaults_latency_and_exact_mask() {
        let context = capture(GRAPH);
        assert!(context.bytes_len() < MAX_NATIVE_PROCESSOR_CONTEXT_BYTES);
        assert_eq!(
            NativeProcessorContext::from_bytes(context.bytes())
                .unwrap()
                .digest(),
            context.digest()
        );
        let value: serde_json::Value = serde_json::from_slice(context.bytes()).unwrap();
        assert_eq!(value["sample_rate_hz"], 48_000);
        assert_eq!(value["reset_origin_q"], "0/1");
        assert_eq!(value["latency_compensation"], "none");
        let nodes = value["nodes"].as_array().unwrap();
        assert_eq!(
            nodes.iter().find(|node| node["id"] == "delay").unwrap()["latency_frames"],
            16
        );
        assert_eq!(
            nodes.iter().find(|node| node["id"] == "sound").unwrap()["initial_params"]["attack"],
            "1/200"
        );
        let mask = BTreeMap::from([("gain".into(), "gain")]);
        let altered = capture(&GRAPH.replace("gain=3/4", "gain=1/2"));
        assert_ne!(context.bytes(), altered.bytes());
        assert_eq!(
            context.projected_for_mask(&mask).unwrap(),
            altered.projected_for_mask(&mask).unwrap()
        );
        assert!(context
            .projected_for_mask(&BTreeMap::from([("sound".into(), "gain")]))
            .is_err());
    }

    #[test]
    fn untrusted_context_rejects_noncanonical_and_inconsistent_metadata() {
        let context = capture(GRAPH);
        let mut spaced = context.bytes().to_vec();
        spaced.push(b' ');
        assert!(NativeProcessorContext::from_bytes(&spaced).is_err());
        let mut value: serde_json::Value = serde_json::from_slice(context.bytes()).unwrap();
        value["nodes"][0]["latency_frames"] = 99.into();
        let bad = canonical_json_bytes(&value).unwrap();
        assert!(NativeProcessorContext::from_bytes(&bad).is_err());
        value["nodes"][0]["latency_frames"] = 0.into();
        value["nodes"][0]["unexpected"] = true.into();
        let bad = canonical_json_bytes(&value).unwrap();
        assert!(NativeProcessorContext::from_bytes(&bad).is_err());
        assert!(NativeProcessorContext::from_bytes(&vec![
            b' ';
            MAX_NATIVE_PROCESSOR_CONTEXT_BYTES + 1
        ])
        .is_err());
    }

    #[test]
    fn local_instrument_binding_contains_resolved_graph_and_controls_without_pcm() {
        let source = r#"maac 1;
project p { score=[0q,1/10q]; tail=0s; rate=48000Hz; tempo=&t; meter=&m; output=&lead:out; }
tempo t { points=[(0q,120bpm,step)]; }
meter m { points=[(0q,4,4)]; }
instrument tone { channels=1;
  voice v { channels=1; amplitude=&amp; output=&osc:out;
    node amp { type="synth.adsr/1"; }
    node osc { type="synth.sine/1"; }
  }
  control release { target=&v.amp.params.release; default=100ms; }
}
node lead { instrument=&tone; params={release=200ms;}; }
"#;
        let context = capture(source);
        let value: serde_json::Value = serde_json::from_slice(context.bytes()).unwrap();
        let binding = &value["nodes"][0]["instrument"];
        assert!(
            binding["program"]["voice"]["nodes"]
                .as_array()
                .unwrap()
                .len()
                >= 2
        );
        assert_eq!(binding["program"]["controls"]["release"]["default"], "1/10");
        assert_eq!(value["nodes"][0]["initial_params"]["release"], "1/5");
        assert!(binding["program"]["source"].get("span").is_none());
        assert!(binding["wavetables"].as_array().unwrap().is_empty());
    }

    #[test]
    fn mixed_native_variants_are_captured_without_embedded_pcm() {
        let temp = tempfile::tempdir().unwrap();
        let pcm: Vec<u8> = [1.0f32, 0.0, -1.0]
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        fs::write(temp.path().join("sample.pcm"), &pcm).unwrap();
        let source = format!(
            r#"maac 1;
project p {{ score=[0q,1q]; tail=0s; rate=48000Hz; tempo=&t; meter=&m; output=&kit:out; }}
tempo t {{ points=[(0q,120bpm,step)]; }}
meter m {{ points=[(0q,4,4)]; }}
asset sample {{ kind=audio; path="sample.pcm"; hash="{}"; format="pcm_f32le_interleaved/1"; rate=48000Hz; channels=1; frames=3; }}
node kit {{ type="core.kit/1"; config={{channels=1;samples=[{{key="k";asset=&sample;}}];}}; }}
node lfo {{ type="core.lfo/1"; config={{period=1s;wave=triangle;}}; }}
node constant {{ type="core.constant/1"; params={{value=-1/4;}}; }}
audio rateclip {{ asset=&sample; at=0q; source=[0frame,3frame]; mode=rate; }}
audio warpclip {{ asset=&sample; at=0q; source=[0frame,3frame]; mode=warp_rate; warp=[(0q,0frame),(1q,3frame)]; }}
"#,
            sha256_digest(&pcm)
        );
        let entry = temp.path().join("main.maac");
        fs::write(&entry, source).unwrap();
        let project = DiskMediaProject::load(&entry, temp.path()).unwrap();
        let plan = project.build_with_limits(&PlanLimits::default()).unwrap();
        let context = NativeProcessorContext::capture(&plan).unwrap();
        NativeProcessorContext::from_bytes(context.bytes()).unwrap();
        let value: serde_json::Value = serde_json::from_slice(context.bytes()).unwrap();
        let nodes = value["nodes"].as_array().unwrap();
        let kinds: BTreeSet<&str> = nodes
            .iter()
            .map(|node| node["processor"]["kind"].as_str().unwrap())
            .collect();
        assert_eq!(
            kinds,
            BTreeSet::from(["audio", "constant", "kit", "lfo", "warp_rate"])
        );
        assert!(nodes.iter().all(|node| node["processor"]["clip"]
            .get("source")
            .is_none_or(|source| source.get("span").is_none())));
        assert!(nodes.iter().all(|node| node.get("bytes").is_none()));
        assert_eq!(
            nodes.iter().find(|node| node["id"] == "kit").unwrap()["initial_params"]["level"],
            "1/1"
        );
    }
}
