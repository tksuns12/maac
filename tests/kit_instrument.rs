//! Kit instruments: other instruments as pieces, played by hit key.

use maac::compiler::compile_bundle_artifact;
use maac::{render_artifact, DiagnosticCode, PlanArtifact, SourceBundle};
use serde_json::Value;

const HEAD: &str = r#"maac 1;
project p { score = [0q, 8q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &drums:out; tail = 1s; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
import basic { builtin = "std/basic/1.0.0"; }
"#;

const KIT: &str = r#"instrument boom_bap {
  channels = 2;
  piece kick { instrument = &basic.kick; key = "kick"; gate = 100ms; }
  piece snare { instrument = &basic.snare; key = "snare"; gate = 100ms; params = { brightness = 4000Hz; }; }
  piece hat { instrument = &basic.closed_hat; key = "hat"; gate = 35ms; choke = "hats"; }
  piece open { instrument = &basic.open_hat; key = "open"; gate = 220ms; choke = "hats"; }
  control kick_level { target = &kick.params.level; default = 0.25; }
}
"#;

const NODE: &str = "node drums { instrument = &boom_bap; params = { kick_level = 0.3; }; }";

/// The closed hat at 7/4q chokes the open hat struck at 3/2q.
const BEAT: &str = r#"  hit k1 { at = 0q; key = "kick"; }
  hit s1 { at = 1q; key = "snare"; velocity = 0.8; }
  hit h1 { at = 1/2q; key = "hat"; }
  hit o1 { at = 3/2q; key = "open"; }
  hit h2 { at = 7/4q; key = "hat"; }"#;

fn kit_source(kit: &str, node: &str, beat: &str, extra: &str) -> String {
    format!(
        "{HEAD}{kit}{node}\npattern beat {{ length = 4q;\n{beat}\n}}\ntrack t {{ target = &drums:events; }}\nplace main {{ pattern = &beat; track = &t; at = 0q; count = 2; }}\n{extra}"
    )
}

/// The same drums as four instrument nodes playing notes, summed in piece
/// order: 100 ms is 1/5q and 35 ms is 7/100q at 120 bpm, and the choked open
/// hat ends at 7/4q.
fn separate_source() -> String {
    let mut text = HEAD.to_owned();
    text.push_str(
        r#"node hat { instrument = &basic.closed_hat; config = { voices = 8; }; }
node kick { instrument = &basic.kick; config = { voices = 8; }; params = { level = 0.3; }; }
node open { instrument = &basic.open_hat; config = { voices = 8; }; }
node snare { instrument = &basic.snare; config = { voices = 8; }; params = { brightness = 4000Hz; }; }
node drums { type = "core.sum/1"; config = { channels = 2; }; }
connect c1 { from = &hat:out; to = &drums:in; }
connect c2 { from = &kick:out; to = &drums:in; }
connect c3 { from = &open:out; to = &drums:in; }
connect c4 { from = &snare:out; to = &drums:in; }
pattern hats { length = 4q; note h1 { at = 1/2q; dur = 7/100q; pitch = C4; } note h2 { at = 7/4q; dur = 7/100q; pitch = C4; } }
pattern kicks { length = 4q; note k1 { at = 0q; dur = 1/5q; pitch = C4; } }
pattern opens { length = 4q; note o1 { at = 3/2q; dur = 1/4q; pitch = C4; } }
pattern snares { length = 4q; note s1 { at = 1q; dur = 1/5q; pitch = C4; velocity = 0.8; } }
track th { target = &hat:events; }
track tk { target = &kick:events; }
track to { target = &open:events; }
track ts { target = &snare:events; }
place ph { pattern = &hats; track = &th; at = 0q; count = 2; }
place pk { pattern = &kicks; track = &tk; at = 0q; count = 2; }
place po { pattern = &opens; track = &to; at = 0q; count = 2; }
place ps { pattern = &snares; track = &ts; at = 0q; count = 2; }
"#,
    );
    text
}

fn plan(source: &str) -> PlanArtifact {
    compile_bundle_artifact(&SourceBundle::new("main.maac", source))
        .unwrap_or_else(|e| panic!("compiles: {e}"))
}

fn samples(plan: &PlanArtifact) -> Vec<f64> {
    let mut out = Vec::new();
    render_artifact(plan, |frame| {
        out.extend_from_slice(frame);
        Ok(())
    })
    .unwrap();
    out
}

fn codes(source: &str) -> Vec<DiagnosticCode> {
    compile_bundle_artifact(&SourceBundle::new("main.maac", source))
        .unwrap_err()
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn a_kit_sounds_exactly_like_its_pieces_played_as_notes() {
    let kit = samples(&plan(&kit_source(KIT, NODE, BEAT, "")));
    let separate = samples(&plan(&separate_source()));
    assert_eq!(kit.len(), separate.len());
    assert!(kit.iter().any(|sample| *sample != 0.0));
    assert_eq!(kit, separate);
}

#[test]
fn a_choke_group_releases_the_other_pieces_gates() {
    let choked = samples(&plan(&kit_source(KIT, NODE, BEAT, "")));
    let open = samples(&plan(&kit_source(
        &KIT.replace(r#" choke = "hats";"#, ""),
        NODE,
        BEAT,
        "",
    )));
    // 7/4q is 0.875 s: everything before the choke is identical.
    let choke = 42_000 * 2;
    assert_eq!(choked[..choke], open[..choke]);
    assert_ne!(choked[choke..], open[choke..]);
}

#[test]
fn the_plan_carries_the_kit_and_renders_the_same_after_a_round_trip() {
    let compiled = plan(&kit_source(KIT, NODE, BEAT, ""));
    let bytes = compiled.to_json().unwrap();
    let wire: Value = serde_json::from_slice(&bytes).unwrap();
    let node = wire["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "drums")
        .unwrap();
    // Artifact plans wrap core processors.
    let processor = &node["processor"]["processor"];
    assert_eq!(processor["kind"], "kit_instrument");
    let kit = &wire["instruments"]["kits"][0];
    assert_eq!(kit["id"], processor["kit"]);
    assert_eq!(kit["pieces"].as_array().unwrap().len(), 4);
    assert_eq!(kit["pieces"][0]["id"], "hat");
    assert_eq!(kit["pieces"][0]["choke"], "hats");
    assert_eq!(kit["pieces"][0]["gate_seconds"], "7/200");
    let reloaded = PlanArtifact::from_json(&bytes).unwrap();
    assert_eq!(samples(&reloaded), samples(&compiled));
}

#[test]
fn kit_controls_take_presets_params_and_automation() {
    let quiet = samples(&plan(&kit_source(
        KIT,
        "node drums { instrument = &boom_bap; preset = &soft; }",
        BEAT,
        "preset soft { instrument = &boom_bap; params = { kick_level = 0.05; }; }",
    )));
    let loud = samples(&plan(&kit_source(KIT, NODE, BEAT, "")));
    assert_ne!(quiet, loud);
    let automated = samples(&plan(&kit_source(
        KIT,
        NODE,
        BEAT,
        "curve swell { clock = score; points = [(0q, 0.1, linear), (8q, 0.4, step)]; }\nautomation fade { target = &drums.params.kick_level; curve = &swell; at = 0q; }",
    )));
    assert_ne!(automated, loud);
}

#[test]
fn hits_need_a_known_key_and_notes_are_refused() {
    assert_eq!(
        codes(&kit_source(
            KIT,
            NODE,
            r#"  hit x { at = 0q; key = "cowbell"; }"#,
            ""
        )),
        vec![DiagnosticCode::Reference]
    );
    assert_eq!(
        codes(&kit_source(
            KIT,
            NODE,
            "  note n { at = 0q; dur = 1q; pitch = C4; }",
            ""
        )),
        vec![DiagnosticCode::Capability]
    );
}

#[test]
fn malformed_kits_are_rejected() {
    let check = |kit: String, code: DiagnosticCode| {
        let codes = codes(&kit_source(&kit, NODE, BEAT, ""));
        assert!(
            codes.contains(&code),
            "{code:?} not in {codes:?} for\n{kit}"
        );
    };
    let kick = r#"piece kick { instrument = &basic.kick; key = "kick"; gate = 100ms; }"#;
    // A piece needs a gate.
    check(
        KIT.replace(
            kick,
            r#"piece kick { instrument = &basic.kick; key = "kick"; }"#,
        ),
        DiagnosticCode::UnknownField,
    );
    // Keys are unique.
    check(
        KIT.replace(r#"key = "snare""#, r#"key = "kick""#),
        DiagnosticCode::DuplicateId,
    );
    // A gate is a physical duration.
    check(
        KIT.replace("gate = 100ms; }", "gate = 1q; }"),
        DiagnosticCode::Unit,
    );
    // A pitch is a spelled pitch, key(), or a frequency.
    check(
        KIT.replace(r#"key = "kick";"#, r#"key = "kick"; pitch = "loud";"#),
        DiagnosticCode::Unit,
    );
    // Graphs and pieces do not mix.
    check(
        KIT.replace(
            "  control kick_level",
            "  voice v { channels = 1; amplitude = &a; output = &a:out; node a { type = \"synth.adsr/1\"; } }\n  control kick_level",
        ),
        DiagnosticCode::UnknownKind,
    );
    // A control exposes `&piece.params.control` of an existing piece.
    check(
        KIT.replace("&kick.params.level", "&kick.level"),
        DiagnosticCode::Reference,
    );
    check(
        KIT.replace("&kick.params.level", "&tom.params.level"),
        DiagnosticCode::Reference,
    );
    check(
        KIT.replace("&kick.params.level", "&kick.params.cutoff"),
        DiagnosticCode::Reference,
    );
    // A control cannot expose a setting the piece fixes.
    check(
        KIT.replace(
            "target = &kick.params.level; default = 0.25;",
            "target = &snare.params.brightness; default = 4000Hz;",
        ),
        DiagnosticCode::Conflict,
    );
    // A piece plays an instrument, not another kit.
    check(
        format!("{KIT}instrument nested {{ channels = 2; piece inner {{ instrument = &boom_bap; key = \"x\"; gate = 1s; }} }}\n"),
        DiagnosticCode::Reference,
    );
}

#[test]
fn a_kit_instance_takes_no_config() {
    let codes = codes(&kit_source(
        KIT,
        "node drums { instrument = &boom_bap; config = { voices = 8; }; }",
        BEAT,
        "",
    ));
    assert_eq!(codes, vec![DiagnosticCode::UnknownField]);
}

use maac::production_data::SCHEMA_BYTES;

/// A production project whose execution identity covers the kit declaration.
fn production(kit: &str) -> SourceBundle {
    let source = kit_source(kit, NODE, BEAT, "").replace(
        "tail = 1s; }",
        r#"tail = 1s; requires = ["maac.production/1"]; }"#,
    ) + &format!(
        r#"asset schema {{ kind = descriptor; path = "production.schema.json"; hash = "{}"; }}
extension deliveries {{ namespace = "maac.production/1"; schema = &schema; render_affecting = true; data = {{ deliveries = {{ release = {{ rate = 48000Hz; resampler = "maac.src.kaiser/1"; targets = {{
  master = {{ role = master; output = &drums:out; encoding = wav_f32le; dither = {{ type = none; }}; }};
}}; }}; }}; }}; }}
"#,
        maac::bundle::sha256_digest(SCHEMA_BYTES)
    );
    let mut bundle = SourceBundle::new("main.maac", source);
    bundle
        .assets
        .insert("production.schema.json".into(), SCHEMA_BYTES.to_vec());
    bundle
}

fn execution_hash(bundle: &SourceBundle) -> Value {
    let plan = compile_bundle_artifact(bundle).unwrap_or_else(|e| panic!("compiles: {e}"));
    let value: Value = serde_json::from_slice(&plan.to_json().unwrap()).unwrap();
    value["production"]["execution_identity"]["execution_hash"].clone()
}

#[test]
fn piece_defaults_normalize_into_the_execution_identity() {
    let base = execution_hash(&production(KIT));
    assert!(base.is_string());
    let explicit = KIT.replace(
        r#"key = "kick"; gate = 100ms;"#,
        r#"key = "kick"; gate = 0.1s; pitch = key(60); voices = 8;"#,
    );
    assert_eq!(base, execution_hash(&production(&explicit)));
    let spelled = KIT.replace(r#"key = "kick";"#, r#"key = "kick"; pitch = C4;"#);
    assert_eq!(base, execution_hash(&production(&spelled)));
    let longer = KIT.replace(
        r#"key = "kick"; gate = 100ms;"#,
        r#"key = "kick"; gate = 120ms;"#,
    );
    assert_ne!(base, execution_hash(&production(&longer)));
}
