use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::tempdir;

const SOURCE: &str = r#"maac 1;
project p { score=[0q,4q]; rate=48000Hz; tempo=&clock; meter=&metre; output=&sine:out; }
tempo clock { points=[(0q,120bpm,step)]; }
meter metre { points=[(0q,4,4)]; }
node sine { type="core.sine/1"; config={ voices=8; }; }
track t { target=&sine:events; }
pattern leaf { length=4q; note n { at=0q; dur=3q; pitch=C4; } }
place play { pattern=&leaf; track=&t; at=0q; }
"#;

fn invoke(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap()
}

fn json(output: &Output) -> Value {
    assert!(output.stderr.is_empty(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn success(cwd: &Path, args: &[&str]) -> Value {
    let output = invoke(cwd, args);
    assert!(output.status.success(), "{args:?}: {output:?}");
    let result = json(&output);
    assert_eq!(result["ok"], true, "{args:?}");
    result
}

fn failure(cwd: &Path, args: &[&str], code: &str) {
    let output = invoke(cwd, args);
    assert!(!output.status.success(), "{args:?}: {output:?}");
    assert_eq!(json(&output)["code"], code, "{args:?}");
}

fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, result: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                result.insert(format!("{relative}/"), Vec::new());
                visit(root, &path, result);
            } else {
                result.insert(relative, fs::read(path).unwrap());
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[test]
fn source_and_retained_plan_queries_return_the_same_complete_events_without_writes() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let project = root.join("song");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("main.maac"), SOURCE).unwrap();

    let before_source_query = snapshot(root);
    let source_result = success(
        root,
        &[
            "--json",
            "query-events",
            "song",
            "--project-root",
            "song",
            "--profile",
            "song",
            "--start-q",
            "5/4",
            "--end-q",
            "3/2",
        ],
    );
    assert_eq!(snapshot(root), before_source_query);
    assert_eq!(source_result["command"], "query-events");
    assert_eq!(source_result["window"]["start_q"], "5/4");
    assert_eq!(source_result["window"]["end_q"], "3/2");
    assert_eq!(source_result["events"].as_array().unwrap().len(), 1);
    let event = &source_result["events"][0];
    assert_eq!(event["address"], "play/0/n");
    assert_eq!(event["source"]["path"], serde_json::json!(["leaf", "n"]));
    assert_eq!(event["score_on_q"], "0/1");
    assert_eq!(event["score_off_q"], "3/1");
    assert_eq!(event["kind"]["kind"], "note");
    assert!(event["on_frame"].as_u64().is_some());
    assert!(event["off_frame"].as_u64().is_some());

    success(
        root,
        &[
            "--json",
            "compile",
            "song",
            "--project-root",
            "song",
            "--profile",
            "song",
            "-o",
            "retained.json",
        ],
    );
    let before_plan_query = snapshot(root);
    let plan_result = success(
        root,
        &[
            "--json",
            "query-events",
            "retained.json",
            "--plan",
            "--profile",
            "song",
            "--start-q",
            "5/4",
            "--end-q",
            "3/2",
        ],
    );
    assert_eq!(snapshot(root), before_plan_query);
    assert_eq!(plan_result["window"], source_result["window"]);
    assert_eq!(plan_result["events"], source_result["events"]);

    let human = invoke(
        root,
        &[
            "query-events",
            "song",
            "--project-root",
            "song",
            "--start-q",
            "5/4",
            "--end-q",
            "3/2",
        ],
    );
    assert!(human.status.success(), "{human:?}");
    let human = String::from_utf8(human.stdout).unwrap();
    assert!(human.contains("play/0/n"), "{human}");
    assert!(human.contains("| leaf/n |"), "{human}");
    assert!(human.contains("score=0..3q"), "{human}");
    assert!(human.contains("frames="), "{human}");
}

#[test]
fn query_rejects_reversed_negative_and_out_of_score_windows() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("main.maac"), SOURCE).unwrap();
    let before = snapshot(root);

    for (start, end) in [("2", "1"), ("-1", "1"), ("3", "5")] {
        failure(
            root,
            &[
                "--json",
                "query-events",
                "main.maac",
                "--start-q",
                start,
                "--end-q",
                end,
            ],
            "E_INTERVAL",
        );
        assert_eq!(snapshot(root), before, "{start}..{end} wrote files");
    }
}

#[test]
fn query_requires_both_score_bounds_and_reports_invalid_rational_syntax_as_json() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("main.maac"), SOURCE).unwrap();

    failure(
        root,
        &["--json", "query-events", "main.maac", "--start-q", "0"],
        "E_USAGE",
    );
    failure(
        root,
        &[
            "--json",
            "query-events",
            "main.maac",
            "--start-q",
            "1e3",
            "--end-q",
            "2",
        ],
        "E_SYNTAX",
    );
}

#[test]
fn embedded_callers_receive_query_data_only_through_artifact_result() {
    let temp = tempdir().unwrap();
    let input = temp.path().join("main.maac");
    fs::write(&input, SOURCE).unwrap();
    let command = maac::cli::Command::QueryEvents {
        input,
        plan: false,
        project_root: None,
        profile: maac::cli::ProfileArg::Default,
        start_q: "1".into(),
        end_q: "2".into(),
    };
    assert_eq!(maac::cli::execute(&command).unwrap_err().code, "E_USAGE");
    let result = maac::cli::execute_artifact(&command).unwrap();
    let (start, end) = result.score_window().unwrap();
    assert_eq!(
        (start.to_string(), end.to_string()),
        ("1".into(), "2".into())
    );
    let events = result.queried_events().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].address, "play/0/n");
}
