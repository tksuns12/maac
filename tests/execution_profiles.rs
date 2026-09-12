use std::io::Cursor;

use maac::dsp::{self, DspEngine, RenderError};
use maac::export::{self, WavFormat};
use maac::plan::{PlanError, PlanLimits};
use maac::{
    check, check_bundle, check_bundle_with_limits, check_with_limits, compile, compile_bundle,
    compile_bundle_with_limits, compile_with_limits, load_plan, load_plan_with_limits, parse,
    parse_rational, render, render_with_limits, DiagnosticCode, Plan, SourceBundle,
};

fn source(frames: u64, shared_nodes: usize) -> String {
    let nodes = (0..shared_nodes)
        .map(|index| {
            format!("node m{index} {{ type = \"synth.mix/1\"; config = {{ channels = 1; }}; }}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        r#"maac 1;
project p {{ score = [0q, {frames}/24000q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument tone {{
  channels = 1;
  voice v {{
    channels = 1; amplitude = &amp; output = &osc:out;
    node amp {{ type = "synth.adsr/1"; params = {{ attack = 0s; decay = 0s; sustain = 1; release = 0s; }}; }}
    node osc {{ type = "synth.sine/1"; }}
  }}
  shared s {{
    channels = 1; output = &m0:out;
    {nodes}
    connect input_mix {{ from = &input:out; to = &m0:in; }}
  }}
}}
node sound {{ instrument = &tone; config = {{ voices = 1; }}; }}
"#
    )
}

fn large_plan(frames: u64, shared_nodes: usize) -> Plan {
    let mut plan = compile(&parse(&source(24, shared_nodes)).unwrap()).unwrap();
    plan.output.score_end_q = parse_rational(&format!("{frames}/24000")).unwrap();
    plan.output.total_frames = frames;
    plan
}

#[test]
fn caller_can_explicitly_authorize_song_sized_work() {
    // Twenty shared nodes and their input edge cost 21 visits for each of
    // 24 million frames: 504 million units, without needing a long render.
    let plan = large_plan(24_000_000, 20);
    assert_eq!(plan.validate().unwrap_err().code, "E_RESOURCE_LIMIT");
    plan.validate_with_limits(&PlanLimits::song()).unwrap();
    let diagnostic = plan.validate().unwrap_err();
    assert!(diagnostic.message.contains("504000000"));
    assert!(diagnostic.message.contains("500000000"));
}

#[test]
fn song_changes_only_execution_work_and_keeps_a_finite_hard_ceiling() {
    assert_eq!(PlanLimits::MAX_EXECUTION_WORK, 500_000_000);
    assert_eq!(maac::graph::MAX_EXECUTION_WORK, 500_000_000);
    assert_eq!(PlanLimits::MAX_SONG_EXECUTION_WORK, 10_000_000_000);
    let mut song = PlanLimits::song();
    assert_eq!(song.max_execution_work, 10_000_000_000);
    song.max_execution_work = PlanLimits::MAX_EXECUTION_WORK;
    assert_eq!(song, PlanLimits::default());

    // Five instances each charge 25 shared visits per frame. Across 80 million
    // frames this lands exactly on 10 billion, within each graph's node cap.
    let mut at_limit = large_plan(80_000_000, 24);
    for index in 1..5 {
        let mut node = at_limit.nodes[0].clone();
        node.id = format!("sound{index}");
        at_limit.nodes.push(node);
    }
    at_limit.validate_with_limits(&PlanLimits::song()).unwrap();
    let mut above_limit = at_limit.clone();
    above_limit.output.score_end_q = parse_rational("80000001/24000").unwrap();
    above_limit.output.total_frames += 1;
    for limit in [
        PlanLimits::song(),
        PlanLimits {
            max_execution_work: u64::MAX,
            ..PlanLimits::song()
        },
    ] {
        assert_eq!(
            above_limit.validate_with_limits(&limit).unwrap_err().code,
            "E_RESOURCE_LIMIT"
        );
    }
    let exact = PlanLimits {
        max_execution_work: 504_000_000,
        ..PlanLimits::song()
    };
    let below = PlanLimits {
        max_execution_work: 503_999_999,
        ..exact
    };
    let plan = large_plan(24_000_000, 20);
    plan.validate_with_limits(&exact).unwrap();
    assert_eq!(
        plan.validate_with_limits(&below).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );
}

#[test]
fn parsed_and_bundle_compilation_preserve_default_guards() {
    let source = source(24_000_000, 20);
    let document = parse(&source).unwrap();
    let bundle = SourceBundle::new("main.maac", source);
    for diagnostics in [
        compile(&document).unwrap_err(),
        check(&document).unwrap_err(),
        compile_bundle(&bundle).unwrap_err(),
        check_bundle(&bundle).unwrap_err(),
    ] {
        assert_eq!(
            diagnostics.first().unwrap().code,
            DiagnosticCode::ResourceLimit
        );
    }
    let song = PlanLimits::song();
    let parsed = compile_with_limits(&document, &song).unwrap();
    let bundled = compile_bundle_with_limits(&bundle, &song).unwrap();
    assert_eq!(parsed.output, bundled.output);
    check_with_limits(&document, &song).unwrap();
    check_bundle_with_limits(&bundle, &song).unwrap();
    let tight = PlanLimits {
        max_nodes: 0,
        ..song
    };
    assert!(compile_with_limits(&document, &tight).is_err());
    assert!(check_with_limits(&document, &tight).is_err());
    assert!(compile_bundle_with_limits(&bundle, &tight).is_err());
    assert!(check_bundle_with_limits(&bundle, &tight).is_err());

    // Profiles do not make imported compositions legal or resolve naked imports.
    let imported = parse("maac 1; import basic { builtin = \"std/basic/1.0.0\"; }").unwrap();
    assert_eq!(
        compile_with_limits(&imported, &song)
            .unwrap_err()
            .first()
            .unwrap()
            .code,
        DiagnosticCode::Reference
    );
    let library = SourceBundle::new("library.maac", "maac 1; library l { version = \"1\"; }");
    check_bundle_with_limits(&library, &song).unwrap();
    assert!(compile_bundle_with_limits(&library, &song).is_err());
}

#[test]
fn explicit_json_round_trip_never_authorizes_default_or_serde_loading() {
    let plan = large_plan(24_000_000, 20);
    let song = PlanLimits::song();
    let bytes = plan.to_json_with_limits(&song).unwrap();
    let text = plan.to_json_string_with_limits(&song).unwrap();
    assert_eq!(bytes, text.as_bytes());
    assert_eq!(Plan::from_json_with_limits(&bytes, &song).unwrap(), plan);
    assert_eq!(Plan::from_json_str_with_limits(&text, &song).unwrap(), plan);
    assert_eq!(load_plan_with_limits(&bytes, &song).unwrap(), plan);
    for result in [
        plan.to_json().map(|_| ()),
        plan.to_json_string().map(|_| ()),
        Plan::from_json(&bytes).map(|_| ()),
        Plan::from_json_str(&text).map(|_| ()),
        load_plan(&bytes).map(|_| ()),
    ] {
        assert_eq!(result.unwrap_err().code, "E_RESOURCE_LIMIT");
    }
    let serde_error = serde_json::from_slice::<Plan>(&bytes).unwrap_err();
    assert!(serde_error.to_string().contains("500000000"));
    assert!(!text.contains("profile"));

    let byte_limit = PlanLimits {
        max_json_bytes: bytes.len() - 1,
        ..song
    };
    for result in [
        Plan::from_json_with_limits(&bytes, &byte_limit).map(|_| ()),
        plan.to_json_with_limits(&byte_limit).map(|_| ()),
    ] {
        assert_eq!(result.unwrap_err().code, "E_RESOURCE_LIMIT");
    }
    let enlarged_byte_limit = PlanLimits {
        max_json_bytes: usize::MAX,
        ..song
    };
    let oversized = vec![b' '; PlanLimits::MAX_JSON_BYTES + 1];
    assert_eq!(
        Plan::from_json_with_limits(&oversized, &enlarged_byte_limit)
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );
}

#[test]
fn explicit_decoding_uses_the_same_strict_wire_and_semantic_checks() {
    let song = PlanLimits::song();
    let text = large_plan(24_000_000, 20)
        .to_json_string_with_limits(&song)
        .unwrap();
    for invalid in [
        text.replacen('{', "{\"profile\":\"song\",", 1),
        text.replacen('{', "{\"version\":2,", 1),
        text.replacen("\"channels\":1", "\"channels\":1,\"extra\":0", 1),
        text.replacen("\"score_start_q\":\"0/1\"", "\"score_start_q\":0", 1),
        text.replacen("\"score_start_q\":\"0/1\"", "\"score_start_q\":\"0/0\"", 1),
    ] {
        assert_ne!(invalid, text);
        assert!(Plan::from_json_str_with_limits(&invalid, &song).is_err());
    }
    let missing = text.replacen(
        "\"output\":{\"node\":\"sound\"",
        "\"output\":{\"node\":\"absent\"",
        1,
    );
    assert_ne!(missing, text);
    assert_eq!(
        Plan::from_json_str_with_limits(&missing, &song)
            .unwrap_err()
            .code,
        "E_REFERENCE"
    );
}

#[test]
fn runtime_requires_each_callers_approval_before_the_first_frame() {
    let plan = large_plan(24_000_000, 20);
    assert!(matches!(DspEngine::new(&plan), Err(RenderError::Plan(_))));
    for error in [
        render(&plan, |_| panic!("default render callback ran")).unwrap_err(),
        dsp::render_plan(&plan, |_| panic!("default alias callback ran")).unwrap_err(),
    ] {
        assert!(matches!(error, RenderError::Plan(_)));
    }
    let song = PlanLimits::song();
    let stop = RenderError::Callback("intentional first-frame stop".into());
    let mut engine = DspEngine::new_with_limits(&plan, &song).unwrap();
    for _ in 0..2 {
        let mut frames = 0;
        assert_eq!(
            engine
                .render(|frame| {
                    assert_eq!(frame.len(), 1);
                    frames += 1;
                    Err(stop.clone())
                })
                .unwrap_err(),
            stop
        );
        assert_eq!(frames, 1);
    }
    assert_eq!(
        render_with_limits(&plan, &song, |_| Err(stop.clone())).unwrap_err(),
        stop
    );
    assert_eq!(
        dsp::render_plan_with_limits(&plan, &song, |_| Err(stop.clone())).unwrap_err(),
        stop
    );
    let tight = PlanLimits {
        max_execution_work: 1,
        ..song
    };
    assert!(matches!(
        render_with_limits(&plan, &tight, |_| panic!("rejected callback ran")),
        Err(RenderError::Plan(_))
    ));
}

#[test]
fn invalid_budgets_write_no_header_and_publish_no_file() {
    let plan = large_plan(24_000_000, 20);
    let mut sink = Cursor::new(Vec::new());
    assert!(export::write_wav(&mut sink, &plan, WavFormat::Float32).is_err());
    assert!(sink.get_ref().is_empty());
    let tight = PlanLimits {
        max_execution_work: 1,
        ..PlanLimits::song()
    };
    assert!(export::write_wav_with_limits(&mut sink, &plan, WavFormat::Float32, &tight).is_err());
    assert!(sink.get_ref().is_empty());
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("song.wav");
    assert!(export::render_wav_to_path(&plan, &destination, WavFormat::Float32, false).is_err());
    assert!(!destination.exists());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    std::fs::write(&destination, b"existing output").unwrap();
    assert!(export::render_wav_to_path_with_limits(
        &plan,
        &destination,
        WavFormat::Float32,
        true,
        &tight
    )
    .is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"existing output");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn ordinary_plan_bytes_and_nonzero_audio_are_identical_under_both_profiles() {
    let mut source = source(240, 1);
    source.push_str("pattern phrase { length = 1/100q; note n { at = 0q; dur = 1/100q; pitch = A4; velocity = 1/2; } } track t { target = &sound:events; } place p1 { pattern = &phrase; track = &t; at = 0q; }");
    let document = parse(&source).unwrap();
    let plan = compile(&document).unwrap();
    let song = PlanLimits::song();
    assert_eq!(compile_with_limits(&document, &song).unwrap(), plan);
    assert_eq!(
        plan.to_json().unwrap(),
        plan.to_json_with_limits(&song).unwrap()
    );
    let mut expected = Vec::new();
    render(&plan, |frame| {
        expected.extend_from_slice(frame);
        Ok(())
    })
    .unwrap();
    assert!(expected.iter().any(|value| *value != 0.0));
    let mut actual = Vec::new();
    render_with_limits(&plan, &song, |frame| {
        actual.extend_from_slice(frame);
        Ok(())
    })
    .unwrap();
    assert_eq!(actual, expected);
    let mut default_wav = Cursor::new(Vec::new());
    let mut song_wav = Cursor::new(Vec::new());
    export::write_wav(&mut default_wav, &plan, WavFormat::Float32).unwrap();
    export::write_wav_with_limits(&mut song_wav, &plan, WavFormat::Float32, &song).unwrap();
    assert_eq!(default_wav.get_ref(), song_wav.get_ref());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("song.wav");
    export::render_wav_to_path_with_limits(&plan, &path, WavFormat::Float32, false, &song).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), *default_wav.get_ref());
}

fn resource_error(result: Result<(), PlanError>) {
    assert_eq!(result.unwrap_err().code, "E_RESOURCE_LIMIT");
}

#[test]
fn song_preserves_custom_structural_memory_and_rate_limits() {
    let plan = large_plan(24_000_000, 20);
    let song = PlanLimits::song();
    for limits in [
        PlanLimits {
            max_nodes: 0,
            ..song
        },
        PlanLimits {
            max_graph_nodes: 1,
            ..song
        },
        PlanLimits {
            max_graph_edges: 0,
            ..song
        },
        PlanLimits {
            max_instrument_programs: 0,
            ..song
        },
        PlanLimits {
            max_instrument_voices: 0,
            ..song
        },
        PlanLimits {
            max_voice_graph_states: 1,
            ..song
        },
        PlanLimits {
            max_duration_seconds: 1,
            ..song
        },
        PlanLimits {
            max_tempo_points: 0,
            ..song
        },
        PlanLimits {
            max_objects: 0,
            ..song
        },
        PlanLimits {
            max_total_string_bytes: 0,
            ..song
        },
    ] {
        resource_error(plan.validate_with_limits(&limits));
        let bytes = plan.to_json_with_limits(&song).unwrap();
        resource_error(Plan::from_json_with_limits(&bytes, &limits).map(|_| ()));
        assert!(DspEngine::new_with_limits(&plan, &limits).is_err());
    }
    for (limits, code) in [
        (
            PlanLimits {
                max_rate_hz: 1,
                ..song
            },
            "E_CAPABILITY",
        ),
        (
            PlanLimits {
                max_channels: 0,
                ..song
            },
            "E_RESOURCE_LIMIT",
        ),
    ] {
        assert_eq!(plan.validate_with_limits(&limits).unwrap_err().code, code);
    }
    let pluck =
        compile(&parse(&source(24, 1).replace("synth.sine/1", "synth.pluck/1")).unwrap()).unwrap();
    resource_error(pluck.validate_with_limits(&PlanLimits {
        max_pluck_delay_cells: 0,
        ..song
    }));
}

#[test]
fn legacy_compilation_and_note_count_limits_are_still_enforced() {
    let legacy = parse(r#"maac 1;
project p { score = [0q, 1/100q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
node sound { type = "core.sine/1"; }
pattern phrase { length = 1/100q; note n { at = 0q; dur = 1/100q; pitch = A4; } }
track t { target = &sound:events; }
place play { pattern = &phrase; track = &t; at = 0q; }
"#).unwrap();
    let song = PlanLimits::song();
    let default = compile(&legacy).unwrap();
    let explicit = compile_with_limits(&legacy, &song).unwrap();
    assert_eq!(default.version, 1);
    assert_eq!(
        default.to_json().unwrap(),
        explicit.to_json_with_limits(&song).unwrap()
    );
    check_with_limits(&legacy, &song).unwrap();
    let tight = PlanLimits {
        max_events: 0,
        ..song
    };
    assert_eq!(
        compile_with_limits(&legacy, &tight)
            .unwrap_err()
            .first()
            .unwrap()
            .code,
        DiagnosticCode::ResourceLimit
    );
    assert_eq!(
        check_with_limits(&legacy, &tight)
            .unwrap_err()
            .first()
            .unwrap()
            .code,
        DiagnosticCode::ResourceLimit
    );
    resource_error(Plan::from_json_with_limits(&default.to_json().unwrap(), &tight).map(|_| ()));
    assert!(DspEngine::new_with_limits(&default, &tight).is_err());
}
