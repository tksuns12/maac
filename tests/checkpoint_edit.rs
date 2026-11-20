//! Resuming a render after an edit. Songs are generated from a small model
//! (a bowed line with joins and slides, polyphonic sine notes, a reverb, a
//! delay loop and two automation lanes), edited at random, and every edit is
//! checked against a fresh render of the edited song: the frames after the
//! reported bound must equal it bit for bit, with the checkpoint taken exactly
//! at the bound and on a coarse grid. A bound that is too late must be caught.

use maac::compiler::compile_bundle_artifact_with_limits;
use maac::dsp::{CheckpointSet, DspEngine, RenderSpan, Reuse};
use maac::plan::PlanLimits;
use maac::{PlanArtifact, SourceBundle};

const PITCHES: [&str; 10] = ["C4", "D4", "E4", "F4", "G4", "A4", "B4", "C5", "D5", "E5"];
/// Eighth notes in the pattern: 8q.
const LENGTH: i32 = 16;
/// An eighth note at 120 bpm and 48 kHz.
const EIGHTH: u64 = 12_000;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn range(&mut self, low: i32, high: i32) -> i32 {
        low + self.below((high - low + 1) as u64) as i32
    }
}

#[derive(Clone, Debug)]
struct Note {
    name: String,
    strings: bool,
    at: i32,
    dur: i32,
    pitch: usize,
    velocity: i32,
    slide_ms: i32,
    slur: bool,
}

#[derive(Clone, Debug)]
struct Knot {
    at: i32,
    value: i32,
    linear: bool,
}

#[derive(Clone, Debug)]
struct Song {
    notes: Vec<Note>,
    next_name: usize,
    /// The fader level, in dB.
    fader: Vec<Knot>,
    /// The reverb mix, in tenths.
    room: Vec<Knot>,
}

fn eighths(count: i32) -> String {
    format!("{count}/2q")
}

fn lane_text(name: &str, target: &str, knots: &[Knot], unit: impl Fn(i32) -> String) -> String {
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
        "curve {name}_curve {{ clock = score; points = [{}]; }}\nautomation {name} {{ target = &{target}; curve = &{name}_curve; at = 0q; }}\n",
        points.join(", ")
    )
}

fn source(song: &Song) -> String {
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
{patterns}{}{}"#,
        lane_text("ride", "fader.params.level", &song.fader, |v| format!(
            "{v}dB"
        )),
        lane_text("swell", "room.params.mix", &song.room, |v| format!(
            "{v}/10"
        )),
    )
}

fn compile(song: &Song) -> Result<PlanArtifact, String> {
    compile_bundle_artifact_with_limits(
        &SourceBundle::new("main.maac", source(song)),
        &PlanLimits::song(),
    )
    .map_err(|error| format!("{error:?}"))
}

fn new_note(rng: &mut Rng, song: &mut Song, strings: bool, at: i32) -> Note {
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

fn base_song(seed: u64) -> Song {
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
        room: Vec::new(),
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
fn edit(song: &Song, rng: &mut Rng, choice: u64) -> (Song, &'static str) {
    let mut next = song.clone();
    let count = next.notes.len();
    let pick = rng.below(count.max(1) as u64) as usize;
    match choice % 14 {
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
        8..=13 => {
            let room = rng.below(3) == 0;
            let (lane, low, high) = if room {
                (&mut next.room, 0, 10)
            } else {
                (&mut next.fader, -24, 0)
            };
            let kind = match choice % 14 - 8 {
                _ if lane.is_empty() => "lane-add",
                0 => "knot-value",
                1 => "knot-move",
                2 => "knot-add",
                3 => "knot-remove",
                4 => "knot-shape",
                _ => "lane-remove",
            };
            match kind {
                "lane-add" => {
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

fn limits() -> PlanLimits {
    PlanLimits::song()
}

fn bits(frames: &[f64]) -> Vec<u64> {
    frames.iter().map(|sample| sample.to_bits()).collect()
}

fn render_full(plan: &PlanArtifact) -> Result<Vec<f64>, String> {
    let mut out = Vec::new();
    DspEngine::new_artifact_with_limits(plan, &limits())
        .map_err(|e| format!("{e:?}"))?
        .render(|frame| {
            out.extend_from_slice(frame);
            Ok(())
        })
        .map_err(|e| format!("{e:?}"))?;
    Ok(out)
}

/// The frames after a resume, from `set`, trusting `bound` when given.
fn resumed_tail(
    set: &CheckpointSet,
    plan: &PlanArtifact,
    bound: Option<u64>,
) -> Result<(u64, Vec<f64>), String> {
    let mut resumed = set.resume_with(plan, bound).map_err(|e| format!("{e:?}"))?;
    let total = plan.output().total_frames;
    let start = resumed.start_frame;
    let mut out = Vec::new();
    resumed
        .engine
        .render_span(RenderSpan { start, end: total }, |frame| {
            out.extend_from_slice(frame);
            Ok(())
        })
        .map_err(|e| format!("{e:?}"))?;
    Ok((start, out))
}

#[derive(Default)]
struct Tally {
    accepted: usize,
    rejected: usize,
    unchanged: usize,
    full: usize,
    partial: usize,
    resumed_late: usize,
    caught_late: usize,
    note_edits_late: usize,
    note_edits_caught: usize,
    kinds: std::collections::BTreeMap<&'static str, usize>,
}

/// Every edit in `edits` of the song `seed`, checked three ways.
fn fuzz(seed: u64, edits: usize, tally: &mut Tally) {
    let base = base_song(seed);
    let base_plan = compile(&base).unwrap_or_else(|error| panic!("seed {seed} base: {error}"));
    let grid = CheckpointSet::render(&base_plan, &limits(), 4_801, |_| Ok(())).unwrap();
    let channels = usize::from(base_plan.output().channels);
    let mut rng = Rng(seed ^ 0xA5A5_5A5A_1234_5678);
    for index in 0..edits {
        let (song, kind) = edit(&base, &mut rng, seed + index as u64);
        let plan = match compile(&song) {
            Ok(plan) => plan,
            Err(_) => {
                tally.rejected += 1;
                continue;
            }
        };
        let Ok(fresh) = render_full(&plan) else {
            tally.rejected += 1;
            continue;
        };
        tally.accepted += 1;
        *tally.kinds.entry(kind).or_default() += 1;
        let reuse = grid.reuse_for(&plan).unwrap();
        let context = format!("seed {seed} edit {kind} reuse {reuse:?}");

        // On the coarse grid.
        let (start, tail) = resumed_tail(&grid, &plan, None)
            .unwrap_or_else(|error| panic!("{context}: grid resume failed: {error}"));
        assert_eq!(
            bits(&fresh[start as usize * channels..]),
            bits(&tail),
            "{context}: grid, resumed at {start}"
        );

        match reuse {
            Reuse::Unchanged => tally.unchanged += 1,
            Reuse::Full(_) => tally.full += 1,
            Reuse::From(bound) => {
                tally.partial += 1;
                // With a checkpoint exactly at the bound, and just after it.
                let total = plan.output().total_frames;
                let late = [bound + 1, bound + 97].map(|frame| frame.min(total - 1));
                let frames = [bound, late[0], late[1]];
                let exact =
                    CheckpointSet::render_at(&base_plan, &limits(), &frames, |_| Ok(())).unwrap();
                let (start, tail) = resumed_tail(&exact, &plan, None).unwrap();
                assert_eq!(start, bound, "{context}: no checkpoint at the bound");
                assert_eq!(
                    bits(&fresh[start as usize * channels..]),
                    bits(&tail),
                    "{context}: exact checkpoint at {bound}"
                );
                // A bound that is too late must give wrong frames.
                for frame in late {
                    if frame <= bound {
                        continue;
                    }
                    // Wrong state may give wrong frames or an outright error
                    // (a note-off for a voice the old state never started).
                    let wrong = match resumed_tail(&exact, &plan, Some(frame)) {
                        Ok((start, tail)) => {
                            assert_eq!(start, frame);
                            bits(&fresh[start as usize * channels..]) != bits(&tail)
                        }
                        Err(_) => true,
                    };
                    tally.resumed_late += 1;
                    tally.caught_late += usize::from(wrong);
                    if matches!(
                        kind,
                        "move" | "pitch" | "duration" | "velocity" | "delete" | "add"
                    ) {
                        tally.note_edits_late += 1;
                        tally.note_edits_caught += usize::from(wrong);
                    }
                }
            }
        }
    }
}

#[test]
fn an_edited_song_resumes_exactly_from_the_reported_bound() {
    let mut tally = Tally::default();
    // Fourteen consecutive edits cover every kind once.
    for seed in [1, 20] {
        fuzz(seed, 14, &mut tally);
    }
    eprintln!(
        "accepted {} rejected {} unchanged {} full {} partial {}; late bounds caught {}/{} (note edits {}/{}); kinds {:?}",
        tally.accepted,
        tally.rejected,
        tally.unchanged,
        tally.full,
        tally.partial,
        tally.caught_late,
        tally.resumed_late,
        tally.note_edits_caught,
        tally.note_edits_late,
        tally.kinds
    );
    assert!(tally.accepted >= 18, "{}", tally.accepted);
    assert!(tally.partial >= 10, "{}", tally.partial);
    // A diff that always says "frame 0" would pass every exactness check.
    assert!(
        tally.full * 4 <= tally.accepted,
        "too many full renders: {}",
        tally.full
    );
    // The fuzz must be able to see a bound that is too late.
    assert!(tally.note_edits_late >= 8, "{}", tally.note_edits_late);
    assert!(
        tally.note_edits_caught * 10 >= tally.note_edits_late * 8,
        "only {} of {} late bounds on note edits were caught",
        tally.note_edits_caught,
        tally.note_edits_late
    );
    for kind in ["move", "pitch", "add", "knot-value"] {
        assert!(
            tally.kinds.get(kind).copied().unwrap_or(0) >= 1,
            "no {kind} edit ran"
        );
    }
}

/// A late note changed in a long song reports a bound near its onset, not 0.
#[test]
fn a_late_note_edit_reports_a_late_bound() {
    let base = base_song(7);
    let plan = compile(&base).unwrap();
    let grid = CheckpointSet::render(&plan, &limits(), 4_801, |_| Ok(())).unwrap();
    let mut edited = base.clone();
    let late = edited
        .notes
        .iter_mut()
        .filter(|n| !n.strings)
        .max_by_key(|n| n.at)
        .unwrap();
    late.pitch = (late.pitch + 1) % PITCHES.len();
    let on = late.at as u64 * EIGHTH;
    let new_plan = compile(&edited).unwrap();
    match grid.reuse_for(&new_plan).unwrap() {
        Reuse::From(frame) => assert_eq!(
            frame, on,
            "a changed winds note starts the bound at its onset"
        ),
        other => panic!("{other:?}"),
    }
}

/// Many more songs and edits; slow. `MAAC_FUZZ_SEEDS=a..b` picks the seeds.
#[test]
#[ignore]
fn an_edited_song_resumes_exactly_over_many_seeds() {
    let range = std::env::var("MAAC_FUZZ_SEEDS").unwrap_or_else(|_| "100..130".into());
    let (low, high) = range.split_once("..").expect("a..b");
    let mut tally = Tally::default();
    for seed in low.parse::<u64>().unwrap()..high.parse::<u64>().unwrap() {
        fuzz(seed, 14, &mut tally);
    }
    eprintln!(
        "accepted {} rejected {} unchanged {} full {} partial {}; late caught {}/{}; note edits {}/{}; kinds {:?}",
        tally.accepted, tally.rejected, tally.unchanged, tally.full, tally.partial,
        tally.caught_late, tally.resumed_late, tally.note_edits_caught, tally.note_edits_late, tally.kinds
    );
    assert!(tally.note_edits_caught * 10 >= tally.note_edits_late * 8);
}
