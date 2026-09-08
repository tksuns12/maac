use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::tempdir;

fn invoke(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .current_dir(cwd)
        .arg("--json")
        .args(args)
        .output()
        .unwrap()
}
fn value(output: &Output) -> Value {
    assert!(output.stderr.is_empty(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn profile_is_a_leaf_option_with_default_and_song_values() {
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("main.maac"),
        "maac 1; library main { version = \"1\"; }",
    )
    .unwrap();
    for profile in ["default", "song"] {
        let result = invoke(temp.path(), &["check", "--profile", profile]);
        assert!(result.status.success(), "{result:?}");
        assert_eq!(value(&result)["ok"], true);
    }
    for args in [
        vec!["check", "--profile", "unlimited"],
        vec!["--profile", "song", "check"],
        vec!["hash", "main.maac", "--profile", "song"],
        vec!["instruments", "--profile", "song"],
    ] {
        let result = invoke(temp.path(), &args);
        assert!(!result.status.success());
        assert_eq!(value(&result)["code"], "E_USAGE");
    }
}

/// Conservative max-release accounting exceeds500M while actual note-off
/// release is zero: real CLI render coverage need not execute500M visits.
fn high_work_source() -> String {
    let mut source = String::from(
        r#"maac 1;
project p { score = [0q, 1/2q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; tail = 0s; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
instrument dense {
 channels = 1;
 voice voice {
  channels = 1; amplitude = &amp; output = &wave0:out;
  node amp { type = "synth.adsr/1"; }
"#,
    );
    for i in 0..63 {
        source.push_str(&format!("node wave{i} {{ type = \"synth.sine/1\"; params = {{ phase = 0.25; level = 0.0001; }}; }}\n"));
    }
    source.push_str(
        r#" }
 control release { target = &voice.amp.params.release; default = 0s; }
}
node sound { instrument = &dense; config = { voices = 1000; }; }
pattern phrase { length = 1/2q;
"#,
    );
    for i in 0..1000 {
        source.push_str(&format!(
            "note n{i} {{ at = 0q; dur = 1/24000q; pitch = A4; velocity = 1; }}\n"
        ));
    }
    source.push_str(
        r#"}
track notes { target = &sound:events; }
place play { pattern = &phrase; track = &notes; at = 0q; }
curve release_later { clock = seconds; points = [(0s, 0s, step), (1/10s, 20s, step)]; }
automation later { target = &sound.params.release; curve = &release_later; at = 0s; }
"#,
    );
    source
}

fn success(cwd: &Path, args: &[&str]) -> Value {
    let output = invoke(cwd, args);
    assert!(output.status.success(), "{args:?}: {output:?}");
    let result = value(&output);
    assert_eq!(result["ok"], true);
    result
}
fn failure(cwd: &Path, args: &[&str], code: &str) {
    let output = invoke(cwd, args);
    assert!(!output.status.success(), "{args:?}: {output:?}");
    assert_eq!(value(&output)["code"], code, "{args:?}");
}

#[test]
fn large_source_requires_explicit_song_at_every_cli_boundary_and_retained_render() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("main.maac"), high_work_source()).unwrap();
    for profile in [vec![], vec!["--profile", "default"]] {
        let mut check = vec!["check"];
        check.extend(profile.iter().copied());
        failure(root, &check, "E_RESOURCE_LIMIT");
        let mut compile = vec!["compile", "-o", "rejected.json"];
        compile.extend(profile.iter().copied());
        failure(root, &compile, "E_RESOURCE_LIMIT");
        let mut build = vec!["build", "-o", "rejected.wav"];
        build.extend(profile.iter().copied());
        failure(root, &build, "E_RESOURCE_LIMIT");
    }
    assert!(!root.join("rejected.json").exists() && !root.join("rejected.wav").exists());
    let checked = success(root, &["check", "--profile", "song"]);
    assert_eq!(checked["notes"], 1000);
    assert_eq!(checked["frames"], 12000);
    success(root, &["compile", "--profile", "song", "-o", "song.json"]);
    success(
        root,
        &[
            "build",
            ".",
            "--profile",
            "song",
            "-o",
            "song.wav",
            "--format",
            "pcm16",
        ],
    );
    let wire: Value = serde_json::from_slice(&fs::read(root.join("song.json")).unwrap()).unwrap();
    assert!(wire.get("profile").is_none());
    fs::remove_file(root.join("main.maac")).unwrap();
    failure(
        root,
        &["render", "song.json", "-o", "default.wav"],
        "E_RESOURCE_LIMIT",
    );
    assert!(!root.join("default.wav").exists());
    success(
        root,
        &[
            "render",
            "song.json",
            "--profile",
            "song",
            "-o",
            "retained.wav",
            "--format",
            "pcm16",
        ],
    );
    assert_eq!(
        fs::read(root.join("song.wav")).unwrap(),
        fs::read(root.join("retained.wav")).unwrap()
    );
    assert_eq!(
        maac::cli::read_plan(&root.join("song.json"))
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );
    maac::cli::read_plan_with_limits(&root.join("song.json"), &maac::plan::PlanLimits::song())
        .unwrap();
}

#[test]
fn song_does_not_relax_graph_or_source_limits_and_budget_failure_preserves_destination() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let source = high_work_source();
    fs::write(root.join("main.maac"), &source).unwrap();
    fs::write(root.join("protected.wav"), b"original").unwrap();
    failure(
        root,
        &["build", "--force", "-o", "protected.wav"],
        "E_RESOURCE_LIMIT",
    );
    assert_eq!(fs::read(root.join("protected.wav")).unwrap(), b"original");
    let too_many = source.replace(
        "node amp {",
        "node excess { type = \"synth.sine/1\"; } node amp {",
    );
    fs::write(root.join("main.maac"), too_many).unwrap();
    failure(root, &["check", "--profile", "song"], "E_RESOURCE_LIMIT");
    fs::write(root.join("main.maac"), vec![b' '; 4 * 1024 * 1024 + 1]).unwrap();
    failure(root, &["check", "--profile", "song"], "E_RESOURCE_LIMIT");
}
