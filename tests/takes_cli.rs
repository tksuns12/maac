use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use maac::editing::{Operation, SourceDocument, Transaction};
use serde_json::{json, Value};

fn invoke(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(args)
        .output()
        .unwrap()
}

fn success(output: Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn failure(output: Output) -> Value {
    assert!(!output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn source(a_hash: &str, b_hash: &str, schema_hash: &str) -> String {
    format!(
        r#"maac 1;
// Preserve the complete take choice in authored source.
project song {{ score=[0q,6/48000q]; tail=0s; rate=48000Hz; tempo=&clock; meter=&metre; output=&mix:out; requires=["maac.takes/1"]; }}
tempo clock {{ points=[(0q,60bpm,step)]; }}
meter metre {{ points=[(0q,4,4)]; }}
asset a {{ kind=audio; path="a.pcm"; hash="{a_hash}"; format="pcm_f32le_interleaved/1"; rate=48000Hz; channels=1; frames=8; }}
asset b {{ kind=audio; path="b.pcm"; hash="{b_hash}"; format="pcm_f32le_interleaved/1"; rate=48000Hz; channels=1; frames=10; }}
asset takes_schema {{ kind=descriptor; path="takes.schema.json"; hash="{schema_hash}"; }}
extension takes {{
  namespace="maac.takes/1"; schema=&takes_schema; render_affecting=true;
  data={{ groups={{ vocals={{
    origin=1/48000s;
    takes={{ first={{ asset=&a; source_origin=0frame; }}; second={{ asset=&b; source_origin=2frame; }}; }};
    regions={{ opening={{ take=first; range=[0frame,2frame]; clip=&left; }}; ending={{ take=second; range=[2frame,4frame]; clip=&right; }}; }};
  }}; }}; }};
}}
audio left {{ asset=&a; at=1/48000s; source=[0frame,2frame]; mode=rate; }}
audio right {{ asset=&b; at=3/48000s; source=[4frame,6frame]; mode=rate; }}
node mix {{ type="core.sum/1"; config={{ channels=1; }}; }}
connect first_route {{ from=&left:out; to=&mix:in; }}
connect second_route {{ from=&right:out; to=&mix:in; }}
"#
    )
}

fn project(root: &Path) -> (std::path::PathBuf, String) {
    let project = root.join("project");
    fs::create_dir(&project).unwrap();
    let a = [0.125f32, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect::<Vec<_>>();
    let b = [
        0.0f32, 0.0, -0.125, -0.25, -0.375, -0.5, -0.625, -0.75, -0.875, -1.0,
    ]
    .into_iter()
    .flat_map(f32::to_le_bytes)
    .collect::<Vec<_>>();
    let schema = maac::takes::SCHEMA_BYTES;
    fs::write(project.join("a.pcm"), &a).unwrap();
    fs::write(project.join("b.pcm"), &b).unwrap();
    fs::write(project.join("takes.schema.json"), schema).unwrap();
    let source = source(
        &maac::bundle::sha256_digest(&a),
        &maac::bundle::sha256_digest(&b),
        &maac::bundle::sha256_digest(schema),
    );
    fs::write(project.join("main.maac"), &source).unwrap();
    (project, source)
}

fn set(object: &str, fields: &[&str], value: Value) -> Operation {
    Operation::Set {
        object: vec![object.into()],
        field: fields.iter().map(|v| v.to_string()).collect(),
        value,
        expect: None,
        expect_absent: false,
    }
}

fn frames(start: u64, end: u64) -> Value {
    json!({"t":"list","items":[{"t":"quantity","n":start.to_string(),"d":"1","u":"frame"},{"t":"quantity","n":end.to_string(),"d":"1","u":"frame"}]})
}

fn selection() -> Vec<Operation> {
    vec![
        set(
            "takes",
            &["data", "groups", "vocals", "regions", "opening", "take"],
            json!({"t":"symbol","v":"second"}),
        ),
        set(
            "left",
            &["asset"],
            json!({"t":"ref","path":["b"],"port":null}),
        ),
        set("left", &["source"], frames(2, 4)),
    ]
}

fn transaction(source: &str, operations: Vec<Operation>) -> Vec<u8> {
    Transaction::new(
        SourceDocument::parse(source).unwrap().revision().into(),
        operations,
    )
    .unwrap()
    .to_json()
    .unwrap()
}

fn build(project: &Path, output: &Path, disk: bool) -> Vec<f32> {
    let mut args = vec![
        Path::new("--json"),
        Path::new("build"),
        project,
        Path::new("-o"),
        output,
    ];
    if disk {
        args.push(Path::new("--disk-media"));
    }
    success(invoke(&args));
    hound::WavReader::open(output)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn comp_selection_renders_edits_undoes_and_reopens_with_all_alternates() {
    let root = tempfile::tempdir().unwrap();
    let (project, source) = project(root.path());
    let initial = build(&project, &root.path().join("initial.wav"), false);
    assert_eq!(initial, vec![0.0, 0.125, 0.25, -0.375, -0.5, 0.0]);
    assert_eq!(
        build(&project, &root.path().join("disk.wav"), true),
        initial
    );
    let base = root.path().join("base");
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &project,
        Path::new("--processor-context"),
        Path::new("--output-dir"),
        &base,
    ]));
    let patch = root.path().join("select.json");
    fs::write(&patch, transaction(&source, selection())).unwrap();
    let result = success(invoke(&[
        Path::new("--json"),
        Path::new("patch"),
        &project,
        &patch,
        Path::new("--disk-media"),
        Path::new("-o"),
        &project.join("main.maac"),
        Path::new("--force"),
    ]));
    assert!(fs::read_to_string(project.join("main.maac"))
        .unwrap()
        .contains("// Preserve the complete take choice"));
    let expected = vec![0.0, -0.125, -0.25, -0.375, -0.5, 0.0];
    assert_eq!(
        build(&project, &root.path().join("selected.wav"), true),
        expected
    );
    let archive = root.path().join("edited archive");
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("patch"),
        &base,
        &patch,
        Path::new("--output-dir"),
        &archive,
    ]));
    fs::remove_dir_all(&project).unwrap();
    fs::remove_dir_all(&base).unwrap();
    let moved = root.path().join("relocated archive");
    fs::rename(archive, &moved).unwrap();
    let reopened = root.path().join("reopened");
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &moved,
        Path::new("--output-dir"),
        &reopened,
    ]));
    assert!(reopened.join("a.pcm").exists());
    assert!(reopened.join("b.pcm").exists());
    assert!(reopened.join("takes.schema.json").exists());
    assert_eq!(
        build(&reopened, &root.path().join("reopened.wav"), true),
        expected
    );
    let inverse = root.path().join("inverse.json");
    fs::write(
        &inverse,
        serde_json::to_vec(&result["edit"]["inverse"]).unwrap(),
    )
    .unwrap();
    let restored = success(invoke(&[
        Path::new("--json"),
        Path::new("patch"),
        &reopened,
        &inverse,
        Path::new("--disk-media"),
        Path::new("-o"),
        &reopened.join("main.maac"),
        Path::new("--force"),
    ]));
    assert_eq!(
        restored["digest"],
        SourceDocument::parse(source).unwrap().revision()
    );
    assert_eq!(
        build(&reopened, &root.path().join("restored.wav"), true),
        initial
    );
}

#[test]
fn partial_selection_edits_and_invalid_take_contracts_fail_before_publication() {
    let root = tempfile::tempdir().unwrap();
    let (project, source) = project(root.path());
    let patch = root.path().join("partial.json");
    let output = root.path().join("owned.maac");
    fs::write(&output, b"owner").unwrap();
    for operations in [
        vec![selection().remove(0)],
        selection().into_iter().skip(1).collect(),
    ] {
        fs::write(&patch, transaction(&source, operations)).unwrap();
        failure(invoke(&[
            Path::new("--json"),
            Path::new("patch"),
            &project,
            &patch,
            Path::new("--disk-media"),
            Path::new("-o"),
            &output,
            Path::new("--force"),
        ]));
        assert_eq!(fs::read(&output).unwrap(), b"owner");
    }
    for (before, after) in [
        ("take=first;", "take=unknown;"),
        ("range=[2frame,4frame]", "range=[1frame,4frame]"),
        ("clip=&right;", "clip=&left;"),
        (
            "source_origin=2frame",
            "source_origin=18446744073709551615frame",
        ),
        ("source_origin=2frame", "source_origin=10frame"),
        ("source=[0frame,2frame]", "source=[0frame,3frame]"),
        ("at=1/48000s", "at=0q"),
        ("mode=rate;", "mode=rate; speed=2;"),
        ("mode=rate;", "mode=rate; reverse=true;"),
        ("path=\"b.pcm\";", "path=\"b.pcm\"; label=\"valid label\";"),
    ] {
        // The final row keeps the contract valid and guards over-rejection of asset labels.
        let changed = source.replace(before, after);
        fs::write(project.join("main.maac"), changed).unwrap();
        let checked = invoke(&[Path::new("--json"), Path::new("check"), &project]);
        if after.contains("valid label") {
            success(checked);
        } else {
            failure(checked);
        }
    }
}

#[test]
fn unused_alternates_remain_pinned_and_schema_resolves_from_package_root() {
    let root = tempfile::tempdir().unwrap();
    let (project, source) = project(root.path());
    // Choose first for both regions, leaving b inactive but still required.
    let source = source
        .replace("take=second;", "take=first;")
        .replace("audio right { asset=&b;", "audio right { asset=&a;")
        .replace("source=[4frame,6frame]", "source=[2frame,4frame]");
    let nested = project.join("nested");
    fs::create_dir(&nested).unwrap();
    let entry = nested.join("song.maac");
    fs::write(&entry, source).unwrap();
    let args = [
        Path::new("--json"),
        Path::new("check"),
        &entry,
        Path::new("--project-root"),
        &project,
        Path::new("--disk-media"),
    ];
    success(invoke(&args));
    let original = fs::read(project.join("b.pcm")).unwrap();
    fs::write(project.join("b.pcm"), vec![0u8; original.len()]).unwrap();
    assert_eq!(failure(invoke(&args))["code"], "E_HASH");
    fs::remove_file(project.join("b.pcm")).unwrap();
    failure(invoke(&args));
    fs::write(project.join("b.pcm"), original).unwrap();
    fs::write(project.join("takes.schema.json"), b"{}").unwrap();
    assert_eq!(failure(invoke(&args))["code"], "E_HASH");
}

#[test]
fn asset_and_clip_renames_preserve_take_references_and_audio() {
    let root = tempfile::tempdir().unwrap();
    let (project, source) = project(root.path());
    let patch = root.path().join("rename.json");
    fs::write(
        &patch,
        transaction(
            &source,
            vec![
                Operation::RenameId {
                    object: vec!["b".into()],
                    new_id: "alternate_b".into(),
                },
                Operation::RenameId {
                    object: vec!["left".into()],
                    new_id: "opening_clip".into(),
                },
            ],
        ),
    )
    .unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("patch"),
        &project,
        &patch,
        Path::new("-o"),
        &project.join("main.maac"),
        Path::new("--force"),
    ]));
    assert_eq!(
        build(&project, &root.path().join("renamed.wav"), false),
        vec![0.0, 0.125, 0.25, -0.375, -0.5, 0.0]
    );
}

#[test]
fn takes_and_production_delivery_coexist_in_retained_plans() {
    let root = tempfile::tempdir().unwrap();
    let (project, source) = project(root.path());
    let production = include_bytes!("../production.schema.json");
    fs::write(project.join("production.schema.json"), production).unwrap();
    let source = source.replace(
        "requires=[\"maac.takes/1\"]",
        "requires=[\"maac.takes/1\",\"maac.production/1\"]",
    ) + &format!(
        r#"
asset production_schema {{ kind=descriptor; path="production.schema.json"; hash="{}"; }}
extension delivery {{ namespace="maac.production/1";schema=&production_schema;render_affecting=true;data={{deliveries={{release={{rate=48000Hz;resampler="maac.src.kaiser/1";targets={{master={{role=master;output=&mix:out;encoding=wav_f32le;dither={{type=none;}};}};}};}};}};}}; }}
"#,
        maac::bundle::sha256_digest(production)
    );
    fs::write(project.join("main.maac"), source).unwrap();
    let plan = root.path().join("plan.json");
    success(invoke(&[
        Path::new("--json"),
        Path::new("compile"),
        &project,
        Path::new("-o"),
        &plan,
    ]));
    let wav = root.path().join("retained.wav");
    success(invoke(&[
        Path::new("--json"),
        Path::new("render"),
        &plan,
        Path::new("-o"),
        &wav,
    ]));
    assert_eq!(
        hound::WavReader::open(wav)
            .unwrap()
            .samples::<f32>()
            .map(Result::unwrap)
            .collect::<Vec<_>>(),
        vec![0.0, 0.125, 0.25, -0.375, -0.5, 0.0]
    );
    success(invoke(&[
        Path::new("--json"),
        Path::new("deliver"),
        &plan,
        Path::new("--delivery"),
        Path::new("release"),
        Path::new("--output-dir"),
        &root.path().join("delivery"),
    ]));
}

#[test]
fn take_requirement_edits_validate_complete_metadata_before_publication() {
    let root = tempfile::tempdir().unwrap();
    let (project, source) = project(root.path());
    let patch = root.path().join("requirements.json");
    let owned_output = root.path().join("owned.maac");
    fs::write(&owned_output, b"owner").unwrap();
    let delete_metadata = || {
        vec![
            Operation::DeleteObject {
                object: vec!["takes".into()],
                expect_object: None,
            },
            Operation::DeleteObject {
                object: vec!["takes_schema".into()],
                expect_object: None,
            },
        ]
    };
    // Removing the extension and descriptor must also remove the required
    // capability. Validate the complete candidate even once no extension remains.
    fs::write(&patch, transaction(&source, delete_metadata())).unwrap();
    let report = failure(invoke(&[
        Path::new("--json"),
        Path::new("patch"),
        &project,
        &patch,
        Path::new("-o"),
        &owned_output,
        Path::new("--force"),
    ]));
    assert_eq!(report["code"], "E_CAPABILITY");
    assert_eq!(fs::read(&owned_output).unwrap(), b"owner");
    assert_eq!(
        fs::read_to_string(project.join("main.maac")).unwrap(),
        source
    );

    let mut remove_all = delete_metadata();
    remove_all.push(set("song", &["requires"], json!({"t":"list","items":[]})));
    fs::write(&patch, transaction(&source, remove_all)).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("patch"),
        &project,
        &patch,
        Path::new("-o"),
        &project.join("main.maac"),
        Path::new("--force"),
    ]));
    success(invoke(&[Path::new("--json"), Path::new("check"), &project]));
    assert_eq!(
        build(&project, &root.path().join("without_metadata.wav"), false),
        vec![0.0, 0.125, 0.25, -0.375, -0.5, 0.0]
    );

    // Adding the requirement to a valid source without metadata must fail with
    // the same contract error and preserve both the output and authored source.
    let without_metadata = fs::read_to_string(project.join("main.maac")).unwrap();
    fs::write(
        &patch,
        transaction(
            &without_metadata,
            vec![set(
                "song",
                &["requires"],
                json!({"t":"list","items":[{"t":"string","v":"maac.takes/1"}]}),
            )],
        ),
    )
    .unwrap();
    let report = failure(invoke(&[
        Path::new("--json"),
        Path::new("patch"),
        &project,
        &patch,
        Path::new("-o"),
        &owned_output,
        Path::new("--force"),
    ]));
    assert_eq!(report["code"], "E_CAPABILITY");
    assert_eq!(fs::read(&owned_output).unwrap(), b"owner");
    assert_eq!(
        fs::read_to_string(project.join("main.maac")).unwrap(),
        without_metadata
    );
}
