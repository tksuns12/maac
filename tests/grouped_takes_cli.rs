use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use maac::editing::{Operation, SourceDocument, Transaction};
use serde_json::{json, Value};

const V1_SCHEMA_DIGEST: &str =
    "sha256:11cd8e70cf09aff986f821f88f46ad97056cfb5514a4344eeaa38a6d2e432960";
const INITIAL: [f32; 12] = [
    0.0, 0.0, 0.09375, 0.078125, 0.171875, 0.125, 0.25, 0.171875, 0.328125, 0.21875, 0.0, 0.0,
];
const SELECTED: [f32; 12] = [
    0.0, 0.0, 0.15625, 0.109375, 0.203125, 0.140625, 0.25, 0.171875, 0.296875, 0.203125, 0.0, 0.0,
];

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

fn pcm(samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}

fn source(hashes: [&str; 4], schema_hash: &str) -> String {
    format!(
        r#"maac 1;
// This authored comment must survive a grouped lane selection.
project song {{ score=[0q,6/48000q]; tail=0s; rate=48000Hz; tempo=&clock; meter=&metre; output=&mix:out; requires=["{capability}"]; }}
tempo clock {{ points=[(0q,60bpm,step)]; }}
meter metre {{ points=[(0q,4,4)]; }}
asset first_close {{ kind=audio; path="first-close.pcm"; hash="{}"; format="pcm_f32le_interleaved/1"; rate=48000Hz; channels=1; frames=8; }}
asset first_room {{ kind=audio; path="first-room.pcm"; hash="{}"; format="pcm_f32le_interleaved/1"; rate=48000Hz; channels=2; frames=8; }}
asset second_close {{ kind=audio; path="second-close.pcm"; hash="{}"; format="pcm_f32le_interleaved/1"; rate=48000Hz; channels=1; frames=8; }}
asset second_room {{ kind=audio; path="second-room.pcm"; hash="{}"; format="pcm_f32le_interleaved/1"; rate=48000Hz; channels=2; frames=8; }}
asset takes_schema {{ kind=descriptor; path="takes-v2.schema.json"; hash="{}"; }}
extension takes {{
  namespace="{capability}"; schema=&takes_schema; render_affecting=true;
  data={{ groups={{ vocals={{
    origin=1/48000s;
    takes={{
      first={{ lanes={{ close={{ asset=&first_close; source_origin=1frame; }}; room={{ asset=&first_room; source_origin=2frame; }}; }}; }};
      second={{ lanes={{ close={{ asset=&second_close; source_origin=3frame; }}; room={{ asset=&second_room; source_origin=4frame; }}; }}; }};
    }};
    regions={{ opening={{ take=first; range=[0frame,4frame]; clips={{ close=&opening_close; room=&opening_room; }}; }}; }};
  }}; }}; }};
}}
audio opening_close {{ asset=&first_close; at=1/48000s; source=[1frame,5frame]; mode=rate; }}
audio opening_room {{ asset=&first_room; at=1/48000s; source=[2frame,6frame]; mode=rate; }}
node close_matrix {{ type="core.matrix/1"; config={{ inputs=1; outputs=2; coefficients=[[1/2],[1/4]]; }}; }}
node mix {{ type="core.sum/1"; config={{ channels=2; }}; }}
connect close_to_matrix {{ from=&opening_close:out; to=&close_matrix:in; }}
connect matrix_to_mix {{ from=&close_matrix:out; to=&mix:in; }}
connect room_to_mix {{ from=&opening_room:out; to=&mix:in; }}
"#,
        hashes[0],
        hashes[1],
        hashes[2],
        hashes[3],
        schema_hash,
        capability = maac::takes::CAPABILITY_V2,
    )
}

fn project(root: &Path) -> (std::path::PathBuf, String) {
    let project = root.join("project");
    fs::create_dir(&project).unwrap();
    let first_close = pcm(&[0.0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875]);
    let first_room = pcm(&[
        0.0, 0.0, 0.015625, 0.03125, 0.03125, 0.046875, 0.046875, 0.0625, 0.0625, 0.078125,
        0.078125, 0.09375, 0.09375, 0.109375, 0.109375, 0.125,
    ]);
    let second_close = pcm(&[0.0, 0.0625, 0.125, 0.1875, 0.25, 0.3125, 0.375, 0.4375]);
    let second_room = pcm(&[
        0.0, 0.0, 0.015625, 0.015625, 0.03125, 0.03125, 0.046875, 0.046875, 0.0625, 0.0625,
        0.078125, 0.078125, 0.09375, 0.09375, 0.109375, 0.109375,
    ]);
    let schema = maac::takes::SCHEMA_V2_BYTES;
    let assets = [first_close, first_room, second_close, second_room];
    for (name, bytes) in [
        ("first-close.pcm", assets[0].as_slice()),
        ("first-room.pcm", assets[1].as_slice()),
        ("second-close.pcm", assets[2].as_slice()),
        ("second-room.pcm", assets[3].as_slice()),
    ] {
        fs::write(project.join(name), bytes).unwrap();
    }
    fs::write(project.join("takes-v2.schema.json"), schema).unwrap();
    let hashes = assets
        .each_ref()
        .map(|asset| maac::bundle::sha256_digest(asset));
    let source = source(
        hashes.each_ref().map(String::as_str),
        &maac::bundle::sha256_digest(schema),
    );
    fs::write(project.join("main.maac"), &source).unwrap();
    (project, source)
}

fn set(object: &str, fields: &[&str], value: Value) -> Operation {
    Operation::Set {
        object: vec![object.into()],
        field: fields.iter().map(|field| (*field).to_string()).collect(),
        value,
        expect: None,
        expect_absent: false,
    }
}

fn frames(start: u64, end: u64) -> Value {
    json!({"t":"list","items":[
        {"t":"quantity","n":start.to_string(),"d":"1","u":"frame"},
        {"t":"quantity","n":end.to_string(),"d":"1","u":"frame"}
    ]})
}

fn reference(asset: &str) -> Value {
    json!({"t":"ref","path":[asset],"port":null})
}

fn select_second(source: &str, include_room: bool) -> Vec<u8> {
    let mut operations = vec![
        set(
            "takes",
            &["data", "groups", "vocals", "regions", "opening", "take"],
            json!({"t":"symbol","v":"second"}),
        ),
        set("opening_close", &["asset"], reference("second_close")),
        set("opening_close", &["source"], frames(3, 7)),
    ];
    if include_room {
        operations.push(set("opening_room", &["asset"], reference("second_room")));
        operations.push(set("opening_room", &["source"], frames(4, 8)));
    }
    make_patch(source, operations)
}

fn make_patch(source: &str, operations: Vec<Operation>) -> Vec<u8> {
    Transaction::new(
        SourceDocument::parse(source).unwrap().revision().into(),
        operations,
    )
    .unwrap()
    .to_json()
    .unwrap()
}

fn render(project: &Path, output: &Path, disk_media: bool) -> Vec<f32> {
    let mut args = vec![
        Path::new("--json"),
        Path::new("build"),
        project,
        Path::new("-o"),
        output,
    ];
    if disk_media {
        args.push(Path::new("--disk-media"));
    }
    success(invoke(&args));
    let mut reader = hound::WavReader::open(output).unwrap();
    assert_eq!(reader.spec().sample_rate, 48_000);
    assert_eq!(reader.spec().channels, 2);
    assert_eq!(reader.duration(), 6);
    reader.samples::<f32>().map(Result::unwrap).collect()
}

fn patch(project: &Path, transaction: &Path, disk_media: bool) -> Value {
    let output = project.join("main.maac");
    let mut args = vec![
        Path::new("--json"),
        Path::new("patch"),
        project,
        transaction,
        Path::new("-o"),
        &output,
        Path::new("--force"),
    ];
    if disk_media {
        args.push(Path::new("--disk-media"));
    }
    success(invoke(&args))
}

fn assert_samples(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual, expected);
}

#[test]
fn v1_schema_digest_remains_compatible() {
    assert_eq!(
        maac::bundle::sha256_digest(maac::takes::SCHEMA_BYTES),
        V1_SCHEMA_DIGEST
    );
}

#[test]
fn grouped_mono_and_stereo_lanes_select_atomically_in_both_media_modes_and_undo() {
    let root = tempfile::tempdir().unwrap();
    let patch_path = root.path().join("select-second.json");

    for (name, disk_media) in [("ordinary", false), ("disk", true)] {
        let workspace = root.path().join(name);
        fs::create_dir(&workspace).unwrap();
        let (project, source) = project(&workspace);
        let initial = render(
            &project,
            &root.path().join(format!("{name}-initial.wav")),
            disk_media,
        );
        assert_samples(&initial, &INITIAL);
        fs::write(&patch_path, select_second(&source, true)).unwrap();
        let result = patch(&project, &patch_path, disk_media);
        let selected = render(
            &project,
            &root.path().join(format!("{name}-selected.wav")),
            disk_media,
        );
        assert_samples(&selected, &SELECTED);
        assert!(result["edit"]["inverse"].is_object());

        if !disk_media {
            let inverse = root.path().join("inverse.json");
            fs::write(
                &inverse,
                serde_json::to_vec(&result["edit"]["inverse"]).unwrap(),
            )
            .unwrap();
            let restored = patch(&project, &inverse, false);
            assert_eq!(
                restored["digest"],
                SourceDocument::parse(&source).unwrap().revision()
            );
            assert_samples(
                &render(&project, &root.path().join("restored.wav"), false),
                &INITIAL,
            );
        }
    }
}

#[test]
fn partial_lane_selection_rejects_without_replacing_output_or_source() {
    let root = tempfile::tempdir().unwrap();
    let (project, source) = project(root.path());
    let patch = root.path().join("partial.json");
    let output = root.path().join("owned.maac");
    fs::write(&output, b"owner").unwrap();
    fs::write(&patch, select_second(&source, false)).unwrap();
    for disk_media in [false, true] {
        let mut args = vec![
            Path::new("--json"),
            Path::new("patch"),
            &project,
            &patch,
            Path::new("-o"),
            &output,
            Path::new("--force"),
        ];
        if disk_media {
            args.push(Path::new("--disk-media"));
        }
        let report = failure(invoke(&args));
        assert!(report["code"].as_str().is_some());
        assert_eq!(fs::read(&output).unwrap(), b"owner");
        assert_eq!(
            fs::read_to_string(project.join("main.maac")).unwrap(),
            source
        );
    }
}

#[test]
fn archive_keeps_all_four_lane_assets_and_schema_after_patch_relocation_and_source_deletion() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let (project, source) = project(&workspace);
    let patch_file = root.path().join("select.json");
    fs::write(&patch_file, select_second(&source, true)).unwrap();
    let base = root.path().join("base.archive");
    let edited = root.path().join("edited.archive");
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &project,
        Path::new("--output-dir"),
        &base,
    ]));
    let direct_result = patch(&project, &patch_file, true);
    assert!(fs::read_to_string(project.join("main.maac"))
        .unwrap()
        .contains("// This authored comment must survive a grouped lane selection."));
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("patch"),
        &base,
        &patch_file,
        Path::new("--output-dir"),
        &edited,
    ]));
    let moved = root.path().join("moved.archive");
    fs::rename(&edited, &moved).unwrap();
    fs::remove_dir_all(&project).unwrap();
    fs::remove_dir_all(&base).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &moved,
    ]));
    let reopened = root.path().join("reopened");
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &moved,
        Path::new("--output-dir"),
        &reopened,
    ]));
    for asset in [
        "first-close.pcm",
        "first-room.pcm",
        "second-close.pcm",
        "second-room.pcm",
        "takes-v2.schema.json",
    ] {
        assert!(reopened.join(asset).is_file(), "missing {asset}");
    }
    assert_samples(
        &render(&reopened, &root.path().join("reopened.wav"), true),
        &SELECTED,
    );
    let inverse = root.path().join("inverse.json");
    fs::write(
        &inverse,
        serde_json::to_vec(&direct_result["edit"]["inverse"]).unwrap(),
    )
    .unwrap();
    let restored = patch(&reopened, &inverse, true);
    assert_eq!(
        restored["digest"],
        SourceDocument::parse(&source).unwrap().revision()
    );
    assert_samples(
        &render(&reopened, &root.path().join("restored.wav"), true),
        &INITIAL,
    );
}

#[test]
fn inactive_alternate_lane_corruption_fails_disk_media_check() {
    let root = tempfile::tempdir().unwrap();
    let (project, _) = project(root.path());
    success(invoke(&[
        Path::new("--json"),
        Path::new("check"),
        &project,
        Path::new("--disk-media"),
    ]));
    let inactive = project.join("second-room.pcm");
    let original = fs::read(&inactive).unwrap();
    fs::write(&inactive, vec![0; original.len()]).unwrap();
    let report = failure(invoke(&[
        Path::new("--json"),
        Path::new("check"),
        &project,
        Path::new("--disk-media"),
    ]));
    assert_eq!(report["code"], "E_HASH");
}

#[test]
fn v2_requirement_addition_and_removal_validate_metadata_before_publication() {
    let root = tempfile::tempdir().unwrap();
    let (project, source) = project(root.path());
    let patch_path = root.path().join("requirements.json");
    let output = root.path().join("owned.maac");
    fs::write(&output, b"owner").unwrap();
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

    fs::write(&patch_path, make_patch(&source, delete_metadata())).unwrap();
    let report = failure(invoke(&[
        Path::new("--json"),
        Path::new("patch"),
        &project,
        &patch_path,
        Path::new("-o"),
        &output,
        Path::new("--force"),
    ]));
    assert_eq!(report["code"], "E_CAPABILITY");
    assert_eq!(fs::read(&output).unwrap(), b"owner");
    assert_eq!(
        fs::read_to_string(project.join("main.maac")).unwrap(),
        source
    );

    let mut remove_requirement = delete_metadata();
    remove_requirement.push(set("song", &["requires"], json!({"t":"list","items":[]})));
    fs::write(&patch_path, make_patch(&source, remove_requirement)).unwrap();
    patch(&project, &patch_path, false);
    success(invoke(&[Path::new("--json"), Path::new("check"), &project]));
    assert_samples(
        &render(&project, &root.path().join("without-takes.wav"), false),
        &INITIAL,
    );

    let without_metadata = fs::read_to_string(project.join("main.maac")).unwrap();
    fs::write(
        &patch_path,
        make_patch(
            &without_metadata,
            vec![set(
                "song",
                &["requires"],
                json!({"t":"list","items":[{"t":"string","v":maac::takes::CAPABILITY_V2}]}),
            )],
        ),
    )
    .unwrap();
    let report = failure(invoke(&[
        Path::new("--json"),
        Path::new("patch"),
        &project,
        &patch_path,
        Path::new("-o"),
        &output,
        Path::new("--force"),
    ]));
    assert_eq!(report["code"], "E_CAPABILITY");
    assert_eq!(fs::read(&output).unwrap(), b"owner");
    assert_eq!(
        fs::read_to_string(project.join("main.maac")).unwrap(),
        without_metadata
    );
}

#[test]
fn grouped_and_v1_take_plans_coexist_with_production_and_render_without_source() {
    let root = tempfile::tempdir().unwrap();
    let (project, source) = project(root.path());
    let production = maac::production_data::SCHEMA_BYTES;
    fs::write(project.join("production.schema.json"), production).unwrap();
    let v1_schema = maac::takes::SCHEMA_BYTES;
    fs::write(project.join("takes.schema.json"), v1_schema).unwrap();
    let source = source.replace(
        "requires=[\"maac.takes/2\"]",
        "requires=[\"maac.takes/2\",\"maac.takes/1\",\"maac.production/1\"]",
    ) + &format!(
        r#"
asset production_schema {{ kind=descriptor; path="production.schema.json"; hash="{}"; }}
extension delivery {{ namespace="maac.production/1"; schema=&production_schema; render_affecting=true; data={{deliveries={{release={{rate=48000Hz;resampler="maac.src.kaiser/1";targets={{master={{role=master;output=&mix:out;encoding=wav_f32le;dither={{type=none;}};}};}};}};}};}}; }}
asset legacy_takes_schema {{ kind=descriptor; path="takes.schema.json"; hash="{}"; }}
extension legacy_takes {{ namespace="maac.takes/1"; schema=&legacy_takes_schema; render_affecting=true; data={{groups={{legacy={{origin=0s;takes={{legacy={{asset=&first_close;source_origin=0frame;}};}};regions={{legacy_region={{take=legacy;range=[0frame,2frame];clip=&legacy_clip;}};}};}};}};}}; }}
audio legacy_clip {{ asset=&first_close; at=0s; source=[0frame,2frame]; mode=rate; }}
"#,
        maac::bundle::sha256_digest(production),
        maac::bundle::sha256_digest(v1_schema)
    );
    fs::write(project.join("main.maac"), source).unwrap();
    let plan = root.path().join("retained.json");
    success(invoke(&[
        Path::new("--json"),
        Path::new("compile"),
        &project,
        Path::new("-o"),
        &plan,
    ]));
    fs::remove_dir_all(&project).unwrap();
    let rendered = root.path().join("retained.wav");
    success(invoke(&[
        Path::new("--json"),
        Path::new("render"),
        &plan,
        Path::new("-o"),
        &rendered,
    ]));
    let mut reader = hound::WavReader::open(rendered).unwrap();
    assert_eq!(reader.spec().channels, 2);
    assert_eq!(
        reader
            .samples::<f32>()
            .map(Result::unwrap)
            .collect::<Vec<_>>(),
        INITIAL
    );
}
