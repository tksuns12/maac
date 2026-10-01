//! Shared inventory for the D02 structure and D05 value-type tests: a valid
//! base composition, each core kind's location and fields as the
//! specification states them, and helpers that validate one instance through
//! the public Document-profile editing context.
#![allow(dead_code)]

use maac::editing::{AuthoredDocument, EditContext, EditError, FoundationEditContext};

pub const HASH: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// A valid composition supplying every object the instances below refer to.
pub fn base() -> String {
    format!(
        r#"maac 1;
project song {{ score = [0q, 8q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &mix:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node mix {{ type = "core.sum/1"; config = {{ channels = 1; }}; }}
node synth {{ type = "core.sine/1"; }}
connect synth_out {{ from = &synth:out; to = &mix:in; }}
node spare {{ type = "core.sine/1"; }}
node kit {{ type = "core.kit/1"; config = {{ channels = 1; samples = [{{ key = "k"; asset = &take; }}]; }}; }}
node wobble {{ type = "core.lfo/1"; config = {{ period = 1q; wave = sine; phase = 0; }}; }}
asset take {{ kind = audio; path = "take.pcm"; hash = "{HASH}"; format = "pcm_f32le_interleaved/1"; rate = 48000Hz; channels = 1; frames = 48000; }}
track lead {{ target = &synth:events; }}
track drums {{ target = &kit:events; }}
track clips {{}}
curve level_shape {{ clock = score; points = [(0q, 1/2, step)]; }}
curve expr_shape {{ clock = normalized; points = [(0, 0, linear), (1, 1, step)]; }}
curve seconds_shape {{ clock = seconds; points = [(0s, 1/2, step)]; }}
node lp {{ type = "core.onepole/1"; config = {{ channels = 1; }}; }}
connect into_lp {{ from = &spare:out; to = &lp:in; }}
pattern cell {{ length = 1q; note n {{ at = 0q; dur = 1/2q; pitch = C4; }} }}
"#
    )
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Location {
    TopLevel,
    /// Inside `pattern host`.
    Pattern,
    /// Inside `note host_note` in `pattern host`.
    Note,
    /// Inside `place host`.
    Place,
}

pub struct Kind {
    /// Inventory name; `audio_leaf` is the §9.1 pattern child form.
    pub name: &'static str,
    pub kind: &'static str,
    pub location: Location,
    pub minimal: &'static str,
    /// Every optional field the specification documents, with valid values.
    pub full: &'static str,
    pub required: &'static [&'static str],
    pub children: &'static [&'static str],
}

pub const NESTED_EVENTS: &[&str] = &["note", "hit", "message", "audio", "use"];

/// §§5–19: kinds, locations, fields and permitted children.
pub const KINDS: &[Kind] = &[
    Kind {
        name: "tempo",
        kind: "tempo",
        location: Location::TopLevel,
        minimal: "points = [(0q, 90bpm, step)];",
        full: r#"points = [(0q, 90bpm, linear), (4q, 120bpm, step)]; label = "t";"#,
        required: &["points"],
        children: &[],
    },
    Kind {
        name: "meter",
        kind: "meter",
        location: Location::TopLevel,
        minimal: "points = [(0q, 3, 4)];",
        full: r#"points = [(0q, 3, 4), (3q, 4, 4)]; label = "m";"#,
        required: &["points"],
        children: &[],
    },
    Kind {
        name: "tuning",
        kind: "tuning",
        location: Location::TopLevel,
        minimal: "period = 1200ct; steps = [0ct, 700ct]; reference_index = 0; reference_frequency = 440Hz;",
        full: r#"period = 1200ct; steps = [0ct, 700ct]; reference_index = -3; reference_frequency = 1/2kHz; label = "j";"#,
        required: &["period", "steps", "reference_index", "reference_frequency"],
        children: &[],
    },
    Kind {
        name: "pattern",
        kind: "pattern",
        location: Location::TopLevel,
        minimal: "length = 2q;",
        full: r#"length = 2q; label = "p";"#,
        required: &["length"],
        children: NESTED_EVENTS,
    },
    Kind {
        name: "track",
        kind: "track",
        location: Location::TopLevel,
        minimal: "",
        full: r#"target = &synth:events; label = "k";"#,
        required: &[],
        children: &[],
    },
    Kind {
        name: "place",
        kind: "place",
        location: Location::TopLevel,
        minimal: "pattern = &cell; track = &lead; at = 0q;",
        full: r#"pattern = &cell; track = &lead; at = 1q; count = 2; stretch = 1/2; transpose = 100ct; boundary = cut; label = "p";"#,
        required: &["pattern", "track", "at"],
        children: &["override", "insert"],
    },
    Kind {
        name: "curve",
        kind: "curve",
        location: Location::TopLevel,
        minimal: "clock = score; points = [(0q, 1, step)];",
        full: r#"clock = seconds; points = [(0s, 1, linear), (1s, 2, step)]; label = "c";"#,
        required: &["clock", "points"],
        children: &[],
    },
    Kind {
        name: "automation",
        kind: "automation",
        location: Location::TopLevel,
        minimal: "target = &synth.params.level; curve = &level_shape; at = 0q;",
        full: r#"target = &synth.params.level; curve = &level_shape; at = 1q; label = "a";"#,
        required: &["target", "curve", "at"],
        children: &[],
    },
    Kind {
        name: "modulate",
        kind: "modulate",
        location: Location::TopLevel,
        minimal: "target = &synth.params.level; from = &wobble:out; amount = 1/10;",
        full: r#"target = &synth.params.level; from = &wobble:out; amount = 1/10; label = "m";"#,
        required: &["target", "from", "amount"],
        children: &[],
    },
    Kind {
        name: "asset",
        kind: "asset",
        location: Location::TopLevel,
        minimal: r#"kind = audio; path = "other.pcm"; hash = "sha256:1111111111111111111111111111111111111111111111111111111111111111"; format = "pcm_f32le_interleaved/1"; rate = 44100Hz; channels = 2; frames = 0;"#,
        full: r#"kind = audio; path = "other.pcm"; hash = "sha256:1111111111111111111111111111111111111111111111111111111111111111"; format = "pcm_f32le_interleaved/1"; rate = 44100Hz; channels = 2; frames = 0; label = "a";"#,
        required: &["kind", "path", "hash", "format", "rate", "channels", "frames"],
        children: &[],
    },
    Kind {
        name: "audio",
        kind: "audio",
        location: Location::TopLevel,
        minimal: "asset = &take; at = 0q; source = [0frame, 4800frame]; mode = rate;",
        full: r#"asset = &take; at = 1/2s; source = [0frame, 4800frame]; mode = rate; track = &clips; speed = 2; reverse = true; gain = 1/2; fade_in = 1ms; fade_out = 2ms; fade_shape = equal_power; label = "a";"#,
        required: &["asset", "at", "source", "mode"],
        children: &[],
    },
    Kind {
        name: "audio_warp",
        kind: "audio",
        location: Location::TopLevel,
        minimal: "asset = &take; at = 0q; source = [0frame, 4800frame]; mode = warp_rate; warp = [(0q, 0frame), (1q, 4800frame)];",
        full: r#"asset = &take; at = 0q; source = [0frame, 4800frame]; mode = warp_preserve; warp = [(0q, 0frame), (1q, 4800frame)]; processor = "core.stretch.ola/1"; track = &clips; gain = 1/2; fade_in = 1ms; fade_out = 2ms; fade_shape = linear; label = "w";"#,
        required: &["asset", "at", "source", "mode", "warp"],
        children: &[],
    },
    Kind {
        name: "node",
        kind: "node",
        location: Location::TopLevel,
        minimal: r#"type = "core.sine/1";"#,
        full: r#"type = "core.sine/1"; config = { voices = 4; }; params = { level = 1/2; attack = 1ms; release = 2ms; }; label = "n";"#,
        required: &["type"],
        children: &[],
    },
    Kind {
        name: "connect",
        kind: "connect",
        location: Location::TopLevel,
        minimal: "from = &spare:out; to = &mix:in;",
        full: r#"from = &spare:out; to = &mix:in; label = "c";"#,
        required: &["from", "to"],
        children: &[],
    },
    Kind {
        name: "region",
        kind: "region",
        location: Location::TopLevel,
        minimal: "span = [0q, 1q];",
        full: r#"span = [bar(1, 1), bar(2, 1)]; label = "r";"#,
        required: &["span"],
        children: &[],
    },
    Kind {
        name: "note",
        kind: "note",
        location: Location::Pattern,
        minimal: "at = 0q; dur = 1q; pitch = C4;",
        full: r#"at = 0q; dur = 1q; pitch = key(60); velocity = 1/2; release_velocity = 1/4; onset_offset = 1ms; release_offset = 2ms; order = 1; label = "n";"#,
        required: &["at", "dur", "pitch"],
        children: &["expression"],
    },
    Kind {
        name: "hit",
        kind: "hit",
        location: Location::Pattern,
        minimal: r#"at = 0q; key = "k";"#,
        full: r#"at = 0q; key = "k"; velocity = 1/2; onset_offset = 1ms; order = 1; label = "h";"#,
        required: &["at", "key"],
        children: &[],
    },
    Kind {
        name: "message",
        kind: "message",
        location: Location::Pattern,
        minimal: r#"at = 0q; protocol = "midi1"; bytes = [144, 60, 100];"#,
        full: r#"at = 0q; protocol = "midi1"; bytes = [144, 60, 100]; onset_offset = 1ms; order = 1; label = "m";"#,
        required: &["at", "protocol", "bytes"],
        children: &[],
    },
    Kind {
        name: "audio_leaf",
        kind: "audio",
        location: Location::Pattern,
        minimal: "asset = &take; at = 0q; source = [0frame, 4800frame]; mode = rate;",
        full: r#"asset = &take; at = 1q; source = [0frame, 4800frame]; mode = rate; speed = 2; reverse = true; gain = 1/2; fade_in = 1ms; fade_out = 2ms; fade_shape = equal_power; label = "l";"#,
        required: &["asset", "at", "source", "mode"],
        children: &[],
    },
    Kind {
        name: "use",
        kind: "use",
        location: Location::Pattern,
        minimal: "pattern = &cell; at = 0q;",
        full: r#"pattern = &cell; at = 1q; count = 2; stretch = 1/2; transpose = 100ct; boundary = cut; label = "u";"#,
        required: &["pattern", "at"],
        children: &[],
    },
    Kind {
        name: "expression",
        kind: "expression",
        location: Location::Note,
        minimal: "kind = timbre; curve = &expr_shape;",
        full: r#"kind = timbre; curve = &expr_shape; label = "e";"#,
        required: &["kind", "curve"],
        children: &[],
    },
    Kind {
        name: "override",
        kind: "override",
        location: Location::Place,
        minimal: r#"event = "0/n"; delete = true;"#,
        full: r#"event = "0/n"; set = { velocity = 1/2; }; label = "o";"#,
        required: &["event"],
        children: &[],
    },
    Kind {
        name: "insert",
        kind: "insert",
        location: Location::Place,
        minimal: "note added { at = 0q; dur = 1q; pitch = D4; }",
        full: r#"label = "i"; note added { at = 0q; dur = 1q; pitch = D4; }"#,
        required: &[],
        children: &["note", "hit", "message", "audio"],
    },
];

/// A minimal child declaration of each kind, valid wherever §4 permits it.
pub fn child_declaration(kind: &str, id: &str) -> String {
    let body = match kind {
        "tempo" => "points = [(0q, 90bpm, step)];",
        "meter" => "points = [(0q, 3, 4)];",
        "tuning" => {
            "period = 1200ct; steps = [0ct]; reference_index = 0; reference_frequency = 440Hz;"
        }
        "pattern" => "length = 1q;",
        "track" => "",
        "place" => "pattern = &cell; track = &lead; at = 0q;",
        "curve" => "clock = score; points = [(0q, 1, step)];",
        "automation" => "target = &spare.params.level; curve = &level_shape; at = 0q;",
        "modulate" => "target = &spare.params.level; from = &wobble:out; amount = 1/10;",
        "asset" => {
            r#"kind = audio; path = "c.pcm"; hash = "sha256:2222222222222222222222222222222222222222222222222222222222222222"; format = "pcm_f32le_interleaved/1"; rate = 48000Hz; channels = 1; frames = 0;"#
        }
        "audio" => "asset = &take; at = 0q; source = [0frame, 4800frame]; mode = rate;",
        "node" => r#"type = "core.sine/1";"#,
        "connect" => "from = &spare:out; to = &mix:in;",
        "region" => "span = [0q, 1q];",
        "project" => "score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre;",
        "note" => "at = 0q; dur = 1q; pitch = E4;",
        "hit" => r#"at = 0q; key = "k";"#,
        "message" => r#"at = 0q; protocol = "midi1"; bytes = [128, 60, 0];"#,
        "use" => "pattern = &cell; at = 0q;",
        "expression" => "kind = gain; curve = &expr_shape;",
        "override" => r#"event = "0/n"; delete = true;"#,
        "insert" => "hit added_hit { at = 0q; key = \"k\"; }",
        _ => "",
    };
    format!("{kind} {id} {{ {body} }}")
}

/// Every kind that can appear as a child, plus one unknown kind.
pub const CHILD_KINDS: &[&str] = &[
    "project",
    "tempo",
    "meter",
    "tuning",
    "pattern",
    "track",
    "place",
    "curve",
    "automation",
    "modulate",
    "asset",
    "audio",
    "node",
    "connect",
    "region",
    "note",
    "hit",
    "message",
    "use",
    "expression",
    "override",
    "insert",
    "widget",
];

/// Wrap one declaration at its location; returns the source and its object path.
pub fn placed(location: Location, declaration: &str, id: &str) -> (String, Vec<String>) {
    match location {
        Location::TopLevel => (format!("{declaration}\n"), vec![id.into()]),
        Location::Pattern => (
            format!("pattern host {{ length = 4q; {declaration} }}\n"),
            vec!["host".into(), id.into()],
        ),
        Location::Note => (
            format!(
                "pattern host {{ length = 4q; note host_note {{ at = 0q; dur = 1q; pitch = C4; {declaration} }} }}\n"
            ),
            vec!["host".into(), "host_note".into(), id.into()],
        ),
        Location::Place => (
            format!("place host {{ pattern = &cell; track = &lead; at = 0q; {declaration} }}\n"),
            vec!["host".into(), id.into()],
        ),
    }
}

pub fn source_for(kind: &Kind, body: &str) -> (String, Vec<String>) {
    let (text, path) = placed(kind.location, &format!("{} x {{ {body} }}", kind.kind), "x");
    (format!("{}{text}", base()), path)
}

// The editing API's own error type; boxing it here would only obscure tests.
#[allow(clippy::result_large_err)]
pub fn validate(source: &str) -> Result<(), EditError> {
    let document = maac::parse(source).unwrap_or_else(|d| panic!("parse failed: {d:?}\n{source}"));
    // The authored layer refuses constructors outside §3.2 before validation.
    let authored = AuthoredDocument::from_document(&document)?;
    FoundationEditContext.validate_document(authored.tree())
}

/// Drop one `name = value;` field from a flat body of fields.
pub fn without(body: &str, name: &str) -> String {
    let start = body
        .find(&format!("{name} = "))
        .unwrap_or_else(|| panic!("{name} not in {body}"));
    let mut depth = 0i32;
    let mut end = start;
    for (offset, ch) in body[start..].char_indices() {
        match ch {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth -= 1,
            ';' if depth == 0 => {
                end = start + offset + 1;
                break;
            }
            _ => {}
        }
    }
    format!("{}{}", &body[..start], &body[end..])
}
