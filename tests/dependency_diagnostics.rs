//! §23 locations for dependency declarations in the entry source (P22).
//!
//! Each case changes one `import`, `sample` or `asset` declaration in a valid
//! composition, or the files it pins, and requires the first diagnostic to
//! name that declaration and the `path`, `hash`, `builtin` or `label` field at
//! fault, with a span over its text. A failure in another source is located in
//! that source, and its message begins by naming it.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;

use maac::bundle::sha256_digest;
use maac::compiler::check_bundle_artifact;
use maac::{Diagnostics, SourceBundle};

const LIBRARY: &str = "maac 1;\nlibrary lib { version = \"1\"; }\n";
const OTHER_HASH: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

fn wav() -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::new(&mut bytes, spec).unwrap();
        for j in 0..16 {
            writer.write_sample(j as f32 / 16.0).unwrap();
        }
        writer.finalize().unwrap();
    }
    bytes.into_inner()
}

fn pcm() -> Vec<u8> {
    vec![0; 16]
}

fn files() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        ("lib.maac".to_owned(), LIBRARY.as_bytes().to_vec()),
        ("tone.wav".to_owned(), wav()),
        ("clip.pcm".to_owned(), pcm()),
    ])
}

fn source() -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &s:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node s {{ type = "core.sine/1"; }}
import sounds {{ path = "lib.maac"; hash = "{lib}"; }}
sample tone {{ path = "tone.wav"; hash = "{tone}"; root = A4; }}
asset clip {{ kind = audio; path = "clip.pcm"; hash = "{clip}"; format = "pcm_f32le_interleaved/1"; rate = 48000Hz; channels = 1; frames = 4; }}
"#,
        lib = sha256_digest(LIBRARY.as_bytes()),
        tone = sha256_digest(&wav()),
        clip = sha256_digest(&pcm()),
    )
}

fn bundle(source: &str, files: &BTreeMap<String, Vec<u8>>) -> SourceBundle {
    let mut bundle = SourceBundle::new("main.maac", source);
    for (path, bytes) in files {
        if path.ends_with(".maac") {
            bundle
                .sources
                .insert(path.clone(), String::from_utf8(bytes.clone()).unwrap());
        } else {
            bundle.assets.insert(path.clone(), bytes.clone());
        }
    }
    bundle
}

/// The first diagnostic's code, object path, field path and span text.
fn first(
    diagnostics: &Diagnostics,
    source: &str,
) -> (String, Vec<String>, Vec<String>, Option<String>) {
    let first = diagnostics.iter().next().expect("a diagnostic");
    (
        first.code.as_str().to_owned(),
        first.object_path.clone(),
        first.field_path.clone(),
        first
            .span
            .map(|span| source[span.start..span.end].to_owned()),
    )
}

fn expect(
    code: &str,
    object: &str,
    field: &str,
    text: &str,
) -> (String, Vec<String>, Vec<String>, Option<String>) {
    let field = if field.is_empty() {
        Vec::new()
    } else {
        vec![field.to_owned()]
    };
    (code.into(), vec![object.into()], field, Some(text.into()))
}

/// (name, from, to, expected code, object, field, span text)
type Case = (
    &'static str,
    &'static str,
    String,
    &'static str,
    &'static str,
    &'static str,
    String,
);

fn cases() -> Vec<Case> {
    let lib = sha256_digest(LIBRARY.as_bytes());
    let tone = sha256_digest(&wav());
    let clip = sha256_digest(&pcm());
    let quoted = |text: &str| format!("\"{text}\"");
    vec![
        ("import hash mismatch", "hash = \"sha256:LIB\"", format!("hash = \"{OTHER_HASH}\""), "E_HASH", "sounds", "hash", quoted(OTHER_HASH)),
        ("import hash spelling", "hash = \"sha256:LIB\"", "hash = \"sha256:xyz\"".into(), "E_HASH", "sounds", "hash", quoted("sha256:xyz")),
        ("import missing source", "path = \"lib.maac\"", "path = \"missing.maac\"".into(), "E_REFERENCE", "sounds", "path", quoted("missing.maac")),
        ("import escapes the root", "path = \"lib.maac\"", "path = \"../lib.maac\"".into(), "E_REFERENCE", "sounds", "path", quoted("../lib.maac")),
        ("import path not a string", "path = \"lib.maac\"", "path = 1".into(), "E_REFERENCE", "sounds", "path", "1".into()),
        ("import shape", "import sounds { path = \"lib.maac\"; hash = \"sha256:LIB\"; }", "import sounds { path = \"lib.maac\"; }".into(), "E_REFERENCE", "sounds", "", "import".into()),
        ("unknown built-in", "import sounds { path = \"lib.maac\"; hash = \"sha256:LIB\"; }", "import sounds { builtin = \"nothing\"; }".into(), "E_REFERENCE", "sounds", "builtin", quoted("nothing")),
        ("import label", "import sounds { path", "import sounds { label = 1; path".into(), "E_UNIT", "sounds", "label", "1".into()),
        ("import cycle at the entry", "import sounds {", format!("import again {{ path = \"main.maac\"; hash = \"{OTHER_HASH}\"; }}\nimport sounds {{"), "E_REFERENCE", "again", "path", quoted("main.maac")),
        ("sample hash mismatch", "hash = \"sha256:TONE\"", format!("hash = \"{OTHER_HASH}\""), "E_HASH", "tone", "hash", quoted(OTHER_HASH)),
        ("sample hash spelling", "hash = \"sha256:TONE\"", "hash = \"md5:abc\"".into(), "E_HASH", "tone", "hash", quoted("md5:abc")),
        ("sample bytes missing", "path = \"tone.wav\"", "path = \"other.wav\"".into(), "E_ASSET", "tone", "path", quoted("other.wav")),
        ("sample escapes the root", "path = \"tone.wav\"", "path = \"../tone.wav\"".into(), "E_REFERENCE", "tone", "path", quoted("../tone.wav")),
        ("sample path not a string", "path = \"tone.wav\"", "path = tone".into(), "E_ASSET", "tone", "path", "tone".into()),
        ("wavetable hash mismatch", "import sounds {", format!("wavetable w {{ path = \"tone.wav\"; hash = \"{OTHER_HASH}\"; cycle_length = 4; }}\nimport sounds {{"), "E_HASH", "w", "hash", quoted(OTHER_HASH)),
        ("asset hash mismatch", "hash = \"sha256:CLIP\"", format!("hash = \"{OTHER_HASH}\""), "E_HASH", "clip", "hash", quoted(OTHER_HASH)),
        ("asset bytes missing", "path = \"clip.pcm\"", "path = \"other.pcm\"".into(), "E_ASSET", "clip", "path", quoted("other.pcm")),
        ("asset platform path", "path = \"clip.pcm\"", "path = \"C:/clip.pcm\"".into(), "E_REFERENCE", "clip", "path", quoted("C:/clip.pcm")),
    ]
    .into_iter()
    .map(|(name, from, to, code, object, field, text)| {
        let pins = |text: &str| {
            text.replace("sha256:LIB", &lib)
                .replace("sha256:TONE", &tone)
                .replace("sha256:CLIP", &clip)
        };
        (name, leak(pins(from)), pins(&to), code, object, field, text)
    })
    .collect()
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

#[test]
fn entry_dependency_failures_are_located_at_their_declaration() {
    let base = source();
    check_bundle_artifact(&bundle(&base, &files())).expect("the base is valid");
    let mut failures = Vec::new();
    let cases = cases();
    for (name, from, to, code, object, field, text) in &cases {
        assert!(base.contains(*from), "{name}: base lacks {from:?}");
        let source = base.replacen(from, to, 1);
        let actual = match check_bundle_artifact(&bundle(&source, &files())) {
            Ok(()) => {
                failures.push(format!("{name}: accepted"));
                continue;
            }
            Err(diagnostics) => first(&diagnostics, &source),
        };
        let expected = expect(code, object, field, text);
        if actual != expected {
            failures.push(format!("{name}: got {actual:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {}:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

#[test]
fn filesystem_loading_locates_entry_dependency_failures() {
    let base = source();
    let write = |dir: &Path, source: &str, files: &BTreeMap<String, Vec<u8>>| {
        std::fs::write(dir.join("main.maac"), source).unwrap();
        for (path, bytes) in files {
            std::fs::write(dir.join(path), bytes).unwrap();
        }
    };
    let load = |dir: &Path| maac::bundle_fs::load_bundle(Path::new("main.maac"), dir);
    let valid = tempfile::tempdir().unwrap();
    write(valid.path(), &base, &files());
    load(valid.path()).expect("the base loads");

    let mut failures = Vec::new();
    let quoted = |text: &str| format!("\"{text}\"");
    let mut changed_file = |name: &str, path: &str, bytes: Option<Vec<u8>>, expected| {
        let dir = tempfile::tempdir().unwrap();
        let mut files = files();
        match bytes {
            Some(bytes) => files.insert(path.to_owned(), bytes),
            None => files.remove(path),
        };
        write(dir.path(), &base, &files);
        match load(dir.path()) {
            Ok(_) => failures.push(format!("{name}: accepted")),
            Err(diagnostics) => {
                let actual = first(&diagnostics, &base);
                if actual != expected {
                    failures.push(format!("{name}: got {actual:?}, expected {expected:?}"));
                }
            }
        }
    };
    changed_file(
        "import file missing",
        "lib.maac",
        None,
        expect("E_REFERENCE", "sounds", "path", &quoted("lib.maac")),
    );
    changed_file(
        "import file changed",
        "lib.maac",
        Some(b"maac 1;\nlibrary lib { version = \"2\"; }\n".to_vec()),
        expect(
            "E_HASH",
            "sounds",
            "hash",
            &quoted(&sha256_digest(LIBRARY.as_bytes())),
        ),
    );
    changed_file(
        "sample file missing",
        "tone.wav",
        None,
        expect("E_ASSET", "tone", "path", &quoted("tone.wav")),
    );
    changed_file(
        "sample file changed",
        "tone.wav",
        Some(pcm()),
        expect("E_HASH", "tone", "hash", &quoted(&sha256_digest(&wav()))),
    );
    changed_file(
        "asset file missing",
        "clip.pcm",
        None,
        expect("E_ASSET", "clip", "path", &quoted("clip.pcm")),
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn failures_in_other_sources_are_located_in_that_source() {
    // As for syntax and validation failures in an imported library, the
    // location is read in the source the message names. A missing sample file
    // is found while resolving; a malformed import while discovering.
    for (declaration, code, field, text) in [
        (
            format!("sample far {{ path = \"far.wav\"; hash = \"{OTHER_HASH}\"; root = A4; }}"),
            "E_ASSET",
            vec!["path".to_owned()],
            "\"far.wav\"",
        ),
        (
            "import far { path = \"far.maac\"; }".to_owned(),
            "E_REFERENCE",
            Vec::new(),
            "import",
        ),
    ] {
        let library = format!("maac 1;\nlibrary lib {{ version = \"1\"; }}\n{declaration}\n");
        let mut files = files();
        files.insert("lib.maac".into(), library.as_bytes().to_vec());
        let source = source().replace(
            &sha256_digest(LIBRARY.as_bytes()),
            &sha256_digest(library.as_bytes()),
        );
        let diagnostics = check_bundle_artifact(&bundle(&source, &files)).unwrap_err();
        let first = diagnostics.iter().next().unwrap();
        assert_eq!(first.code.as_str(), code, "{}", first.message);
        assert!(
            first.message.starts_with("source `lib.maac`: "),
            "{}",
            first.message
        );
        assert_eq!(first.object_path, ["far"]);
        assert_eq!(first.field_path, field);
        let span = first.span.expect("a span in the library");
        assert_eq!(&library[span.start..span.end], text);
    }
}
