//! D14: every §23 source-validation code has a public trigger, and each
//! resulting diagnostic carries the §23 contract: stable code, source object
//! path, relevant field path, source span, and a nonempty message.

use maac::diagnostic::{Diagnostic, DiagnosticCode};
use maac::{check_versioned, parse};

fn base(extra: &str) -> String {
    format!(
        r#"maac 1;
project p {{
  score = [0q, 4q];
  rate = 48000Hz;
  tempo = &clock;
  meter = &metre;
  output = &master:out;
}}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node master {{ type = "core.sum/1"; config = {{ channels = 1; }}; }}
node tone {{ type = "core.sine/1"; }}
connect tone_out {{ from = &tone:out; to = &master:in; }}
track melody {{ target = &tone:events; }}
{extra}
"#
    )
}

/// The first diagnostic with `code` from parsing plus full validation.
fn diagnostic(source: &str, code: DiagnosticCode) -> Diagnostic {
    let all: Vec<Diagnostic> = match parse(source) {
        Err(errors) => errors.into_iter().collect(),
        Ok(document) => check_versioned(&document)
            .expect_err("vector must fail validation")
            .into_iter()
            .collect(),
    };
    all.iter()
        .find(|diagnostic| diagnostic.code == code)
        .cloned()
        .unwrap_or_else(|| panic!("expected {code:?}, got {all:#?}"))
}

struct Vector {
    code: DiagnosticCode,
    source: String,
    object: &'static [&'static str],
    field: &'static [&'static str],
    /// Text the reported span must contain.
    excerpt: &'static str,
}

fn vector(
    code: DiagnosticCode,
    extra: &str,
    object: &'static [&'static str],
    field: &'static [&'static str],
    excerpt: &'static str,
) -> Vector {
    Vector {
        code,
        source: base(extra),
        object,
        field,
        excerpt,
    }
}

/// A vector that edits the shared base text instead of appending to it.
fn edited(
    code: DiagnosticCode,
    from: &str,
    to: &str,
    object: &'static [&'static str],
    field: &'static [&'static str],
    excerpt: &'static str,
) -> Vector {
    let source = base("");
    assert!(source.contains(from), "edit anchor {from:?}");
    Vector {
        code,
        source: source.replacen(from, to, 1),
        object,
        field,
        excerpt,
    }
}

fn source_vectors() -> Vec<Vector> {
    use DiagnosticCode::*;
    let riff = "pattern riff { length = 1q; note n { at = 0q; dur = 1q; pitch = C4; } }\n";
    let pressure =
        "curve squeeze { clock = normalized; points = [(0, 0, linear), (1, 1, step)]; }\n";
    vec![
        vector(
            Syntax,
            "node broken { type = ; }",
            &["broken"],
            &["type"],
            ";",
        ),
        edited(Version, "maac 1;", "maac 2;", &[], &[], "2"),
        vector(
            DuplicateId,
            "node tone { type = \"core.sine/1\"; }",
            &["tone"],
            &[],
            "tone",
        ),
        vector(
            DuplicateField,
            "node amp { type = \"core.gain/1\"; config = { channels = 1; channels = 2; }; }",
            &["amp"],
            &["config", "channels"],
            "channels",
        ),
        vector(
            DuplicateField,
            "node twice { type = \"core.gain/1\"; type = \"core.gain/1\"; config = { channels = 1; }; }",
            &["twice"],
            &["type"],
            "type",
        ),
        vector(
            UnknownField,
            "region r { span = [0q, 1q]; mood = \"bright\"; }",
            &["r"],
            &["mood"],
            "mood",
        ),
        vector(UnknownKind, "mystery thing { value = 1; }", &["thing"], &[], "mystery"),
        vector(
            Reference,
            "node amp { type = \"core.gain/1\"; config = { channels = 1; }; }\nconnect bad { from = &missing:out; to = &amp:in; }",
            &["bad"],
            &["from"],
            "&missing:out",
        ),
        vector(
            Unit,
            "region r { span = [0s, 1s]; }",
            &["r"],
            &["span"],
            "0s",
        ),
        vector(
            Range,
            &format!("{}place pl {{ pattern = &riff; track = &melody; at = 0q; }}", riff.replace("pitch = C4;", "pitch = C4; velocity = 2;")),
            &["riff", "n"],
            &["velocity"],
            "2",
        ),
        edited(
            Tempo,
            "(0q, 120bpm, step)",
            "(0q, 0bpm, step)",
            &["clock"],
            &["points", "0"],
            "0bpm",
        ),
        edited(
            MeterBoundary,
            "(0q, 4, 4)",
            "(0q, 4, 4), (5q, 3, 4)",
            &["metre"],
            &["points"],
            "5q",
        ),
        vector(
            Interval,
            "region r { span = [2q, 1q]; }",
            &["r"],
            &["span"],
            "[2q, 1q]",
        ),
        vector(
            SubsampleNote,
            "pattern riff { length = 1q; note n { at = 1/1000000q; dur = 1/1000000q; pitch = C4; } }\nplace pl { pattern = &riff; track = &melody; at = 0q; }",
            &["riff", "n"],
            &["dur"],
            "1/1000000q",
        ),
        vector(
            PatternCycle,
            "pattern a { length = 1q; use loop { pattern = &a; at = 0q; } }\nplace pl { pattern = &a; track = &melody; at = 0q; }",
            &["a", "loop"],
            &["pattern"],
            "&a",
        ),
        vector(
            InstanceTarget,
            &format!("{riff}place pl {{ pattern = &riff; track = &melody; at = 0q; override edit {{ event = \"0/missing\"; set = {{ pitch = C5; }}; }} }}"),
            &["pl", "edit"],
            &["event"],
            "\"0/missing\"",
        ),
        vector(
            ResourceLimit,
            &format!(
                "node amp {{ type = \"core.gain/1\"; config = {{ channels = 1; }}; params = {{ gain = 1{}; }}; }}",
                "0".repeat(20_000)
            ),
            &["amp"],
            &["params", "gain"],
            "10000",
        ),
        vector(
            AutomationWriter,
            "node fader { type = \"core.fader/1\"; config = { channels = 1; }; }\ncurve c { clock = score; points = [(0q, 0dB, step)]; }\nautomation a { target = &fader.params.level; curve = &c; at = 0q; }\nautomation b { target = &fader.params.level; curve = &c; at = 2q; }",
            &["b"],
            &["target"],
            "&fader.params.level",
        ),
        vector(
            Capability,
            &format!(
                "{pressure}{}place pl {{ pattern = &riff; track = &melody; at = 0q; }}",
                riff.replace(
                    "pitch = C4; }",
                    "pitch = C4; expression press { kind = pressure; curve = &squeeze; } }"
                )
            ),
            // Receiver capability is known only after expansion, so the
            // diagnostic names the authored note that produced the event.
            &["riff", "n"],
            &[],
            "pressure",
        ),
        vector(
            PortType,
            "node wide { type = \"core.gain/1\"; config = { channels = 2; }; }\nconnect narrow { from = &tone:out; to = &wide:in; }",
            &["narrow"],
            &[],
            "connect",
        ),
        vector(
            AlgebraicLoop,
            "node amp { type = \"core.gain/1\"; config = { channels = 1; }; }\nconnect self_loop { from = &amp:out; to = &amp:in; }",
            &["self_loop"],
            &["to"],
            "&amp:in",
        ),
        vector(
            AlgebraicLoop,
            "node a { type = \"core.gain/1\"; config = { channels = 1; }; }\nnode b { type = \"core.gain/1\"; config = { channels = 1; }; }\nconnect a_to_b { from = &a:out; to = &b:in; }\nconnect b_to_a { from = &b:out; to = &a:in; }\nconnect b_out { from = &b:out; to = &master:in; }",
            &["a_to_b"],
            &["to"],
            "&b:in",
        ),
        vector(
            Nonfinite,
            &format!(
                "node m {{ type = \"core.matrix/1\"; config = {{ inputs = 1; outputs = 1; coefficients = [[1{}]]; }}; }}",
                "0".repeat(400)
            ),
            &["m"],
            &["config", "coefficients"],
            "10000",
        ),
    ]
}

fn assert_contract(vector: &Vector) {
    let diagnostic = diagnostic(&vector.source, vector.code);
    let label = vector.code.as_str();
    assert!(!diagnostic.message.trim().is_empty(), "{label}: message");
    assert_eq!(
        diagnostic.object_path, vector.object,
        "{label}: object path"
    );
    assert_eq!(diagnostic.field_path, vector.field, "{label}: field path");
    let span = diagnostic.span.unwrap_or_else(|| panic!("{label}: span"));
    let text = span.slice(&vector.source);
    assert!(
        text.contains(vector.excerpt),
        "{label}: span {span:?} covers {text:?}, expected {:?}",
        vector.excerpt
    );
}

#[test]
fn every_source_validation_code_meets_the_diagnostic_contract() {
    let mut failures = Vec::new();
    for vector in source_vectors() {
        if let Err(panic) = std::panic::catch_unwind(|| assert_contract(&vector)) {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_default();
            failures.push(message);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}
