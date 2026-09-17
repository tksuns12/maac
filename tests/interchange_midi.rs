use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact;
use maac::interchange::{export_midi1_smf, AdapterPolicy, MIDI_ADAPTER_ID, MIDI_TARGET_PROFILE};
use std::collections::BTreeSet;

fn bundle(body: &str) -> SourceBundle {
    SourceBundle::new(
        "main.maac",
        format!(
            r#"maac 1;
project p {{ score=[0q,4q]; rate=48000Hz; tempo=&t; meter=&m; output=&s:out; }}
tempo t {{ points=[(0q,120bpm,step)]; }}
meter m {{ points=[(0q,4,4)]; }}
node s {{ type="core.sine/1"; config={{ voices=8; }}; }}
track notes {{ target=&s:events; }}
{body}
"#
        ),
    )
}

fn codes(export: &maac::interchange::MidiExport) -> BTreeSet<&str> {
    export
        .loss_report
        .losses
        .iter()
        .map(|loss| loss.code.as_str())
        .collect()
}

#[test]
fn midi_export_states_exact_profile_and_emits_valid_smf_header() {
    let artifact = compile_bundle_artifact(&bundle(
        r#"pattern pat { length=2q; note n { at=0q; dur=1q; pitch=440Hz; velocity=1; } }
place play { pattern=&pat; track=&notes; at=0q; }"#,
    ))
    .unwrap();
    let export = export_midi1_smf(&artifact, &AdapterPolicy::default()).unwrap();
    assert_eq!(&export.bytes[..4], b"MThd");
    assert_eq!(&export.bytes[8..10], &[0, 0]);
    assert_eq!(&export.bytes[10..12], &[0, 1]);
    assert_eq!(&export.bytes[12..14], &[0xe7, 0x28]);
    assert_eq!(export.loss_report.adapter_id, MIDI_ADAPTER_ID);
    assert_eq!(export.loss_report.target_profile, MIDI_TARGET_PROFILE);
    assert!(codes(&export).contains("midi.synthesis_routing_omission"));
    assert!(export.bytes.windows(3).any(|w| w == [0x90, 69, 127]));
    assert!(export.bytes.windows(3).any(|w| w[0] == 0x80 && w[1] == 69));
}

#[test]
fn midi_reports_microtonal_timing_and_overlap_identity_losses() {
    let artifact = compile_bundle_artifact(&bundle(
        r#"pattern pat { length=2q;
  note a { at=0q; dur=1q; pitch=445Hz; velocity=1; onset_offset=1/48000s; }
  note b { at=1/2q; dur=1q; pitch=440Hz; velocity=1; }
  note c { at=3/4q; dur=1q; pitch=440Hz; velocity=1; }
}
place play { pattern=&pat; track=&notes; at=0q; }"#,
    ))
    .unwrap();
    let export = export_midi1_smf(&artifact, &AdapterPolicy::default()).unwrap();
    let found = codes(&export);
    assert!(found.contains("midi.microtonal_pitch"));
    assert!(found.contains("midi.timing_resolution"));
    assert!(found.contains("midi.overlapping_same_key_identity"));
    let overlap = export
        .loss_report
        .losses
        .iter()
        .find(|loss| loss.code == "midi.overlapping_same_key_identity")
        .unwrap();
    assert_eq!(overlap.source_paths.len(), 2);
}

#[test]
fn faithful_mode_refuses_unapproved_loss_and_accepts_explicit_approvals() {
    let artifact = compile_bundle_artifact(&bundle(
        r#"pattern pat { length=1q; note n { at=0q; dur=1/2q; pitch=445Hz; velocity=1; } }
place play { pattern=&pat; track=&notes; at=0q; }"#,
    ))
    .unwrap();
    let error = export_midi1_smf(&artifact, &AdapterPolicy::faithful()).unwrap_err();
    assert_eq!(error.code, "E_CAPABILITY");
    let report = error.loss_report.unwrap();
    assert!(!report.losses.is_empty());

    let mut policy = AdapterPolicy::faithful();
    for loss in &report.losses {
        policy = policy.approve(loss.code.clone());
    }
    export_midi1_smf(&artifact, &policy).unwrap();
}

#[test]
fn midi1_channel_messages_are_preserved_and_other_protocols_are_reported() {
    let artifact = compile_bundle_artifact(&bundle(
        r#"pattern pat { length=1q;
  message good { at=0q; protocol="midi1"; bytes=[176,1,64]; }
  message bad { at=1/2q; protocol="ump"; bytes=[1,2,3,4]; }
}
place play { pattern=&pat; track=&notes; at=0q; }"#,
    ))
    .unwrap();
    let export = export_midi1_smf(&artifact, &AdapterPolicy::default()).unwrap();
    assert!(export.bytes.windows(3).any(|w| w == [176, 1, 64]));
    assert!(codes(&export).contains("midi.message_protocol"));
}

#[test]
fn continuous_tempo_and_audio_are_explicit_losses() {
    use maac::bundle::sha256_digest;
    let bytes: Vec<u8> = [0f32, 0.5, -0.5]
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let source = format!(
        r#"maac 1;
project p {{ score=[0q,2q]; rate=48000Hz; tempo=&t; meter=&m; output=&clip:out; }}
tempo t {{ points=[(0q,120bpm,linear),(2q,180bpm,step)]; }}
meter m {{ points=[(0q,4,4)]; }}
asset take {{ kind=audio; path="take.pcm"; hash="{}"; format="pcm_f32le_interleaved/1"; rate=48000Hz; channels=1; frames=3; }}
track group {{}}
audio clip {{ asset=&take; at=0q; source=[0frame,3frame]; mode=rate; track=&group; }}
"#,
        sha256_digest(&bytes)
    );
    let mut bundle = SourceBundle::new("main.maac", source);
    bundle.assets.insert("take.pcm".into(), bytes);
    let artifact = compile_bundle_artifact(&bundle).unwrap();
    let export = export_midi1_smf(&artifact, &AdapterPolicy::default()).unwrap();
    let found = codes(&export);
    assert!(found.contains("midi.audio_omission"));
    let audio = export
        .loss_report
        .losses
        .iter()
        .find(|loss| loss.code == "midi.audio_omission")
        .unwrap();
    assert!(!audio.source_paths.is_empty());
}

#[test]
fn tempo_ramp_and_per_note_expression_have_required_loss_records() {
    let source = r#"maac 1;
project p { score=[0q,2q]; rate=48000Hz; tempo=&t; meter=&m; output=&s:out; }
tempo t { points=[(0q,120bpm,linear),(2q,180bpm,step)]; }
meter m { points=[(0q,4,4)]; }
node s { type="core.sine/1"; config={ voices=2; }; }
track notes { target=&s:events; }
curve bend { clock=normalized; points=[(0,0ct,linear),(1,100ct,step)]; }
pattern pat { length=1q; note n { at=0q; dur=1q; pitch=A4; expression e { kind=pitch; curve=&bend; } } }
place play { pattern=&pat; track=&notes; at=0q; }
"#;
    let artifact = compile_bundle_artifact(&SourceBundle::new("main.maac", source)).unwrap();
    let export = export_midi1_smf(&artifact, &AdapterPolicy::default()).unwrap();
    let found = codes(&export);
    assert!(found.contains("midi.tempo_ramp_discretization"));
    assert!(found.contains("midi.per_note_expression"));
    for item in &export.loss_report.losses {
        assert!(!item.source_paths.is_empty(), "{}", item.code);
        assert!(!item.property.is_empty());
        assert!(!item.output_limitation.is_empty());
        assert!(!item.decision.detail.is_empty());
    }
    let wire = export.loss_report.to_json().unwrap();
    let decoded: maac::interchange::LossReport = serde_json::from_slice(&wire).unwrap();
    assert_eq!(decoded, export.loss_report);
}

#[test]
fn loss_report_wire_is_versioned_strict_and_bounded() {
    let artifact = compile_bundle_artifact(&bundle(
        r#"pattern pat { length=1q; note n { at=0q; dur=1/2q; pitch=A4; } }
place play { pattern=&pat; track=&notes; at=0q; }"#,
    ))
    .unwrap();
    let export = export_midi1_smf(&artifact, &AdapterPolicy::default()).unwrap();
    let wire = export.loss_report.to_json().unwrap();
    let decoded = maac::interchange::LossReport::from_json(&wire).unwrap();
    assert_eq!(decoded, export.loss_report);

    let duplicate = br#"{"format":"maac.interchange-loss-report","format":"maac.interchange-loss-report","version":1,"adapter_id":"x","target_profile":"y","losses":[]}"#;
    assert_eq!(
        maac::interchange::LossReport::from_json(duplicate)
            .unwrap_err()
            .code,
        "E_SYNTAX"
    );
    let float_version = br#"{"format":"maac.interchange-loss-report","version":1.0,"adapter_id":"x","target_profile":"y","losses":[]}"#;
    assert_eq!(
        maac::interchange::LossReport::from_json(float_version)
            .unwrap_err()
            .code,
        "E_SYNTAX"
    );
}

#[test]
fn zero_velocity_notes_are_omitted_instead_of_becoming_note_offs() {
    let artifact = compile_bundle_artifact(&bundle(
        r#"pattern pat { length=1q; note n { at=0q; dur=1/2q; pitch=A4; velocity=0; } }
place play { pattern=&pat; track=&notes; at=0q; }"#,
    ))
    .unwrap();
    let export = export_midi1_smf(&artifact, &AdapterPolicy::default()).unwrap();
    assert!(codes(&export).contains("midi.zero_velocity_note"));
    assert!(!export.bytes.windows(3).any(|w| w == [0x90, 69, 0]));
}

#[test]
fn overlap_report_identifies_nonadjacent_notes_covered_by_a_long_note() {
    let artifact = compile_bundle_artifact(&bundle(
        r#"pattern pat { length=4q;
  note long { at=0q; dur=3q; pitch=A4; velocity=1; }
  note short_a { at=1/2q; dur=1/4q; pitch=A4; velocity=1; }
  note short_b { at=1q; dur=1/4q; pitch=A4; velocity=1; }
}
place play { pattern=&pat; track=&notes; at=0q; }"#,
    ))
    .unwrap();
    let export = export_midi1_smf(&artifact, &AdapterPolicy::default()).unwrap();
    let overlaps = export
        .loss_report
        .losses
        .iter()
        .filter(|loss| loss.code == "midi.overlapping_same_key_identity")
        .collect::<Vec<_>>();
    assert_eq!(overlaps.len(), 2);
    assert!(overlaps.iter().all(|loss| loss.source_paths.len() == 2));
}
