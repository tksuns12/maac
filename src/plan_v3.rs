//! Version 3 standalone plans retain exact score coordinates for certified ramp timing.
use crate::instrument_plan::InstrumentResources;
use crate::plan::*;
use crate::tempo::{RampTempoMap, TimeValue, TimingBudget};
use num_traits::{ToPrimitive, Zero};
use serde::{de, Deserialize, Deserializer, Serialize};
use std::{cell::RefCell, cmp::Ordering};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AutomationAnchor {
    Score {
        #[serde(with = "rational_serde")]
        q: Rational,
    },
    Seconds {
        #[serde(with = "rational_serde")]
        seconds: Rational,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedEventV3 {
    pub address: String,
    pub source: SourceMapping,
    pub target: EventTarget,
    pub kind: EventKind,
    #[serde(with = "rational_serde")]
    pub score_on_q: Rational,
    #[serde(default, with = "optional_rational_serde")]
    pub score_off_q: Option<Rational>,
    #[serde(with = "rational_serde")]
    pub onset_offset_seconds: Rational,
    #[serde(with = "rational_serde")]
    pub release_offset_seconds: Rational,
    pub release_velocity: f64,
    pub on_frame: u64,
    #[serde(default)]
    pub off_frame: Option<u64>,
    #[serde(default)]
    pub order: i32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationV3 {
    pub id: String,
    pub target: PortRef,
    pub clock: AutomationClock,
    pub at: AutomationAnchor,
    pub points: Vec<AutomationPoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlanV3 {
    pub version: u32,
    pub output: OutputSettings,
    pub tempo: TempoMap,
    #[serde(default)]
    pub events: Vec<ResolvedEventV3>,
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub connections: Vec<Connection>,
    #[serde(default)]
    pub automation: Vec<AutomationV3>,
    #[serde(default)]
    pub regions: Vec<Region>,
    #[serde(default)]
    pub source_mappings: Vec<SourceMapping>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruments: Option<InstrumentResources>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub production: Option<crate::production_data::ProductionSettings>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanV3Wire {
    version: u32,
    output: OutputSettings,
    tempo: TempoMap,
    #[serde(default)]
    events: Vec<ResolvedEventV3>,
    #[serde(default)]
    nodes: Vec<Node>,
    #[serde(default)]
    connections: Vec<Connection>,
    #[serde(default)]
    automation: Vec<AutomationV3>,
    #[serde(default)]
    regions: Vec<Region>,
    #[serde(default)]
    source_mappings: Vec<SourceMapping>,
    #[serde(default)]
    instruments: Option<InstrumentResources>,
    #[serde(default)]
    production: Option<crate::production_data::ProductionSettings>,
}

impl From<PlanV3Wire> for PlanV3 {
    fn from(value: PlanV3Wire) -> Self {
        Self {
            version: value.version,
            output: value.output,
            tempo: value.tempo,
            events: value.events,
            nodes: value.nodes,
            connections: value.connections,
            automation: value.automation,
            regions: value.regions,
            source_mappings: value.source_mappings,
            instruments: value.instruments,
            production: value.production,
        }
    }
}

impl<'de> Deserialize<'de> for PlanV3 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let plan = Self::from(PlanV3Wire::deserialize(deserializer)?);
        plan.validate().map_err(de::Error::custom)?;
        Ok(plan)
    }
}
impl PlanV3 {
    pub(crate) fn view(&self) -> PlanView<'_> {
        PlanView {
            version: self.version,
            output: &self.output,
            tempo: &self.tempo,
            events: EventSlice::V3(&self.events),
            automation: AutomationSlice::V3(&self.automation),
            nodes: &self.nodes,
            connections: &self.connections,
            regions: &self.regions,
            source_mappings: &self.source_mappings,
            instruments: self.instruments.as_ref(),
            production: self.production.as_ref(),
        }
    }
    pub(crate) fn preflight_with_limits(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        self.view().preflight_timing(&limits.bounded())
    }
    pub fn validate(&self) -> Result<(), PlanError> {
        self.validate_with_limits(&PlanLimits::default())
    }
    pub fn validate_with_limits(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        if self.version != 3 {
            return Err(err("E_VERSION", "version", "PlanV3 requires version 3"));
        }
        self.view().validate_with_limits(limits)
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self, PlanError> {
        Self::from_json_with_limits(bytes, &PlanLimits::default())
    }
    pub fn from_json_with_limits(bytes: &[u8], limits: &PlanLimits) -> Result<Self, PlanError> {
        let limits = limits.bounded();
        check_bytes(bytes, &limits)?;
        let plan = Self::from(serde_json::from_slice::<PlanV3Wire>(bytes).map_err(json_error)?);
        plan.validate_with_limits(&limits)?;
        Ok(plan)
    }
    pub fn from_json_str(input: &str) -> Result<Self, PlanError> {
        Self::from_json(input.as_bytes())
    }
    pub fn from_json_str_with_limits(input: &str, limits: &PlanLimits) -> Result<Self, PlanError> {
        Self::from_json_with_limits(input.as_bytes(), limits)
    }
    pub fn to_json(&self) -> Result<Vec<u8>, PlanError> {
        self.to_json_with_limits(&PlanLimits::default())
    }
    pub fn to_json_with_limits(&self, limits: &PlanLimits) -> Result<Vec<u8>, PlanError> {
        let limits = limits.bounded();
        self.validate_with_limits(&limits)?;
        let bytes = serde_json::to_vec(self).map_err(json_error)?;
        check_bytes(&bytes, &limits)?;
        Ok(bytes)
    }
    pub fn to_json_string(&self) -> Result<String, PlanError> {
        self.to_json_string_with_limits(&PlanLimits::default())
    }
    pub fn to_json_string_with_limits(&self, limits: &PlanLimits) -> Result<String, PlanError> {
        String::from_utf8(self.to_json_with_limits(limits)?)
            .map_err(|e| err("E_SYNTAX", "json", e.to_string()))
    }
    pub fn audio_output_channels(&self, output: &PortRef) -> Result<u8, PlanError> {
        self.view().audio_output_channels(output)
    }
    pub fn instrument_program(&self, id: &str) -> Option<&crate::graph::InstrumentProgram> {
        self.view().instrument_program(id)
    }
    pub fn instrument_control_spec(
        &self,
        node: &Node,
        control: &str,
    ) -> Option<crate::graph::ParameterSpec> {
        self.view().instrument_control_spec(node, control)
    }
    pub fn resolved_node_params(
        &self,
        node: &Node,
    ) -> Result<std::collections::BTreeMap<String, Rational>, PlanError> {
        self.view().resolved_node_params(node)
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum VersionedPlan {
    Legacy(Plan),
    V3(PlanV3),
}
impl Serialize for VersionedPlan {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Legacy(plan) => plan.serialize(serializer),
            Self::V3(plan) => plan.serialize(serializer),
        }
    }
}
impl VersionedPlan {
    pub fn from_json(bytes: &[u8]) -> Result<Self, PlanError> {
        Self::from_json_with_limits(bytes, &PlanLimits::default())
    }
    pub fn from_json_with_limits(bytes: &[u8], limits: &PlanLimits) -> Result<Self, PlanError> {
        let limits = limits.bounded();
        check_bytes(bytes, &limits)?;
        #[derive(Deserialize)]
        struct Probe {
            version: u32,
        }
        match serde_json::from_slice::<Probe>(bytes)
            .map_err(json_error)?
            .version
        {
            1 | 2 => Plan::from_json_with_limits(bytes, &limits).map(Self::Legacy),
            3 => PlanV3::from_json_with_limits(bytes, &limits).map(Self::V3),
            _ => Err(err(
                "E_VERSION",
                "version",
                "unsupported performance-plan version",
            )),
        }
    }
    pub fn to_json(&self) -> Result<Vec<u8>, PlanError> {
        self.to_json_with_limits(&PlanLimits::default())
    }
    pub fn to_json_with_limits(&self, limits: &PlanLimits) -> Result<Vec<u8>, PlanError> {
        match self {
            Self::Legacy(p) => p.to_json_with_limits(limits),
            Self::V3(p) => p.to_json_with_limits(limits),
        }
    }
}
fn check_bytes(bytes: &[u8], limits: &PlanLimits) -> Result<(), PlanError> {
    if bytes.len() > limits.max_json_bytes {
        Err(err(
            "E_RESOURCE_LIMIT",
            "json",
            "plan JSON exceeds byte allowance",
        ))
    } else {
        Ok(())
    }
}
fn json_error(e: serde_json::Error) -> PlanError {
    err(serde_error_code(&e.to_string()), "json", e.to_string())
}
fn time_error(e: crate::music::MusicError) -> PlanError {
    err(e.code.as_str(), "timing", e.message)
}

/// Relative exact recipes avoid subtracting huge absolute clock approximations.
pub(crate) struct TimingContext {
    pub(crate) map: RampTempoMap,
    pub(crate) origin: Rational,
    pub(crate) end: TimeValue,
    pub(crate) duration: TimeValue,
    zero: TimeValue,
    score_end: Rational,
    budget: RefCell<TimingBudget>,
}
impl TimingContext {
    pub(crate) fn new_with_limits(
        tempo: &TempoMap,
        output: &OutputSettings,
        limits: &PlanLimits,
    ) -> Result<Self, PlanError> {
        let points = tempo
            .points
            .iter()
            .map(|p| {
                let shape = match p.shape {
                    Interpolation::Step => crate::music::TempoShape::Step,
                    Interpolation::Linear => crate::music::TempoShape::Linear,
                    Interpolation::Exponential => {
                        return Err(err("E_TEMPO", "tempo", "exponential tempo is invalid"))
                    }
                };
                Ok(crate::music::TempoPoint::new(
                    p.q.clone(),
                    p.bpm.clone(),
                    shape,
                ))
            })
            .collect::<Result<Vec<_>, PlanError>>()?;
        let map = RampTempoMap::new(points).map_err(time_error)?;
        let end = map
            .seconds_between(&output.score_start_q, &output.score_end_q)
            .map_err(time_error)?;
        let duration = end.add_offset(&output.tail_seconds).map_err(time_error)?;
        Ok(Self {
            score_end: output.score_end_q.clone(),
            map,
            origin: output.score_start_q.clone(),
            end,
            duration,
            zero: TimeValue::from_rational(Rational::zero()).map_err(time_error)?,
            budget: RefCell::new(TimingBudget::new(limits.max_work)),
        })
    }
    pub(crate) fn compare_times(
        &self,
        a: &TimeValue,
        b: &TimeValue,
    ) -> Result<Ordering, PlanError> {
        a.compare_with_budget(b, &mut self.budget.borrow_mut())
            .map_err(time_error)
    }
    pub(crate) fn ceil_time(
        &self,
        time: &TimeValue,
        rate: u64,
    ) -> Result<num_bigint::BigInt, PlanError> {
        time.ceil_frames_with_budget(rate, &mut self.budget.borrow_mut())
            .map_err(time_error)
    }
    pub(crate) fn approximate_time(&self, time: &TimeValue) -> Result<f64, PlanError> {
        time.approximate_seconds_with_budget(&mut self.budget.borrow_mut())
            .map_err(time_error)
    }
    fn frame(&self, time: &TimeValue, rate: u64) -> Result<u64, PlanError> {
        self.ceil_time(time, rate)?
            .to_u64()
            .ok_or_else(|| err("E_TIME_PRECISION", "timing", "frame ceiling out of range"))
    }
    pub(crate) fn relative_at_score(
        &self,
        q: &Rational,
        offset: &Rational,
    ) -> Result<TimeValue, PlanError> {
        self.map
            .seconds_between(&self.origin, q)
            .and_then(|t| t.add_offset(offset))
            .map_err(time_error)
    }
    pub(crate) fn frame_at_score(
        &self,
        q: &Rational,
        offset: &Rational,
        rate: u64,
    ) -> Result<num_bigint::BigInt, PlanError> {
        self.ceil_time(&self.relative_at_score(q, offset)?, rate)
    }
    pub(crate) fn relative_at_seconds(&self, seconds: &Rational) -> Result<TimeValue, PlanError> {
        self.map
            .seconds_between(&self.origin, &Rational::zero())
            .and_then(|t| t.add_offset(seconds))
            .map_err(time_error)
    }
    pub(crate) fn frame_at_seconds(
        &self,
        seconds: &Rational,
        rate: u64,
    ) -> Result<num_bigint::BigInt, PlanError> {
        self.ceil_time(&self.relative_at_seconds(seconds)?, rate)
    }
    pub(crate) fn validate_automation(
        &self,
        lane: &AutomationView<'_>,
        rate: u64,
    ) -> Result<(), PlanError> {
        for point in lane.points {
            match (lane.clock, lane.anchor) {
                (AutomationClock::Score, AutomationAnchorView::Score(q)) => {
                    self.frame_at_score(&(q + &point.position), &Rational::zero(), rate)?;
                }
                (AutomationClock::Seconds, AutomationAnchorView::Score(q)) => {
                    self.frame_at_score(q, &point.position, rate)?;
                }
                (AutomationClock::Seconds, AutomationAnchorView::Seconds(s)) => {
                    self.frame_at_seconds(&(s + &point.position), rate)?;
                }
                _ => {
                    return Err(err(
                        "E_INTERVAL",
                        "automation.at",
                        "score clock requires score anchor",
                    ))
                }
            }
        }
        Ok(())
    }
    pub(crate) fn duration_frames(
        &self,
        output: &OutputSettings,
        limits: &PlanLimits,
    ) -> Result<u64, PlanError> {
        let max =
            TimeValue::from_rational(Rational::from_integer(limits.max_duration_seconds.into()))
                .map_err(time_error)?;
        if self.compare_times(&self.duration, &max)? == Ordering::Greater {
            return Err(err(
                "E_RESOURCE_LIMIT",
                "output",
                "render duration exceeds limit",
            ));
        }
        self.frame(&self.duration, u64::from(output.sample_rate_hz))
    }
    pub(crate) fn validate_output(
        &self,
        output: &OutputSettings,
        limits: &PlanLimits,
    ) -> Result<(), PlanError> {
        let expected = self.duration_frames(output, limits)?;
        if expected != output.total_frames {
            return Err(err(
                "E_INTERVAL",
                "output.total_frames",
                format!("expected {expected} frames, found {}", output.total_frames),
            ));
        }
        Ok(())
    }
    pub(crate) fn schedule_view(
        &self,
        event: &EventView<'_>,
        rate: u64,
    ) -> Result<(u64, Option<u64>), PlanError> {
        if event.score_on_q < &self.origin
            || event.score_on_q >= &self.score_end
            || event
                .score_off_q
                .as_ref()
                .is_some_and(|q| q <= event.score_on_q)
        {
            return Err(err(
                "E_INTERVAL",
                "events",
                "score onset or gate outside valid interval",
            ));
        }
        let on = self.relative_at_score(event.score_on_q, event.onset_offset_seconds)?;
        if self.compare_times(&on, &self.zero)? == Ordering::Less
            || self.compare_times(&on, &self.end)? != Ordering::Less
        {
            return Err(err(
                "E_INTERVAL",
                "events",
                "resolved onset lies outside score interval",
            ));
        }
        let on_frame = self.frame(&on, rate)?;
        let off = if let Some(q) = event.score_off_q {
            let candidate = self.relative_at_score(q, event.release_offset_seconds)?;
            let off = if self.compare_times(&candidate, &self.end)? == Ordering::Greater {
                &self.end
            } else {
                &candidate
            };
            if self.compare_times(off, &on)? != Ordering::Greater {
                return Err(err(
                    "E_INTERVAL",
                    "events",
                    "effective physical gate must be positive",
                ));
            }
            let off_frame = self.frame(off, rate)?;
            if matches!(event.kind, EventKind::Note { .. }) && on_frame == off_frame {
                return Err(err(
                    "E_SUBSAMPLE_NOTE",
                    "events",
                    "positive note gate collapsed to one frame",
                ));
            }
            Some(off_frame)
        } else {
            None
        };
        Ok((on_frame, off))
    }
    pub(crate) fn schedule(
        &self,
        event: &ResolvedEventV3,
        rate: u64,
    ) -> Result<(u64, Option<u64>), PlanError> {
        self.schedule_view(&EventView::from(event), rate)
    }
}
