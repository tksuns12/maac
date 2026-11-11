//! `vary`: a declared, seeded departure from the score that a placement
//! names (§9.4). Each event's velocity and timing move by a draw from the
//! seed and its address.

use maac::compiler::compile_bundle_artifact;
use maac::editing::{FoundationEditContext, SourceDocument};
use maac::{DiagnosticCode, SourceBundle};
use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::Signed;
use serde_json::Value;

fn source(extra: &str, pattern: &str, places: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 32q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node synth {{ type = "core.sine/1"; params = {{ attack = 0s; release = 0s; level = 0.1; }}; }}
vary loose {{ seed = 7; velocity = 3/50; timing = 8ms; }}
{extra}
pattern line {{ length = 4q;
{pattern}
}}
track t {{ target = &synth:events; }}
{places}
"#
    )
}

const LINE: &str = r#"  note a { at = 1/2q; dur = 1/2q; pitch = D4; velocity = 3/5; }
  note b { at = 1q; dur = 1/2q; pitch = F4; velocity = 3/5; }
  note c { at = 2q; dur = 1q; pitch = A4; velocity = 3/5; }"#;

const PLACE: &str =
    "place main { pattern = &line; track = &t; at = 4q; count = 3; vary = &loose; }";

/// The documented draw, written out independently of the compiler.
fn offset(seed: u64, address: &str, field: &str, amount: &BigRational) -> BigRational {
    let mut hash = 0xCBF2_9CE4_8422_2325u64 ^ seed;
    for byte in format!("{address}/{field}").bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01B3);
    }
    let mut state = hash;
    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (z ^ (z >> 31)) >> 11
    };
    let (u1, u2) = (next(), next());
    let scale = BigInt::from(1u64 << 53);
    amount * BigRational::new(BigInt::from(u1) + BigInt::from(u2) - &scale, scale)
}

fn rational(text: &str) -> BigRational {
    text.parse().unwrap()
}

fn events(source: &str) -> Vec<Value> {
    let artifact = compile_bundle_artifact(&SourceBundle::new("main.maac", source))
        .unwrap_or_else(|e| panic!("compiles: {e}"));
    let wire: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    wire["events"].as_array().unwrap().clone()
}

fn event<'e>(events: &'e [Value], address: &str) -> &'e Value {
    events
        .iter()
        .find(|e| e["address"] == address)
        .unwrap_or_else(|| panic!("no event {address}"))
}

fn velocity(events: &[Value], address: &str) -> BigRational {
    rational(event(events, address)["kind"]["velocity"].as_str().unwrap())
}

fn onset(events: &[Value], address: &str) -> BigRational {
    rational(
        event(events, address)["onset_offset_seconds"]
            .as_str()
            .unwrap(),
    )
}

fn codes(source: &str) -> Vec<DiagnosticCode> {
    compile_bundle_artifact(&SourceBundle::new("main.maac", source))
        .unwrap_err()
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn each_value_is_the_documented_draw() {
    let events = events(&source("", LINE, PLACE));
    let amount = rational("3/50");
    let timing = rational("1/125");
    for address in ["main/0/a", "main/1/b", "main/2/c"] {
        assert_eq!(
            velocity(&events, address),
            rational("3/5") + offset(7, address, "velocity", &amount),
            "{address}"
        );
        assert_eq!(
            onset(&events, address),
            offset(7, address, "timing", &timing),
            "{address}"
        );
    }
}

#[test]
fn values_stay_within_their_amounts_and_repeats_differ() {
    let events = events(&source("", LINE, PLACE));
    let mut seen = std::collections::BTreeSet::new();
    for repetition in 0..3 {
        for leaf in ["a", "b", "c"] {
            let address = format!("main/{repetition}/{leaf}");
            let moved = velocity(&events, &address) - rational("3/5");
            assert!(moved.abs() <= rational("3/50"), "{address}");
            assert!(
                onset(&events, &address).abs() <= rational("1/125"),
                "{address}"
            );
            seen.insert(velocity(&events, &address));
        }
    }
    assert_eq!(seen.len(), 9);
}

#[test]
fn an_edit_elsewhere_redraws_nothing() {
    let before = events(&source("", LINE, PLACE));
    let added = format!("{LINE}\n  note d {{ at = 3q; dur = 1/2q; pitch = C5; }}");
    let after = events(&source("", &added, PLACE));
    let fewer = events(&source("", LINE, &PLACE.replace("count = 3", "count = 2")));
    for address in ["main/0/a", "main/1/b", "main/1/c"] {
        assert_eq!(velocity(&before, address), velocity(&after, address));
        assert_eq!(onset(&before, address), onset(&after, address));
        assert_eq!(velocity(&before, address), velocity(&fewer, address));
    }
    // Another seed draws other values.
    let reseeded = events(&source("", LINE, PLACE).replace("seed = 7", "seed = 8"));
    assert_ne!(
        velocity(&before, "main/0/a"),
        velocity(&reseeded, "main/0/a")
    );
}

#[test]
fn notes_that_touch_still_touch() {
    let events = events(&source("", LINE, PLACE));
    for repetition in 0..3 {
        let a = event(&events, &format!("main/{repetition}/a"));
        assert_eq!(
            a["release_offset_seconds"],
            event(&events, &format!("main/{repetition}/b"))["onset_offset_seconds"]
        );
        assert_eq!(
            a["off_frame"],
            event(&events, &format!("main/{repetition}/b"))["on_frame"]
        );
    }
    // b ends before c starts: its gate end moves with its own onset.
    let b = event(&events, "main/0/b");
    assert_eq!(b["release_offset_seconds"], b["onset_offset_seconds"]);
}

#[test]
fn overrides_are_final_and_inserts_are_not_varied() {
    let place = PLACE.replace(
        " }",
        r#"
  override exact { event = "1/a"; set = { velocity = 0.9; onset_offset = 0s; }; }
  insert extra { note x { at = 3q; dur = 1/2q; pitch = G4; velocity = 1/2; } }
}"#,
    );
    let events = events(&source("", LINE, &place));
    assert_eq!(velocity(&events, "main/1/a"), rational("9/10"));
    assert_eq!(onset(&events, "main/1/a"), rational("0"));
    assert_eq!(velocity(&events, "main/extra/x"), rational("1/2"));
    assert_eq!(onset(&events, "main/extra/x"), rational("0"));
}

#[test]
fn velocity_stays_within_zero_and_one() {
    let loud = LINE.replace("velocity = 3/5", "velocity = 1");
    let wide = source("", &loud, PLACE).replace("velocity = 3/50", "velocity = 1/2");
    let events = events(&wide);
    for repetition in 0..3 {
        for leaf in ["a", "b", "c"] {
            let velocity = velocity(&events, &format!("main/{repetition}/{leaf}"));
            assert!(velocity <= rational("1") && velocity >= rational("1/2"));
        }
    }
}

#[test]
fn malformed_variations_are_rejected() {
    let with = |vary: &str| {
        codes(&source("", LINE, PLACE).replace(
            "vary loose { seed = 7; velocity = 3/50; timing = 8ms; }",
            vary,
        ))
    };
    for (vary, code) in [
        ("vary loose { seed = 0; }", DiagnosticCode::Range),
        ("vary loose { seed = 4294967296; }", DiagnosticCode::Range),
        ("vary loose { seed = 3/2; }", DiagnosticCode::Range),
        ("vary loose { velocity = 1/10; }", DiagnosticCode::Range),
        (
            "vary loose { seed = 7; velocity = 3/5; }",
            DiagnosticCode::Range,
        ),
        (
            "vary loose { seed = 7; timing = 60ms; }",
            DiagnosticCode::Range,
        ),
        (
            "vary loose { seed = 7; timing = 1/8q; }",
            DiagnosticCode::Unit,
        ),
        (
            "vary loose { seed = 7; pitch = 5ct; }",
            DiagnosticCode::UnknownField,
        ),
    ] {
        assert!(with(vary).contains(&code), "{vary}: {code:?}");
    }
    let wrong = source("", LINE, &PLACE.replace("vary = &loose", "vary = &line"));
    assert!(codes(&wrong).contains(&DiagnosticCode::Reference));
}

#[test]
fn a_varied_placement_is_not_materialized() {
    let document = SourceDocument::parse(source("", LINE, PLACE)).unwrap();
    let error = FoundationEditContext
        .prepare_materialize_instance(document.authored(), "main", "copy")
        .unwrap_err();
    assert_eq!(error.code, "E_CAPABILITY");
}

/// F67: a draw that moves an event outside the score names that event, not
/// the plan's event list (§23), under a steady tempo and under a ramp, which
/// schedule events by different routes.
#[test]
fn an_event_moved_out_of_the_score_is_named() {
    let timing = rational("1/125");
    let seed = (1..100)
        .find(|&seed| offset(seed, "opening/0/n", "timing", &timing).is_negative())
        .expect("a seed that draws the first note early");
    let pattern = "  note n { at = 0q; dur = 1q; pitch = C4; }";
    let place = "place opening { pattern = &line; track = &t; at = 0q; vary = &loose; }";
    let steady = source("", pattern, place).replace("seed = 7;", &format!("seed = {seed};"));
    let ramp = steady.replace(
        "(0q, 120bpm, step)",
        "(0q, 120bpm, linear), (32q, 132bpm, step)",
    );
    for text in [steady, ramp] {
        let diagnostics = compile_bundle_artifact(&SourceBundle::new("main.maac", text.as_str()))
            .unwrap_err()
            .into_vec();
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == DiagnosticCode::Interval)
            .unwrap_or_else(|| panic!("expected E_INTERVAL, got {diagnostics:#?}"));
        assert_eq!(diagnostic.object_path, ["line", "n"], "{diagnostic:#?}");
        assert!(
            diagnostic.message.contains("`opening/0/n`"),
            "{}",
            diagnostic.message
        );
        let span = diagnostic.span.expect("span");
        assert!(
            span.slice(&text).contains("note n"),
            "{:?}",
            span.slice(&text)
        );
    }
}

/// Each wrong field is reported at that field, with the range it must keep.
#[test]
fn a_wrong_vary_is_reported_at_its_field() {
    for (vary, code, message, field) in [
        (
            "seed = 0;",
            DiagnosticCode::Range,
            "vary.seed must be an integer from 1 to 4294967295",
            "seed",
        ),
        (
            "seed = 7; velocity = 3/5;",
            DiagnosticCode::Range,
            "vary.velocity must be from 0 to 1/2",
            "velocity",
        ),
        (
            "seed = 7; timing = 51ms;",
            DiagnosticCode::Range,
            "vary.timing must be from 0ms to 50ms",
            "timing",
        ),
        (
            "velocity = 1/10;",
            DiagnosticCode::Range,
            "vary requires field `seed`",
            "seed",
        ),
        (
            "seed = 7; pitch = 1;",
            DiagnosticCode::UnknownField,
            "unknown field `pitch` on vary",
            "pitch",
        ),
    ] {
        let text = source("", LINE, PLACE).replace(
            "vary loose { seed = 7; velocity = 3/50; timing = 8ms; }",
            &format!("vary loose {{ {vary} }}"),
        );
        let diagnostics = compile_bundle_artifact(&SourceBundle::new("main.maac", text.as_str()))
            .unwrap_err()
            .into_vec();
        let [diagnostic] = diagnostics.as_slice() else {
            panic!("{vary}: expected one diagnostic, got {diagnostics:#?}");
        };
        assert_eq!(diagnostic.code, code, "{vary}");
        assert_eq!(diagnostic.message, message, "{vary}");
        assert_eq!(diagnostic.object_path, ["loose"], "{vary}");
        assert_eq!(diagnostic.field_path, [field], "{vary}");
    }
}
