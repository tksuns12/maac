use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact;
use maac::plan::EventKind;
use maac::{parse_rational, PlanArtifact, Rational};

fn q(text: &str) -> Rational {
    parse_rational(text).unwrap()
}

fn source(body: &str, tempo: &str) -> SourceBundle {
    SourceBundle::new(
        "main.maac",
        format!(
            r#"maac 1;
project song {{ score=[0q,4q]; rate=48000Hz; tempo=&clock; meter=&metre; output=&sine:out; }}
tempo clock {{ points={tempo}; }}
meter metre {{ points=[(0q,4,4)]; }}
node sine {{ type="core.sine/1"; config={{ voices=8; }}; }}
track t {{ target=&sine:events; }}
{body}"#
        ),
    )
}

#[test]
fn narrow_window_returns_full_spilling_notes_and_structural_sources() {
    let bundle = source(
        r#"
pattern leaf { length=1q; note n { at=0q; dur=3q; pitch=C4; onset_offset=20ms; release_offset=20ms; } }
pattern parent { length=2q; use u { pattern=&leaf; at=0q; count=2; boundary=spill; } }
place p { pattern=&parent; track=&t; at=0q; count=2; }
"#,
        "[(0q,120bpm,step)]",
    );
    let artifact = compile_bundle_artifact(&bundle).unwrap();
    let events = artifact
        .query_events_score_window(&q("5/4"), &q("3/2"))
        .unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.address.as_str())
            .collect::<Vec<_>>(),
        ["p/0/u/0/n", "p/0/u/1/n"]
    );
    assert_eq!(events[0].score_on_q, q("0"));
    assert_eq!(events[0].score_off_q, Some(q("3")));
    assert_eq!(events[1].score_on_q, q("1"));
    assert_eq!(events[1].score_off_q, Some(q("4")));
    assert_eq!(events[0].source.path, ["leaf", "n"]);
    assert_eq!(events[0].onset_offset_seconds, q("1/50"));
    assert_eq!(events[0].release_offset_seconds, q("1/50"));
    assert!(events[0].source.span.is_some());
    assert!(matches!(events[0].kind, EventKind::Note { .. }));

    let at_score_end = artifact
        .query_events_score_window(&q("7/2"), &q("4"))
        .unwrap();
    let last = at_score_end
        .iter()
        .find(|event| event.address == "p/1/u/1/n")
        .unwrap();
    assert_eq!(last.score_off_q, Some(q("6")));
    assert_eq!(last.off_frame, Some(96_000));

    let tight_limits = maac::plan::PlanLimits {
        max_events: 0,
        ..maac::plan::PlanLimits::default()
    };
    let error = artifact
        .query_events_score_window_with_limits(&q("1"), &q("1"), &tight_limits)
        .unwrap_err();
    assert_eq!(error.code, "E_RESOURCE_LIMIT");

    let round_trip = PlanArtifact::from_json(&artifact.to_json().unwrap()).unwrap();
    assert_eq!(
        round_trip
            .query_events_score_window(&q("5/4"), &q("3/2"))
            .unwrap(),
        events
    );

    let mut reordered_json: serde_json::Value =
        serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    reordered_json["events"].as_array_mut().unwrap().reverse();
    let reordered = PlanArtifact::from_json(&serde_json::to_vec(&reordered_json).unwrap()).unwrap();
    assert_eq!(
        reordered
            .query_events_score_window(&q("5/4"), &q("3/2"))
            .unwrap(),
        events
    );
}

#[test]
fn half_open_boundaries_and_final_occurrence_edits_are_observed() {
    let bundle = source(
        r#"
pattern notes {
  length=3q;
  note ends { at=0q; dur=1q; pitch=C4; }
  note crosses { at=1/2q; dur=2q; pitch=D4; }
  note starts { at=1q; dur=1q; pitch=E4; }
  message left { at=1q; protocol="raw/test"; bytes=[1]; }
  message right { at=2q; protocol="raw/test"; bytes=[2]; }
}

place p { pattern=&notes; track=&t; at=0q;
  override move { event="0/starts"; set={ at=2q; }; }
  override gone { event="0/crosses"; delete=true; }
  insert once { note inserted { at=1q; dur=1/2q; pitch=G4; } }
}
"#,
        "[(0q,120bpm,step)]",
    );
    let artifact = compile_bundle_artifact(&bundle).unwrap();
    let events = artifact
        .query_events_score_window(&q("1"), &q("2"))
        .unwrap();
    let addresses: Vec<_> = events.iter().map(|event| event.address.as_str()).collect();
    assert_eq!(addresses, ["p/0/left", "p/once/inserted"]);
    assert_eq!(events[1].score_off_q, Some(q("3/2")));
    assert!(matches!(events[0].kind, EventKind::Message { .. }));
    assert!(artifact
        .query_events_score_window(&q("3"), &q("4"))
        .unwrap()
        .is_empty());
}

#[test]
fn inherited_cut_ends_the_gate_before_window_selection() {
    let bundle = source(
        r#"
pattern leaf { length=1q; note n { at=3/4q; dur=1q; pitch=C4; } }
pattern parent { length=1q; use u { pattern=&leaf; at=0q; boundary=cut; } }
place p { pattern=&parent; track=&t; at=0q; }
"#,
        "[(0q,120bpm,step)]",
    );
    let artifact = compile_bundle_artifact(&bundle).unwrap();
    let inside = artifact
        .query_events_score_window(&q("7/8"), &q("1"))
        .unwrap();
    assert_eq!(inside.len(), 1);
    assert_eq!(inside[0].address, "p/0/u/0/n");
    assert_eq!(inside[0].score_on_q, q("3/4"));
    assert_eq!(inside[0].score_off_q, Some(q("1")));
    assert!(artifact
        .query_events_score_window(&q("1"), &q("5/4"))
        .unwrap()
        .is_empty());
}

#[test]
fn invalid_score_windows_are_rejected() {
    let artifact = compile_bundle_artifact(&source("", "[(0q,120bpm,step)]")).unwrap();
    assert!(artifact
        .query_events_score_window(&q("1"), &q("1"))
        .unwrap()
        .is_empty());
    for (start, end) in [("2", "1"), ("-1", "1"), ("3", "5")] {
        let error = artifact
            .query_events_score_window(&q(start), &q(end))
            .unwrap_err();
        assert_eq!(error.code, "E_INTERVAL", "{start}..{end}");
    }
    let oversized = Rational::from_integer(num_bigint::BigInt::from(1u8) << 4_097usize);
    let error = artifact
        .query_events_score_window(&oversized, &q("1"))
        .unwrap_err();
    assert_eq!(error.code, "E_RESOURCE_LIMIT");
}

#[test]
fn empty_window_still_rejects_invalid_retained_plan() {
    let bundle = source("", "[(0q,120bpm,step)]");
    let document = maac::parse(&bundle.sources["main.maac"]).unwrap();
    let mut plan = maac::compile(&document).unwrap();
    plan.output.total_frames = 0;
    let artifact = PlanArtifact::from(maac::plan_v3::VersionedPlan::Legacy(plan));
    assert!(artifact
        .query_events_score_window(&q("1"), &q("1"))
        .is_err());
}

#[test]
fn score_window_semantics_are_stable_with_ramp_tempo() {
    let bundle = source(
        r#"pattern ptn { length=4q; note n { at=0q; dur=3q; pitch=C4; } }
place p { pattern=&ptn; track=&t; at=0q; }"#,
        "[(0q,120bpm,linear),(4q,240bpm,step)]",
    );
    let artifact = compile_bundle_artifact(&bundle).unwrap();
    assert_eq!(artifact.version(), 3);
    let events = artifact
        .query_events_score_window(&q("2"), &q("5/2"))
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].score_on_q, q("0"));
    assert_eq!(events[0].score_off_q, Some(q("3")));
}

#[test]
fn native_hit_is_a_point_at_the_left_window_boundary() {
    let sample_bytes: Vec<u8> = [1f32, 0.5f32]
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();
    let source = format!(
        r#"maac 1;
project song {{ score=[0q,4q]; rate=48000Hz; tempo=&clock; meter=&metre; output=&kit:out; }}
tempo clock {{ points=[(0q,120bpm,step)]; }}
meter metre {{ points=[(0q,4,4)]; }}
asset sample {{ kind=audio; path="sample.pcm"; hash="{}"; format="pcm_f32le_interleaved/1"; rate=24000Hz; channels=1; frames=2; }}
node kit {{ type="core.kit/1"; config={{ channels=1; samples=[{{ key="kick"; asset=&sample; }}]; }}; }}
track t {{ target=&kit:events; }}
pattern hits {{ length=2q; hit kick {{ at=1q; key="kick"; }} }}
place p {{ pattern=&hits; track=&t; at=0q; }}"#,
        maac::bundle::sha256_digest(&sample_bytes)
    );
    let mut bundle = SourceBundle::new("main.maac", source);
    bundle.assets.insert("sample.pcm".into(), sample_bytes);
    let artifact = compile_bundle_artifact(&bundle).unwrap();
    assert_eq!(artifact.version(), 4);
    assert!(artifact
        .query_events_score_window(&q("0"), &q("1"))
        .unwrap()
        .is_empty());
    let events = artifact
        .query_events_score_window(&q("1"), &q("2"))
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].address, "p/0/kick");
    assert_eq!(events[0].score_on_q, q("1"));
    assert_eq!(events[0].score_off_q, None);
    assert!(matches!(events[0].kind, EventKind::Hit { .. }));
}
