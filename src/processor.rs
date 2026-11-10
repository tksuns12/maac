//! What each `synth.*/1` processor is, declared once: its identity, its
//! parameters in the order its kernel reads them, where it may be placed,
//! its channels, when it reads its audio input, its work and memory, and the
//! config N(A) writes for it. Lowering, validation, normalization, the
//! work ceiling and the voice compiler read these declarations; only the
//! kernels and their state are specific to each processor.
//!
//! A declaration is plain data. [`GraphProcessor::declaration`] is the one
//! place that maps a processor to its declaration, so a new processor that
//! has none does not compile. The plan bytes of a processor are its serde
//! form in `graph.rs`, not this module, so a declaration cannot change them.

use num_bigint::BigInt;

use crate::graph::{
    bow_sample_work, modal_sample_work, DelayClock, GraphProcessor, GraphStage, GraphUnit as Unit,
    ParameterRate as Rate, ParameterSpec, DEFAULT_NOISE_SEED, MAX_DELAY_SECONDS,
};
use crate::plan::Rational;

/// An exact rational written as two integers, so a declaration is a
/// constant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Exact {
    numerator: i64,
    denominator: i64,
}

pub(crate) const fn int(value: i64) -> Exact {
    Exact {
        numerator: value,
        denominator: 1,
    }
}

pub(crate) const fn exact(numerator: i64, denominator: i64) -> Exact {
    Exact {
        numerator,
        denominator,
    }
}

impl Exact {
    pub(crate) fn rational(self) -> Rational {
        Rational::new(BigInt::from(self.numerator), BigInt::from(self.denominator))
    }
}

/// One end of a parameter's range, which the range includes unless it is
/// open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Bound {
    value: Exact,
    open: bool,
}

pub(crate) const fn closed(value: Exact) -> Bound {
    Bound { value, open: false }
}

pub(crate) const fn open(value: Exact) -> Bound {
    Bound { value, open: true }
}

/// How a parameter differs in a shared graph, which has no note.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InShared {
    Same,
    /// It is read at another rate: an LFO's `phase` at reset, not note-on.
    Rate(Rate),
    /// Its range ends lower: a filter cannot track a note's key.
    Max(Bound),
}

/// A parameter: its name, unit, rate, default and range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Param {
    pub(crate) name: &'static str,
    unit: Unit,
    rate: Rate,
    default: Exact,
    min: Bound,
    max: Bound,
    in_shared: InShared,
}

impl Param {
    pub(crate) const fn new(
        name: &'static str,
        unit: Unit,
        rate: Rate,
        default: Exact,
        min: Bound,
        max: Bound,
    ) -> Self {
        Self {
            name,
            unit,
            rate,
            default,
            min,
            max,
            in_shared: InShared::Same,
        }
    }

    pub(crate) const fn in_shared(self, change: InShared) -> Self {
        Self {
            in_shared: change,
            ..self
        }
    }

    /// The parameter as a node in a `stage` graph has it.
    pub(crate) fn spec(&self, stage: GraphStage) -> ParameterSpec {
        let (rate, max) = match (stage, self.in_shared) {
            (GraphStage::Shared, InShared::Rate(rate)) => (rate, self.max),
            (GraphStage::Shared, InShared::Max(max)) => (self.rate, max),
            _ => (self.rate, self.max),
        };
        ParameterSpec {
            unit: self.unit,
            rate,
            default: self.default.rational(),
            min: self.min.value.rational(),
            max: max.value.rational(),
            min_open: self.min.open,
            max_open: max.open,
        }
    }
}

/// The graphs a processor may be a node of.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Placement {
    /// An instrument's voice graph only: it needs a note.
    InstrumentVoice,
    /// Any graph, voice or shared, of an instrument or an effect.
    AnyGraph,
    /// An effect's graph only.
    Effect,
}

/// A channel count: fixed, or the node's `channels` config.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Channels {
    One,
    Two,
    Config,
}

/// When a node reads its audio input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Read {
    /// In the same sample: the input is a same-sample dependence.
    Now,
    /// From earlier frames, as a delay line does, so a cycle may close
    /// through it.
    Later,
}

/// A processor's audio input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Input {
    None,
    /// Exactly one connection.
    One(Channels, Read),
    /// Any number of connections, summed.
    Many(Channels, Read),
}

/// Per-sample work, in units of a one-unit oscillator node.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Work {
    Units(u64),
    /// One unit per four resonators of its `modes` and `strings`.
    ModalBank,
    /// Per player, a string and, unless `body = none`, a body.
    BowSection,
}

/// The storage a node holds beyond its parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Memory {
    None,
    /// A plucked string's delay line in every voice.
    PluckString,
    /// Two delay lines per player in every voice.
    BowStrings,
    /// A delay line as long as `max`, per channel.
    DelayLine,
}

/// A config value N(A) writes when the source leaves it out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConfigValue {
    Number(i64),
    Boolean(bool),
    Symbol(&'static str),
}

/// A config key N(A) writes with this value when the source leaves it out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ConfigDefault {
    pub(crate) key: &'static str,
    pub(crate) value: ConfigValue,
    /// The language gained the key after the execution hash was defined, so
    /// the hash leaves it out while it holds this value.
    pub(crate) late: bool,
}

const fn default(key: &'static str, value: ConfigValue) -> ConfigDefault {
    ConfigDefault {
        key,
        value,
        late: false,
    }
}

const fn late(key: &'static str, value: ConfigValue) -> ConfigDefault {
    ConfigDefault {
        key,
        value,
        late: true,
    }
}

/// The `config` record N(A) writes for a node.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NormalConfig {
    /// No record: the processor takes no config.
    None,
    /// A record with these defaults for keys the source leaves out. It is
    /// written, and empty, for `synth.timbre/1`, `synth.pressure/1`,
    /// `synth.velocity/1` and `synth.key/1` too, which take no config:
    /// execution hashes and the locks that pin them depend on it.
    Defaults(&'static [ConfigDefault]),
    /// A sample player's defaults, which depend on its zones.
    Sample,
}

/// Everything the crate knows about one processor besides its kernel.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Declaration {
    pub(crate) identity: &'static str,
    /// In the order the kernel reads them: a node's parameter slots.
    pub(crate) params: &'static [Param],
    pub(crate) placement: Placement,
    pub(crate) output: Channels,
    pub(crate) input: Input,
    pub(crate) work: Work,
    pub(crate) memory: Memory,
    pub(crate) config: NormalConfig,
}

impl Declaration {
    pub(crate) fn param(&self, name: &str) -> Option<&'static Param> {
        self.params.iter().find(|param| param.name == name)
    }

    /// The slot of the parameter `name`. In a constant, a name the
    /// declaration lacks fails to compile.
    pub(crate) const fn slot(&self, name: &str) -> usize {
        let mut index = 0;
        while index < self.params.len() {
            if same(self.params[index].name, name) {
                return index;
            }
            index += 1;
        }
        panic!("the declaration has no such parameter")
    }
}

const fn same(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

/// Every declaration. A `synth.delay/1` has one per clock.
pub(crate) const ALL: [&Declaration; 28] = [
    &SINE,
    &SAW,
    &SQUARE,
    &TRIANGLE,
    &NOISE,
    &PLUCK,
    &RANDOM,
    &MODAL,
    &BOW,
    &WAVETABLE,
    &SAMPLE,
    &ADSR,
    &TIMBRE,
    &PRESSURE,
    &VELOCITY,
    &KEY,
    &LFO,
    &GAIN,
    &ONEPOLE,
    &HIGHPASS,
    &SVF,
    &DRIVE,
    &MIX,
    &PAN,
    &BALANCE,
    &WIDTH,
    &DELAY_SECONDS,
    &DELAY_SCORE,
];

/// The declaration of the processor `identity` names; for a delay line, the
/// seconds clock's. Its config defaults are the same on either clock.
pub(crate) fn declaration_of(identity: &str) -> Option<&'static Declaration> {
    ALL.into_iter()
        .find(|declaration| declaration.identity == identity)
}

impl GraphProcessor {
    /// The processor's declaration.
    pub(crate) const fn declaration(&self) -> &'static Declaration {
        match self {
            Self::Sine => &SINE,
            Self::Saw => &SAW,
            Self::Square => &SQUARE,
            Self::Triangle => &TRIANGLE,
            Self::Noise { .. } => &NOISE,
            Self::Pluck { .. } => &PLUCK,
            Self::Random { .. } => &RANDOM,
            Self::Modal { .. } => &MODAL,
            Self::Bow { .. } => &BOW,
            Self::Wavetable { .. } => &WAVETABLE,
            Self::Sample { .. } => &SAMPLE,
            Self::Adsr { .. } => &ADSR,
            Self::Timbre => &TIMBRE,
            Self::Pressure => &PRESSURE,
            Self::Velocity => &VELOCITY,
            Self::Key => &KEY,
            Self::Lfo => &LFO,
            Self::Gain { .. } => &GAIN,
            Self::OnePole { .. } => &ONEPOLE,
            Self::HighPass { .. } => &HIGHPASS,
            Self::Svf { .. } => &SVF,
            Self::Drive { .. } => &DRIVE,
            Self::Mix { .. } => &MIX,
            Self::Pan => &PAN,
            Self::Balance => &BALANCE,
            Self::Width => &WIDTH,
            Self::Delay {
                clock: DelayClock::Seconds,
                ..
            } => &DELAY_SECONDS,
            Self::Delay {
                clock: DelayClock::Score,
                ..
            } => &DELAY_SCORE,
        }
    }

    /// The `channels` of the node's config, if it has one.
    pub(crate) const fn configured_channels(&self) -> Option<u8> {
        match self {
            Self::Bow { channels, .. }
            | Self::Sample { channels, .. }
            | Self::Gain { channels }
            | Self::OnePole { channels }
            | Self::HighPass { channels }
            | Self::Svf { channels, .. }
            | Self::Drive { channels }
            | Self::Mix { channels }
            | Self::Delay { channels, .. } => Some(*channels),
            Self::Sine
            | Self::Saw
            | Self::Square
            | Self::Triangle
            | Self::Noise { .. }
            | Self::Pluck { .. }
            | Self::Random { .. }
            | Self::Modal { .. }
            | Self::Wavetable { .. }
            | Self::Adsr { .. }
            | Self::Timbre
            | Self::Pressure
            | Self::Velocity
            | Self::Key
            | Self::Lfo
            | Self::Pan
            | Self::Balance
            | Self::Width => None,
        }
    }

    fn count(&self, channels: Channels) -> u8 {
        match channels {
            Channels::One => 1,
            Channels::Two => 2,
            Channels::Config => self
                .configured_channels()
                .expect("a processor declared with configured channels has them"),
        }
    }

    pub(crate) fn output_channels(&self) -> u8 {
        self.count(self.declaration().output)
    }

    /// The channels its audio input carries, or `None` if it has none.
    pub(crate) fn required_input_channels(&self) -> Option<u8> {
        match self.declaration().input {
            Input::None => None,
            Input::One(channels, _) | Input::Many(channels, _) => Some(self.count(channels)),
        }
    }

    pub(crate) fn accepts_many_inputs(&self) -> bool {
        matches!(self.declaration().input, Input::Many(..))
    }

    /// Whether its audio input is read from earlier frames, so it is no
    /// same-sample dependence.
    pub(crate) fn reads_input_later(&self) -> bool {
        matches!(
            self.declaration().input,
            Input::One(_, Read::Later) | Input::Many(_, Read::Later)
        )
    }

    /// Whether it needs a note, so a shared graph may not hold it.
    pub(crate) fn forbidden_in_shared(&self) -> bool {
        self.declaration().placement == Placement::InstrumentVoice
    }

    /// Whether only an effect's graph may hold it.
    pub(crate) fn only_in_effects(&self) -> bool {
        self.declaration().placement == Placement::Effect
    }

    /// Its per-sample work.
    pub(crate) fn sample_work(&self) -> u64 {
        match (self.declaration().work, self) {
            (Work::Units(units), _) => units,
            (Work::ModalBank, Self::Modal { modes, strings }) => {
                modal_sample_work(*modes, *strings)
            }
            (Work::BowSection, Self::Bow { players, body, .. }) => bow_sample_work(*players, *body),
            (work, _) => unreachable!("{} is not charged as {work:?}", self.identity()),
        }
    }
}

/// The parameters of every fixed waveform.
const OSCILLATOR: &[Param] = &[
    Param::new(
        "ratio",
        Unit::Dimensionless,
        Rate::Sample,
        int(1),
        closed(int(-64)),
        closed(int(64)),
    ),
    Param::new(
        "frequency",
        Unit::Hertz,
        Rate::Sample,
        int(0),
        closed(int(-24_000)),
        closed(int(24_000)),
    ),
    Param::new(
        "phase",
        Unit::Dimensionless,
        Rate::NoteOn,
        int(0),
        closed(int(0)),
        closed(int(1)),
    ),
    Param::new(
        "level",
        Unit::Dimensionless,
        Rate::Sample,
        int(1),
        closed(int(0)),
        closed(int(16)),
    ),
];

pub(crate) const SINE: Declaration = Declaration {
    identity: "synth.sine/1",
    params: OSCILLATOR,
    placement: Placement::InstrumentVoice,
    output: Channels::One,
    input: Input::None,
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::None,
};

pub(crate) const SAW: Declaration = Declaration {
    identity: "synth.saw/1",
    params: OSCILLATOR,
    placement: Placement::InstrumentVoice,
    output: Channels::One,
    input: Input::None,
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::None,
};

pub(crate) const SQUARE: Declaration = Declaration {
    identity: "synth.square/1",
    params: OSCILLATOR,
    placement: Placement::InstrumentVoice,
    output: Channels::One,
    input: Input::None,
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::None,
};

pub(crate) const TRIANGLE: Declaration = Declaration {
    identity: "synth.triangle/1",
    params: OSCILLATOR,
    placement: Placement::InstrumentVoice,
    output: Channels::One,
    input: Input::None,
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::None,
};

pub(crate) const NOISE: Declaration = Declaration {
    identity: "synth.noise/1",
    params: &[Param::new(
        "level",
        Unit::Dimensionless,
        Rate::Sample,
        int(1),
        closed(int(0)),
        closed(int(16)),
    )],
    placement: Placement::InstrumentVoice,
    output: Channels::One,
    input: Input::None,
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::Defaults(&[
        late("per_note", ConfigValue::Boolean(false)),
        default("seed", ConfigValue::Number(DEFAULT_NOISE_SEED as i64)),
    ]),
};

pub(crate) const PLUCK: Declaration = Declaration {
    identity: "synth.pluck/1",
    params: &[
        Param::new(
            "ratio",
            Unit::Dimensionless,
            Rate::Sample,
            int(1),
            closed(exact(1, 8)),
            closed(int(8)),
        ),
        Param::new(
            "decay",
            Unit::Seconds,
            Rate::Sample,
            int(3),
            closed(exact(1, 20)),
            closed(int(30)),
        ),
        Param::new(
            "damping",
            Unit::Dimensionless,
            Rate::Sample,
            exact(1, 2),
            closed(int(0)),
            closed(int(1)),
        ),
        Param::new(
            "level",
            Unit::Dimensionless,
            Rate::Sample,
            int(1),
            closed(int(0)),
            closed(int(16)),
        ),
        Param::new(
            "hardness",
            Unit::Dimensionless,
            Rate::NoteOn,
            int(1),
            closed(exact(1, 100)),
            closed(int(1)),
        ),
        Param::new(
            "position",
            Unit::Dimensionless,
            Rate::NoteOn,
            int(0),
            closed(int(0)),
            closed(exact(1, 2)),
        ),
        Param::new(
            "pickup",
            Unit::Dimensionless,
            Rate::Sample,
            int(0),
            closed(int(0)),
            closed(exact(1, 2)),
        ),
    ],
    placement: Placement::InstrumentVoice,
    output: Channels::One,
    input: Input::None,
    work: Work::Units(16),
    memory: Memory::PluckString,
    config: NormalConfig::Defaults(&[default(
        "seed",
        ConfigValue::Number(DEFAULT_NOISE_SEED as i64),
    )]),
};

pub(crate) const RANDOM: Declaration = Declaration {
    identity: "synth.random/1",
    params: &[],
    placement: Placement::InstrumentVoice,
    output: Channels::One,
    input: Input::None,
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::Defaults(&[default("seed", ConfigValue::Number(1))]),
};

pub(crate) const MODAL: Declaration = Declaration {
    identity: "synth.modal/1",
    params: &[
        Param::new(
            "ratio",
            Unit::Dimensionless,
            Rate::Sample,
            int(1),
            closed(exact(1, 8)),
            closed(int(8)),
        ),
        Param::new(
            "stretch",
            Unit::Dimensionless,
            Rate::Sample,
            int(0),
            closed(int(0)),
            closed(exact(1, 16)),
        ),
        Param::new(
            "decay",
            Unit::Seconds,
            Rate::Sample,
            int(2),
            closed(exact(1, 100)),
            closed(int(60)),
        ),
        Param::new(
            "damping",
            Unit::Dimensionless,
            Rate::Sample,
            int(0),
            closed(int(0)),
            closed(int(4)),
        ),
        Param::new(
            "position",
            Unit::Dimensionless,
            Rate::Sample,
            int(0),
            closed(int(0)),
            closed(exact(1, 2)),
        ),
        Param::new(
            "detune",
            Unit::Dimensionless,
            Rate::Sample,
            int(0),
            closed(int(0)),
            closed(int(50)),
        ),
        Param::new(
            "level",
            Unit::Dimensionless,
            Rate::Sample,
            int(1),
            closed(int(0)),
            closed(int(16)),
        ),
    ],
    placement: Placement::InstrumentVoice,
    output: Channels::One,
    input: Input::One(Channels::One, Read::Now),
    work: Work::ModalBank,
    memory: Memory::None,
    config: NormalConfig::Defaults(&[default("strings", ConfigValue::Number(1))]),
};

pub(crate) const BOW: Declaration = Declaration {
    identity: "synth.bow/1",
    params: &[
        Param::new(
            "ratio",
            Unit::Dimensionless,
            Rate::Sample,
            int(1),
            closed(exact(1, 8)),
            closed(int(8)),
        ),
        Param::new(
            "dynamics",
            Unit::Dimensionless,
            Rate::Sample,
            exact(1, 2),
            closed(int(0)),
            closed(int(1)),
        ),
        Param::new(
            "position",
            Unit::Dimensionless,
            Rate::Sample,
            exact(1, 8),
            closed(exact(1, 10)),
            closed(exact(1, 7)),
        ),
        Param::new(
            "vibrato",
            Unit::Dimensionless,
            Rate::Sample,
            exact(25, 2),
            closed(int(0)),
            closed(int(50)),
        ),
        Param::new(
            "rate",
            Unit::Hertz,
            Rate::Sample,
            exact(28, 5),
            closed(int(3)),
            closed(int(8)),
        ),
        Param::new(
            "attack",
            Unit::Seconds,
            Rate::Sample,
            exact(2, 25),
            closed(exact(1, 200)),
            closed(exact(1, 2)),
        ),
        Param::new(
            "timing",
            Unit::Seconds,
            Rate::Sample,
            exact(9, 500),
            closed(int(0)),
            closed(exact(1, 25)),
        ),
        Param::new(
            "spread",
            Unit::Dimensionless,
            Rate::Sample,
            int(1),
            closed(int(0)),
            closed(int(1)),
        ),
        Param::new(
            "width",
            Unit::Dimensionless,
            Rate::Sample,
            exact(7, 10),
            closed(int(0)),
            closed(int(2)),
        ),
        Param::new(
            "pan",
            Unit::Dimensionless,
            Rate::Sample,
            int(0),
            closed(int(-1)),
            closed(int(1)),
        ),
        Param::new(
            "level",
            Unit::Dimensionless,
            Rate::Sample,
            int(1),
            closed(int(0)),
            closed(int(16)),
        ),
    ],
    placement: Placement::InstrumentVoice,
    output: Channels::Config,
    input: Input::None,
    work: Work::BowSection,
    memory: Memory::BowStrings,
    config: NormalConfig::Defaults(&[
        default("body", ConfigValue::Symbol("violin")),
        default("channels", ConfigValue::Number(1)),
        default("players", ConfigValue::Number(1)),
        default("seed", ConfigValue::Number(1)),
    ]),
};

pub(crate) const WAVETABLE: Declaration = Declaration {
    identity: "synth.wavetable/1",
    params: &[
        Param::new(
            "ratio",
            Unit::Dimensionless,
            Rate::Sample,
            int(1),
            closed(int(-64)),
            closed(int(64)),
        ),
        Param::new(
            "frequency",
            Unit::Hertz,
            Rate::Sample,
            int(0),
            closed(int(-24_000)),
            closed(int(24_000)),
        ),
        Param::new(
            "phase",
            Unit::Dimensionless,
            Rate::NoteOn,
            int(0),
            closed(int(0)),
            closed(int(1)),
        ),
        Param::new(
            "level",
            Unit::Dimensionless,
            Rate::Sample,
            int(1),
            closed(int(0)),
            closed(int(16)),
        ),
        Param::new(
            "position",
            Unit::Dimensionless,
            Rate::Sample,
            int(0),
            closed(int(0)),
            closed(int(1)),
        ),
    ],
    placement: Placement::InstrumentVoice,
    output: Channels::One,
    input: Input::None,
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::Defaults(&[]),
};

pub(crate) const SAMPLE: Declaration = Declaration {
    identity: "synth.sample/1",
    params: &[
        Param::new(
            "ratio",
            Unit::Dimensionless,
            Rate::Sample,
            int(1),
            closed(int(-64)),
            closed(int(64)),
        ),
        Param::new(
            "frequency",
            Unit::Hertz,
            Rate::Sample,
            int(0),
            closed(int(-24_000)),
            closed(int(24_000)),
        ),
        Param::new(
            "level",
            Unit::Dimensionless,
            Rate::Sample,
            int(1),
            closed(int(0)),
            closed(int(16)),
        ),
    ],
    placement: Placement::InstrumentVoice,
    output: Channels::Config,
    input: Input::None,
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::Sample,
};

pub(crate) const ADSR: Declaration = Declaration {
    identity: "synth.adsr/1",
    params: &[
        Param::new(
            "attack",
            Unit::Seconds,
            Rate::NoteOn,
            int(0),
            closed(int(0)),
            closed(int(1800)),
        ),
        Param::new(
            "decay",
            Unit::Seconds,
            Rate::NoteOn,
            int(0),
            closed(int(0)),
            closed(int(1800)),
        ),
        Param::new(
            "sustain",
            Unit::Dimensionless,
            Rate::NoteOn,
            int(1),
            closed(int(0)),
            closed(int(1)),
        ),
        Param::new(
            "release",
            Unit::Seconds,
            Rate::NoteOff,
            int(0),
            closed(int(0)),
            closed(int(1800)),
        ),
        Param::new(
            "curve",
            Unit::Dimensionless,
            Rate::NoteOn,
            int(0),
            closed(int(0)),
            closed(int(16)),
        ),
    ],
    placement: Placement::InstrumentVoice,
    output: Channels::One,
    input: Input::None,
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::Defaults(&[late("on_join", ConfigValue::Symbol("hold"))]),
};

pub(crate) const TIMBRE: Declaration = Declaration {
    identity: "synth.timbre/1",
    params: &[],
    placement: Placement::InstrumentVoice,
    output: Channels::One,
    input: Input::None,
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::Defaults(&[]),
};

pub(crate) const PRESSURE: Declaration = Declaration {
    identity: "synth.pressure/1",
    params: &[],
    placement: Placement::InstrumentVoice,
    output: Channels::One,
    input: Input::None,
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::Defaults(&[]),
};

pub(crate) const VELOCITY: Declaration = Declaration {
    identity: "synth.velocity/1",
    params: &[],
    placement: Placement::InstrumentVoice,
    output: Channels::One,
    input: Input::None,
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::Defaults(&[]),
};

pub(crate) const KEY: Declaration = Declaration {
    identity: "synth.key/1",
    params: &[],
    placement: Placement::InstrumentVoice,
    output: Channels::One,
    input: Input::None,
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::Defaults(&[]),
};

pub(crate) const LFO: Declaration = Declaration {
    identity: "synth.lfo/1",
    params: &[
        Param::new(
            "frequency",
            Unit::Hertz,
            Rate::Sample,
            int(1),
            closed(int(-200)),
            closed(int(200)),
        ),
        Param::new(
            "phase",
            Unit::Dimensionless,
            Rate::NoteOn,
            int(0),
            closed(int(0)),
            closed(int(1)),
        )
        .in_shared(InShared::Rate(Rate::Reset)),
        Param::new(
            "level",
            Unit::Dimensionless,
            Rate::Sample,
            int(1),
            closed(int(0)),
            closed(int(16)),
        ),
    ],
    placement: Placement::AnyGraph,
    output: Channels::One,
    input: Input::None,
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::None,
};

pub(crate) const GAIN: Declaration = Declaration {
    identity: "synth.gain/1",
    params: &[Param::new(
        "level",
        Unit::Dimensionless,
        Rate::Sample,
        int(1),
        closed(int(0)),
        closed(int(16)),
    )],
    placement: Placement::AnyGraph,
    output: Channels::Config,
    input: Input::One(Channels::Config, Read::Now),
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::Defaults(&[]),
};

pub(crate) const ONEPOLE: Declaration = Declaration {
    identity: "synth.onepole/1",
    params: &[Param::new(
        "cutoff",
        Unit::Hertz,
        Rate::Sample,
        int(1000),
        open(int(0)),
        open(int(24_000)),
    )],
    placement: Placement::AnyGraph,
    output: Channels::Config,
    input: Input::One(Channels::Config, Read::Now),
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::Defaults(&[]),
};

pub(crate) const HIGHPASS: Declaration = Declaration {
    identity: "synth.highpass/1",
    params: &[Param::new(
        "cutoff",
        Unit::Hertz,
        Rate::Sample,
        int(1000),
        open(int(0)),
        open(int(24_000)),
    )],
    placement: Placement::AnyGraph,
    output: Channels::Config,
    input: Input::One(Channels::Config, Read::Now),
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::Defaults(&[]),
};

pub(crate) const SVF: Declaration = Declaration {
    identity: "synth.svf/1",
    params: &[
        Param::new(
            "cutoff",
            Unit::Hertz,
            Rate::Sample,
            int(1000),
            closed(int(0)),
            open(int(24_000)),
        ),
        Param::new(
            "ratio",
            Unit::Dimensionless,
            Rate::Sample,
            int(0),
            closed(int(0)),
            closed(int(64)),
        )
        .in_shared(InShared::Max(closed(int(0)))),
        Param::new(
            "q",
            Unit::Dimensionless,
            Rate::Sample,
            exact(707, 1000),
            closed(exact(1, 10)),
            closed(int(40)),
        ),
    ],
    placement: Placement::AnyGraph,
    output: Channels::Config,
    input: Input::One(Channels::Config, Read::Now),
    work: Work::Units(2),
    memory: Memory::None,
    config: NormalConfig::Defaults(&[]),
};

pub(crate) const DRIVE: Declaration = Declaration {
    identity: "synth.drive/1",
    params: &[
        Param::new(
            "drive",
            Unit::Dimensionless,
            Rate::Sample,
            int(1),
            closed(int(0)),
            closed(int(64)),
        ),
        Param::new(
            "bias",
            Unit::Dimensionless,
            Rate::Sample,
            int(0),
            closed(int(-4)),
            closed(int(4)),
        ),
        Param::new(
            "level",
            Unit::Dimensionless,
            Rate::Sample,
            int(1),
            closed(int(0)),
            closed(int(16)),
        ),
        Param::new(
            "reference",
            Unit::Dimensionless,
            Rate::Sample,
            int(0),
            closed(int(0)),
            closed(int(1)),
        ),
    ],
    placement: Placement::AnyGraph,
    output: Channels::Config,
    input: Input::One(Channels::Config, Read::Now),
    work: Work::Units(2),
    memory: Memory::None,
    config: NormalConfig::Defaults(&[]),
};

pub(crate) const MIX: Declaration = Declaration {
    identity: "synth.mix/1",
    params: &[],
    placement: Placement::AnyGraph,
    output: Channels::Config,
    input: Input::Many(Channels::Config, Read::Now),
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::Defaults(&[]),
};

pub(crate) const PAN: Declaration = Declaration {
    identity: "synth.pan/1",
    params: &[Param::new(
        "pan",
        Unit::Dimensionless,
        Rate::Sample,
        int(0),
        closed(int(-1)),
        closed(int(1)),
    )],
    placement: Placement::AnyGraph,
    output: Channels::Two,
    input: Input::One(Channels::One, Read::Now),
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::None,
};

pub(crate) const BALANCE: Declaration = Declaration {
    identity: "synth.balance/1",
    params: &[Param::new(
        "balance",
        Unit::Dimensionless,
        Rate::Sample,
        int(0),
        closed(int(-1)),
        closed(int(1)),
    )],
    placement: Placement::AnyGraph,
    output: Channels::Two,
    input: Input::One(Channels::Two, Read::Now),
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::None,
};

pub(crate) const WIDTH: Declaration = Declaration {
    identity: "synth.width/1",
    params: &[Param::new(
        "width",
        Unit::Dimensionless,
        Rate::Sample,
        int(1),
        closed(int(-4)),
        closed(int(4)),
    )],
    placement: Placement::AnyGraph,
    output: Channels::Two,
    input: Input::One(Channels::Two, Read::Now),
    work: Work::Units(1),
    memory: Memory::None,
    config: NormalConfig::None,
};

pub(crate) const DELAY_SECONDS: Declaration = Declaration {
    identity: "synth.delay/1",
    params: &[
        Param::new(
            "time",
            Unit::Seconds,
            Rate::Sample,
            exact(1, 2),
            open(int(0)),
            closed(int(MAX_DELAY_SECONDS)),
        ),
        Param::new(
            "feedback",
            Unit::Dimensionless,
            Rate::Sample,
            int(0),
            closed(int(0)),
            closed(exact(99, 100)),
        ),
        Param::new(
            "damping",
            Unit::Dimensionless,
            Rate::Sample,
            int(0),
            closed(int(0)),
            closed(int(1)),
        ),
    ],
    placement: Placement::Effect,
    output: Channels::Config,
    input: Input::One(Channels::Config, Read::Later),
    work: Work::Units(1),
    memory: Memory::DelayLine,
    config: NormalConfig::Defaults(&[default("clock", ConfigValue::Symbol("seconds"))]),
};

pub(crate) const DELAY_SCORE: Declaration = Declaration {
    identity: "synth.delay/1",
    params: &[
        Param::new(
            "time",
            Unit::Quarter,
            Rate::Sample,
            int(1),
            open(int(0)),
            closed(int(256)),
        ),
        Param::new(
            "feedback",
            Unit::Dimensionless,
            Rate::Sample,
            int(0),
            closed(int(0)),
            closed(exact(99, 100)),
        ),
        Param::new(
            "damping",
            Unit::Dimensionless,
            Rate::Sample,
            int(0),
            closed(int(0)),
            closed(int(1)),
        ),
    ],
    placement: Placement::Effect,
    output: Channels::Config,
    input: Input::One(Channels::Config, Read::Later),
    work: Work::Units(2),
    memory: Memory::DelayLine,
    config: NormalConfig::Defaults(&[default("clock", ConfigValue::Symbol("seconds"))]),
};
