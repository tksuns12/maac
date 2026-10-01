//! §9–§11 `audio` leaves inside patterns and placement inserts.

use maac::bundle::{sha256_digest, SourceBundle};
use maac::compiler::compile_bundle_artifact;
use maac::{DiagnosticCode, PlanArtifact};
use serde_json::Value;

const FRAMES: usize = 4_800;

/// A 0.1 s ramp whose samples are all distinct, so misplaced frames show.
fn ramp_bytes(scale: f32) -> Vec<u8> {
    (0..FRAMES)
        .flat_map(|j| (scale * (j as f32 + 1.) / FRAMES as f32).to_le_bytes())
        .collect()
}

/// One second per quarter note at 48 kHz, over an 8q score.
fn bundle(body: &str) -> SourceBundle {
    let mono = ramp_bytes(0.5);
    let stereo: Vec<u8> = mono.iter().copied().chain(mono.iter().copied()).collect();
    let source = format!(
        r#"maac 1;
project p {{ score = [0q, 8q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &out:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
asset tone {{ kind = audio; path = "tone.pcm"; hash = "{}"; format = "pcm_f32le_interleaved/1"; rate = 48000Hz; channels = 1; frames = {FRAMES}; }}
asset wide {{ kind = audio; path = "wide.pcm"; hash = "{}"; format = "pcm_f32le_interleaved/1"; rate = 48000Hz; channels = 2; frames = {FRAMES}; }}
track clips {{}}
node out {{ type = "core.sum/1"; config = {{ channels = 1; }}; }}
{body}
"#,
        sha256_digest(&mono),
        sha256_digest(&stereo),
    );
    let mut bundle = SourceBundle::new("score.maac", source);
    bundle.assets.insert("tone.pcm".into(), mono);
    bundle.assets.insert("wide.pcm".into(), stereo);
    bundle
}

fn compiled(body: &str) -> PlanArtifact {
    compile_bundle_artifact(&bundle(body)).unwrap_or_else(|error| panic!("{error:?}"))
}

fn render(body: &str) -> Vec<f64> {
    let mut output = Vec::new();
    maac::render_artifact(&compiled(body), |frame| {
        output.extend_from_slice(frame);
        Ok(())
    })
    .unwrap();
    output
}

fn codes(body: &str) -> Vec<DiagnosticCode> {
    compile_bundle_artifact(&bundle(body))
        .expect_err("source must be refused")
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

/// Top-level clips at explicit global positions, all summed into `out`.
fn top_level(clips: &[(&str, &str)]) -> String {
    clips
        .iter()
        .enumerate()
        .map(|(n, (at, extra))| {
            format!(
                "audio c{n} {{ asset = &tone; at = {at}; source = [0frame, {FRAMES}frame]; {extra} }}\n\
                 connect r{n} {{ from = &c{n}:out; to = &out:in; }}\n"
            )
        })
        .collect()
}

const RATE: &str = "mode = rate;";

fn assert_same(a: &[f64], b: &[f64]) {
    assert_eq!(a.len(), b.len());
    for (n, (x, y)) in a.iter().zip(b).enumerate() {
        assert!((x - y).abs() < 1e-12, "frame {n}: {x} vs {y}");
    }
}

#[test]
fn repeated_placement_matches_top_level_clips() {
    let placed = render(
        "pattern loop { length = 2q; audio hit { asset = &tone; at = 1/2q; source = [0frame, 4800frame]; mode = rate; gain = 1/2; } }\n\
         place loops { pattern = &loop; track = &clips; at = 1q; count = 3; }\n\
         connect route { from = &loops:out; to = &out:in; }",
    );
    let expected = render(&top_level(&[
        ("3/2q", "mode = rate; gain = 1/2;"),
        ("7/2q", "mode = rate; gain = 1/2;"),
        ("11/2q", "mode = rate; gain = 1/2;"),
    ]));
    assert_same(&placed, &expected);
    assert!(placed.iter().any(|sample| *sample != 0.));
}

#[test]
fn each_placement_is_one_clip_set_node() {
    let artifact = compiled(
        "pattern loop { length = 2q; audio hit { asset = &tone; at = 0q; source = [0frame, 4800frame]; mode = rate; } }\n\
         place loops { pattern = &loop; track = &clips; at = 0q; count = 2; }\n\
         connect route { from = &loops:out; to = &out:in; }",
    );
    let wire: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    assert_eq!(wire["version"], 5);
    let nodes = wire["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2, "only `loops` and `out`: {nodes:?}");
    let set = &nodes.iter().find(|node| node["id"] == "loops").unwrap()["processor"];
    assert_eq!(set["kind"], "clips");
    assert_eq!(set["channels"], 1);
    let members = set["clips"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    for (member, at) in members.iter().zip(["0/1", "2/1"]) {
        assert_eq!(member["kind"], "audio");
        let clip = &member["clip"];
        assert_eq!(clip["at"]["q"], at);
        assert_eq!(clip["track"], "clips");
        assert_eq!(clip["source"]["object"], "hit");
        assert_eq!(clip["source"]["path"], serde_json::json!(["loop", "hit"]));
    }
    assert_eq!(wire["connections"].as_array().unwrap().len(), 1);
    assert_eq!(artifact.audio_clip_count(), 2);
    // The retained plan reloads and renders without its source.
    PlanArtifact::from_json(&artifact.to_json().unwrap()).unwrap();
}

#[test]
fn occurrences_are_not_bounded_by_the_plan_node_limit() {
    // 1,024 occurrences, four times the 256-node plan limit, in one node.
    let body = "pattern tick { length = 1/128q; audio a { asset = &tone; at = 0q; source = [0frame, 480frame]; mode = rate; gain = 1/4; } }\n\
                pattern bar { length = 8q; use ticks { pattern = &tick; at = 0q; count = 1024; } }\n\
                place p1 { pattern = &bar; track = &clips; at = 0q; }\n\
                connect route { from = &p1:out; to = &out:in; }";
    let artifact = compiled(body);
    assert_eq!(artifact.audio_clip_count(), 1024);
    let samples = render(body);
    // Onsets every 7.8125 ms overlap each 10 ms slice, so the sum at a frame
    // where two members sound is twice one member's sample.
    assert!(samples.iter().any(|sample| *sample != 0.));
    let one = 0.5 * 1. / FRAMES as f64 * 0.25;
    assert!((samples[0] - one).abs() < 1e-9, "{}", samples[0]);
}

#[test]
fn clip_sets_render_bit_identically_across_bucket_boundaries() {
    // Members start just before, on and after the 4,096-frame bucket edges.
    let leaves: String = [4095, 4096, 4097, 8191, 12_288]
        .iter()
        .enumerate()
        .map(|(n, frame)| {
            format!(
                "audio a{n} {{ asset = &tone; at = {frame}/48000q; source = [0frame, 4800frame]; mode = rate; }}\n"
            )
        })
        .collect();
    let placed = render(&format!(
        "pattern cell {{ length = 1q; {leaves} }}\n\
         place p1 {{ pattern = &cell; track = &clips; at = 0q; }}\n\
         connect route {{ from = &p1:out; to = &out:in; }}"
    ));
    let mut expected_body = String::from("node mix { type = \"core.sum/1\"; config = { channels = 1; }; }\nconnect mixed { from = &mix:out; to = &out:in; }\n");
    for (n, frame) in [4095, 4096, 4097, 8191, 12_288].iter().enumerate() {
        expected_body.push_str(&format!(
            "audio c{n} {{ asset = &tone; at = {frame}/48000q; source = [0frame, 4800frame]; mode = rate; }}\nconnect r{n} {{ from = &c{n}:out; to = &mix:in; }}\n"
        ));
    }
    let expected = render(&expected_body);
    assert_eq!(placed.len(), expected.len());
    for (n, (x, y)) in placed.iter().zip(&expected).enumerate() {
        assert_eq!(x.to_bits(), y.to_bits(), "frame {n}: {x} vs {y}");
    }
}

#[test]
fn a_placement_with_every_occurrence_deleted_is_silent() {
    let samples = render(
        "pattern cell { length = 1q; audio a { asset = &tone; at = 0q; source = [0frame, 4800frame]; mode = rate; } }\n\
         place p1 { pattern = &cell; track = &clips; at = 0q; override gone { event = \"0/a\"; delete = true; } }\n\
         connect route { from = &p1:out; to = &out:in; }",
    );
    assert!(samples.iter().all(|sample| *sample == 0.));
}

#[test]
fn retained_clip_sets_are_validated() {
    let artifact = compiled(
        "pattern cell { length = 1q; audio a { asset = &tone; at = 0q; source = [0frame, 4800frame]; mode = rate; } }\n\
         place p1 { pattern = &cell; track = &clips; at = 0q; count = 2; }\n\
         connect route { from = &p1:out; to = &out:in; }",
    );
    let wire: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    let index = wire["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .position(|node| node["id"] == "p1")
        .unwrap();
    let load = |edit: &dyn Fn(&mut Value)| {
        let mut wire = wire.clone();
        edit(&mut wire["nodes"][index]);
        PlanArtifact::from_json(&serde_json::to_vec(&wire).unwrap())
            .unwrap_err()
            .code
    };
    assert_eq!(
        load(&|node| node["processor"]["channels"] = 2.into()),
        "E_PORT_TYPE"
    );
    assert_eq!(
        load(&|node| node["params"] = serde_json::json!({"level": "1/1"})),
        "E_UNKNOWN_FIELD"
    );
    assert_eq!(
        load(&|node| node["processor"]["clips"][0]["clip"]["start_frame"] = 7.into()),
        "E_INTERVAL"
    );
    // A version 5 plan cannot carry warp members.
    let warp = load(&|node| {
        let member = &mut node["processor"]["clips"][0];
        member["kind"] = "warp_rate".into();
        let clip = member["clip"].as_object_mut().unwrap();
        clip.remove("at");
        clip.remove("speed");
        clip.remove("reverse");
        clip.insert("at_q".into(), "0/1".into());
        clip.insert(
            "warp".into(),
            serde_json::json!([{"q": "0/1", "source_frame": 0}, {"q": "1/10", "source_frame": 4800}]),
        );
    });
    assert_eq!(warp, "E_VERSION");
}

#[test]
fn nested_stretch_scales_warp_anchors_and_moves_onsets() {
    let placed = render(
        "pattern cell { length = 1q; audio w { asset = &tone; at = 1/4q; source = [0frame, 4800frame]; mode = warp_rate; warp = [(0q, 0frame), (1/8q, 4800frame)]; } }\n\
         pattern outer { length = 4q; use twice { pattern = &cell; at = 0q; count = 2; stretch = 2; } }\n\
         place p1 { pattern = &outer; track = &clips; at = 0q; }\n\
         connect route { from = &p1:out; to = &out:in; }",
    );
    let warp = "mode = warp_rate; warp = [(0q, 0frame), (1/4q, 4800frame)];";
    let expected = render(&top_level(&[("1/2q", warp), ("5/2q", warp)]));
    assert_same(&placed, &expected);
}

#[test]
fn warp_preserve_leaves_use_the_core_stretch() {
    let placed = render(
        "pattern cell { length = 1q; audio w { asset = &tone; at = 0q; source = [0frame, 4800frame]; mode = warp_preserve; processor = \"core.stretch.ola/1\"; warp = [(0q, 0frame), (1/5q, 4800frame)]; } }\n\
         place p1 { pattern = &cell; track = &clips; at = 1q; stretch = 2; }\n\
         connect route { from = &p1:out; to = &out:in; }",
    );
    let expected = render(&top_level(&[(
        "1q",
        "mode = warp_preserve; processor = \"core.stretch.ola/1\"; warp = [(0q, 0frame), (2/5q, 4800frame)];",
    )]));
    assert_same(&placed, &expected);
}

#[test]
fn transposition_and_cut_do_not_apply_to_audio() {
    let leaf = "pattern cell { length = 1q; audio a { asset = &tone; at = 3/4q; source = [0frame, 4800frame]; mode = rate; } }";
    let plain = render(&format!(
        "{leaf}\nplace p1 {{ pattern = &cell; track = &clips; at = 0q; count = 2; }}\nconnect route {{ from = &p1:out; to = &out:in; }}"
    ));
    let transformed = render(&format!(
        "{leaf}\nplace p1 {{ pattern = &cell; track = &clips; at = 0q; count = 2; transpose = 1200ct; boundary = cut; }}\nconnect route {{ from = &p1:out; to = &out:in; }}"
    ));
    assert_same(&plain, &transformed);
}

#[test]
fn overrides_and_inserts_edit_final_occurrences() {
    let placed = render(
        "pattern loop { length = 2q; audio a { asset = &tone; at = 0q; source = [0frame, 4800frame]; mode = rate; } }\n\
         place loops {\n\
           pattern = &loop; track = &clips; at = 0q; count = 3;\n\
           override gone { event = \"1/a\"; delete = true; }\n\
           override soft { event = \"2/a\"; set = { at = 9/2q; gain = 1/4; fade_out = 10ms; fade_shape = equal_power; }; }\n\
           insert extra { audio b { asset = &tone; at = 1q; source = [0frame, 2400frame]; mode = rate; speed = 2; } }\n\
         }\n\
         connect route { from = &loops:out; to = &out:in; }",
    );
    let expected = render(&format!(
        "{}audio x {{ asset = &tone; at = 1q; source = [0frame, 2400frame]; mode = rate; speed = 2; }}\nconnect rx {{ from = &x:out; to = &out:in; }}",
        top_level(&[
            ("0q", RATE),
            (
                "9/2q",
                "mode = rate; gain = 1/4; fade_out = 10ms; fade_shape = equal_power;"
            ),
        ])
    ));
    assert_same(&placed, &expected);
}

#[test]
fn placements_mix_events_and_audio() {
    // A targeted track receives the notes; the placement output carries audio.
    let artifact = compiled(
        "node synth { type = \"core.sine/1\"; }\n\
         track lead { target = &synth:events; }\n\
         pattern both { length = 1q; note n { at = 0q; dur = 1/2q; pitch = A4; } audio a { asset = &tone; at = 0q; source = [0frame, 4800frame]; mode = rate; } }\n\
         place p1 { pattern = &both; track = &lead; at = 0q; count = 2; }\n\
         connect notes { from = &synth:out; to = &out:in; }\n\
         connect route { from = &p1:out; to = &out:in; }",
    );
    let wire: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    let addresses: Vec<&str> = wire["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["address"].as_str().unwrap())
        .collect();
    assert_eq!(addresses, ["p1/0/n", "p1/1/n"]);
    assert_eq!(wire["nodes"].as_array().unwrap().len(), 3);
}

#[test]
fn stereo_leaves_give_a_stereo_output() {
    let artifact = compile_bundle_artifact(&{
        let mut bundle = bundle(
            "pattern cell { length = 1q; audio a { asset = &wide; at = 0q; source = [0frame, 4800frame]; mode = rate; } }\n\
             place p1 { pattern = &cell; track = &clips; at = 0q; }",
        );
        let text = bundle.sources.get_mut("score.maac").unwrap();
        *text = text.replace("output = &out:out;", "output = &p1:out;");
        bundle
    })
    .unwrap();
    let wire: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    assert_eq!(wire["output"]["channels"], 2);
}

#[test]
fn invalid_pattern_audio_is_refused() {
    let cell = |leaf: &str| {
        format!(
            "pattern cell {{ length = 1q; {leaf} }}\nplace p1 {{ pattern = &cell; track = &clips; at = 0q; }}\nconnect route {{ from = &p1:out; to = &out:in; }}"
        )
    };
    let leaf = |extra: &str| {
        cell(&format!(
            "audio a {{ asset = &tone; source = [0frame, 4800frame]; mode = rate; {extra} }}"
        ))
    };
    for (body, code) in [
        (leaf("at = 0q; track = &clips;"), DiagnosticCode::UnknownField),
        (leaf("at = 1s;"), DiagnosticCode::Unit),
        (leaf("at = bar(1, 0);"), DiagnosticCode::Unit),
        (leaf("at = 1q;"), DiagnosticCode::Interval),
        (leaf("at = 0q; warp = [(0q, 0frame), (1q, 4800frame)];"), DiagnosticCode::Range),
        (
            cell("audio a { asset = &tone; at = 0q; source = [0frame, 4801frame]; mode = rate; }"),
            DiagnosticCode::Range,
        ),
        (
            cell("audio a { asset = &tone; at = 0q; source = [0frame, 4800frame]; mode = warp_preserve; warp = [(0q, 0frame), (1q, 4800frame)]; }"),
            DiagnosticCode::Range,
        ),
        (
            cell("audio a { asset = &tone; at = 0q; source = [0frame, 4800frame]; mode = rate; } audio b { asset = &wide; at = 0q; source = [0frame, 4800frame]; mode = rate; }"),
            DiagnosticCode::PortType,
        ),
        // Notes still need an event target.
        (
            cell("audio a { asset = &tone; at = 0q; source = [0frame, 4800frame]; mode = rate; } note n { at = 0q; dur = 1q; pitch = A4; }"),
            DiagnosticCode::Reference,
        ),
    ] {
        let found = codes(&body);
        assert!(found.contains(&code), "{body}\n{found:?}");
    }
}

#[test]
fn placement_output_and_overrides_are_checked() {
    // A placement without audio leaves has no `out` port.
    let found = codes(
        "pattern empty { length = 1q; }\nplace p1 { pattern = &empty; track = &clips; at = 0q; }\nconnect route { from = &p1:out; to = &out:in; }",
    );
    assert!(found.contains(&DiagnosticCode::Reference), "{found:?}");

    let with = |override_set: &str| {
        format!(
            "pattern cell {{ length = 1q; audio a {{ asset = &tone; at = 0q; source = [0frame, 4800frame]; mode = rate; }} }}\n\
             place p1 {{ pattern = &cell; track = &clips; at = 0q; override o {{ event = \"0/a\"; set = {{ {override_set} }}; }} }}\n\
             connect route {{ from = &p1:out; to = &out:in; }}"
        )
    };
    for (set, code) in [
        ("speed = 2;", DiagnosticCode::UnknownField),
        ("velocity = 1/2;", DiagnosticCode::UnknownField),
        ("gain = -1;", DiagnosticCode::Range),
        ("fade_shape = cubic;", DiagnosticCode::Range),
    ] {
        let found = codes(&with(set));
        assert!(found.contains(&code), "{set}: {found:?}");
    }
}

#[test]
fn legacy_plans_refuse_pattern_audio() {
    let document = maac::parse(&bundle(
        "pattern cell { length = 1q; audio a { asset = &tone; at = 0q; source = [0frame, 4800frame]; mode = rate; } }\nplace p1 { pattern = &cell; track = &clips; at = 0q; }\nconnect route { from = &p1:out; to = &out:in; }",
    )
    .sources["score.maac"])
    .unwrap();
    let found: Vec<DiagnosticCode> = maac::compiler::compile(&document)
        .expect_err("version 2 plans have no audio")
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert!(found.contains(&DiagnosticCode::Capability), "{found:?}");
}

/// The artifact's execution identity, and the editing N(A) of the same source.
fn identity(body: &str) -> (String, Value, Value) {
    let mut bundle = bundle(body);
    let text = bundle.sources.get_mut("score.maac").unwrap();
    *text = text.replace(
        "output = &out:out; }",
        "output = &out:out; requires = [\"maac.production/1\"]; }",
    );
    // The editing normalizer excludes production extensions by design.
    let plain = text.clone();
    let schema = maac::production_data::SCHEMA_BYTES;
    text.push_str(&format!(
        "asset schema {{ kind = descriptor; path = \"production.schema.json\"; hash = \"{}\"; }}\n\
         extension deliveries {{ namespace = \"maac.production/1\"; schema = &schema; render_affecting = true; data = {{ deliveries = {{ release = {{ rate = 48000Hz; resampler = \"maac.src.kaiser/1\"; targets = {{ master = {{ role = master; output = &out:out; encoding = wav_f32le; dither = {{ type = none; }}; }}; }}; }}; }}; }}; }}\n",
        sha256_digest(schema)
    ));
    bundle
        .assets
        .insert("production.schema.json".into(), schema.to_vec());
    let artifact = compile_bundle_artifact(&bundle).unwrap_or_else(|e| panic!("{e:?}"));
    let wire: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    let identity = &wire["production"]["execution_identity"];
    let execution: Value =
        serde_json::from_str(identity["normalized_source_json"].as_str().unwrap()).unwrap();
    let authored =
        maac::editing::AuthoredDocument::from_document(&maac::parse(&plain).unwrap()).unwrap();
    let editing = maac::editing::FoundationEditContext
        .normalize_document(&authored)
        .unwrap();
    (
        identity["execution_hash"].as_str().unwrap().to_owned(),
        execution,
        editing,
    )
}

#[test]
fn leaf_defaults_do_not_change_execution_identity() {
    let body = |leaf: &str| {
        format!(
            "pattern cell {{ length = 1q; audio a {{ asset = &tone; at = 0q; source = [0frame, 4800frame]; {leaf} }} }}\n\
             place p1 {{ pattern = &cell; track = &clips; at = 0q; }}\n\
             connect route {{ from = &p1:out; to = &out:in; }}"
        )
    };
    let (omitted, execution, editing) = identity(&body("mode = rate;"));
    let (explicit, _, _) = identity(&body(
        "mode = rate; speed = 1; reverse = false; gain = 1; fade_in = 0s; fade_out = 0ms; fade_shape = linear;",
    ));
    let (louder, _, _) = identity(&body("mode = rate; gain = 1/2;"));
    assert_eq!(omitted, explicit);
    assert_ne!(omitted, louder);
    // The leaf's defaults are expanded identically in the editing view.
    let leaf = |view: &Value| view["objects"]["cell"]["children"]["a"]["fields"].clone();
    assert_eq!(leaf(&execution), leaf(&editing));
    assert_eq!(leaf(&execution)["fade_shape"]["v"], "linear");
}

#[test]
fn materialized_placements_keep_their_audio() {
    let body = "pattern cell { length = 1q; audio a { asset = &tone; at = 1/2q; source = [0frame, 4800frame]; mode = warp_rate; warp = [(0q, 0frame), (1/8q, 4800frame)]; } }\n\
         pattern outer { length = 2q; use twice { pattern = &cell; at = 0q; count = 2; } }\n\
         place play {\n\
           pattern = &outer; track = &clips; at = 0q; count = 2; stretch = 3/2;\n\
           override soft { event = \"1/twice/0/a\"; set = { gain = 1/4; }; }\n\
           insert extra { audio b { asset = &tone; at = 7q; source = [0frame, 2400frame]; mode = rate; } }\n\
         }\n\
         connect route { from = &play:out; to = &out:in; }";
    let mut bundle = bundle(body);
    let rendered = |bundle: &SourceBundle| {
        let mut output = Vec::new();
        maac::render_artifact(&compile_bundle_artifact(bundle).unwrap(), |frame| {
            output.extend_from_slice(frame);
            Ok(())
        })
        .unwrap();
        output
    };
    let before = rendered(&bundle);
    let text = bundle.sources[&bundle.entry].clone();
    let context = maac::editing::BundleEditContext::new(&bundle).unwrap();
    let mut document = maac::editing::SourceDocument::parse(&text).unwrap();
    let plan = context
        .prepare_materialize_instance(document.authored(), "play", "copy")
        .unwrap();
    // Four repeated audio occurrences and one inserted leaf.
    assert_eq!(plan.mappings.len(), 5);
    document.apply(&plan.transaction, &context).unwrap();
    assert!(!document.source().contains("pattern = &outer; track"));
    bundle
        .sources
        .insert(bundle.entry.clone(), document.source().into());
    assert_same(&rendered(&bundle), &before);
}

#[test]
fn audio_leaf_edits_report_their_occurrence_addresses() {
    let bundle = bundle(
        "pattern cell { length = 1q; audio a { asset = &tone; at = 0q; source = [0frame, 4800frame]; mode = rate; } }\n\
         place p1 { pattern = &cell; track = &clips; at = 0q; count = 2; }\n\
         connect route { from = &p1:out; to = &out:in; }",
    );
    let context = maac::editing::BundleEditContext::new(&bundle).unwrap();
    let mut document =
        maac::editing::SourceDocument::parse(&bundle.sources[&bundle.entry]).unwrap();
    let transaction = maac::editing::Transaction::new(
        document.revision().to_owned(),
        vec![maac::editing::Operation::Set {
            object: vec!["cell".into(), "a".into()],
            field: vec!["gain".into()],
            value: serde_json::json!({"t":"number","n":"1","d":"2"}),
            expect: None,
            expect_absent: false,
        }],
    )
    .unwrap();
    let applied = document.apply(&transaction, &context).unwrap();
    assert_eq!(
        applied.impact.affected_expanded_event_addresses,
        ["p1/0/a", "p1/1/a"]
    );
    assert!(!applied.impact.full_render_invalidated);
}

#[test]
fn library_patterns_cannot_reach_composition_assets() {
    // Library declarations are context-free, so an exported pattern cannot
    // name the composition's assets, and a library declares no assets.
    let library = "maac 1;\nlibrary loops { version = \"1\"; }\n\
        pattern cell { length = 1q; audio a { asset = &tone; at = 0q; source = [0frame, 4800frame]; mode = rate; } }\n";
    let mut bundle = bundle(&format!(
        "import lib {{ path = \"loops.maac\"; hash = \"{}\"; }}\n\
         place p1 {{ pattern = &lib.cell; track = &clips; at = 0q; }}",
        sha256_digest(library.as_bytes())
    ));
    bundle.sources.insert("loops.maac".into(), library.into());
    let found: Vec<DiagnosticCode> = compile_bundle_artifact(&bundle)
        .expect_err("library audio leaves are refused")
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert!(found.contains(&DiagnosticCode::Reference), "{found:?}");
}

#[test]
fn checked_in_example_builds_its_occurrences() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let bundle =
        maac::bundle_fs::load_bundle(std::path::Path::new("examples/pattern-audio.maac"), root)
            .unwrap();
    let artifact = compile_bundle_artifact(&bundle).unwrap();
    let wire: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    let groove = wire["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "groove")
        .unwrap();
    // Four bars of twelve leaves, one deleted, one inserted.
    assert_eq!(groove["processor"]["clips"].as_array().unwrap().len(), 48);
    assert_eq!(artifact.audio_clip_count(), 48);
    assert_eq!(wire["events"].as_array().unwrap().len(), 0);
}
