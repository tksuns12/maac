//! The bowed-string section specified in docs/instruments.md#bowed-strings.

use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

use crate::dsp::{RenderError, Result};

pub const MAX_PLAYERS: u8 = 16;
/// A voice reads its line this far ahead: 500 ms.
pub const LOOKAHEAD_FRAMES: u64 = 24_000;
/// The longest `slide`, in seconds.
pub const MAX_SLIDE_SECONDS: f64 = 0.25;
const RATE_HZ: f64 = 48_000.0;
/// Cells per delay line: the lowest playable frequency is about 11.7 Hz.
const RING_CELLS: usize = 4096;
/// Bodies per family, and the random draws they are chosen from.
const BODY_TABLE: usize = 16;

/// The body family of `config.body`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BowBody {
    Violin,
    Viola,
    Cello,
    Bass,
    None,
}

impl BowBody {
    pub fn name(self) -> &'static str {
        match self {
            Self::Violin => "violin",
            Self::Viola => "viola",
            Self::Cello => "cello",
            Self::Bass => "bass",
            Self::None => "none",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "violin" => Self::Violin,
            "viola" => Self::Viola,
            "cello" => Self::Cello,
            "bass" => Self::Bass,
            "none" => Self::None,
            _ => return None,
        })
    }

    /// Every body frequency is multiplied by this.
    fn scale(self) -> f64 {
        match self {
            Self::Violin | Self::None => 1.0,
            Self::Viola => 0.8,
            Self::Cello => 0.42,
            Self::Bass => 0.3,
        }
    }

    /// The written range whose evenness chooses the bodies, as MIDI keys.
    #[cfg(test)]
    fn range(self) -> (i32, i32) {
        match self {
            Self::Violin | Self::None => (55, 105),
            Self::Viola => (48, 88),
            Self::Cello => (36, 81),
            Self::Bass => (28, 67),
        }
    }
}

/// One note of a connected line: when it starts and ends, its pitch, and how
/// it joins the note before it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LineNote {
    pub on_frame: u64,
    pub off_frame: u64,
    pub pitch_hz: f64,
    pub slur: bool,
    pub slide_seconds: f64,
}

/// A voice's line: its notes in onset order, the first of which started the
/// voice, and that first note's event address, which seeds what varies from
/// line to line.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Line {
    pub notes: Vec<LineNote>,
    pub address: String,
}

/// The parameters of one frame, after controls and modulation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BowParams {
    pub ratio: f64,
    pub dynamics: f64,
    pub position: f64,
    pub vibrato: f64,
    pub rate: f64,
    pub attack: f64,
    pub timing: f64,
    pub spread: f64,
    pub width: f64,
    pub pan: f64,
    pub level: f64,
}

/// A resonator of a body: `s ← R e^{iω} s + x`, heard as `a Im(s)`.
#[derive(Clone, Copy, Debug)]
struct BodyMode {
    pole_re: f64,
    pole_im: f64,
    amplitude: f64,
}

#[derive(Debug)]
struct Body {
    modes: Vec<BodyMode>,
}

/// A counter-based generator (SplitMix64): the same seed gives the same
/// sequence on every platform.
#[derive(Clone, Copy, Debug)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in [lo, hi).
    fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    /// Standard normal, by Box–Muller's cosine branch.
    fn normal(&mut self) -> f64 {
        let u = 1.0 - self.unit();
        let v = self.unit();
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }
}

/// FNV-1a over the bytes, mixed into a seed.
fn mix(seed: u64, bytes: &[u8]) -> u64 {
    let mut hash = 0xCBF2_9CE4_8422_2325u64 ^ seed;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01B3);
    }
    hash
}

/// The level of a dense body mode at `f` Hz, before scaling: the bridge hill.
fn hill_db(f: f64) -> f64 {
    if f < 250.0 {
        -12.0 * (250.0 / f).log2()
    } else if f < 650.0 {
        0.0
    } else if f < 1600.0 {
        -6.0
    } else if f < 4000.0 {
        -6.0 + 9.0
            * (std::f64::consts::PI * (f / 1600.0).log2() / (4000.0f64 / 1600.0).log2()).sin()
    } else {
        -6.0 - 12.0 * (f / 4000.0).log2()
    }
}

/// Draw `index` of a family's bodies: frequency, Q and level of each mode.
fn draw_body(family: BowBody, index: u32) -> Vec<(f64, f64, f64, f64)> {
    let mut rng = Rng::new(mix(u64::from(index), family.name().as_bytes()));
    let mut modes: Vec<(f64, f64, f64)> = vec![
        (275.0, 22.0, 0.0),
        (405.0, 30.0, -4.0),
        (470.0, 40.0, 2.0),
        (540.0, 40.0, 1.0),
        (590.0, 35.0, -3.0),
    ];
    let mut f = 620.0;
    while f < 10_000.0 {
        f *= 2f64.powf(rng.uniform(0.04, 0.13));
        let q = rng.uniform(15.0, 35.0);
        modes.push((f, q, hill_db(f) + rng.uniform(-4.0, 1.0)));
    }
    modes
        .into_iter()
        .map(|(f, q, db)| {
            let frequency = f * family.scale() * 2f64.powf(rng.normal() * 0.03);
            let sign = if rng.unit() < 0.5 { -1.0 } else { 1.0 };
            (frequency, q, 10f64.powf(db / 20.0) * frequency / q, sign)
        })
        .collect()
}

/// Build a body from its modes, normalized so its impulse response has unit
/// energy.
fn build_body(modes: &[(f64, f64, f64, f64)]) -> Body {
    let modes: Vec<BodyMode> = modes
        .iter()
        .map(|&(frequency, q, amplitude, sign)| {
            let radius = (-std::f64::consts::PI * frequency / (q * RATE_HZ)).exp();
            let omega = std::f64::consts::TAU * frequency / RATE_HZ;
            BodyMode {
                pole_re: radius * omega.cos(),
                pole_im: radius * omega.sin(),
                amplitude: amplitude * sign,
            }
        })
        .collect();
    // Σ_n h[n]² for h[n] = Σ a R^n sin(ωn), in closed form.
    let mut energy = 0.0;
    for a in &modes {
        for b in &modes {
            let (ra, wa) = polar(a);
            let (rb, wb) = polar(b);
            let r = ra * rb;
            let cosine_sum = |phi: f64| (1.0 - r * phi.cos()) / (1.0 - 2.0 * r * phi.cos() + r * r);
            energy += a.amplitude * b.amplitude * 0.5 * (cosine_sum(wa - wb) - cosine_sum(wa + wb));
        }
    }
    let norm = 1.0 / energy.sqrt();
    Body {
        modes: modes
            .into_iter()
            .map(|mode| BodyMode {
                amplitude: mode.amplitude * norm,
                ..mode
            })
            .collect(),
    }
}

fn polar(mode: &BodyMode) -> (f64, f64) {
    (
        mode.pole_re.hypot(mode.pole_im),
        mode.pole_im.atan2(mode.pole_re),
    )
}

/// The draws chosen for each family's table, the most even of 120. A test
/// recomputes them.
const VIOLIN_BODIES: [u32; BODY_TABLE] = [
    6, 7, 12, 14, 30, 34, 35, 37, 46, 52, 59, 60, 76, 84, 86, 102,
];
const VIOLA_BODIES: [u32; BODY_TABLE] = [
    6, 17, 28, 33, 43, 51, 52, 55, 68, 83, 95, 97, 99, 102, 103, 117,
];
const CELLO_BODIES: [u32; BODY_TABLE] = [
    0, 2, 9, 11, 26, 45, 49, 59, 62, 67, 75, 78, 81, 87, 101, 102,
];
const BASS_BODIES: [u32; BODY_TABLE] = [
    1, 12, 13, 21, 29, 30, 33, 39, 63, 64, 74, 80, 91, 99, 105, 108,
];

fn table(family: BowBody) -> &'static [Body] {
    static TABLES: OnceLock<[Vec<Body>; 4]> = OnceLock::new();
    let tables = TABLES.get_or_init(|| {
        [
            (BowBody::Violin, VIOLIN_BODIES),
            (BowBody::Viola, VIOLA_BODIES),
            (BowBody::Cello, CELLO_BODIES),
            (BowBody::Bass, BASS_BODIES),
        ]
        .map(|(family, draws)| {
            draws
                .iter()
                .map(|&draw| build_body(&draw_body(family, draw)))
                .collect()
        })
    });
    match family {
        BowBody::Violin | BowBody::None => &tables[0],
        BowBody::Viola => &tables[1],
        BowBody::Cello => &tables[2],
        BowBody::Bass => &tables[3],
    }
}

/// What a player draws for one note of the line.
#[derive(Clone, Copy, Debug)]
struct PlayerNote {
    /// The player's onset, in frames: the note's, moved by the player's timing.
    onset: f64,
    /// The pitch the player's finger finds, in cents from A4.
    cents: f64,
    /// The player's slide into the note, in frames; zero for a finger change.
    slide: f64,
}

/// The change into a note: where the pitch leaves the old note and lands.
#[derive(Clone, Copy, Debug)]
struct Change {
    start: f64,
    end: f64,
    slide: bool,
}

#[derive(Clone, Debug)]
struct Player {
    neck: Vec<f64>,
    bridge: Vec<f64>,
    write: usize,
    loss: f64,
    noise: f64,
    /// The bow's noise, apart from the draws for notes so that one does not
    /// shift the other.
    noise_rng: Rng,
    rng: Rng,
    body: usize,
    resonators: Vec<(f64, f64)>,
    tuning: f64,
    depth_scale: f64,
    rate_offset: f64,
    position_scale: f64,
    gain: f64,
    drift: [(f64, f64); 2],
    phase: f64,
    notes: Vec<PlayerNote>,
    /// The latest note whose change has begun.
    current: usize,
    depth: Option<f64>,
    /// The centre of each bow change, in note order.
    bow_changes: Vec<f64>,
    /// Bow changes wholly past, and the direction they left the bow in.
    settled: usize,
    direction: f64,
}

/// The values a section takes when its line starts.
#[derive(Clone, Copy, Debug)]
struct Latched {
    attack: f64,
    timing: f64,
    spread: f64,
}

#[derive(Clone, Debug)]
pub(crate) struct Bow {
    players: usize,
    body: BowBody,
    channels: usize,
    seed: u32,
    line: Option<Arc<Line>>,
    lift: Option<u64>,
    latched: Option<Latched>,
    dynamics: Option<f64>,
    section: Vec<Player>,
}

const MS: f64 = RATE_HZ / 1000.0;
/// A bow change is centred this long before the note it changes into.
const EARLY: f64 = 10.0 * MS;
/// Each half of the reversal. The bow does not stop between them: a bow
/// held still on the string clamps it, and a section of them leaves a hole.
const TURN: f64 = 10.0 * MS;
/// The press that makes the string catch again, and its width.
const CATCH: f64 = 0.25;
const CATCH_WIDTH: f64 = 40.0 * MS;
/// A slide into a bow change ends this long before the onset.
const SLIDE_CLEAR: f64 = EARLY + TURN;
const FINGER: f64 = 4.0 * MS;
const LIFT: f64 = 250.0 * MS;

impl Bow {
    pub(crate) fn new(players: u8, body: BowBody, channels: u8, seed: u32) -> Result<Self> {
        if !(1..=MAX_PLAYERS).contains(&players) || !(1..=2).contains(&channels) || seed == 0 {
            return Err(RenderError::RenderState(format!(
                "bowed section needs 1..={MAX_PLAYERS} players, 1 or 2 channels and a nonzero seed"
            )));
        }
        Ok(Self {
            players: usize::from(players),
            body,
            channels: usize::from(channels),
            seed,
            line: None,
            lift: None,
            latched: None,
            dynamics: None,
            section: Vec::new(),
        })
    }

    /// Give the section the line its voice plays. A line with no notes is
    /// rejected.
    pub(crate) fn play(&mut self, line: Arc<Line>) -> Result<()> {
        if line.notes.is_empty() {
            return Err(RenderError::RenderState("bowed line has no notes".into()));
        }
        self.line = Some(line);
        Ok(())
    }

    /// The voice's note-off: the bow lifts.
    pub(crate) fn lift(&mut self, frame: u64) {
        self.lift.get_or_insert(frame);
    }

    /// Draw the players when the line starts.
    fn start(&mut self, params: &BowParams, address_seed: u64) -> Result<()> {
        let latched = Latched {
            attack: params.attack,
            timing: params.timing,
            spread: params.spread,
        };
        let bodies = table(self.body).len();
        let mut section = Vec::with_capacity(self.players);
        for index in 0..self.players {
            // A player's traits come from the seed alone, so the same player
            // plays every line alike; what changes from line to line comes
            // from the line's first address.
            let mut traits = Rng::new(mix(u64::from(self.seed), &(index as u64).to_le_bytes()));
            let spread = latched.spread;
            let tuning = traits.normal() * 4.0 * spread;
            let depth_scale = 1.0 + traits.uniform(-0.2, 0.2) * spread;
            let rate_offset = traits.uniform(-0.5, 0.5) * spread;
            let position_scale = 1.0 + traits.uniform(-0.12, 0.12) * spread;
            let gain = 10f64.powf(traits.normal() * 1.5 * spread / 20.0);
            let drift = [
                (
                    traits.uniform(0.08, 0.2),
                    traits.uniform(0.0, std::f64::consts::TAU),
                ),
                (
                    traits.uniform(0.2, 0.5),
                    traits.uniform(0.0, std::f64::consts::TAU),
                ),
            ];
            let mut rng = Rng::new(mix(address_seed, &(index as u64).to_le_bytes()));
            let noise_rng = Rng::new(mix(
                address_seed ^ 0x0005_DEEC_E66D,
                &(index as u64).to_le_bytes(),
            ));
            let phase = rng.uniform(0.0, std::f64::consts::TAU);
            let body = (self.seed as usize + index) % bodies;
            section.push(Player {
                neck: ring()?,
                bridge: ring()?,
                write: 0,
                loss: 0.0,
                noise: 0.0,
                noise_rng,
                rng,
                body,
                resonators: vec![(0.0, 0.0); table(self.body)[body].modes.len()],
                tuning,
                depth_scale,
                rate_offset,
                position_scale,
                gain,
                drift,
                phase,
                notes: Vec::new(),
                current: 0,
                depth: None,
                bow_changes: Vec::new(),
                settled: 0,
                direction: 1.0,
            });
        }
        self.section = section;
        self.latched = Some(latched);
        Ok(())
    }
}

fn ring() -> Result<Vec<f64>> {
    let mut cells = Vec::new();
    cells
        .try_reserve_exact(RING_CELLS)
        .map_err(|_| RenderError::RenderState("bowed string allocation failed".into()))?;
    cells.resize(RING_CELLS, 0.0);
    Ok(cells)
}

impl Bow {
    /// Advance the section one frame and return its output: the players
    /// summed, or spread across two channels.
    pub(crate) fn sample(
        &mut self,
        frame: u64,
        params: &BowParams,
        pitch_hz: f64,
    ) -> Result<[f64; 2]> {
        let line = self
            .line
            .clone()
            .ok_or_else(|| RenderError::RenderState("bowed section has no line".into()))?;
        if self.latched.is_none() {
            self.start(params, mix(u64::from(self.seed), line.address.as_bytes()))?;
        }
        let latched = self.latched.expect("latched at the first frame");
        // A frame reads only the notes that start within the look-ahead.
        let horizon = frame.saturating_add(LOOKAHEAD_FRAMES);
        let visible = line.notes.partition_point(|note| note.on_frame <= horizon);
        let notes = &line.notes[..visible];
        // The voice's note is the latest one it has reached; its pitch
        // expression is the ratio of the voice's frequency to that note's.
        let reached = notes.partition_point(|note| note.on_frame <= frame).max(1) - 1;
        let expression = if pitch_hz == notes[reached].pitch_hz {
            0.0
        } else {
            1200.0 * (pitch_hz / notes[reached].pitch_hz).log2()
        };
        if !expression.is_finite() {
            return Err(RenderError::Nonfinite(
                "bowed pitch expression is nonfinite".into(),
            ));
        }
        let smoothing = (-1.0 / (0.1 * RATE_HZ)).exp();
        let dynamics = match self.dynamics {
            Some(previous) => smoothing * previous + (1.0 - smoothing) * params.dynamics,
            None => params.dynamics,
        };
        self.dynamics = Some(dynamics);
        let bodies = if self.body == BowBody::None {
            None
        } else {
            Some(table(self.body))
        };
        let x = frame as f64;
        let count = self.section.len();
        let mut output = [0.0; 2];
        for (index, player) in self.section.iter_mut().enumerate() {
            player.draw(notes, latched);
            let string =
                player.string_sample(x, notes, params, latched, dynamics, expression, self.lift)?;
            let heard = match bodies {
                Some(bodies) => player.body_sample(&bodies[player.body], string),
                None => string,
            } * player.gain;
            if self.channels == 1 {
                output[0] += heard;
            } else {
                let place = if count == 1 {
                    params.pan
                } else {
                    params.pan + params.width * (index as f64 / (count - 1) as f64 - 0.5)
                }
                .clamp(-1.0, 1.0);
                let angle = (place + 1.0) * std::f64::consts::FRAC_PI_4;
                output[0] += heard * angle.cos();
                output[1] += heard * angle.sin();
            }
        }
        for value in &mut output {
            *value *= params.level;
            if !value.is_finite() {
                return Err(RenderError::Nonfinite(
                    "bowed section output is nonfinite".into(),
                ));
            }
        }
        Ok(output)
    }

    /// The output `sample` would return, without advancing the state.
    pub(crate) fn preview(
        &self,
        frame: u64,
        params: &BowParams,
        pitch_hz: f64,
    ) -> Result<[f64; 2]> {
        self.clone().sample(frame, params, pitch_hz)
    }
}

impl Player {
    /// Draw what the player does for each newly visible note: a timing, an
    /// intonation and a slide length, three draws per note in that order.
    fn draw(&mut self, notes: &[LineNote], latched: Latched) {
        while self.notes.len() < notes.len() {
            let index = self.notes.len();
            let note = &notes[index];
            let timing = if index > 0 && note.slur {
                latched.timing * 5.0 / 9.0
            } else {
                latched.timing
            };
            let jitter = (self.rng.normal() * timing).clamp(-3.0 * timing, 3.0 * timing);
            // A stroke after a rest only ever comes late.
            let jitter = if index == 0 { jitter.abs() } else { jitter };
            let cents = 1200.0 * (note.pitch_hz / 440.0).log2()
                + self.tuning
                + self.rng.normal() * 3.0 * latched.spread;
            let slide =
                note.slide_seconds * (1.0 + self.rng.uniform(-0.2, 0.2) * latched.spread) * RATE_HZ;
            let onset = note.on_frame as f64 + jitter * RATE_HZ;
            if index > 0 && !note.slur {
                self.bow_changes.push(onset - EARLY);
            }
            self.notes.push(PlayerNote {
                onset,
                cents,
                slide: if index == 0 { 0.0 } else { slide },
            });
        }
    }

    /// The change into note `k` (for `k >= 1`).
    fn change(&self, notes: &[LineNote], k: usize) -> Change {
        let player = &self.notes[k];
        if player.slide > 0.0 {
            let (start, end) = if notes[k].slur {
                (
                    player.onset - 0.65 * player.slide,
                    player.onset + 0.35 * player.slide,
                )
            } else {
                let end = player.onset - SLIDE_CLEAR;
                (end - player.slide, end)
            };
            Change {
                start,
                end,
                slide: true,
            }
        } else {
            Change {
                start: player.onset - FINGER / 2.0,
                end: player.onset + FINGER / 2.0,
                slide: false,
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn string_sample(
        &mut self,
        x: f64,
        notes: &[LineNote],
        params: &BowParams,
        latched: Latched,
        dynamics: f64,
        expression: f64,
        lift: Option<u64>,
    ) -> Result<f64> {
        // The latest change that has begun sets the pitch.
        for k in self.current + 1..notes.len() {
            if self.change(notes, k).start <= x {
                self.current = k;
            }
        }
        let k = self.current;
        let mut cents = self.notes[k].cents;
        if k > 0 {
            let change = self.change(notes, k);
            if x < change.end {
                let u = (x - change.start) / (change.end - change.start);
                let previous = self.notes[k - 1].cents;
                let shape = if change.slide { u * u } else { u };
                cents = previous + (cents - previous) * shape;
            }
        }
        // Vibrato depth: dips at each change, a widening across long notes,
        // and a bloom as the line starts, smoothed over 30 ms.
        let mut dips: f64 = 1.0;
        let mut lighten = 1.0;
        for j in k.saturating_sub(2).max(1)..notes.len().min(k + 3) {
            let change = self.change(notes, j);
            if change.slide {
                if (change.start..change.end).contains(&x) {
                    dips = dips.min(0.2);
                    let u = (x - change.start) / (change.end - change.start);
                    lighten *= 1.0 - 0.15 * (std::f64::consts::PI * u).sin();
                } else if (change.end..change.end + 300.0 * MS).contains(&x) {
                    dips = dips.min(0.2 + 0.8 * (x - change.end) / (300.0 * MS));
                }
            } else if (change.end..change.end + 180.0 * MS).contains(&x) {
                dips = dips.min(0.6 + 0.4 * (x - change.end) / (180.0 * MS));
            }
        }
        let held = self.notes.partition_point(|note| note.onset <= x).max(1) - 1;
        let gate = (notes[held].off_frame - notes[held].on_frame) as f64;
        let widen = if gate >= 1.2 * RATE_HZ {
            let span = notes[held].off_frame as f64 - self.notes[held].onset;
            1.0 + 0.25 * ((x - self.notes[held].onset) / span).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let first = self.notes[0].onset;
        let bloom_end = first + 250.0 * MS;
        let line_start = notes[0].on_frame as f64;
        let bloom = if x < bloom_end {
            0.3 + 0.7 * ((x - line_start) / (bloom_end - line_start)).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let target = bloom.min(widen * dips);
        let smoothing = (-1.0 / (0.03 * RATE_HZ)).exp();
        let depth = match self.depth {
            Some(previous) => smoothing * previous + (1.0 - smoothing) * target,
            None => target,
        };
        self.depth = Some(depth);
        let vibrato = params.vibrato * self.depth_scale * depth * self.phase.sin();
        self.phase = (self.phase
            + std::f64::consts::TAU * (params.rate + self.rate_offset) / RATE_HZ)
            .rem_euclid(std::f64::consts::TAU);
        let seconds = x / RATE_HZ;
        let drift = latched.spread
            * (3.0 * (std::f64::consts::TAU * self.drift[0].0 * seconds + self.drift[0].1).sin()
                + 2.0
                    * (std::f64::consts::TAU * self.drift[1].0 * seconds + self.drift[1].1).sin());
        let frequency =
            440.0 * 2f64.powf((cents + drift + vibrato + expression) / 1200.0) * params.ratio;

        // The bow: its direction turns at each bow change, it presses as the
        // string catches, and it rises at the first stroke and lifts at the
        // end.
        let reach = TURN.max(5.0 * CATCH_WIDTH);
        while self
            .bow_changes
            .get(self.settled)
            .is_some_and(|&centre| x >= centre + reach)
        {
            self.settled += 1;
            self.direction = -self.direction;
        }
        let mut sign = self.direction;
        let mut direction = self.direction;
        let mut catch = 0.0;
        for &centre in &self.bow_changes[self.settled..] {
            if centre - reach > x {
                break;
            }
            if (x - centre).abs() < 5.0 * CATCH_WIDTH {
                catch += CATCH * (-((x - centre) / CATCH_WIDTH).powi(2)).exp();
            }
            let start = centre - TURN;
            if x < start {
                continue;
            }
            sign = if x < centre {
                direction * (std::f64::consts::FRAC_PI_2 * (x - start) / TURN).cos()
            } else if x < centre + TURN {
                -direction * (std::f64::consts::FRAC_PI_2 * (x - centre) / TURN).sin()
            } else {
                -direction
            };
            direction = -direction;
        }
        let press = first + 20.0 * MS;
        if (x - press).abs() < 5.0 * CATCH_WIDTH {
            catch += CATCH * (-((x - press) / CATCH_WIDTH).powi(2)).exp();
        }
        let attack = latched.attack * RATE_HZ;
        let mut envelope = if x < first {
            0.0
        } else if x < first + attack {
            (std::f64::consts::FRAC_PI_2 * (x - first) / attack).sin()
        } else {
            1.0
        };
        if let Some(lift) = lift {
            envelope *= (1.0 - (x - lift as f64) / LIFT).clamp(0.0, 1.0);
        }
        let dynamics = dynamics * lighten;
        let speed = sign * envelope * (0.03 + 0.085 * dynamics);
        let force = (0.5 + 0.35 * dynamics + catch).min(0.95);
        if !frequency.is_finite() || frequency <= 0.0 {
            return Err(RenderError::Nonfinite(
                "bowed string frequency is nonfinite".into(),
            ));
        }
        let position = (params.position * self.position_scale).max(bow_floor(frequency));
        self.waveguide(
            RATE_HZ / frequency - loop_correction(frequency, dynamics),
            position,
            speed,
            force,
        )
    }

    /// One frame of a string whose loop is `length` frames: two delay lines
    /// meeting at the bow, a lossy reflection at the bridge and a rigid one
    /// at the nut.
    fn waveguide(&mut self, length: f64, position: f64, speed: f64, force: f64) -> Result<f64> {
        if !length.is_finite() || length < 4.0 || length > (RING_CELLS - 2) as f64 {
            return Err(RenderError::RenderState(format!(
                "bowed string loop of {length} frames is outside the playable range"
            )));
        }
        let bridge_delay = (length * position).max(1.0);
        let neck_delay = length - bridge_delay;
        let nut_out = read(&self.neck, self.write, neck_delay);
        let bridge_out = read(&self.bridge, self.write, bridge_delay);
        self.loss = 0.95 * (1.0 - 0.658) * bridge_out + 0.658 * self.loss;
        let bridge_reflection = -self.loss;
        let nut_reflection = -nut_out;
        let string_velocity = bridge_reflection + nut_reflection;
        self.noise = 0.995 * self.noise + 0.005 * (2.0 * self.noise_rng.unit() - 1.0);
        let bow_velocity = speed * (1.0 + 0.4 * self.noise);
        let difference = bow_velocity - string_velocity;
        let slope = 5.0 - 4.0 * force;
        let friction = ((difference * slope).abs() + 0.75).powi(-4).min(1.0);
        let injected = difference * friction;
        self.neck[self.write] = bridge_reflection + injected;
        self.bridge[self.write] = nut_reflection + injected;
        self.write = (self.write + 1) % RING_CELLS;
        Ok(bridge_out)
    }

    fn body_sample(&mut self, body: &Body, input: f64) -> f64 {
        let mut sum = 0.0;
        for (state, mode) in self.resonators.iter_mut().zip(&body.modes) {
            let re = mode.pole_re * state.0 - mode.pole_im * state.1 + input;
            let im = mode.pole_re * state.1 + mode.pole_im * state.0;
            *state = (re, im);
            sum += mode.amplitude * im;
        }
        sum
    }
}

/// A delay line read `delay` frames back, interpolated linearly.
fn read(cells: &[f64], write: usize, delay: f64) -> f64 {
    let mut position = write as f64 - delay;
    while position < 0.0 {
        position += RING_CELLS as f64;
    }
    let whole = position as usize;
    let fraction = position - whole as f64;
    let next = if whole + 1 >= RING_CELLS {
        0
    } else {
        whole + 1
    };
    cells[whole] * (1.0 - fraction) + cells[next] * fraction
}

/// The bow sits at least this fraction of the string from the bridge, a floor
/// that rises with pitch to 0.16 at 1.5 kHz: a high note's stopped string is
/// short, and nearer the bridge the string would leave its normal motion.
fn bow_floor(frequency: f64) -> f64 {
    0.16 * (frequency / 1500.0).min(1.0)
}

/// The lowest key of the tuning table, A0.
const TUNING_LOW_KEY: f64 = 21.0;

/// The `dynamics` at which each column of the tuning table was measured.
const TUNING_DYNAMICS: [f64; 3] = [0.2, 0.5, 0.9];

/// How much shorter than one period, in frames, the loop is made so the
/// string plays in tune: measured by `measure_tuning_table` for each key from
/// A0 to F#7 at three dynamics, since a harder bow plays slightly flat, and
/// interpolated in between. Outside the table the nearest entry holds.
const TUNING: [[f64; 3]; 82] = [
    [1.5372, 1.5702, 1.5916],
    [1.5779, 1.6087, 1.6287],
    [1.5736, 1.6062, 1.6241],
    [1.5832, 1.6145, 1.6333],
    [1.5544, 1.5866, 1.6047],
    [1.5734, 1.6056, 1.6230],
    [1.5398, 1.5740, 1.5922],
    [1.5809, 1.6115, 1.6303],
    [1.5583, 1.5900, 1.6079],
    [1.5719, 1.6039, 1.6216],
    [1.5815, 1.6134, 1.6320],
    [1.5775, 1.6109, 1.6286],
    [1.5777, 1.6104, 1.6292],
    [1.5505, 1.5845, 1.6044],
    [1.5719, 1.6037, 1.6213],
    [1.5612, 1.5940, 1.6119],
    [1.5865, 1.6189, 1.6374],
    [1.5747, 1.6067, 1.6250],
    [1.5557, 1.5899, 1.6089],
    [1.5636, 1.5974, 1.6158],
    [1.5725, 1.6052, 1.6236],
    [1.5744, 1.6075, 1.6259],
    [1.5599, 1.5932, 1.6118],
    [1.5777, 1.6102, 1.6281],
    [1.5629, 1.5976, 1.6170],
    [1.5745, 1.6064, 1.6248],
    [1.5757, 1.6085, 1.6268],
    [1.5714, 1.6046, 1.6233],
    [1.5629, 1.5967, 1.6153],
    [1.5718, 1.6048, 1.6234],
    [1.5691, 1.6022, 1.6207],
    [1.5763, 1.5931, 1.6121],
    [1.5734, 1.6043, 1.6219],
    [1.5715, 1.6021, 1.6204],
    [1.5825, 1.6120, 1.6297],
    [1.5822, 1.6112, 1.6284],
    [1.5711, 1.5901, 1.6071],
    [1.5711, 1.6015, 1.6197],
    [1.5508, 1.5847, 1.6045],
    [1.5482, 1.5872, 1.6082],
    [1.5409, 1.5840, 1.6057],
    [1.5254, 1.5718, 1.5939],
    [1.5144, 1.5605, 1.5811],
    [1.5105, 1.5505, 1.5690],
    [1.4935, 1.5339, 1.5568],
    [1.4766, 1.5140, 1.5446],
    [1.4644, 1.5371, 1.5865],
    [1.4790, 1.5865, 1.6269],
    [1.5214, 1.6099, 1.6414],
    [1.5802, 1.6399, 1.6606],
    [1.5830, 1.6283, 1.6511],
    [1.6014, 1.6336, 1.6472],
    [1.6004, 1.6266, 1.6389],
    [1.5630, 1.5879, 1.6007],
    [1.5623, 1.5868, 1.5998],
    [1.5501, 1.5759, 1.5888],
    [1.5368, 1.5590, 1.5697],
    [1.5130, 1.5343, 1.5457],
    [1.4884, 1.5061, 1.5194],
    [1.4522, 1.4790, 1.5041],
    [1.4476, 1.5114, 1.5514],
    [1.4346, 1.5289, 1.5752],
    [1.4690, 1.5502, 1.5839],
    [1.5322, 1.5950, 1.6182],
    [1.5493, 1.5876, 1.6019],
    [1.5403, 1.5673, 1.5782],
    [1.4638, 1.4794, 1.4956],
    [1.4062, 1.4682, 1.5053],
    [1.5046, 1.5389, 1.5547],
    [1.4728, 1.4813, 1.4877],
    [1.4423, 1.4599, 1.4754],
    [1.4162, 1.4385, 1.4581],
    [1.4763, 1.4997, 1.5217],
    [1.5194, 1.5355, 1.5492],
    [1.5338, 1.5442, 1.5533],
    [1.5785, 1.5451, 1.5517],
    [1.6603, 1.6011, 1.5996],
    [1.6910, 1.6098, 1.6041],
    [1.7117, 1.6915, 1.6811],
    [1.6816, 1.6994, 1.6935],
    [1.6799, 0.8000, 1.6156],
    [2.5000, 2.5000, 2.5000],
];

fn loop_correction(frequency: f64, dynamics: f64) -> f64 {
    let key = 69.0 + 12.0 * (frequency / 440.0).log2();
    let position = (key - TUNING_LOW_KEY).clamp(0.0, (TUNING.len() - 1) as f64);
    let index = position.floor() as usize;
    let fraction = position - index as f64;
    let at = |row: &[f64; 3]| {
        let d = dynamics.clamp(TUNING_DYNAMICS[0], TUNING_DYNAMICS[2]);
        let (column, low, high) = if d <= TUNING_DYNAMICS[1] {
            (0, TUNING_DYNAMICS[0], TUNING_DYNAMICS[1])
        } else {
            (1, TUNING_DYNAMICS[1], TUNING_DYNAMICS[2])
        };
        let u = (d - low) / (high - low);
        row[column] * (1.0 - u) + row[column + 1] * u
    };
    match TUNING.get(index + 1) {
        Some(next) => at(&TUNING[index]) * (1.0 - fraction) + at(next) * fraction,
        None => at(&TUNING[index]),
    }
}

/// How unevenly a body passes a family's range: the spread, in decibels, of
/// the energy it passes from each semitone's harmonics below 9 kHz, harmonic
/// `k` weighted by `1/k²`.
#[cfg(test)]
fn unevenness(body: &Body, family: BowBody) -> f64 {
    let (low, high) = family.range();
    let mut quietest = f64::INFINITY;
    let mut loudest = f64::NEG_INFINITY;
    for key in low..=high {
        let fundamental = 440.0 * 2f64.powf(f64::from(key - 69) / 12.0);
        let mut energy = 0.0;
        let mut harmonic = 1;
        while f64::from(harmonic) * fundamental < 9000.0 {
            let response = response(body, f64::from(harmonic) * fundamental);
            energy += response / f64::from(harmonic * harmonic);
            harmonic += 1;
        }
        let level = 10.0 * energy.log10();
        quietest = quietest.min(level);
        loudest = loudest.max(level);
    }
    loudest - quietest
}

/// `|H(f)|²` of a body: each mode is `R sin ω z⁻¹ / (1 − 2R cos ω z⁻¹ + R² z⁻²)`.
#[cfg(test)]
fn response(body: &Body, frequency: f64) -> f64 {
    let theta = std::f64::consts::TAU * frequency / RATE_HZ;
    let (z1_re, z1_im) = (theta.cos(), -theta.sin());
    let (z2_re, z2_im) = ((2.0 * theta).cos(), -(2.0 * theta).sin());
    let (mut re, mut im) = (0.0, 0.0);
    for mode in &body.modes {
        let (radius, omega) = polar(mode);
        let gain = mode.amplitude * radius * omega.sin();
        let (num_re, num_im) = (gain * z1_re, gain * z1_im);
        let den_re = 1.0 - 2.0 * radius * omega.cos() * z1_re + radius * radius * z2_re;
        let den_im = -2.0 * radius * omega.cos() * z1_im + radius * radius * z2_im;
        let magnitude = den_re * den_re + den_im * den_im;
        re += (num_re * den_re + num_im * den_im) / magnitude;
        im += (num_im * den_re - num_re * den_im) / magnitude;
    }
    re * re + im * im
}

/// The draws a family's table holds: the 16 most even of 120, in draw order.
#[cfg(test)]
fn choose_bodies(family: BowBody) -> [u32; BODY_TABLE] {
    let mut scored: Vec<(f64, u32)> = (0..120)
        .map(|draw| {
            (
                unevenness(&build_body(&draw_body(family, draw)), family),
                draw,
            )
        })
        .collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut chosen: Vec<u32> = scored[..BODY_TABLE].iter().map(|&(_, draw)| draw).collect();
    chosen.sort_unstable();
    chosen.try_into().expect("sixteen draws")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(dynamics: f64) -> BowParams {
        BowParams {
            ratio: 1.0,
            dynamics,
            position: 0.125,
            vibrato: 0.0,
            rate: 5.6,
            attack: 0.08,
            timing: 0.0,
            spread: 0.0,
            width: 0.7,
            pan: 0.0,
            level: 1.0,
        }
    }

    fn line(notes: &[(u64, u64, f64, bool, f64)]) -> Arc<Line> {
        Arc::new(Line {
            notes: notes
                .iter()
                .map(
                    |&(on_frame, off_frame, pitch_hz, slur, slide_seconds)| LineNote {
                        on_frame,
                        off_frame,
                        pitch_hz,
                        slur,
                        slide_seconds,
                    },
                )
                .collect(),
            address: "line".into(),
        })
    }

    /// `frames` of one section playing `line` from frame 0.
    fn render(bow: &mut Bow, params: &BowParams, frames: u64) -> Vec<[f64; 2]> {
        let notes = bow.line.clone().unwrap();
        (0..frames)
            .map(|frame| {
                let reached = notes
                    .notes
                    .partition_point(|note| note.on_frame <= frame)
                    .max(1)
                    - 1;
                bow.sample(frame, params, notes.notes[reached].pitch_hz)
                    .unwrap()
            })
            .collect()
    }

    /// One bare string held at a constant bow, with loop correction `comp`.
    fn tone(frequency: f64, comp: f64, dynamics: f64, frames: usize) -> Vec<f64> {
        let mut bow = Bow::new(1, BowBody::None, 1, 1).unwrap();
        bow.play(line(&[(0, frames as u64, frequency, false, 0.0)]))
            .unwrap();
        bow.start(&params(dynamics), 1).unwrap();
        let player = &mut bow.section[0];
        let speed = 0.03 + 0.085 * dynamics;
        let force = 0.5 + 0.35 * dynamics;
        let position = 0.125f64.max(bow_floor(frequency));
        (0..frames)
            .map(|_| {
                player
                    .waveguide(RATE_HZ / frequency - comp, position, speed, force)
                    .unwrap()
            })
            .collect()
    }

    /// The level of `samples` at `frequency`, Hann-windowed.
    fn level(samples: &[f64], frequency: f64) -> f64 {
        let n = samples.len() as f64;
        let (mut re, mut im) = (0.0, 0.0);
        for (index, value) in samples.iter().enumerate() {
            let t = index as f64;
            let window = 0.5 - 0.5 * (std::f64::consts::TAU * t / n).cos();
            let angle = std::f64::consts::TAU * frequency * t / RATE_HZ;
            re += value * window * angle.cos();
            im -= value * window * angle.sin();
        }
        (re * re + im * im).sqrt()
    }

    /// The played pitch near `expected`, in cents from it: the strongest of a
    /// grid of candidates a quarter cent apart, refined by a parabola.
    fn cents_off(samples: &[f64], expected: f64) -> f64 {
        let candidates: Vec<f64> = (-120..=120).map(|step| f64::from(step) * 0.25).collect();
        let levels: Vec<f64> = candidates
            .iter()
            .map(|cents| level(samples, expected * 2f64.powf(cents / 1200.0)))
            .collect();
        let best = (1..levels.len() - 1)
            .max_by(|&a, &b| levels[a].total_cmp(&levels[b]))
            .unwrap();
        let (a, b, c) = (
            levels[best - 1].ln(),
            levels[best].ln(),
            levels[best + 1].ln(),
        );
        candidates[best] + 0.25 * 0.5 * (a - c) / (a - 2.0 * b + c)
    }

    /// Print the tuning table: for each key, the correction that zeroes the
    /// pitch error, found by a secant through corrections 1 and 2.
    #[test]
    #[ignore = "prints the TUNING table; run with --release --ignored"]
    fn measure_tuning_table() {
        let mut rows = Vec::new();
        for key in 21..=102 {
            let frequency = 440.0 * 2f64.powf(f64::from(key - 69) / 12.0);
            let row: Vec<String> = TUNING_DYNAMICS
                .iter()
                .map(|&dynamics| {
                    // At least 40 periods, after half a second to settle.
                    let window = ((40.0 * RATE_HZ / frequency) as usize).max(24_000);
                    let error = |comp: f64| {
                        let samples = tone(frequency, comp, dynamics, 24_000 + window);
                        cents_off(&samples[24_000..], frequency)
                    };
                    // Secant steps from corrections 1 and 2 until within 0.1 cent.
                    let (mut a, mut b) = (1.0, 2.0);
                    let (mut ea, mut eb) = (error(a), error(b));
                    for _ in 0..6 {
                        if eb.abs() < 0.1 || eb == ea {
                            break;
                        }
                        let next = (b - eb * (b - a) / (eb - ea)).clamp(0.8, 2.5);
                        (a, ea) = (b, eb);
                        b = next;
                        eb = error(b);
                    }
                    format!("{b:.4}")
                })
                .collect();
            rows.push(format!("[{}]", row.join(", ")));
        }
        let lines: Vec<String> = rows.chunks(3).map(|chunk| chunk.join(", ")).collect();
        println!(
            "const TUNING: [[f64; 3]; 82] = [\n    {},\n];",
            lines.join(",\n    ")
        );
    }

    #[test]
    #[ignore = "prints the body tables; run with --release --ignored"]
    fn measure_body_tables() {
        for family in [
            BowBody::Violin,
            BowBody::Viola,
            BowBody::Cello,
            BowBody::Bass,
        ] {
            println!("{}: {:?}", family.name(), choose_bodies(family));
        }
    }

    #[test]
    fn body_tables_hold_the_most_even_draws() {
        assert_eq!(choose_bodies(BowBody::Violin), VIOLIN_BODIES);
        assert_eq!(choose_bodies(BowBody::Viola), VIOLA_BODIES);
        assert_eq!(choose_bodies(BowBody::Cello), CELLO_BODIES);
        assert_eq!(choose_bodies(BowBody::Bass), BASS_BODIES);
    }

    #[test]
    fn bodies_have_unit_energy() {
        for family in [BowBody::Violin, BowBody::Bass] {
            let body = &table(family)[0];
            let mut state = vec![(0.0, 0.0); body.modes.len()];
            let mut energy = 0.0;
            for frame in 0..96_000 {
                let input = if frame == 0 { 1.0 } else { 0.0 };
                let mut sum = 0.0;
                for (state, mode) in state.iter_mut().zip(&body.modes) {
                    let re = mode.pole_re * state.0 - mode.pole_im * state.1 + input;
                    let im = mode.pole_re * state.1 + mode.pole_im * state.0;
                    *state = (re, im);
                    sum += mode.amplitude * im;
                }
                energy += sum * sum;
            }
            assert!((energy - 1.0).abs() < 1e-9, "{family:?} energy {energy}");
        }
    }

    /// The pitch of a held note at `dynamics`, in cents from `frequency`,
    /// over 40 periods once it has settled: after half a second, or a second
    /// below G3, where a soft bow wavers by a few cents at first.
    fn held_cents(frequency: f64, dynamics: f64) -> f64 {
        let window = ((40.0 * RATE_HZ / frequency) as usize).max(12_000);
        let settle = if frequency < 196.0 { 48_000 } else { 24_000 };
        let frames = settle + window;
        let mut bow = Bow::new(1, BowBody::None, 1, 1).unwrap();
        bow.play(line(&[(0, frames as u64, frequency, false, 0.0)]))
            .unwrap();
        let output = render(&mut bow, &params(dynamics), frames as u64);
        let samples: Vec<f64> = output[settle..].iter().map(|frame| frame[0]).collect();
        cents_off(&samples, frequency)
    }

    #[test]
    fn plays_in_tune_from_the_basses_to_the_violins() {
        for key in (28..=95).step_by(7) {
            let frequency = 440.0 * 2f64.powf(f64::from(key - 69) / 12.0);
            for dynamics in [0.2, 0.5, 0.9] {
                let cents = held_cents(frequency, dynamics);
                assert!(
                    cents.abs() < 3.0,
                    "key {key} at {dynamics}: {cents:.2} cents"
                );
            }
        }
    }

    /// The 10 ms frames, from 200 ms into a held note, where the octave is
    /// louder than the fundamental: the string has left its normal motion.
    fn octave_frames(
        frequency: f64,
        dynamics: f64,
        position: f64,
        frames: u64,
        attack: f64,
    ) -> usize {
        let mut bow = Bow::new(1, BowBody::None, 1, 1).unwrap();
        bow.play(line(&[(0, frames, frequency, false, 0.0)]))
            .unwrap();
        let mut settings = params(dynamics);
        settings.position = position;
        settings.attack = attack;
        let output = render(&mut bow, &settings, frames);
        let samples: Vec<f64> = output.iter().map(|frame| frame[0]).collect();
        let window = ((4.0 * RATE_HZ / frequency) as usize).max(480);
        (9_600..samples.len() - window)
            .step_by(480)
            .filter(|&start| {
                let segment = &samples[start..start + window];
                level(segment, 2.0 * frequency) > level(segment, frequency)
            })
            .count()
    }

    #[test]
    #[ignore = "sweeps every family's range; run with --release --ignored"]
    fn stays_in_normal_motion_everywhere() {
        let mut failures = Vec::new();
        let positions: Vec<f64> = std::env::var("BOW_POSITIONS")
            .map(|list| {
                list.split(',')
                    .map(|value| value.parse().unwrap())
                    .collect()
            })
            .unwrap_or_else(|_| vec![0.1, 0.125, 1.0 / 7.0]);
        let families =
            std::env::var("BOW_FAMILIES").unwrap_or_else(|_| "violin,viola,cello,bass".into());
        let attack: f64 = std::env::var("BOW_ATTACK")
            .map(|value| value.parse().unwrap())
            .unwrap_or(0.08);
        for (family, low, high) in [
            (BowBody::Violin, 55, 95),
            (BowBody::Viola, 48, 88),
            (BowBody::Cello, 36, 81),
            (BowBody::Bass, 28, 67),
        ] {
            if !families.split(',').any(|name| name == family.name()) {
                continue;
            }
            for key in low..=high {
                let frequency = 440.0 * 2f64.powf(f64::from(key - 69) / 12.0);
                for &position in &positions {
                    for dynamics in [0.0, 0.2, 0.5, 0.9, 1.0] {
                        let bad = octave_frames(frequency, dynamics, position, 28_800, attack);
                        if bad > 0 {
                            failures.push(format!(
                                "{}:{key}@p{position:.3}/d{dynamics}:{bad}",
                                family.name()
                            ));
                        }
                    }
                }
            }
        }
        println!("{} failures: {}", failures.len(), failures.join(" "));
    }

    #[test]
    fn stays_in_normal_motion_on_sampled_notes() {
        for (key, position) in [
            (55, 0.125),
            (72, 0.125),
            (82, 0.125),
            (94, 0.125),
            (38, 0.1),
            (41, 0.1),
            (46, 0.1),
            (53, 0.1),
        ] {
            let frequency = 440.0 * 2f64.powf(f64::from(key - 69) / 12.0);
            for dynamics in [0.2, 0.5] {
                let bad = octave_frames(frequency, dynamics, position, 28_800, 0.08);
                assert_eq!(bad, 0, "key {key} at {position} and {dynamics}");
            }
        }
    }

    #[test]
    fn identical_sources_render_identically() {
        let notes = [
            (0, 30_000, 523.25, false, 0.0),
            (30_000, 60_000, 587.33, true, 0.12),
        ];
        let mut first = Bow::new(4, BowBody::Violin, 2, 7).unwrap();
        let mut second = Bow::new(4, BowBody::Violin, 2, 7).unwrap();
        first.play(line(&notes)).unwrap();
        second.play(line(&notes)).unwrap();
        let mut spread = params(0.6);
        spread.spread = 1.0;
        spread.timing = 0.018;
        spread.vibrato = 12.5;
        let a = render(&mut first, &spread, 40_000);
        let b = render(&mut second, &spread, 40_000);
        assert!(a
            .iter()
            .zip(&b)
            .all(|(a, b)| a[0].to_bits() == b[0].to_bits() && a[1].to_bits() == b[1].to_bits()));
    }

    #[test]
    fn a_frame_reads_only_half_a_second_ahead() {
        let full = [
            (0, 20_000, 440.0, false, 0.0),
            (20_000, 60_000, 493.88, false, 0.2),
            (60_000, 90_000, 523.25, true, 0.0),
        ];
        let mut settings = params(0.6);
        settings.spread = 1.0;
        settings.timing = 0.03;
        settings.vibrato = 12.5;
        let mut whole = Bow::new(3, BowBody::Violin, 1, 3).unwrap();
        whole.play(line(&full)).unwrap();
        let mut cut = Bow::new(3, BowBody::Violin, 1, 3).unwrap();
        // Without the third note, and with the second ending where the third began.
        cut.play(line(&full[..2])).unwrap();
        let frames = 60_000 - LOOKAHEAD_FRAMES;
        let a = render(&mut whole, &settings, frames);
        let b = render(&mut cut, &settings, frames);
        assert!(a
            .iter()
            .zip(&b)
            .all(|(a, b)| a[0].to_bits() == b[0].to_bits()));
    }

    #[test]
    fn preview_matches_the_next_sample_without_advancing() {
        let mut bow = Bow::new(2, BowBody::Viola, 2, 5).unwrap();
        bow.play(line(&[(0, 9_000, 330.0, false, 0.0)])).unwrap();
        let settings = params(0.5);
        render(&mut bow, &settings, 4_000);
        let preview = bow.preview(4_000, &settings, 330.0).unwrap();
        let again = bow.preview(4_000, &settings, 330.0).unwrap();
        let actual = bow.sample(4_000, &settings, 330.0).unwrap();
        assert_eq!(preview[0].to_bits(), again[0].to_bits());
        assert_eq!(preview[0].to_bits(), actual[0].to_bits());
        assert_eq!(preview[1].to_bits(), actual[1].to_bits());
    }
}
