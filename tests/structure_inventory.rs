//! Core object inventory and nesting (evidence row D02, spec §§2 and 4–19).
//!
//! `KINDS` restates, from the specification text, each core kind's location,
//! required and optional fields, and permitted children. Every case runs the
//! complete Document-profile validator through the public editing context:
//!
//! - each kind's minimal and fully populated instances are accepted;
//! - removing any required field, adding an unknown field, or giving `label`
//!   a non-string value is refused at that object;
//! - every (parent, child) pair of kinds, plus an unknown kind, is accepted
//!   exactly when §4 permits that nesting;
//! - duplicate fields and IDs are refused for every kind and container.
//!
//! `extension` is outside the editing context (§26 requires its schema
//! adapter); its inventory is checked through bundle compilation in
//! `tests/production_data.rs`.

#[path = "support/inventory.rs"]
mod inventory;

use inventory::*;
use maac::DiagnosticCode;

#[test]
fn the_base_composition_is_valid() {
    validate(&base()).unwrap();
}

#[test]
fn every_kind_accepts_its_minimal_and_fully_populated_forms() {
    let mut failures = Vec::new();
    for kind in KINDS {
        for (form, body) in [("minimal", kind.minimal), ("full", kind.full)] {
            let (source, _) = source_for(kind, body);
            if let Err(error) = validate(&source) {
                failures.push(format!("{} {form}: {error:?}", kind.name));
            }
        }
    }
    // The project's documented optional fields.
    let full_project = base().replace(
        "output = &mix:out; }",
        r#"output = &mix:out; tail = 1s; seed = 7; requires = []; label = "song"; }"#,
    );
    if let Err(error) = validate(&full_project) {
        failures.push(format!("project full: {error:?}"));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn missing_required_fields_are_refused_at_their_object() {
    let mut failures = Vec::new();
    for kind in KINDS {
        for field in kind.required {
            let (source, path) = source_for(kind, &without(kind.minimal, field));
            match validate(&source) {
                Ok(()) => failures.push(format!("{} without {field}: accepted", kind.name)),
                Err(error) if error.object_path != path => failures.push(format!(
                    "{} without {field}: located at {:?}",
                    kind.name, error.object_path
                )),
                Err(_) => {}
            }
        }
    }
    for field in ["score", "rate", "tempo", "meter"] {
        let project = "project song { score = [0q, 8q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &mix:out; }";
        let source = base().replace(project, &without(project, field));
        match validate(&source) {
            Ok(()) => failures.push(format!("project without {field}: accepted")),
            Err(error) if error.object_path != ["song"] => failures.push(format!(
                "project without {field}: located at {:?}",
                error.object_path
            )),
            Err(_) => {}
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn unknown_fields_and_non_string_labels_are_refused_on_every_kind() {
    let mut failures = Vec::new();
    for kind in KINDS {
        let (source, path) = source_for(kind, &format!("{} bogus = 1;", kind.minimal));
        match validate(&source) {
            Err(error)
                if error.code == "E_UNKNOWN_FIELD"
                    && error.object_path == path
                    && error.field_path == ["bogus"] => {}
            other => failures.push(format!("{} bogus: {other:?}", kind.name)),
        }
        let (source, path) = source_for(kind, &format!("{} label = 1;", kind.minimal));
        match validate(&source) {
            Err(error) if error.object_path == path && error.field_path == ["label"] => {}
            other => failures.push(format!("{} label = 1: {other:?}", kind.name)),
        }
    }
    let source = base().replace("output = &mix:out; }", "output = &mix:out; bogus = 1; }");
    match validate(&source) {
        Err(error) if error.code == "E_UNKNOWN_FIELD" && error.object_path == ["song"] => {}
        other => failures.push(format!("project bogus: {other:?}")),
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_nesting_combination_follows_the_inventory() {
    let mut failures = Vec::new();
    let mut checked = 0;
    // Each kind as a parent, holding each child kind in turn.
    for parent in KINDS {
        for child in CHILD_KINDS {
            // An insert holds exactly one leaf; its minimal form already has one.
            let permitted = parent.children.contains(child) && parent.kind != "insert";
            let body = format!("{} {}", parent.minimal, child_declaration(child, "inner"));
            let (source, path) = source_for(parent, &body);
            let mut inner = path.clone();
            inner.push("inner".into());
            checked += 1;
            match (validate(&source), permitted) {
                (Ok(()), true) => {}
                (Err(error), false) if error.object_path.starts_with(&path) => {}
                (result, _) => failures.push(format!(
                    "{} > {child} (permitted: {permitted}): {result:?}",
                    parent.name
                )),
            }
        }
    }
    // An insert holding a single leaf of each permitted kind, and nothing else.
    for child in CHILD_KINDS {
        let (source, path) = placed(
            Location::Place,
            &format!("insert x {{ {} }}", child_declaration(child, "leaf")),
            "x",
        );
        let source = format!("{}{source}", base());
        let permitted = ["note", "hit", "message", "audio"].contains(child);
        checked += 1;
        match (validate(&source), permitted) {
            (Ok(()), true) => {}
            (Err(error), false) if error.object_path.starts_with(&path) => {}
            (result, _) => failures.push(format!("insert > {child} alone: {result:?}")),
        }
    }
    let empty_insert = format!(
        "{}place host {{ pattern = &cell; track = &lead; at = 0q; insert x {{}} }}\n",
        base()
    );
    if validate(&empty_insert).is_ok() {
        failures.push("an empty insert was accepted".into());
    }
    // The project as a parent, and every kind at the top level.
    for child in CHILD_KINDS {
        let source = base().replace(
            "output = &mix:out; }",
            &format!(
                "output = &mix:out; {} }}",
                child_declaration(child, "inner")
            ),
        );
        checked += 1;
        match validate(&source) {
            Err(error) if error.object_path.starts_with(&["song".to_owned()]) => {}
            result => failures.push(format!("project > {child}: {result:?}")),
        }
        let top_level = ![
            "note",
            "hit",
            "message",
            "use",
            "expression",
            "override",
            "insert",
            "widget",
            "project",
        ]
        .contains(child);
        let source = format!("{}{}\n", base(), child_declaration(child, "top"));
        checked += 1;
        match (validate(&source), top_level) {
            (Ok(()), true) => {}
            (Err(error), false)
                if error.object_path.first().map(String::as_str) == Some("top")
                    || *child == "project" => {}
            (result, _) => failures.push(format!("top level {child}: {result:?}")),
        }
    }
    assert!(checked > 500, "{checked}");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn unknown_kinds_are_refused_with_their_code() {
    let source = format!("{}widget w {{}}\n", base());
    let error = validate(&source).unwrap_err();
    assert_eq!(
        (error.code.as_str(), error.object_path.as_slice()),
        ("E_UNKNOWN_KIND", ["w".to_owned()].as_slice())
    );
}

#[test]
fn duplicate_fields_and_ids_are_refused_for_every_kind_and_container() {
    let codes = |source: &str| -> Vec<DiagnosticCode> {
        maac::parse(source)
            .expect_err("duplicates must be refused")
            .iter()
            .map(|d| d.code)
            .collect()
    };
    for kind in KINDS {
        let field = kind.required.first().copied().unwrap_or("label");
        let body = if kind.required.is_empty() {
            format!(r#"{} label = "a"; label = "b";"#, kind.minimal)
        } else {
            format!("{} {}", kind.minimal, kind.minimal)
        };
        let (source, _) = source_for(kind, &body);
        assert!(
            codes(&source).contains(&DiagnosticCode::DuplicateField),
            "{} duplicate {field}",
            kind.name
        );
    }
    let top_level: Vec<&Kind> = KINDS
        .iter()
        .filter(|kind| kind.location == Location::TopLevel)
        .collect();
    for first in &top_level {
        for second in &top_level {
            let source = format!(
                "{}{} dup {{ {} }}\n{} dup {{ {} }}\n",
                base(),
                first.kind,
                first.minimal,
                second.kind,
                second.minimal
            );
            assert!(
                codes(&source).contains(&DiagnosticCode::DuplicateId),
                "{} and {} share an ID",
                first.name,
                second.name
            );
        }
    }
    for container in KINDS.iter().filter(|kind| !kind.children.is_empty()) {
        let child = container.children[0];
        let body = format!(
            "{} {} {} {}",
            container.minimal,
            child_declaration(child, "twin"),
            child_declaration(child, "twin"),
            ""
        );
        let (source, _) = source_for(container, &body);
        assert!(
            codes(&source).contains(&DiagnosticCode::DuplicateId),
            "{} twin children",
            container.name
        );
        let collision = format!(
            "{} twin = 1; {}",
            container.minimal,
            child_declaration(child, "twin")
        );
        let (source, _) = source_for(container, &collision);
        assert!(
            codes(&source).contains(&DiagnosticCode::DuplicateId),
            "{} field/child collision",
            container.name
        );
    }
}

/// §26: an `extension` needs its schema adapter, so its inventory is checked
/// through bundle compilation with the production extension.
#[test]
fn extension_fields_and_children_follow_the_inventory() {
    use maac::bundle::{sha256_digest, SourceBundle};
    let schema = maac::production_data::SCHEMA_BYTES;
    let check = |extension_body: &str| {
        let source = format!(
            "maac 1;\n\
             project song {{ score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &out:out; requires = [\"maac.production/1\"]; }}\n\
             tempo clock {{ points = [(0q, 120bpm, step)]; }}\n\
             meter metre {{ points = [(0q, 4, 4)]; }}\n\
             node out {{ type = \"core.sum/1\"; config = {{ channels = 1; }}; }}\n\
             asset schema {{ kind = descriptor; path = \"production.schema.json\"; hash = \"{}\"; }}\n\
             extension deliveries {{ {extension_body} }}\n",
            sha256_digest(schema)
        );
        let mut bundle = SourceBundle::new("main.maac", source);
        bundle
            .assets
            .insert("production.schema.json".into(), schema.to_vec());
        maac::compiler::check_bundle_artifact(&bundle)
    };
    let minimal = "namespace = \"maac.production/1\"; schema = &schema; render_affecting = true; \
                   data = { deliveries = { release = { rate = 48000Hz; resampler = \"maac.src.kaiser/1\"; \
                   targets = { master = { role = master; output = &out:out; encoding = wav_f32le; dither = { type = none; }; }; }; }; }; };";
    check(minimal).unwrap_or_else(|e| panic!("{e:?}"));
    check(&format!("{minimal} label = \"release\";")).unwrap_or_else(|e| panic!("{e:?}"));
    let codes = |body: &str| -> Vec<DiagnosticCode> {
        check(body)
            .expect_err("extension must be refused")
            .iter()
            .map(|d| d.code)
            .collect()
    };
    assert!(codes(&format!("{minimal} bogus = 1;")).contains(&DiagnosticCode::UnknownField));
    assert!(!codes(&format!("{minimal} label = 1;")).is_empty());
    for field in ["namespace", "schema", "render_affecting", "data"] {
        assert!(
            !codes(&without(minimal, field)).is_empty(),
            "without {field}"
        );
    }
    for child in CHILD_KINDS {
        assert!(
            !codes(&format!("{minimal} {}", child_declaration(child, "inner"))).is_empty(),
            "extension > {child}"
        );
    }
}
