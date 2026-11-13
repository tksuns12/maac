//! A project around one node that plays notes, at 48 kHz and 120 bpm, so
//! one frame is 1/24000q and 1/48000s. Tests that check exact frames write
//! their scene with it and compile it, in place of a hand-built plan.
//!
//! Each test file uses the part of this it needs.
#![allow(dead_code)]

use maac::bundle::SourceBundle;
use maac::compiler::{compile_bundle, compile_bundle_artifact};
use maac::plan::Plan;
use maac::PlanArtifact;

/// The source of the project. Notes are written at frames; everything else
/// is MaaC text.
pub struct FrameProject {
    /// Where the score starts, in quarter notes: `0`, `1`, `3/2`.
    pub start_q: String,
    /// The length of the score in 1/24000q, which is its frames at 120 bpm;
    /// and the frames of tail after it.
    pub frames: u64,
    pub tail_frames: u64,
    /// The points of the tempo map.
    pub tempo: String,
    /// The ID of the node that plays the notes.
    pub sound: String,
    /// The node the project renders: `sound`, or one declared in `more`.
    pub output: String,
    /// The children of the one pattern, which is placed at the score start.
    pub notes: String,
    /// Fields of that placement.
    pub place: String,
    /// Further top-level declarations.
    pub more: String,
}

impl FrameProject {
    /// A project whose notes go to node `sound`, declared in `declarations`.
    pub fn new(frames: u64, tail_frames: u64, sound: &str, declarations: &str) -> Self {
        Self {
            start_q: "0".to_owned(),
            frames,
            tail_frames,
            tempo: "(0q, 120bpm, step)".to_owned(),
            sound: sound.to_owned(),
            output: sound.to_owned(),
            notes: String::new(),
            place: String::new(),
            more: format!("{declarations}\n"),
        }
    }

    /// A `core.sine/1` named `sine` with `params`, written as fields:
    /// `attack = 0s; level = 1;`.
    pub fn sine(frames: u64, tail_frames: u64, voices: u32, params: &str) -> Self {
        Self::new(
            frames,
            tail_frames,
            "sine",
            &format!(
                "node sine {{ type = \"core.sine/1\"; config = {{ voices = {voices}; }}; params = {{ {params} }}; }}"
            ),
        )
    }

    /// A node named `instrument` with `voices` voices of the instrument
    /// `id`, which `declaration` declares.
    pub fn instrument(frames: u64, voices: u32, id: &str, declaration: &str) -> Self {
        Self::new(
            frames,
            0,
            "instrument",
            &format!(
                "{declaration}\nnode instrument {{ instrument = &{id}; config = {{ voices = {voices}; }}; }}"
            ),
        )
    }

    /// A note from `on` to `off`, in 1/24000q of the pattern, with further
    /// `fields` and children. Its velocity is 1 unless `fields` gives one.
    pub fn note(mut self, id: &str, on: u64, off: u64, pitch: &str, fields: &str) -> Self {
        let velocity = if fields.contains("velocity =") {
            ""
        } else {
            "velocity = 1; "
        };
        self.notes.push_str(&format!(
            "note {id} {{ at = {on}/24000q; dur = {}/24000q; pitch = {pitch}; {velocity}{fields} }}\n",
            off - on
        ));
        self
    }

    /// Further top-level declarations.
    pub fn with(mut self, declarations: &str) -> Self {
        self.more.push_str(declarations);
        self.more.push('\n');
        self
    }

    /// Start the score at `start_q` quarter notes.
    pub fn starting_at(mut self, start_q: &str) -> Self {
        self.start_q = start_q.to_owned();
        self
    }

    /// Render the sound through a `core.pan/1` named `pan`.
    pub fn through_pan(mut self, pan: &str) -> Self {
        self.output = "pan".to_owned();
        let sound = self.sound.clone();
        self.with(&format!(
            "node pan {{ type = \"core.pan/1\"; params = {{ pan = {pan}; }}; }}\n\
             connect {sound}_to_pan {{ from = &{sound}:out; to = &pan:in; }}"
        ))
    }

    pub fn source(&self) -> String {
        let Self {
            start_q,
            frames,
            tail_frames,
            tempo,
            sound,
            output,
            notes,
            place,
            more,
        } = self;
        let end_q = maac::parse_rational(start_q).expect("the score start is a rational")
            + maac::parse_rational(&format!("{frames}/24000")).unwrap();
        format!(
            r#"maac 1;
project p {{ score = [{start_q}q, {end_q}q]; tail = {tail_frames}/48000s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &{output}:out; }}
tempo clock {{ points = [{tempo}]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
track notes {{ target = &{sound}:events; }}
pattern phrase {{ length = {frames}/24000q;
{notes}}}
place play {{ pattern = &phrase; track = &notes; at = {start_q}q; {place} }}
{more}"#
        )
    }

    /// The compiled plan.
    pub fn plan(&self) -> Plan {
        let source = self.source();
        compile_bundle(&SourceBundle::new("main.maac", source.as_str()))
            .unwrap_or_else(|diagnostics| panic!("{diagnostics:?}\n{source}"))
    }

    /// The compiled plan artifact, which also holds what a `Plan` cannot:
    /// control nodes, modulations, audio and kits.
    pub fn artifact(&self) -> PlanArtifact {
        let source = self.source();
        compile_bundle_artifact(&SourceBundle::new("main.maac", source.as_str()))
            .unwrap_or_else(|diagnostics| panic!("{diagnostics:?}\n{source}"))
    }
}
