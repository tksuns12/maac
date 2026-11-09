//! Desk moves with the music (§12): several automations on one parameter,
//! as long as they do not overlap in time, and automations written in a
//! placement, timed from the placement or from one of its events.

use maac::compiler::compile_bundle_artifact_with_limits;
use maac::editing::{
    apply_transaction, AuthoredDocument, FoundationEditContext, Operation, SourceDocument,
    Transaction,
};
use maac::plan::PlanLimits;
use maac::{parse, render_artifact_with_limits, DiagnosticCode, PlanArtifact, SourceBundle};
use serde_json::{json, Value};

const STEADY: &str = "tempo clock { points = [(0q, 120bpm, step)]; }";
const RAMP: &str = "tempo clock { points = [(0q, 120bpm, linear), (8q, 132bpm, step)]; }";

/// A held tone through a gain whose base value is 1, with `desk` written
/// beside it.
fn source(tempo: &str, desk: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 8q]; tail = 40ms; rate = 48000Hz; tempo = &clock; meter = &metre; output = &fader:out; }}
{tempo}
meter metre {{ points = [(0q, 4, 4)]; }}
node tone {{ type = "core.sine/1"; params = {{ attack = 8ms; release = 30ms; level = 1/4; }}; }}
node fader {{ type = "core.gain/1"; config = {{ channels = 1; }}; params = {{ gain = 1; }}; }}
connect tone_to_fader {{ from = &tone:out; to = &fader:in; }}
pattern held {{ length = 8q; note n {{ at = 0q; dur = 15/2q; pitch = A4; velocity = 1/2; }} }}
track notes {{ target = &tone:events; }}
place play {{ pattern = &held; track = &notes; at = 0q; }}
{desk}
"#
    )
}

fn compile(source: &str) -> Result<PlanArtifact, maac::Diagnostics> {
    compile_bundle_artifact_with_limits(
        &SourceBundle::new("main.maac", source),
        &PlanLimits::song(),
    )
}

fn render(source: &str) -> Vec<f64> {
    let plan = compile(source).unwrap_or_else(|error| panic!("{error:?}"));
    let mut out = Vec::new();
    render_artifact_with_limits(&plan, &PlanLimits::song(), |frame| {
        out.extend_from_slice(frame);
        Ok(())
    })
    .unwrap();
    out
}

/// The one error a document fails with.
fn failure(source: &str) -> maac::Diagnostic {
    let diagnostics = compile(source).expect_err("the document should fail");
    let mut all = diagnostics.iter();
    let first = all.next().expect("one diagnostic").clone();
    assert!(all.next().is_none(), "{diagnostics:?}");
    first
}

const DIP: &str = "curve dip { clock = score; points = [(0q, 0, linear), (1q, 1, step)]; }";

#[test]
fn several_moves_render_as_the_curve_joined_by_hand() {
    // IDs in the opposite order to time: the plan orders moves by time.
    let two = format!(
        "{DIP}
automation z_first {{ target = &fader.params.gain; curve = &dip; at = 2q; }}
automation a_second {{ target = &fader.params.gain; curve = &dip; at = 5q; }}"
    );
    let joined = "curve joined { clock = score; points = [(0q, 1, step), (2q, 0, linear), (3q, 1, step), (5q, 0, linear), (6q, 1, step)]; }
automation ride { target = &fader.params.gain; curve = &joined; at = 0q; }";
    for tempo in [STEADY, RAMP] {
        let moves = render(&source(tempo, &two));
        assert_eq!(moves, render(&source(tempo, joined)));
        assert_ne!(moves, render(&source(tempo, "")));
    }
}

#[test]
fn a_move_that_starts_where_the_last_ends_takes_over() {
    // The first move ramps to 1 at 3q, where the second starts again at 0.
    // No single curve says this, so the render is checked on each side of
    // 3q, which is frame 72000 at 120 bpm.
    let first = format!(
        "{DIP}
automation dip_1 {{ target = &fader.params.gain; curve = &dip; at = 2q; }}"
    );
    let second = format!(
        "{DIP}
automation dip_2 {{ target = &fader.params.gain; curve = &dip; at = 3q; }}"
    );
    let touching = format!(
        "{first}
automation dip_2 {{ target = &fader.params.gain; curve = &dip; at = 3q; }}"
    );
    let touching = render(&source(STEADY, &touching));
    let first = render(&source(STEADY, &first));
    let second = render(&source(STEADY, &second));
    assert_eq!(touching[..72_000], first[..72_000]);
    assert_eq!(touching[72_000..], second[72_000..]);
}

#[test]
fn the_tail_holds_the_last_move_that_started() {
    // A move anchored after the score's end never starts, so the tail
    // holds the value of the move before it.
    let one = format!(
        "{DIP}
automation dip_1 {{ target = &fader.params.gain; curve = &dip; at = 7q; }}"
    );
    let late = format!(
        "{one}
curve mute {{ clock = score; points = [(0q, 0, step)]; }}
automation never {{ target = &fader.params.gain; curve = &mute; at = 9q; }}"
    );
    for tempo in [STEADY, RAMP] {
        assert_eq!(render(&source(tempo, &late)), render(&source(tempo, &one)));
    }
}

#[test]
fn moves_on_one_parameter_may_not_overlap() {
    for tempo in [STEADY, RAMP] {
        let overlap = source(
            tempo,
            &format!(
                "{DIP}
automation dip_1 {{ target = &fader.params.gain; curve = &dip; at = 2q; }}
automation dip_2 {{ target = &fader.params.gain; curve = &dip; at = 5/2q; }}"
            ),
        );
        let error = failure(&overlap);
        assert_eq!(error.code, DiagnosticCode::AutomationWriter);
        assert_eq!(error.object_path, vec!["dip_2".to_string()]);
        assert_eq!(error.field_path, vec!["at".to_string()]);
        assert!(error.message.contains("`dip_1`"), "{}", error.message);
        let span = error.span.expect("the later move's `at`");
        assert_eq!(&overlap[span.start..span.end], "at = 5/2q;");
    }
}

#[test]
fn two_moves_may_not_start_together_on_any_clock() {
    // At 120 bpm, 2q is 1 s: a seconds move at 1 s starts with the score move.
    let together = source(
        STEADY,
        &format!(
            "{DIP}
curve half {{ clock = seconds; points = [(0s, 1/2, step)]; }}
automation dip_1 {{ target = &fader.params.gain; curve = &dip; at = 2q; }}
automation half_way {{ target = &fader.params.gain; curve = &half; at = 1s; }}"
        ),
    );
    let error = failure(&together);
    assert_eq!(error.code, DiagnosticCode::AutomationWriter);
    assert!(error.message.contains("same instant"), "{}", error.message);

    // A seconds move after the score move's end is valid on either tempo.
    for tempo in [STEADY, RAMP] {
        compile(&source(
            tempo,
            &format!(
                "{DIP}
curve half {{ clock = seconds; points = [(0s, 1/2, step)]; }}
automation dip_1 {{ target = &fader.params.gain; curve = &dip; at = 2q; }}
automation half_way {{ target = &fader.params.gain; curve = &half; at = 2s; }}"
            ),
        ))
        .unwrap_or_else(|error| panic!("{error:?}"));
    }
}

// Moves in a placement.

/// Two notes a bar, placed four times, with `children` in the placement and
/// `desk` beside it. The groove swings `b` from 3/2q to 5/3q.
fn beat(tempo: &str, place: &str, children: &str, desk: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 12q]; tail = 40ms; rate = 48000Hz; tempo = &clock; meter = &metre; output = &fader:out; }}
{tempo}
meter metre {{ points = [(0q, 4, 4)]; }}
node tone {{ type = "core.sine/1"; params = {{ attack = 2ms; release = 30ms; level = 1/4; }}; }}
node fader {{ type = "core.gain/1"; config = {{ channels = 1; }}; params = {{ gain = 1; }}; }}
connect tone_to_fader {{ from = &tone:out; to = &fader:in; }}
groove lazy {{ grid = 1/2q; ratio = 2/3; }}
vary loose {{ seed = 3; velocity = 1/10; timing = 6ms; }}
{DIP}
pattern bar {{ length = 2q;
  note a {{ at = 0q; dur = 1/2q; pitch = A4; }}
  note b {{ at = 3/2q; dur = 1/4q; pitch = E5; }}
}}
track notes {{ target = &tone:events; }}
place play {{ pattern = &bar; track = &notes; {place}
{children}
}}
{desk}
"#
    )
}

const THROW: &str =
    r#"automation throw { event = "2/b"; at = -1/4q; target = &fader.params.gain; curve = &dip; }"#;

/// The score position of each automation in the compiled plan.
fn anchors(source: &str) -> Vec<(String, String)> {
    let plan = compile(source).unwrap_or_else(|error| panic!("{error:?}"));
    let plan: Value = serde_json::from_slice(&plan.to_json().unwrap()).unwrap();
    plan["automation"]
        .as_array()
        .unwrap()
        .iter()
        .map(|lane| {
            let at = match &lane["at"] {
                Value::String(q) => q.clone(),
                anchor => anchor["q"].as_str().unwrap().to_owned(),
            };
            (lane["id"].as_str().unwrap().to_owned(), at)
        })
        .collect()
}

fn anchor(source: &str, id: &str) -> String {
    anchors(source)
        .into_iter()
        .find(|(lane, _)| lane == id)
        .unwrap_or_else(|| panic!("no automation `{id}`"))
        .1
}

#[test]
fn a_move_in_a_placement_plays_at_its_event() {
    // Repetition 2 starts at 4q, and the groove puts its `b` at 4q + 5/3q.
    let place = "at = 0q; count = 4; groove = &lazy;";
    for tempo in [STEADY, RAMP] {
        let thrown = beat(tempo, place, THROW, "");
        assert_eq!(anchor(&thrown, "play__throw"), "65/12");
        let global = beat(
            tempo,
            place,
            "",
            "automation throw { target = &fader.params.gain; curve = &dip; at = 65/12q; }",
        );
        assert_eq!(render(&thrown), render(&global));
    }
}

#[test]
fn moves_follow_the_placement_and_its_events() {
    let children = format!(
        "{THROW}\n  automation drop {{ at = 1q; target = &tone.params.level; curve = &dip; }}"
    );
    let moved = beat(STEADY, "at = 2q; count = 4;", &children, "");
    assert_eq!(anchor(&moved, "play__throw"), "29/4"); // 2q + 4q + 3/2q - 1/4q
    assert_eq!(anchor(&moved, "play__drop"), "3/1");

    // An override that moves the event moves its throw. `set.at` counts
    // from the placement's `at`.
    let overridden = beat(
        STEADY,
        "at = 2q; count = 4;",
        &format!("{THROW}\n  override later {{ event = \"2/b\"; set = {{ at = 7q; }}; }}"),
        "",
    );
    assert_eq!(anchor(&overridden, "play__throw"), "35/4");

    // A variation moves the hit in seconds, not its written position.
    let varied = beat(STEADY, "at = 2q; count = 4; vary = &loose;", THROW, "");
    assert_eq!(anchor(&varied, "play__throw"), "29/4");
}

#[test]
fn a_move_can_name_an_inserted_event() {
    let source = beat(
        STEADY,
        "at = 0q; count = 4;",
        r#"insert extra { note c { at = 7q; dur = 1/4q; pitch = B4; } }
  automation throw { event = "extra/c"; target = &fader.params.gain; curve = &dip; }"#,
        "",
    );
    assert_eq!(anchor(&source, "play__throw"), "7/1");
}

#[test]
fn a_move_whose_event_is_not_there_is_refused() {
    for (place, children) in [
        // Repetition 5 of four.
        (
            "at = 0q; count = 4;",
            r#"automation throw { event = "5/b"; target = &fader.params.gain; curve = &dip; }"#
                .to_owned(),
        ),
        // A leaf the pattern does not have.
        (
            "at = 0q; count = 4;",
            r#"automation throw { event = "2/z"; target = &fader.params.gain; curve = &dip; }"#
                .to_owned(),
        ),
        // An event an override deletes.
        (
            "at = 0q; count = 4;",
            format!("{THROW}\n  override gone {{ event = \"2/b\"; delete = true; }}"),
        ),
    ] {
        let source = beat(STEADY, place, &children, "");
        let error = failure(&source);
        assert_eq!(
            error.code,
            DiagnosticCode::InstanceTarget,
            "{}",
            error.message
        );
        assert_eq!(
            error.object_path,
            vec!["play".to_string(), "throw".to_string()]
        );
        assert_eq!(error.field_path, vec!["event".to_string()]);
    }
}

#[test]
fn moves_in_a_placement_and_global_moves_share_one_timeline() {
    // The throw starts at 21/4q, inside the global move from 5q to 6q.
    let source = beat(
        STEADY,
        "at = 0q; count = 4;",
        THROW,
        "automation ride { target = &fader.params.gain; curve = &dip; at = 5q; }",
    );
    let error = failure(&source);
    assert_eq!(error.code, DiagnosticCode::AutomationWriter);
    assert_eq!(
        error.object_path,
        vec!["play".to_string(), "throw".to_string()]
    );
    assert!(error.message.contains("`ride`"), "{}", error.message);

    // Two throws of one curve on one send, each at its own hit.
    let two = beat(
        STEADY,
        "at = 0q; count = 4;",
        &format!(
            "{THROW}\n  automation again {{ event = \"3/b\"; at = -1/4q; target = &fader.params.gain; curve = &dip; }}"
        ),
        "",
    );
    assert_eq!(anchor(&two, "play__again"), "29/4");
}

fn edit(document: &mut AuthoredDocument, operation: Operation) -> Result<(), String> {
    let transaction = Transaction::new(document.revision().to_owned(), vec![operation]).unwrap();
    apply_transaction(document, &transaction, &FoundationEditContext)
        .map(|_| ())
        .map_err(|error| error.code.clone())
}

#[test]
fn edits_keep_a_move_on_its_event() {
    let source = beat(STEADY, "at = 0q; count = 4;", THROW, "");
    // Renaming the leaf rewrites the address the throw names.
    let mut document = AuthoredDocument::from_document(&parse(&source).unwrap()).unwrap();
    edit(
        &mut document,
        Operation::RenameId {
            object: vec!["bar".into(), "b".into()],
            new_id: "back".into(),
        },
    )
    .unwrap();
    assert_eq!(
        document.tree()["objects"]["play"]["children"]["throw"]["fields"]["event"]["v"],
        "2/back"
    );
    // A smaller count would leave the throw without its hit.
    let error = edit(
        &mut document,
        Operation::Set {
            object: vec!["play".into()],
            field: vec!["count".into()],
            value: json!({"t":"number","n":"2","d":"1"}),
            expect: None,
            expect_absent: false,
        },
    )
    .unwrap_err();
    assert_eq!(error, "E_INSTANCE_TARGET");
    // So would an override that deletes it.
    let error = edit(
        &mut document,
        Operation::InsertObject {
            parent: vec!["play".into()],
            id: "gone".into(),
            object_value: json!({"kind":"override","fields":{
                "event":{"t":"string","v":"2/back"},
                "delete":{"t":"boolean","v":true}},"children":{}}),
        },
    )
    .unwrap_err();
    assert_eq!(error, "E_INSTANCE_TARGET");
    // Deleting another hit is fine.
    edit(
        &mut document,
        Operation::InsertObject {
            parent: vec!["play".into()],
            id: "gone".into(),
            object_value: json!({"kind":"override","fields":{
                "event":{"t":"string","v":"1/back"},
                "delete":{"t":"boolean","v":true}},"children":{}}),
        },
    )
    .unwrap();
}

#[test]
fn materializing_a_placement_keeps_its_moves_where_they_were() {
    let source = beat(
        STEADY,
        "at = 0q; count = 4;",
        &format!(
            "{THROW}\n  automation drop {{ at = 1q; target = &tone.params.level; curve = &dip; }}"
        ),
        "",
    );
    let mut document = SourceDocument::parse(&source).unwrap();
    let plan = FoundationEditContext
        .prepare_materialize_instance(document.authored(), "play", "own")
        .unwrap();
    document
        .apply(&plan.transaction, &FoundationEditContext)
        .unwrap();
    let event = document.authored().tree()["objects"]["play"]["children"]["throw"]["fields"]
        ["event"]["v"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(event.starts_with("0/_repeat2/0/"), "{event}");
    let materialized = document.source().to_owned();
    assert_eq!(anchors(&materialized), anchors(&source));
    assert_eq!(render(&materialized), render(&source));
}
