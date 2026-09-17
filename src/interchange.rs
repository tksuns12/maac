//! Interchange adapters and explicit MaaC/1 §25 loss reporting.

use crate::plan::{EventKind, Interpolation, ProcessorView, Rational};
use crate::PlanArtifact;
use num_traits::ToPrimitive;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashMap};
use std::fmt;

pub const LOSS_REPORT_FORMAT: &str = "maac.interchange-loss-report";
pub const LOSS_REPORT_VERSION: u32 = 1;
pub const MIDI_ADAPTER_ID: &str = "maac.adapter.midi1-smf/1";
pub const MIDI_TARGET_PROFILE: &str =
    "MIDI 1.0 / Standard MIDI File 1.0 format 0 / SMPTE -25 fps, 40 ticks/frame";
pub const MAX_LOSS_REPORT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_LOSS_ENTRIES: usize = 4096;
const MAX_SMF_DELTA: u64 = 0x0fff_ffff;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LossDecisionKind {
    Approximation,
    Omission,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LossDecision {
    pub kind: LossDecisionKind,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterchangeLoss {
    pub code: String,
    pub source_paths: Vec<Vec<String>>,
    pub property: String,
    pub output_limitation: String,
    pub decision: LossDecision,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LossReport {
    pub format: String,
    pub version: u32,
    pub adapter_id: String,
    pub target_profile: String,
    pub losses: Vec<InterchangeLoss>,
}

struct UniqueJson(Value);
impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("unique-key interchange loss-report JSON")
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Bool(value)))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueJson(value.into()))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueJson(value.into()))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Err(E::custom("JSON floats are forbidden"))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(value.into())))
            }
            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(value)))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut out = Vec::new();
                while let Some(UniqueJson(value)) = seq.next_element()? {
                    if out.len() > MAX_LOSS_ENTRIES * 256 {
                        return Err(de::Error::custom("loss-report value limit exceeded"));
                    }
                    out.push(value);
                }
                Ok(UniqueJson(Value::Array(out)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut out = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if out.contains_key(&key) {
                        return Err(de::Error::custom(format!("duplicate JSON key `{key}`")));
                    }
                    let UniqueJson(value) = map.next_value()?;
                    out.insert(key, value);
                }
                Ok(UniqueJson(Value::Object(out)))
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}

impl LossReport {
    fn midi(losses: Vec<InterchangeLoss>) -> Self {
        Self {
            format: LOSS_REPORT_FORMAT.into(),
            version: LOSS_REPORT_VERSION,
            adapter_id: MIDI_ADAPTER_ID.into(),
            target_profile: MIDI_TARGET_PROFILE.into(),
            losses,
        }
    }

    pub fn to_json(&self) -> Result<Vec<u8>, AdapterError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|error| AdapterError {
            code: "E_SCHEMA".into(),
            message: error.to_string(),
            loss_report: None,
        })?;
        if bytes.len() > MAX_LOSS_REPORT_BYTES {
            return Err(AdapterError {
                code: "E_RESOURCE_LIMIT".into(),
                message: "loss report exceeds 4 MiB".into(),
                loss_report: None,
            });
        }
        Ok(bytes)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, AdapterError> {
        if bytes.len() > MAX_LOSS_REPORT_BYTES {
            return Err(AdapterError {
                code: "E_RESOURCE_LIMIT".into(),
                message: "loss report exceeds 4 MiB".into(),
                loss_report: None,
            });
        }
        let mut deserializer = serde_json::Deserializer::from_slice(bytes);
        let UniqueJson(value) =
            UniqueJson::deserialize(&mut deserializer).map_err(|error| AdapterError {
                code: "E_SYNTAX".into(),
                message: error.to_string(),
                loss_report: None,
            })?;
        deserializer.end().map_err(|error| AdapterError {
            code: "E_SYNTAX".into(),
            message: error.to_string(),
            loss_report: None,
        })?;
        let report: Self = serde_json::from_value(value).map_err(|error| AdapterError {
            code: "E_SCHEMA".into(),
            message: error.to_string(),
            loss_report: None,
        })?;
        report.validate()?;
        Ok(report)
    }

    pub fn validate(&self) -> Result<(), AdapterError> {
        if self.format != LOSS_REPORT_FORMAT || self.version != LOSS_REPORT_VERSION {
            return Err(AdapterError {
                code: "E_CAPABILITY".into(),
                message: "unsupported interchange loss-report format/version".into(),
                loss_report: None,
            });
        }
        if self.adapter_id.is_empty() || self.target_profile.is_empty() {
            return Err(AdapterError {
                code: "E_SCHEMA".into(),
                message: "adapter_id and target_profile must be nonempty".into(),
                loss_report: None,
            });
        }
        if self.adapter_id.len() > 4096 || self.target_profile.len() > 4096 {
            return Err(AdapterError {
                code: "E_RESOURCE_LIMIT".into(),
                message: "adapter_id or target_profile exceeds the string bound".into(),
                loss_report: None,
            });
        }
        if self.losses.len() > MAX_LOSS_ENTRIES {
            return Err(AdapterError {
                code: "E_RESOURCE_LIMIT".into(),
                message: "loss report exceeds 4096 entries".into(),
                loss_report: None,
            });
        }
        for item in &self.losses {
            if item.code.is_empty()
                || item.source_paths.is_empty()
                || item.property.is_empty()
                || item.output_limitation.is_empty()
                || item.decision.detail.is_empty()
            {
                return Err(AdapterError {
                    code: "E_SCHEMA".into(),
                    message: "loss entries require code, source_paths, property, limitation, and decision detail".into(),
                    loss_report: None,
                });
            }
            if item.source_paths.len() > 256
                || item.source_paths.iter().any(|path| {
                    path.is_empty()
                        || path.len() > 64
                        || path
                            .iter()
                            .any(|segment| segment.is_empty() || segment.len() > 4096)
                })
                || [
                    &item.code,
                    &item.property,
                    &item.output_limitation,
                    &item.decision.detail,
                ]
                .into_iter()
                .any(|text| text.len() > 4096)
            {
                return Err(AdapterError {
                    code: "E_RESOURCE_LIMIT".into(),
                    message: "loss entry exceeds path or string bounds".into(),
                    loss_report: None,
                });
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AdapterPolicy {
    pub faithful: bool,
    pub approved_loss_codes: BTreeSet<String>,
}

impl AdapterPolicy {
    pub fn faithful() -> Self {
        Self {
            faithful: true,
            approved_loss_codes: BTreeSet::new(),
        }
    }

    pub fn approve(mut self, code: impl Into<String>) -> Self {
        self.approved_loss_codes.insert(code.into());
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MidiExport {
    pub bytes: Vec<u8>,
    pub loss_report: LossReport,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterError {
    pub code: String,
    pub message: String,
    pub loss_report: Option<Box<LossReport>>,
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for AdapterError {}

#[derive(Clone)]
struct MidiEvent {
    tick: u64,
    class: u8,
    order: i32,
    address: String,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct LossCollector {
    items: Vec<InterchangeLoss>,
    overflowed: bool,
}

impl LossCollector {
    fn push(&mut self, item: InterchangeLoss) {
        if self.items.len() < MAX_LOSS_ENTRIES {
            self.items.push(item);
        } else {
            self.overflowed = true;
        }
    }

    fn overflowed(&self) -> bool {
        self.overflowed
    }

    fn into_vec(self) -> Vec<InterchangeLoss> {
        self.items
    }
}

impl std::ops::Deref for LossCollector {
    type Target = Vec<InterchangeLoss>;

    fn deref(&self) -> &Self::Target {
        &self.items
    }
}

impl std::ops::DerefMut for LossCollector {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.items
    }
}

fn source_path(source: &crate::plan::SourceMapping) -> Vec<String> {
    let mut path = vec![source.object.clone()];
    path.extend(source.path.iter().cloned());
    path
}

fn loss(
    code: &str,
    paths: Vec<Vec<String>>,
    property: &str,
    limitation: &str,
    kind: LossDecisionKind,
    detail: impl Into<String>,
) -> InterchangeLoss {
    InterchangeLoss {
        code: code.into(),
        source_paths: paths,
        property: property.into(),
        output_limitation: limitation.into(),
        decision: LossDecision {
            kind,
            detail: detail.into(),
        },
    }
}

fn frame_to_tick(frame: u64, rate: u32) -> (u64, bool) {
    let scaled = u128::from(frame) * 1000;
    let rate = u128::from(rate);
    let q = scaled / rate;
    let r = scaled % rate;
    let rounded = if r * 2 >= rate { q + 1 } else { q };
    (u64::try_from(rounded).unwrap_or(u64::MAX), r != 0)
}

fn midi_velocity(value: &Rational) -> u8 {
    let scaled = value * num_bigint::BigInt::from(127u8);
    if scaled.numer().sign() == num_bigint::Sign::Minus {
        return 0;
    }
    let two = num_bigint::BigInt::from(2u8);
    let rounded = (scaled.numer() * &two + scaled.denom()) / (scaled.denom() * two);
    rounded.to_u8().unwrap_or(127).min(127)
}

fn midi_pitch(hz: f64) -> Option<(u8, f64)> {
    if !hz.is_finite() || hz <= 0.0 {
        return None;
    }
    let exact = 69.0 + 12.0 * (hz / 440.0).log2();
    let nearest = exact.round();
    if !(0.0..=127.0).contains(&nearest) {
        return None;
    }
    Some((nearest as u8, (exact - nearest) * 100.0))
}

fn valid_channel_message(bytes: &[u8]) -> bool {
    let Some(&status) = bytes.first() else {
        return false;
    };
    let expected = match status >> 4 {
        0x8 | 0x9 | 0xA | 0xB | 0xE => 3,
        0xC | 0xD => 2,
        _ => return false,
    };
    bytes.len() == expected && bytes[1..].iter().all(|value| *value < 0x80)
}

fn vlq(mut value: u64) -> Vec<u8> {
    let mut out = [0u8; 10];
    let mut i = out.len() - 1;
    out[i] = (value & 0x7f) as u8;
    while {
        value >>= 7;
        value != 0
    } {
        i -= 1;
        out[i] = ((value & 0x7f) as u8) | 0x80;
    }
    out[i..].to_vec()
}

fn smf(events: &[MidiEvent]) -> Result<Vec<u8>, AdapterError> {
    let mut track = Vec::new();
    let mut last = 0u64;
    for event in events {
        let delta = event.tick.checked_sub(last).ok_or_else(|| AdapterError {
            code: "E_INTERVAL".into(),
            message: "MIDI event order moved backwards".into(),
            loss_report: None,
        })?;
        if delta > MAX_SMF_DELTA {
            return Err(AdapterError {
                code: "E_RESOURCE_LIMIT".into(),
                message: "MIDI delta-time exceeds the Standard MIDI File 4-byte VLQ limit".into(),
                loss_report: None,
            });
        }
        track.extend(vlq(delta));
        track.extend_from_slice(&event.bytes);
        last = event.tick;
    }
    track.extend_from_slice(&[0x00, 0xff, 0x2f, 0x00]);
    let len = u32::try_from(track.len()).map_err(|_| AdapterError {
        code: "E_RESOURCE_LIMIT".into(),
        message: "MIDI track exceeds 32-bit SMF chunk length".into(),
        loss_report: None,
    })?;
    let mut out = Vec::with_capacity(track.len() + 22);
    out.extend_from_slice(b"MThd");
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0xe728u16.to_be_bytes());
    out.extend_from_slice(b"MTrk");
    out.extend_from_slice(&len.to_be_bytes());
    out.extend(track);
    Ok(out)
}

/// Export a retained Performance artifact to the initial MIDI adapter profile.
/// The target is explicitly MIDI 1.0 SMF format 0 with a 1ms SMPTE tick. Every
/// semantic reduction is returned in the versioned loss report. Faithful mode
/// rejects any loss whose stable code was not explicitly approved by the caller.
pub fn export_midi1_smf(
    artifact: &PlanArtifact,
    policy: &AdapterPolicy,
) -> Result<MidiExport, AdapterError> {
    artifact.validate().map_err(|error| AdapterError {
        code: error.code.clone(),
        message: error.to_string(),
        loss_report: None,
    })?;
    let view = artifact.view();
    let rate = view.output.sample_rate_hz;
    let mut losses = LossCollector::default();
    let mut midi = Vec::new();
    let mut note_intervals: HashMap<u8, Vec<(u64, u64, Vec<String>)>> = HashMap::new();

    let performance_paths = view
        .events()
        .map(|event| source_path(event.source))
        .collect::<Vec<_>>();
    if !performance_paths.is_empty()
        && view
            .tempo
            .points
            .iter()
            .any(|point| point.shape != Interpolation::Step)
    {
        losses.push(loss(
            "midi.tempo_ramp_discretization",
            performance_paths,
            "tempo",
            "SMF tempo metadata cannot represent MaaC continuous tempo ramps exactly",
            LossDecisionKind::Omission,
            "event timing is retained on the fixed 1ms SMPTE timeline; continuous tempo-ramp metadata is omitted",
        ));
    }

    let audio_nodes = view
        .nodes
        .iter()
        .filter(|node| {
            matches!(
                node.processor,
                ProcessorView::Audio(_) | ProcessorView::WarpRate(_)
            )
        })
        .map(|node| vec![node.id.clone()])
        .collect::<Vec<_>>();
    if !audio_nodes.is_empty() || view.audio_assets.is_some_and(|assets| !assets.is_empty()) {
        let mut audio_paths = audio_nodes;
        if let Some(assets) = view.audio_assets {
            audio_paths.extend(assets.iter().map(|asset| vec![asset.id.clone()]));
        }
        audio_paths.sort();
        audio_paths.dedup();
        losses.push(loss(
            "midi.audio_omission",
            audio_paths,
            "audio",
            "MIDI 1.0 SMF carries no MaaC audio clips or recorded asset payloads",
            LossDecisionKind::Omission,
            "audio graph content is omitted",
        ));
    }

    if view.nodes.len() > 0 || !view.connections.is_empty() {
        losses.push(loss(
            "midi.synthesis_routing_omission",
            view.nodes.iter().map(|node| vec![node.id.clone()]).collect(),
            "synthesis_routing_state",
            "MIDI 1.0 SMF does not preserve MaaC processor implementations, synthesis state, or routing topology",
            LossDecisionKind::Omission,
            "note/message performance is exported without MaaC synthesis and routing state",
        ));
    }

    let automation_paths = view
        .automations()
        .map(|lane| vec![lane.id.clone()])
        .collect::<Vec<_>>();
    if !automation_paths.is_empty() {
        losses.push(loss(
            "midi.automation_omission",
            automation_paths,
            "automation",
            "generic MaaC parameter automation has no target-independent MIDI 1.0 mapping",
            LossDecisionKind::Omission,
            "automation lanes are omitted",
        ));
    }

    for event in view.events() {
        let path = source_path(event.source);
        let (on_tick, on_loss) = frame_to_tick(event.on_frame, rate);
        let mut timing_loss = on_loss;
        match event.kind {
            EventKind::Note {
                pitch_hz,
                velocity,
                pitch_expression,
                gain_expression,
                timbre_expression,
                pressure_expression,
            } => {
                let Some((key, cents)) = midi_pitch(*pitch_hz) else {
                    losses.push(loss(
                        "midi.pitch_range",
                        vec![path],
                        "pitch",
                        "MIDI 1.0 note numbers are limited to 0..127",
                        LossDecisionKind::Omission,
                        "note is omitted because its resolved pitch is outside the target note-number range",
                    ));
                    continue;
                };
                if cents.abs() > 1e-7 {
                    losses.push(loss(
                        "midi.microtonal_pitch",
                        vec![path.clone()],
                        "pitch",
                        "the initial MIDI adapter emits semitone note numbers without per-note pitch-bend allocation",
                        LossDecisionKind::Approximation,
                        format!("resolved pitch is rounded to MIDI note {key} ({cents:+.6} cents difference)"),
                    ));
                }
                if pitch_expression.is_some()
                    || gain_expression.is_some()
                    || timbre_expression.is_some()
                    || pressure_expression.is_some()
                {
                    losses.push(loss(
                        "midi.per_note_expression",
                        vec![path.clone()],
                        "expression",
                        "the initial MIDI 1.0 profile does not allocate independent channels for MaaC per-note expression",
                        LossDecisionKind::Omission,
                        "per-note pitch/gain/timbre/pressure expression is omitted",
                    ));
                }
                let off_frame = event.off_frame.ok_or_else(|| AdapterError {
                    code: "E_INTERVAL".into(),
                    message: format!("note {} lacks a certified off frame", event.address),
                    loss_report: None,
                })?;
                let (off_tick, off_loss) = frame_to_tick(off_frame, rate);
                timing_loss |= off_loss;
                let velocity = midi_velocity(velocity);
                if velocity == 0 {
                    losses.push(loss(
                        "midi.zero_velocity_note",
                        vec![path],
                        "velocity",
                        "MIDI 1.0 note-on velocity zero is interpreted as note-off rather than a silent note-on",
                        LossDecisionKind::Omission,
                        "silent MaaC note is omitted instead of emitting a target note-off at its onset",
                    ));
                    continue;
                }
                midi.push(MidiEvent {
                    tick: on_tick,
                    class: 2,
                    order: event.order,
                    address: event.address.clone(),
                    bytes: vec![0x90, key, velocity],
                });
                let release = (event.release_velocity.clamp(0.0, 1.0) * 127.0).round() as u8;
                midi.push(MidiEvent {
                    tick: off_tick,
                    class: 1,
                    order: event.order,
                    address: event.address.clone(),
                    bytes: vec![0x80, key, release],
                });
                note_intervals
                    .entry(key)
                    .or_default()
                    .push((event.on_frame, off_frame, path));
            }
            EventKind::Hit { .. } => {
                losses.push(loss(
                    "midi.hit_mapping",
                    vec![path],
                    "hit_key",
                    "MaaC kit hit keys have no implicit General MIDI drum-note mapping",
                    LossDecisionKind::Omission,
                    "hit is omitted because no explicit target drum mapping was supplied",
                ));
            }
            EventKind::Message { protocol, bytes } => {
                if protocol == "midi1" && valid_channel_message(bytes) {
                    midi.push(MidiEvent {
                        tick: on_tick,
                        class: 0,
                        order: event.order,
                        address: event.address.clone(),
                        bytes: bytes.clone(),
                    });
                } else {
                    losses.push(loss(
                        "midi.message_protocol",
                        vec![path],
                        "message",
                        "the initial SMF adapter accepts only complete MIDI 1.0 channel messages",
                        LossDecisionKind::Omission,
                        format!("message protocol `{protocol}` or payload shape is not representable by this profile"),
                    ));
                }
            }
        }
        if timing_loss {
            losses.push(loss(
                "midi.timing_resolution",
                vec![source_path(event.source)],
                "timing",
                "the target SMPTE timeline has 1ms tick resolution",
                LossDecisionKind::Approximation,
                "one or more certified event frame boundaries are rounded to the nearest 1ms tick",
            ));
        }
    }

    'keys: for (key, intervals) in &mut note_intervals {
        intervals.sort_by_key(|(on, off, _)| (*on, *off));
        let mut active: Vec<(u64, u64, Vec<String>)> = Vec::new();
        for current in intervals.iter() {
            active.retain(|(_, off, _)| *off > current.0);
            for prior in &active {
                losses.push(loss(
                    "midi.overlapping_same_key_identity",
                    vec![prior.2.clone(), current.2.clone()],
                    "note_identity",
                    "MIDI 1.0 note-off messages do not carry MaaC note identity for overlapping notes on the same channel/key",
                    LossDecisionKind::Approximation,
                    format!("overlapping notes share MIDI key {key} on channel 1; note-off identity may be ambiguous"),
                ));
                if losses.overflowed() {
                    break 'keys;
                }
            }
            active.push(current.clone());
        }
    }

    losses.sort_by(|a, b| {
        (&a.code, &a.source_paths, &a.property, &a.decision.detail).cmp(&(
            &b.code,
            &b.source_paths,
            &b.property,
            &b.decision.detail,
        ))
    });
    if losses.overflowed() {
        return Err(AdapterError {
            code: "E_RESOURCE_LIMIT".into(),
            message: "interchange loss report exceeds 4096 entries".into(),
            loss_report: None,
        });
    }
    let report = LossReport::midi(losses.into_vec());
    report.validate()?;
    if policy.faithful {
        if let Some(unapproved) = report
            .losses
            .iter()
            .find(|item| !policy.approved_loss_codes.contains(&item.code))
        {
            return Err(AdapterError {
                code: "E_CAPABILITY".into(),
                message: format!(
                    "faithful MIDI export requires approval for loss `{}`",
                    unapproved.code
                ),
                loss_report: Some(Box::new(report)),
            });
        }
    }

    midi.sort_by(|a, b| {
        (a.tick, a.class, a.order, a.address.as_bytes()).cmp(&(
            b.tick,
            b.class,
            b.order,
            b.address.as_bytes(),
        ))
    });
    let bytes = smf(&midi)?;
    Ok(MidiExport {
        bytes,
        loss_report: report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_velocity_preserves_large_exact_rationals() {
        let base = num_bigint::BigInt::from(1u8) << 200usize;
        let value = Rational::new(&base + 2u8, (&base << 1usize) + 3u8);
        assert_eq!(midi_velocity(&value), 64);
    }

    #[test]
    fn loss_collection_is_bounded_before_report_construction() {
        let mut losses = LossCollector::default();
        for index in 0..=MAX_LOSS_ENTRIES {
            losses.push(loss(
                "test.loss",
                vec![vec![format!("n{index}")]],
                "property",
                "limitation",
                LossDecisionKind::Omission,
                "detail",
            ));
        }
        assert_eq!(losses.len(), MAX_LOSS_ENTRIES);
        assert!(losses.overflowed());
    }

    #[test]
    fn smf_rejects_delta_time_beyond_four_byte_vlq_limit() {
        let event = MidiEvent {
            tick: MAX_SMF_DELTA + 1,
            class: 0,
            order: 0,
            address: "event".into(),
            bytes: vec![0x90, 60, 1],
        };
        let error = smf(&[event]).unwrap_err();
        assert_eq!(error.code, "E_RESOURCE_LIMIT");
        assert!(error.message.contains("4-byte VLQ"));
    }
}
