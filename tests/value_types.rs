//! Dimensional types and constructor locations (evidence row D05, §§3, 5–19).
//!
//! `FIELDS` restates, from the specification, the value type of every
//! dimensional core field. For each one, every accepted spelling validates and
//! each wrong unit, bare number, wrong constructor or other value type is
//! refused at that object and field. `CONSTRUCTORS` checks each §3.2
//! constructor in every location that permits it and in representative
//! locations that do not. Every case runs the public Document-profile
//! validator.

#[path = "support/inventory.rs"]
mod inventory;

use inventory::*;

/// One dimensional field: a kind, a body with `@` where the value goes, the
/// spellings the specification accepts, and refused spellings with codes.
struct FieldType {
    field: &'static str,
    kind: &'static str,
    body: &'static str,
    accepted: &'static [&'static str],
    refused: &'static [(&'static str, &'static str)],
}

const UNIT: &str = "E_UNIT";
const RANGE: &str = "E_RANGE";

/// Values that are not physical seconds.
const NOT_SECONDS: &[(&str, &str)] = &[
    ("1", UNIT),
    ("1q", UNIT),
    ("1Hz", UNIT),
    ("1ct", UNIT),
    ("\"1ms\"", UNIT),
];
/// Values that are not dimensionless numbers.
const NOT_NUMBER: &[(&str, &str)] = &[
    ("1q", UNIT),
    ("1s", UNIT),
    ("0dB", UNIT),
    ("\"1\"", UNIT),
    ("key(60)", UNIT),
];

/// §§3.1, 5–19: every dimensional field of the core inventory.
const FIELDS: &[FieldType] = &[
    // §5 project and maps; `@` sits in the project via `project_field`.
    FieldType {
        field: "pattern.length",
        kind: "pattern",
        body: "length = @;",
        accepted: &["2q", "1/3q", "0.5q"],
        refused: &[("1", UNIT), ("1s", UNIT), ("bar(2, 1)", UNIT), ("0q", RANGE), ("key(60)", UNIT)],
    },
    FieldType {
        field: "tempo.points position",
        kind: "tempo",
        body: "points = [(@, 120bpm, step)];",
        accepted: &["0q"],
        refused: &[("0", UNIT), ("0s", UNIT), ("bar(1, 1)", UNIT)],
    },
    FieldType {
        field: "tempo.points tempo",
        kind: "tempo",
        body: "points = [(0q, @, step)];",
        accepted: &["120bpm", "90.5bpm", "1/3bpm"],
        refused: &[("120", UNIT), ("120Hz", UNIT), ("0bpm", "E_TEMPO")],
    },
    FieldType {
        field: "meter.points position",
        kind: "meter",
        body: "points = [(@, 3, 4)];",
        accepted: &["0q"],
        refused: &[("0", UNIT), ("0s", UNIT)],
    },
    FieldType {
        field: "meter.points numerator",
        kind: "meter",
        body: "points = [(0q, @, 4)];",
        accepted: &["7"],
        refused: &[("7q", UNIT), ("0", RANGE), ("3/2", RANGE)],
    },
    FieldType {
        field: "tuning.period",
        kind: "tuning",
        body: "period = @; steps = [0ct]; reference_index = 0; reference_frequency = 440Hz;",
        accepted: &["1200ct", "1901.955ct"],
        refused: &[("1200", UNIT), ("1Hz", UNIT), ("0ct", RANGE)],
    },
    FieldType {
        field: "tuning.steps",
        kind: "tuning",
        body: "period = 1200ct; steps = [@]; reference_index = 0; reference_frequency = 440Hz;",
        accepted: &["0ct"],
        refused: &[("0", UNIT), ("0q", UNIT)],
    },
    FieldType {
        field: "tuning.reference_index",
        kind: "tuning",
        body: "period = 1200ct; steps = [0ct]; reference_index = @; reference_frequency = 440Hz;",
        accepted: &["0", "-7", "40"],
        refused: &[("0ct", UNIT), ("1/2", RANGE)],
    },
    FieldType {
        field: "tuning.reference_frequency",
        kind: "tuning",
        body: "period = 1200ct; steps = [0ct]; reference_index = 0; reference_frequency = @;",
        accepted: &["440Hz", "0.44kHz"],
        refused: &[("440", UNIT), ("440ct", UNIT), ("0Hz", RANGE)],
    },
    // §8 notes, hits and messages (pattern-local).
    FieldType {
        field: "note.at",
        kind: "note",
        body: "at = @; dur = 1q; pitch = C4;",
        accepted: &["0q", "3/2q"],
        refused: &[("0", UNIT), ("0s", UNIT), ("bar(1, 1)", UNIT), ("-1q", RANGE), ("4q", "E_INTERVAL")],
    },
    FieldType {
        field: "note.dur",
        kind: "note",
        body: "at = 0q; dur = @; pitch = C4;",
        accepted: &["1q", "1/3q"],
        refused: &[("1", UNIT), ("1s", UNIT), ("bar(1, 2)", UNIT), ("0q", RANGE)],
    },
    FieldType {
        field: "note.velocity",
        kind: "note",
        body: "at = 0q; dur = 1q; pitch = C4; velocity = @;",
        accepted: &["0", "1", "0.8", "4/5"],
        refused: &[("1q", UNIT), ("0dB", UNIT), ("3/2", RANGE)],
    },
    FieldType {
        field: "note.release_velocity",
        kind: "note",
        body: "at = 0q; dur = 1q; pitch = C4; release_velocity = @;",
        accepted: &["0", "1/2"],
        refused: &[("1q", UNIT), ("2", RANGE)],
    },
    FieldType {
        field: "note.onset_offset",
        kind: "note",
        body: "at = 0q; dur = 1q; pitch = C4; onset_offset = @;",
        accepted: &["20ms", "-1/100s", "0s"],
        refused: NOT_SECONDS,
    },
    FieldType {
        field: "note.release_offset",
        kind: "note",
        body: "at = 0q; dur = 1q; pitch = C4; release_offset = @;",
        accepted: &["20ms", "-1/100s"],
        refused: NOT_SECONDS,
    },
    FieldType {
        field: "note.order",
        kind: "note",
        body: "at = 0q; dur = 1q; pitch = C4; order = @;",
        accepted: &["0", "-3", "12"],
        refused: &[("1q", UNIT), ("1/2", RANGE)],
    },
    FieldType {
        field: "hit.at",
        kind: "hit",
        body: "at = @; key = \"k\";",
        accepted: &["0q", "1/4q"],
        refused: &[("0", UNIT), ("0s", UNIT), ("bar(1, 1)", UNIT)],
    },
    FieldType {
        field: "hit.velocity",
        kind: "hit",
        body: "at = 0q; key = \"k\"; velocity = @;",
        accepted: &["1/2"],
        refused: NOT_NUMBER,
    },
    FieldType {
        field: "hit.onset_offset",
        kind: "hit",
        body: "at = 0q; key = \"k\"; onset_offset = @;",
        accepted: &["5ms"],
        refused: NOT_SECONDS,
    },
    FieldType {
        field: "hit.order",
        kind: "hit",
        body: "at = 0q; key = \"k\"; order = @;",
        accepted: &["-1", "2"],
        refused: &[("1q", UNIT), ("1/2", RANGE)],
    },
    FieldType {
        field: "message.at",
        kind: "message",
        body: "at = @; protocol = \"midi1\"; bytes = [144, 60, 100];",
        accepted: &["0q", "1/4q"],
        refused: &[("0", UNIT), ("0s", UNIT), ("bar(1, 1)", UNIT)],
    },
    FieldType {
        field: "message.onset_offset",
        kind: "message",
        body: "at = 0q; protocol = \"midi1\"; bytes = [144, 60, 100]; onset_offset = @;",
        accepted: &["5ms"],
        refused: NOT_SECONDS,
    },
    FieldType {
        field: "message.order",
        kind: "message",
        body: "at = 0q; protocol = \"midi1\"; bytes = [144, 60, 100]; order = @;",
        accepted: &["3"],
        refused: &[("1q", UNIT), ("1/2", RANGE)],
    },
    FieldType {
        field: "message.bytes",
        kind: "message",
        body: "at = 0q; protocol = \"midi1\"; bytes = [@];",
        accepted: &["0", "255"],
        refused: &[("256", RANGE), ("-1", RANGE), ("1/2", RANGE), ("1q", UNIT)],
    },
    // §9 uses and §10 placements.
    FieldType {
        field: "use.at",
        kind: "use",
        body: "pattern = &cell; at = @;",
        accepted: &["0q", "1q"],
        refused: &[("0", UNIT), ("0s", UNIT), ("bar(1, 1)", UNIT), ("-1q", RANGE)],
    },
    FieldType {
        field: "use.count",
        kind: "use",
        body: "pattern = &cell; at = 0q; count = @;",
        accepted: &["1", "3"],
        refused: &[("0", RANGE), ("1/2", RANGE), ("2q", UNIT)],
    },
    FieldType {
        field: "use.stretch",
        kind: "use",
        body: "pattern = &cell; at = 0q; stretch = @;",
        accepted: &["1/2", "1.5"],
        refused: &[("0", RANGE), ("2q", UNIT), ("\"2\"", UNIT)],
    },
    FieldType {
        field: "use.transpose",
        kind: "use",
        body: "pattern = &cell; at = 0q; transpose = @;",
        accepted: &["0ct", "-1200ct", "50.5ct"],
        refused: &[("12", UNIT), ("1Hz", UNIT), ("1q", UNIT)],
    },
    FieldType {
        field: "place.at",
        kind: "place",
        body: "pattern = &cell; track = &lead; at = @;",
        accepted: &["0q", "7/2q", "bar(2, 1)", "bar(1, 5/2)"],
        refused: &[("0", UNIT), ("0s", UNIT), ("key(60)", UNIT), ("bar(1, 5)", RANGE)],
    },
    FieldType {
        field: "place.count",
        kind: "place",
        body: "pattern = &cell; track = &lead; at = 0q; count = @;",
        accepted: &["2"],
        refused: &[("0", RANGE), ("2q", UNIT)],
    },
    FieldType {
        field: "place.stretch",
        kind: "place",
        body: "pattern = &cell; track = &lead; at = 0q; stretch = @;",
        accepted: &["1/2"],
        refused: &[("0", RANGE), ("2q", UNIT)],
    },
    FieldType {
        field: "place.transpose",
        kind: "place",
        body: "pattern = &cell; track = &lead; at = 0q; transpose = @;",
        accepted: &["-200ct"],
        refused: &[("12", UNIT), ("1q", UNIT)],
    },
    // §12 curves and automation.
    FieldType {
        field: "curve.points score position",
        kind: "curve",
        body: "clock = score; points = [(@, 1, step)];",
        accepted: &["0q"],
        refused: &[("0", UNIT), ("0s", UNIT), ("bar(1, 1)", UNIT)],
    },
    FieldType {
        field: "curve.points seconds position",
        kind: "curve",
        body: "clock = seconds; points = [(@, 1, step)];",
        accepted: &["0s", "0ms"],
        refused: &[("0", UNIT), ("0q", UNIT)],
    },
    FieldType {
        field: "curve.points normalized position",
        kind: "curve",
        body: "clock = normalized; points = [(0, 1, linear), (@, 1, step)];",
        accepted: &["1"],
        refused: &[("1q", UNIT), ("1s", UNIT)],
    },
    FieldType {
        field: "automation.at (score curve)",
        kind: "automation",
        body: "target = &synth.params.level; curve = &level_shape; at = @;",
        accepted: &["0q", "1/2q", "bar(1, 3)"],
        refused: &[("0", UNIT), ("0s", UNIT), ("key(60)", UNIT)],
    },
    FieldType {
        field: "automation.at (seconds curve)",
        kind: "automation",
        body: "target = &synth.params.level; curve = &seconds_shape; at = @;",
        accepted: &["0q", "bar(1, 2)", "1/2s", "20ms"],
        refused: &[("0", UNIT), ("0Hz", UNIT)],
    },
    FieldType {
        field: "modulate.amount (Hz parameter)",
        kind: "modulate",
        body: "target = &lp.params.cutoff; from = &wobble:out; amount = @;",
        accepted: &["100Hz", "1/10kHz", "-50Hz"],
        refused: &[("100", UNIT), ("1q", UNIT), ("100ct", UNIT)],
    },
    FieldType {
        field: "modulate.amount",
        kind: "modulate",
        body: "target = &synth.params.level; from = &wobble:out; amount = @;",
        accepted: &["1/10", "-1/2"],
        refused: &[("1/10q", UNIT), ("1Hz", UNIT), ("0dB", UNIT)],
    },
    // §11 override replacements use the leaf field's type, in final q.
    FieldType {
        field: "override set.at",
        kind: "override",
        body: "event = \"0/n\"; set = { at = @; };",
        accepted: &["0q", "1/2q"],
        refused: &[("0", UNIT), ("0s", UNIT), ("bar(1, 2)", UNIT)],
    },
    FieldType {
        field: "override set.dur",
        kind: "override",
        body: "event = \"0/n\"; set = { dur = @; };",
        accepted: &["1q", "1/4q"],
        refused: &[("1", UNIT), ("1s", UNIT), ("0q", RANGE)],
    },
    FieldType {
        field: "override set.onset_offset",
        kind: "override",
        body: "event = \"0/n\"; set = { onset_offset = @; };",
        accepted: &["20ms", "-1/100s"],
        refused: NOT_SECONDS,
    },
    // §14 assets and arranged audio.
    FieldType {
        field: "asset.rate",
        kind: "asset",
        body: r#"kind = audio; path = "o.pcm"; hash = "sha256:1111111111111111111111111111111111111111111111111111111111111111"; format = "pcm_f32le_interleaved/1"; rate = @; channels = 1; frames = 0;"#,
        accepted: &["44100Hz", "48kHz"],
        refused: &[("44100", UNIT), ("44100.5Hz", RANGE), ("0Hz", RANGE)],
    },
    FieldType {
        field: "asset.channels",
        kind: "asset",
        body: r#"kind = audio; path = "o.pcm"; hash = "sha256:1111111111111111111111111111111111111111111111111111111111111111"; format = "pcm_f32le_interleaved/1"; rate = 48000Hz; channels = @; frames = 0;"#,
        accepted: &["1", "2"],
        refused: &[("0", RANGE), ("1/2", RANGE), ("2Hz", UNIT)],
    },
    FieldType {
        field: "asset.frames",
        kind: "asset",
        body: r#"kind = audio; path = "o.pcm"; hash = "sha256:1111111111111111111111111111111111111111111111111111111111111111"; format = "pcm_f32le_interleaved/1"; rate = 48000Hz; channels = 1; frames = @;"#,
        accepted: &["0", "96000"],
        refused: &[("-1", RANGE), ("1/2", RANGE), ("10frame", UNIT)],
    },
    FieldType {
        field: "audio.at",
        kind: "audio",
        body: "asset = &take; at = @; source = [0frame, 4800frame]; mode = rate;",
        accepted: &["0q", "1/2q", "bar(1, 2)", "1/2s", "250ms"],
        refused: &[("0", UNIT), ("0Hz", UNIT), ("key(60)", UNIT)],
    },
    FieldType {
        field: "audio.source",
        kind: "audio",
        body: "asset = &take; at = 0q; source = [0frame, @]; mode = rate;",
        accepted: &["1frame", "48000frame"],
        refused: &[("4800", UNIT), ("1q", UNIT), ("1/2frame", RANGE), ("48001frame", RANGE)],
    },
    FieldType {
        field: "audio.speed",
        kind: "audio",
        body: "asset = &take; at = 0q; source = [0frame, 4800frame]; mode = rate; speed = @;",
        accepted: &["2", "1/3"],
        refused: &[("0", RANGE), ("2q", UNIT), ("1Hz", UNIT)],
    },
    FieldType {
        field: "audio.gain",
        kind: "audio",
        body: "asset = &take; at = 0q; source = [0frame, 4800frame]; mode = rate; gain = @;",
        accepted: &["0", "3/2"],
        refused: &[("-1", RANGE), ("0dB", UNIT), ("1q", UNIT)],
    },
    FieldType {
        field: "audio.fade_in",
        kind: "audio",
        body: "asset = &take; at = 0q; source = [0frame, 4800frame]; mode = rate; fade_in = @;",
        accepted: &["0s", "5ms"],
        refused: &[("5", UNIT), ("1q", UNIT), ("-1ms", RANGE)],
    },
    FieldType {
        field: "audio.fade_out",
        kind: "audio",
        body: "asset = &take; at = 0q; source = [0frame, 4800frame]; mode = rate; fade_out = @;",
        accepted: &["0s", "5ms"],
        refused: &[("5", UNIT), ("1q", UNIT), ("-1ms", RANGE)],
    },
    FieldType {
        field: "audio.warp local q",
        kind: "audio",
        body: "asset = &take; at = 0q; source = [0frame, 4800frame]; mode = warp_rate; warp = [(0q, 0frame), (@, 4800frame)];",
        accepted: &["1q", "1/2q"],
        refused: &[("1", UNIT), ("1s", UNIT), ("bar(1, 2)", UNIT)],
    },
    FieldType {
        field: "audio.warp source frame",
        kind: "audio",
        body: "asset = &take; at = 0q; source = [0frame, 4800frame]; mode = warp_rate; warp = [(0q, 0frame), (1q, @)];",
        accepted: &["4800frame"],
        refused: &[("4800", UNIT), ("1q", UNIT)],
    },
    FieldType {
        field: "audio leaf at",
        kind: "audio_leaf",
        body: "asset = &take; at = @; source = [0frame, 4800frame]; mode = rate;",
        accepted: &["0q", "1/2q"],
        refused: &[("0", UNIT), ("0s", UNIT), ("bar(1, 1)", UNIT), ("-1q", RANGE)],
    },
    // §15 node parameters use their descriptor's native units.
    FieldType {
        field: "node.params.level",
        kind: "node",
        body: "type = \"core.sine/1\"; params = { level = @; };",
        accepted: &["1/2", "0"],
        refused: NOT_NUMBER,
    },
    FieldType {
        field: "node.params.attack",
        kind: "node",
        body: "type = \"core.sine/1\"; params = { attack = @; };",
        accepted: &["5ms", "1/10s"],
        refused: &[("5", UNIT), ("1q", UNIT), ("-1ms", RANGE)],
    },
    // §19 regions.
    FieldType {
        field: "region.span",
        kind: "region",
        body: "span = [@, 8q];",
        accepted: &["0q", "1/2q", "bar(1, 1)", "bar(2, 3/2)"],
        refused: &[("0", UNIT), ("0s", UNIT), ("key(60)", UNIT)],
    },
];

fn kind(name: &str) -> &'static Kind {
    KINDS
        .iter()
        .find(|kind| kind.name == name)
        .unwrap_or_else(|| panic!("no kind {name}"))
}

fn case(field: &FieldType, value: &str) -> (String, Vec<String>) {
    source_for(kind(field.kind), &field.body.replace('@', value))
}

/// The top-level field the value sits in, such as `params` for
/// `params = { attack = @; }`.
fn field_name(field: &FieldType) -> &str {
    let at = field.body.find('@').unwrap();
    let mut depth = 0i32;
    let mut start = 0;
    for (index, ch) in field.body[..at].char_indices() {
        match ch {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth -= 1,
            ';' if depth == 0 => start = index + 1,
            _ => {}
        }
    }
    field.body[start..at].split('=').next().unwrap().trim()
}

#[test]
fn every_dimensional_field_accepts_its_type_and_refuses_others() {
    let mut failures = Vec::new();
    let mut checked = 0;
    for field in FIELDS {
        for value in field.accepted {
            let (source, _) = case(field, value);
            checked += 1;
            if let Err(error) = validate(&source) {
                failures.push(format!("{} = {value}: refused {error:?}", field.field));
            }
        }
        let name = field_name(field);
        for (value, code) in field.refused {
            let (source, path) = case(field, value);
            checked += 1;
            match validate(&source) {
                Err(error)
                    if error.code == *code
                        && error.object_path == path
                        && error.field_path.first().map(String::as_str) == Some(name) => {}
                other => failures.push(format!(
                    "{} = {value}: expected {code} at {path:?}.{name}, got {other:?}",
                    field.field
                )),
            }
        }
    }
    assert!(checked > 250, "{checked}");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The project's dimensional fields, set in the base composition.
#[test]
fn project_fields_accept_their_types_and_refuse_others() {
    let project = "project song { score = [0q, 8q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &mix:out; }";
    let with = |field: &str, value: &str| {
        let kept = if project.contains(&format!("{field} = ")) {
            without(project, field)
        } else {
            project.to_owned()
        };
        let body = kept.replacen(" }", &format!(" {field} = {value}; }}"), 1);
        base().replace(project, &body)
    };
    let mut failures = Vec::new();
    for (field, accepted, refused) in [
        (
            "score",
            &[
                "[0q, 8q]",
                "[-1q, 8q]",
                "[bar(1, 1), bar(3, 1)]",
                "[bar(0, 1), 8q]",
            ][..],
            &[
                ("[0, 8]", UNIT),
                ("[0s, 8s]", UNIT),
                ("[8q, 0q]", "E_INTERVAL"),
                ("(0q, 8q)", UNIT),
            ][..],
        ),
        (
            "rate",
            &["48000Hz", "48kHz"][..],
            &[("48000", UNIT), ("48000.5Hz", RANGE), ("0Hz", RANGE)][..],
        ),
        (
            "tail",
            &["0s", "2s", "500ms"][..],
            &[("2", UNIT), ("1q", UNIT), ("-1s", RANGE)][..],
        ),
        (
            "seed",
            &["0", "18446744073709551615"][..],
            &[
                ("-1", RANGE),
                ("1/2", RANGE),
                ("18446744073709551616", RANGE),
                ("1q", UNIT),
            ][..],
        ),
    ] {
        for value in accepted {
            if let Err(error) = validate(&with(field, value)) {
                failures.push(format!("project.{field} = {value}: refused {error:?}"));
            }
        }
        for (value, code) in refused {
            match validate(&with(field, value)) {
                Err(error)
                    if error.code == *code
                        && error.object_path == ["song"]
                        && error.field_path.first().map(String::as_str) == Some(field) => {}
                other => failures.push(format!(
                    "project.{field} = {value}: expected {code}, got {other:?}"
                )),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// §12: curve values take the dimension of the parameter that consumes them.
#[test]
fn curve_values_must_match_their_target_dimension() {
    let automation = |values: &str, target: &str| {
        format!(
            "{}curve shaped {{ clock = score; points = [(0q, {values}, step)]; }}\n\
             automation x {{ target = {target}; curve = &shaped; at = 0q; }}\n",
            base()
        )
    };
    for (values, target) in [
        ("1/2", "&synth.params.level"),
        ("2kHz", "&lp.params.cutoff"),
        ("500Hz", "&lp.params.cutoff"),
    ] {
        validate(&automation(values, target))
            .unwrap_or_else(|e| panic!("{values} -> {target}: {e:?}"));
    }
    for (values, target) in [
        ("1/2Hz", "&synth.params.level"),
        ("1/2", "&lp.params.cutoff"),
        ("1q", "&lp.params.cutoff"),
    ] {
        let error = validate(&automation(values, target)).expect_err("dimension mismatch");
        assert_eq!(error.code, UNIT, "{values} -> {target}: {error:?}");
    }
}

/// §3.2 constructors in every pitch and global score-position location.
#[test]
fn constructors_are_accepted_exactly_where_section_3_2_places_them() {
    let tuning = "tuning just { period = 1200ct; steps = [0ct, 702ct]; reference_index = 0; reference_frequency = 440Hz; }\n";
    let pitches = [
        "C4",
        "Bb3",
        "key(60)",
        "key(-3)",
        "degree(3, &just)",
        "440Hz",
        "ratio(3/2, 440Hz)",
    ];
    let not_pitches = [
        ("60", UNIT),
        ("1q", UNIT),
        ("100ct", UNIT),
        ("bar(1, 1)", UNIT),
        ("c4", RANGE),
        ("\"C4\"", UNIT),
        ("key(1/2)", RANGE),
        ("degree(1, &cell)", "E_REFERENCE"),
        ("ratio(0, 440Hz)", RANGE),
        ("0Hz", RANGE),
    ];
    let mut failures = Vec::new();
    // Pitch locations: a pattern note, an override replacement and an insert.
    let pitch_sources = |value: &str| {
        [
            (
                "note.pitch",
                format!(
                    "{}{tuning}pattern host {{ length = 4q; note x {{ at = 0q; dur = 1q; pitch = {value}; }} }}\n",
                    base()
                ),
                vec!["host".to_owned(), "x".to_owned()],
            ),
            (
                "override set.pitch",
                format!(
                    "{}{tuning}place host {{ pattern = &cell; track = &lead; at = 0q; override x {{ event = \"0/n\"; set = {{ pitch = {value}; }}; }} }}\n",
                    base()
                ),
                vec!["host".to_owned(), "x".to_owned()],
            ),
            (
                "insert note.pitch",
                format!(
                    "{}{tuning}place host {{ pattern = &cell; track = &lead; at = 0q; insert i {{ note x {{ at = 0q; dur = 1q; pitch = {value}; }} }} }}\n",
                    base()
                ),
                vec!["host".to_owned(), "i".to_owned(), "x".to_owned()],
            ),
        ]
    };
    for value in pitches {
        for (location, source, _) in pitch_sources(value) {
            if let Err(error) = validate(&source) {
                failures.push(format!("{location} = {value}: refused {error:?}"));
            }
        }
    }
    for (value, code) in not_pitches {
        for (location, source, path) in pitch_sources(value) {
            match validate(&source) {
                Err(error) if error.code == code && error.object_path == path => {}
                other => failures.push(format!(
                    "{location} = {value}: expected {code}, got {other:?}"
                )),
            }
        }
    }
    // `bar` and the pitch constructors outside their locations.
    for (location, source, code) in [
        ("note.velocity", "pattern host { length = 4q; note x { at = 0q; dur = 1q; pitch = C4; velocity = key(60); } }", UNIT),
        ("note.dur", "pattern host { length = 4q; note x { at = 0q; dur = bar(1, 2); pitch = C4; } }", UNIT),
        ("use.transpose", "pattern host { length = 4q; use x { pattern = &cell; at = 0q; transpose = ratio(3/2, 440Hz); } }", UNIT),
        ("tempo.points", "tempo x { points = [(bar(1, 1), 120bpm, step)]; }", UNIT),
        ("node.params.level", "node x { type = \"core.sine/1\"; params = { level = key(60); }; }", UNIT),
        ("override set.at", "place host { pattern = &cell; track = &lead; at = 0q; override x { event = \"0/n\"; set = { at = bar(1, 2); }; } }", UNIT),
        // §3.2: no other function name is core syntax.
        ("unknown constructor", "pattern host { length = 4q; note x { at = 0q; dur = 1q; pitch = random(); } }", "E_SYNTAX"),
        ("unknown constructor", "place x { pattern = &cell; track = &lead; at = humanize(0q); }", "E_SYNTAX"),
    ] {
        match validate(&format!("{}{source}\n", base())) {
            Err(error) if error.code == code => {}
            other => failures.push(format!("{location} with a misplaced constructor: expected {code}, got {other:?}")),
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
