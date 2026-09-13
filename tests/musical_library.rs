use std::collections::BTreeMap;

use maac::bundle::{sha256_digest, SourceBundle};
use maac::plan::EventKind;
use maac::{check_bundle, compile_bundle, DiagnosticCode, Rational};

const MUSICAL_LIBRARY: &str = r#"maac 1;
library phrases { version = "1.0.0"; }
tuning open_fifths {
  period = 1200ct;
  steps = [0ct, 700ct];
  reference_index = 0;
  reference_frequency = 220Hz;
}
curve bend { clock = normalized; points = [(0, 0ct, linear), (1, 100ct, step)]; }
curve motion { clock = score; points = [(0q, 1/4, linear), (1q, 3/4, step)]; }
pattern leaf {
  length = 1q;
  note tone {
    at = 0q;
    dur = 1/2q;
    pitch = degree(1, &open_fifths);
    expression pitch_bend { kind = pitch; curve = &bend; }
  }
}
pattern phrase {
  length = 1q;
  use nested { pattern = &leaf; at = 0q; }
}
"#;

fn project_source(pin: &str, bpm: u32, place_at: u32, automation_at: u32) -> String {
    format!(
        r#"maac 1;
project song {{ score = [0q, 4q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; }}
tempo clock {{ points = [(0q, {bpm}bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
import music {{ path = "lib/phrases.maac"; hash = "{pin}"; }}
node synth {{ type = "core.sine/1"; }}
track notes {{ target = &synth:events; }}
place play {{ pattern = &music.phrase; track = &notes; at = {place_at}q; }}
automation move {{ target = &synth.params.level; curve = &music.motion; at = {automation_at}q; }}
"#
    )
}

fn project_bundle(source: String) -> SourceBundle {
    SourceBundle {
        entry: "scores/main.maac".into(),
        sources: BTreeMap::from([
            ("scores/main.maac".into(), source),
            ("scores/lib/phrases.maac".into(), MUSICAL_LIBRARY.into()),
        ]),
        assets: BTreeMap::new(),
    }
}

fn first_code(error: &maac::Diagnostics) -> DiagnosticCode {
    error.first().expect("a diagnostic").code
}

#[test]
fn two_projects_reuse_one_pinned_musical_library_without_capturing_context() {
    let pin = sha256_digest(MUSICAL_LIBRARY.as_bytes());
    let fast_bundle = project_bundle(project_source(&pin, 120, 0, 0));
    let slow_bundle = project_bundle(project_source(&pin, 60, 2, 1));

    check_bundle(&fast_bundle).expect("the first project should check");
    check_bundle(&slow_bundle).expect("the second project should check");
    let fast = compile_bundle(&fast_bundle).expect("the first project should compile");
    let slow = compile_bundle(&slow_bundle).expect("the second project should compile");

    assert_eq!(fast.events.len(), 1);
    assert_eq!(slow.events.len(), 1);
    assert_eq!(fast.events[0].address, "play/0/nested/0/tone");
    assert_eq!(slow.events[0].address, "play/0/nested/0/tone");
    assert_eq!(fast.events[0].source.object, "tone");
    assert_eq!(
        fast.events[0].source.path,
        vec!["music", "leaf", "tone"]
    );
    assert_eq!(slow.events[0].source.path, fast.events[0].source.path);
    assert_eq!(fast.events[0].score_on_q, Rational::from_integer(0.into()));
    assert_eq!(slow.events[0].score_on_q, Rational::from_integer(2.into()));
    assert_eq!(fast.events[0].on_seconds, Rational::from_integer(0.into()));
    assert_eq!(slow.events[0].on_seconds, Rational::from_integer(2.into()));

    let fast_pitch = match &fast.events[0].kind {
        EventKind::Note {
            pitch_hz,
            pitch_expression,
            ..
        } => {
            assert!(pitch_expression.is_some());
            *pitch_hz
        }
        other => panic!("expected a note, got {other:?}"),
    };
    let slow_pitch = match &slow.events[0].kind {
        EventKind::Note { pitch_hz, .. } => *pitch_hz,
        other => panic!("expected a note, got {other:?}"),
    };
    let expected_pitch = 220.0 * 2.0_f64.powf(700.0 / 1200.0);
    assert!((fast_pitch - expected_pitch).abs() < 1.0e-9);
    assert_eq!(fast_pitch, slow_pitch);

    assert_eq!(fast.automation[0].points, slow.automation[0].points);
    assert_eq!(fast.automation[0].at, Rational::from_integer(0.into()));
    assert_eq!(slow.automation[0].at, Rational::from_integer(1.into()));
    let fast_resources = fast.instruments.expect("bundle provenance");
    let slow_resources = slow.instruments.expect("bundle provenance");
    assert_eq!(fast_resources.dependencies[0].hash, pin);
    assert_eq!(slow_resources.dependencies[0].hash, pin);
    assert!(fast_resources
        .source_files
        .iter()
        .any(|source| source.path == "scores/lib/phrases.maac" && source.hash == pin));
}

#[test]
fn altered_pin_is_rejected_before_musical_reuse() {
    let wrong_pin = format!("sha256:{}", "0".repeat(64));
    let error = check_bundle(&project_bundle(project_source(&wrong_pin, 120, 0, 0)))
        .expect_err("an altered dependency pin must fail");
    assert_eq!(first_code(&error), DiagnosticCode::Hash);
}

#[test]
fn aliases_isolate_the_same_library_and_keep_authored_mapping_paths() {
    let pin = sha256_digest(MUSICAL_LIBRARY.as_bytes());
    let source = format!(
        r#"maac 1;
project song {{ score = [0q, 3q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
import left {{ path = "lib/phrases.maac"; hash = "{pin}"; }}
import right {{ path = "lib/phrases.maac"; hash = "{pin}"; }}
node synth {{ type = "core.sine/1"; }}
track notes {{ target = &synth:events; }}
place a {{ pattern = &left.phrase; track = &notes; at = 0q; }}
place b {{ pattern = &right.phrase; track = &notes; at = 1q; }}
"#
    );
    let plan = compile_bundle(&project_bundle(source)).expect("both aliases should compile");

    assert_eq!(plan.events.len(), 2);
    assert_eq!(plan.events[0].address, "a/0/nested/0/tone");
    assert_eq!(plan.events[0].source.path, vec!["left", "leaf", "tone"]);
    assert_eq!(plan.events[1].address, "b/0/nested/0/tone");
    assert_eq!(
        plan.events[1].source.path,
        vec!["right", "leaf", "tone"]
    );
}

#[test]
fn library_only_check_validates_every_export_but_compile_remains_a_conflict() {
    let library = SourceBundle::new("phrases.maac", MUSICAL_LIBRARY);
    check_bundle(&library).expect("all musical exports should be valid");
    assert_eq!(
        first_code(&compile_bundle(&library).expect_err("a library is not a composition")),
        DiagnosticCode::Conflict
    );

    let invalid = MUSICAL_LIBRARY.replace(
        "curve motion { clock = score; points = [(0q, 1/4, linear), (1q, 3/4, step)]; }",
        "curve motion { clock = score; points = [(0q, 1/4, linear), (1q, 3/4, linear)]; }",
    );
    assert_eq!(
        first_code(&check_bundle(&SourceBundle::new("phrases.maac", invalid))
            .expect_err("an unused malformed export must still fail")),
        DiagnosticCode::Range
    );
}
