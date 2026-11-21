//! A generated song and random edits of it, shared by the checkpoint tests.
//! The song is a bowed line with joins and slides, polyphonic sine notes, a
//! reverb, a delay loop and two automation lanes.
#![allow(dead_code)]

use maac::compiler::compile_bundle_artifact_with_limits;
use maac::plan::PlanLimits;
use maac::{PlanArtifact, SourceBundle};

pub const PITCHES: [&str; 12] = [
    "C4", "D4", "E4", "F#4", "G4", "A4", "Bb4", "C5", "D5", "Eb5", "F5", "G5",
];
/// Eighth notes in the pattern: 8q.
pub const LENGTH: i32 = 16;
/// An eighth note at 120 bpm and 48 kHz.
pub const EIGHTH: u64 = 12_000;

pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    pub fn range(&mut self, low: i32, high: i32) -> i32 {
        low + self.below((high - low + 1) as u64) as i32
    }
}

#[derive(Clone, Debug)]
pub struct Note {
    pub name: String,
    pub strings: bool,
    pub at: i32,
    pub dur: i32,
    pub pitch: usize,
    pub velocity: i32,
    pub slide_ms: i32,
    pub slur: bool,
}

#[derive(Clone, Debug)]
pub struct Knot {
    pub at: i32,
    pub value: i32,
    pub linear: bool,
}

#[derive(Clone, Debug)]
pub struct Song {
    pub notes: Vec<Note>,
    pub next_name: usize,
    /// The fader level, in dB, and where its lane starts, in eighths.
    pub fader: Vec<Knot>,
    pub fader_at: i32,
    /// The reverb mix, in tenths, and where its lane starts.
    pub room: Vec<Knot>,
    pub room_at: i32,
}

pub fn eighths(count: i32) -> String {
    format!("{count}/2q")
}

pub fn lane_text(
    name: &str,
    target: &str,
    knots: &[Knot],
    start: i32,
    unit: impl Fn(i32) -> String,
) -> String {
    if knots.is_empty() {
        return String::new();
    }
    let points: Vec<String> = knots
        .iter()
        .enumerate()
        .map(|(index, knot)| {
            // The last knot of a curve must be a step.
            let linear = knot.linear && index + 1 < knots.len();
            format!(
                "({}, {}, {})",
                eighths(knot.at),
                unit(knot.value),
                if linear { "linear" } else { "step" }
            )
        })
        .collect();
    format!(
        "curve {name}_curve {{ clock = score; points = [{}]; }}\nautomation {name} {{ target = &{target}; curve = &{name}_curve; at = {}; }}\n",
        points.join(", "),
        eighths(start)
    )
}

pub fn source(song: &Song) -> String {
    let mut patterns = String::new();
    for (strings, pattern) in [(true, "line"), (false, "chords")] {
        let mut notes: Vec<&Note> = song.notes.iter().filter(|n| n.strings == strings).collect();
        notes.sort_by_key(|n| (n.at, n.name.clone()));
        let body: Vec<String> = notes
            .iter()
            .map(|n| {
                let mut text = format!(
                    "note {} {{ at = {}; dur = {}; pitch = {}; velocity = {}/10;",
                    n.name,
                    eighths(n.at),
                    eighths(n.dur),
                    PITCHES[n.pitch],
                    n.velocity
                );
                // Only a note that starts where another ends may slide or slur.
                let touching = song
                    .notes
                    .iter()
                    .any(|m| m.strings && m.name != n.name && m.at + m.dur == n.at);
                if n.slide_ms > 0 && touching {
                    text += &format!(" slide = {}ms;", n.slide_ms);
                }
                if n.slur && touching {
                    text += " slur = true;";
                }
                text + " }"
            })
            .collect();
        if !body.is_empty() {
            let track = if strings {
                "strings_line"
            } else {
                "winds_line"
            };
            patterns += &format!(
                "pattern {pattern} {{ length = 8q;\n  {}\n}}\nplace {pattern}_at {{ pattern = &{pattern}; track = &{track}; at = 0q; }}\n",
                body.join("\n  ")
            );
        }
    }
    format!(
        r#"maac 1;
project p {{ score = [0q, 8q]; rate = 48000Hz; tail = 1s; tempo = &clock; meter = &metre; output = &fader:out; requires = ["maac.production/1"]; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node strings {{ instrument = &bowed; config = {{ voices = 4; }}; }}
node winds {{ instrument = &tone; config = {{ voices = 6; }}; }}
node room {{ type = "fx.reverb/1"; config = {{ channels = 1; }}; params = {{ mix = 3/10; }}; }}
node mix {{ type = "core.sum/1"; config = {{ channels = 1; }}; }}
node delay {{ type = "core.delay/1"; config = {{ channels = 1; frames = 2400; }}; }}
node feedback {{ type = "core.gain/1"; config = {{ channels = 1; }}; params = {{ gain = 2/5; }}; }}
node fader {{ type = "core.fader/1"; config = {{ channels = 1; }}; params = {{ level = -6dB; }}; }}
connect strings_room {{ from = &strings:out; to = &room:in; }}
connect room_mix {{ from = &room:out; to = &mix:in; }}
connect winds_mix {{ from = &winds:out; to = &mix:in; }}
connect delayed_signal {{ from = &mix:out; to = &delay:in; }}
connect echo_gain {{ from = &delay:out; to = &feedback:in; }}
connect echo_return {{ from = &feedback:out; to = &mix:in; }}
connect mix_fader {{ from = &mix:out; to = &fader:in; }}
track strings_line {{ target = &strings:events; }}
track winds_line {{ target = &winds:events; }}
{patterns}{}{}
instrument bowed {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &section:out;
  node amp {{ type = "synth.adsr/1"; params = {{ sustain = 1; release = 400ms; }}; }}
  node section {{ type = "synth.bow/1"; config = {{ players = 2; }}; params = {{ level = 1/10; }}; }}
 }}
}}
instrument tone {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &osc:out;
  node amp {{ type = "synth.adsr/1"; params = {{ release = 400ms; }}; }}
  node osc {{ type = "synth.sine/1"; params = {{ level = 1/10; }}; }}
 }}
}}
"#,
        lane_text(
            "ride",
            "fader.params.level",
            &song.fader,
            song.fader_at,
            |v| format!("{v}dB")
        ),
        lane_text(
            "swell",
            "room.params.mix",
            &song.room,
            song.room_at,
            |v| format!("{v}/10")
        ),
    )
}

pub fn compile(song: &Song) -> Result<PlanArtifact, String> {
    compile_bundle_artifact_with_limits(
        &SourceBundle::new("main.maac", source(song)),
        &PlanLimits::song(),
    )
    .map_err(|error| format!("{error:?}"))
}

pub fn new_note(rng: &mut Rng, song: &mut Song, strings: bool, at: i32) -> Note {
    let dur = rng.range(1, 4).min(LENGTH - at).max(1);
    song.next_name += 1;
    Note {
        name: format!("n{}", song.next_name),
        strings,
        at,
        dur,
        pitch: rng.below(PITCHES.len() as u64) as usize,
        velocity: rng.range(3, 9),
        slide_ms: if strings && rng.below(3) == 0 { 120 } else { 0 },
        slur: strings && rng.below(4) == 0,
    }
}

pub fn base_song(seed: u64) -> Song {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut song = Song {
        notes: Vec::new(),
        next_name: 0,
        fader: vec![
            Knot {
                at: 0,
                value: -12,
                linear: true,
            },
            Knot {
                at: 6,
                value: -4,
                linear: false,
            },
            Knot {
                at: 12,
                value: -9,
                linear: true,
            },
        ],
        fader_at: 0,
        room: Vec::new(),
        room_at: 0,
    };
    // A bowed line whose notes often touch, so that they join.
    let mut cursor = 0;
    while cursor < LENGTH - 1 && song.notes.len() < 6 {
        let note = new_note(&mut rng, &mut song, true, cursor);
        cursor = note.at + note.dur + rng.range(0, 1);
        song.notes.push(note);
    }
    for _ in 0..5 {
        let at = rng.range(0, LENGTH - 2);
        let note = new_note(&mut rng, &mut song, false, at);
        song.notes.push(note);
    }
    song
}

/// An edit and what it is called.
pub fn edit(song: &Song, rng: &mut Rng, choice: u64) -> (Song, &'static str) {
    let mut next = song.clone();
    let count = next.notes.len();
    let pick = rng.below(count.max(1) as u64) as usize;
    match choice % 15 {
        0 if count > 0 => {
            let note = &mut next.notes[pick];
            note.at = rng.range(0, LENGTH - note.dur);
            (next, "move")
        }
        1 if count > 0 => {
            next.notes[pick].pitch = rng.below(PITCHES.len() as u64) as usize;
            (next, "pitch")
        }
        2 if count > 0 => {
            let note = &mut next.notes[pick];
            note.dur = rng.range(1, 4).min(LENGTH - note.at).max(1);
            (next, "duration")
        }
        3 if count > 0 => {
            next.notes[pick].velocity = rng.range(3, 9);
            (next, "velocity")
        }
        4 if count > 1 => {
            next.notes.remove(pick);
            (next, "delete")
        }
        5 | 6 => {
            let strings = rng.below(2) == 0;
            let at = rng.range(0, LENGTH - 2);
            let note = new_note(rng, &mut next, strings, at);
            next.notes.push(note);
            (next, "add")
        }
        7 => {
            let note = next.notes.iter_mut().filter(|n| n.strings).nth(pick % 3);
            if let Some(note) = note {
                note.slide_ms = if note.slide_ms == 0 { 120 } else { 0 };
                note.slur = !note.slur;
            }
            (next, "slide")
        }
        8..=14 => {
            let room = rng.below(3) == 0;
            let (lane, start, low, high) = if room {
                (&mut next.room, &mut next.room_at, 0, 10)
            } else {
                (&mut next.fader, &mut next.fader_at, -24, 0)
            };
            let kind = match choice % 15 - 8 {
                _ if lane.is_empty() => "lane-add",
                0 => "knot-value",
                1 => "knot-move",
                2 => "knot-add",
                3 => "knot-remove",
                4 => "knot-shape",
                5 => "lane-start",
                _ => "lane-remove",
            };
            match kind {
                "lane-start" => *start = rng.range(0, 4),
                "lane-add" => {
                    *start = rng.range(0, 3);
                    *lane = vec![
                        Knot {
                            at: rng.range(0, 4),
                            value: rng.range(low, high),
                            linear: true,
                        },
                        Knot {
                            at: rng.range(6, 14),
                            value: rng.range(low, high),
                            linear: rng.below(2) == 0,
                        },
                    ];
                }
                "knot-value" => {
                    let i = rng.below(lane.len() as u64) as usize;
                    lane[i].value = rng.range(low, high);
                }
                "knot-move" => {
                    let i = rng.below(lane.len() as u64) as usize;
                    let floor = if i == 0 { 0 } else { lane[i - 1].at + 1 };
                    let ceiling = lane.get(i + 1).map_or(LENGTH, |k| k.at - 1);
                    if floor <= ceiling {
                        lane[i].at = rng.range(floor, ceiling);
                    }
                }
                "knot-add" => {
                    let at = rng.range(1, LENGTH - 1);
                    if lane.iter().all(|k| k.at != at) {
                        let place = lane.partition_point(|k| k.at < at);
                        lane.insert(
                            place,
                            Knot {
                                at,
                                value: rng.range(low, high),
                                linear: rng.below(2) == 0,
                            },
                        );
                    }
                }
                "knot-remove" => {
                    if lane.len() > 1 {
                        let i = rng.below(lane.len() as u64) as usize;
                        lane.remove(i);
                    }
                }
                "knot-shape" => {
                    let i = rng.below(lane.len() as u64) as usize;
                    lane[i].linear = !lane[i].linear;
                }
                _ => lane.clear(),
            }
            (next, kind)
        }
        _ => (next, "noop"),
    }
}
