//! §23 locations for the recognized extensions' source diagnostics (P21).
//!
//! Each case changes one thing in a valid `maac.production/1` or
//! `maac.takes/1`/`maac.takes/2` composition and requires `check_bundle_artifact`'s
//! first diagnostic to have the expected code, the authored object path, the
//! field path inside that object, and a span over the offending text. Before
//! P21 these failures named objects the author never wrote, such as
//! `production`, `takes` or `project`.

use std::collections::BTreeMap;

use maac::bundle::sha256_digest;
use maac::compiler::check_bundle_artifact;
use maac::SourceBundle;

struct Case {
    name: &'static str,
    /// One or more edits, separated by ` && ` in both `from` and `to`.
    from: &'static str,
    to: &'static str,
    code: &'static str,
    object: &'static [&'static str],
    field: &'static [&'static str],
    /// The reported span's text; a trailing `...` makes it a prefix.
    text: &'static str,
}

fn run(base: &str, assets: &BTreeMap<String, Vec<u8>>, cases: &[Case]) {
    let bundle = |source: &str| {
        let mut bundle = SourceBundle::new("main.maac", source);
        bundle.assets = assets.clone();
        bundle
    };
    check_bundle_artifact(&bundle(base)).unwrap_or_else(|e| panic!("the base must be valid: {e}"));
    let mut failures = Vec::new();
    for case in cases {
        let mut source = base.to_owned();
        for (from, to) in case.from.split(" && ").zip(case.to.split(" && ")) {
            assert!(source.contains(from), "{}: base lacks {from:?}", case.name);
            source = source.replacen(from, to, 1);
        }
        let diagnostics = match check_bundle_artifact(&bundle(&source)) {
            Ok(()) => {
                failures.push(format!("{}: accepted", case.name));
                continue;
            }
            Err(diagnostics) => diagnostics,
        };
        let first = diagnostics.iter().next().unwrap();
        let text = first.span.map(|span| &source[span.start..span.end]);
        let text_matches = text.is_some_and(|text| match case.text.strip_suffix("...") {
            Some(prefix) => text.starts_with(prefix),
            None => text == case.text,
        });
        if first.code.as_str() != case.code
            || first.object_path != case.object
            || first.field_path != case.field
            || !text_matches
        {
            failures.push(format!(
                "{}: got {} {:?} {:?} {:?} ({}); expected {} {:?} {:?} {:?}",
                case.name,
                first.code.as_str(),
                first.object_path,
                first.field_path,
                text,
                first.message,
                case.code,
                case.object,
                case.field,
                case.text,
            ));
        }
        // Every reported object path names objects the author wrote.
        let document = maac::parse(&source).unwrap_or_else(|e| panic!("{}: {e}", case.name));
        let mut objects = &document.objects;
        for id in &first.object_path {
            match objects.get(id) {
                Some(object) => objects = &object.children,
                None => {
                    failures.push(format!("{}: unauthored object `{id}`", case.name));
                    break;
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

fn production() -> (String, BTreeMap<String, Vec<u8>>) {
    let schema = maac::production_data::SCHEMA_BYTES;
    let source = format!(
        r#"maac 1;
project p {{ score = [0q, 1/100q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &master:out; requires = ["maac.production/1"]; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
asset schema {{ kind = descriptor; path = "production.schema.json"; hash = "{hash}"; }}
node sine {{ type = "core.sine/1"; params = {{ attack = 0s; release = 0s; level = 0.2; }}; }}
node master {{ type = "core.pan/1"; params = {{ pan = 0; }}; }}
connect to_master {{ from = &sine:out; to = &master:in; }}
pattern phrase {{ length = 1/100q; note n {{ at = 0q; dur = 1/100q; pitch = A4; }} }}
track melody {{ target = &sine:events; }}
place play {{ pattern = &phrase; track = &melody; at = 0q; }}
extension deliveries {{ namespace = "maac.production/1"; schema = &schema; render_affecting = true; data = {{ deliveries = {{ release = {{ rate = 48000Hz; resampler = "maac.src.kaiser/1"; targets = {{
  main = {{ role = master; output = &master:out; encoding = wav_pcm24le; dither = {{ type = tpdf; seed = 7; }}; limits = {{ integrated_loudness = {{ unit = LUFS; min = -16; max = -12; }}; sample_peak = {{ unit = dBFS; max = -1; }}; true_peak = {{ unit = dBTP; max = -1; }}; }}; }};
  dry = {{ role = stem; output = &sine:out; encoding = wav_f32le; dither = {{ type = none; }}; }};
}}; }}; }}; }}; }}
"#,
        hash = sha256_digest(schema)
    );
    let assets = BTreeMap::from([("production.schema.json".into(), schema.to_vec())]);
    (source, assets)
}

#[test]
fn production_diagnostics_are_located_in_the_source() {
    let (base, assets) = production();
    let cases = [
        Case {
            name: "second production extension",
            from: "extension deliveries {",
            to: "extension another { namespace = \"maac.production/1\"; schema = &schema; render_affecting = true; data = {}; }\nextension deliveries {",
            code: "E_CAPABILITY",
            object: &["deliveries"],
            field: &[],
            text: "extension",
        },
        Case {
            name: "render_affecting false",
            from: "render_affecting = true",
            to: "render_affecting = false",
            code: "E_CAPABILITY",
            object: &["deliveries"],
            field: &["render_affecting"],
            text: "false",
        },
        Case {
            name: "two projects",
            from: "tempo clock",
            to: "project q { score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; }\ntempo clock",
            code: "E_REFERENCE",
            object: &["deliveries"],
            field: &[],
            text: "extension",
        },
        Case {
            name: "engine rate",
            from: "rate = 48000Hz; tempo",
            to: "rate = 44100Hz; tempo",
            code: "E_CAPABILITY",
            object: &["p"],
            field: &["rate"],
            text: "44100Hz",
        },
        Case {
            name: "project output without a port",
            from: "output = &master:out; requires",
            to: "output = &master; requires",
            code: "E_REFERENCE",
            object: &["p"],
            field: &["output"],
            text: "&master",
        },
        Case {
            name: "schema is not a reference",
            from: "schema = &schema;",
            to: "schema = \"schema\";",
            code: "E_REFERENCE",
            object: &["deliveries"],
            field: &["schema"],
            text: "\"schema\"",
        },
        Case {
            name: "schema names a port",
            from: "schema = &schema;",
            to: "schema = &schema:out;",
            code: "E_REFERENCE",
            object: &["deliveries"],
            field: &["schema"],
            text: "&schema:out",
        },
        Case {
            name: "schema descriptor missing",
            from: "schema = &schema;",
            to: "schema = &nothing;",
            code: "E_REFERENCE",
            object: &["deliveries"],
            field: &["schema"],
            text: "&nothing",
        },
        Case {
            name: "schema is not an asset",
            from: "schema = &schema;",
            to: "schema = &sine;",
            code: "E_ASSET",
            object: &["deliveries"],
            field: &["schema"],
            text: "&sine",
        },
        Case {
            name: "schema asset kind",
            from: "kind = descriptor;",
            to: "kind = audio;",
            code: "E_ASSET",
            object: &["schema"],
            field: &["kind"],
            text: "audio",
        },
        Case {
            name: "unknown extension field",
            from: "render_affecting = true;",
            to: "render_affecting = true; bogus = 1;",
            code: "E_UNKNOWN_FIELD",
            object: &["deliveries"],
            field: &["bogus"],
            text: "bogus",
        },
        Case {
            name: "extension label bound",
            from: "render_affecting = true;",
            to: "render_affecting = true; label = \"LONG_LABEL\";",
            code: "E_RESOURCE_LIMIT",
            object: &["deliveries"],
            field: &["label"],
            text: "\"...",
        },
        Case {
            name: "unknown data field",
            from: "data = { deliveries",
            to: "data = { extra = 1; deliveries",
            code: "E_UNKNOWN_FIELD",
            object: &["deliveries"],
            field: &["data", "extra"],
            text: "extra",
        },
        Case {
            name: "delivery rate unit",
            from: "rate = 48000Hz; resampler",
            to: "rate = 48000; resampler",
            code: "E_UNIT",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "rate"],
            text: "48000",
        },
        Case {
            name: "delivery rate not an integer",
            from: "rate = 48000Hz; resampler",
            to: "rate = 44100.5Hz; resampler",
            code: "E_RANGE",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "rate"],
            text: "44100.5Hz",
        },
        Case {
            name: "delivery rate unsupported",
            from: "rate = 48000Hz; resampler",
            to: "rate = 22050Hz; resampler",
            code: "E_CAPABILITY",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "rate"],
            text: "22050Hz",
        },
        Case {
            name: "resampler unsupported",
            from: "\"maac.src.kaiser/1\"",
            to: "\"other/1\"",
            code: "E_CAPABILITY",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "resampler"],
            text: "\"other/1\"",
        },
        Case {
            name: "resampler missing",
            from: "resampler = \"maac.src.kaiser/1\";",
            to: "",
            code: "E_REFERENCE",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "resampler"],
            text: "{ rate...",
        },
        Case {
            name: "two masters",
            from: "role = stem; output = &sine:out",
            to: "role = master; output = &master:out",
            code: "E_RANGE",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets"],
            text: "{...",
        },
        Case {
            name: "target role",
            from: "role = stem",
            to: "role = boss",
            code: "E_RANGE",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets", "dry", "role"],
            text: "boss",
        },
        Case {
            name: "target encoding",
            from: "encoding = wav_pcm24le",
            to: "encoding = mp3",
            code: "E_CAPABILITY",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets", "main", "encoding"],
            text: "mp3",
        },
        Case {
            name: "dither type",
            from: "type = none",
            to: "type = shaped",
            code: "E_CAPABILITY",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets", "dry", "dither", "type"],
            text: "shaped",
        },
        Case {
            name: "dither seed",
            from: "seed = 7",
            to: "seed = -7",
            code: "E_RANGE",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets", "main", "dither", "seed"],
            text: "-7",
        },
        Case {
            name: "dither seed missing",
            from: "type = tpdf; seed = 7;",
            to: "type = tpdf;",
            code: "E_REFERENCE",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets", "main", "dither", "seed"],
            text: "{ type = tpdf; }",
        },
        Case {
            name: "seed without tpdf",
            from: "type = none;",
            to: "type = none; seed = 1;",
            code: "E_UNKNOWN_FIELD",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets", "dry", "dither", "seed"],
            text: "seed",
        },
        Case {
            name: "float dither",
            from: "dither = { type = none; }",
            to: "dither = { type = tpdf; seed = 1; }",
            code: "E_CAPABILITY",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets", "dry", "dither"],
            text: "{ type = tpdf; seed = 1; }",
        },
        Case {
            name: "unknown target field",
            from: "role = stem;",
            to: "role = stem; gain = 1;",
            code: "E_UNKNOWN_FIELD",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets", "dry", "gain"],
            text: "gain",
        },
        Case {
            name: "target source missing",
            from: "output = &sine:out; encoding",
            to: "output = &nothing:out; encoding",
            code: "E_REFERENCE",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets", "dry", "output"],
            text: "&nothing:out",
        },
        Case {
            name: "master is not the project output",
            from: "output = &master:out; encoding",
            to: "output = &sine:out; encoding",
            code: "E_REFERENCE",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets", "main", "output"],
            text: "&sine:out",
        },
        Case {
            name: "target output without a port",
            from: "output = &sine:out; encoding",
            to: "output = &sine; encoding",
            code: "E_REFERENCE",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets", "dry", "output"],
            text: "&sine",
        },
        Case {
            name: "target is not an audio output",
            from: "output = &sine:out; encoding",
            to: "output = &sine:events; encoding",
            code: "E_PORT_TYPE",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets", "dry", "output"],
            text: "&sine:events",
        },
        Case {
            name: "unknown limit",
            from: "limits = { integrated_loudness",
            to: "limits = { range = 1; integrated_loudness",
            code: "E_UNKNOWN_FIELD",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets", "main", "limits", "range"],
            text: "range",
        },
        Case {
            name: "loudness unit",
            from: "unit = LUFS",
            to: "unit = dBFS",
            code: "E_UNIT",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets", "main", "limits", "integrated_loudness", "unit"],
            text: "dBFS",
        },
        Case {
            name: "loudness without bounds",
            from: "unit = LUFS; min = -16; max = -12;",
            to: "unit = LUFS;",
            code: "E_UNIT",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets", "main", "limits", "integrated_loudness"],
            text: "{ unit = LUFS; }",
        },
        Case {
            name: "loudness bounds reversed",
            from: "min = -16; max = -12;",
            to: "min = -12; max = -16;",
            code: "E_RANGE",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets", "main", "limits", "integrated_loudness", "max"],
            text: "-16",
        },
        Case {
            name: "peak unit",
            from: "unit = dBFS",
            to: "unit = dBTP",
            code: "E_UNIT",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets", "main", "limits", "sample_peak", "unit"],
            text: "dBTP",
        },
        Case {
            name: "unknown logarithmic unit",
            from: "unit = dBTP",
            to: "unit = dB",
            code: "E_UNIT",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets", "main", "limits", "true_peak", "unit"],
            text: "dB",
        },
        Case {
            name: "limit with a unit",
            from: "unit = dBFS; max = -1;",
            to: "unit = dBFS; max = -1Hz;",
            code: "E_UNIT",
            object: &["deliveries"],
            field: &["data", "deliveries", "release", "targets", "main", "limits", "sample_peak", "max"],
            text: "-1Hz",
        },
    ];
    let long_label = format!("\"{}\"", "x".repeat(4097));
    let cases: Vec<Case> = cases
        .into_iter()
        .map(|case| match case.name {
            "extension label bound" => Case {
                to: leak(case.to.replace("\"LONG_LABEL\"", &long_label)),
                ..case
            },
            _ => case,
        })
        .collect();
    run(&base, &assets, &cases);
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn leak_path(path: &[&str]) -> &'static [&'static str] {
    Box::leak(
        path.iter()
            .map(|segment| leak((*segment).to_owned()))
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    )
}

fn takes() -> (String, BTreeMap<String, Vec<u8>>) {
    let schema = maac::takes::SCHEMA_BYTES;
    let source = format!(
        r#"maac 1;
project p {{ score=[0q,1q]; rate=48000Hz; tempo=&t; meter=&m; output=&mix:out; requires=["maac.takes/1"]; }}
tempo t {{ points=[(0q,120bpm,step)]; }}
meter m {{ points=[(0q,4,4)]; }}
asset schema {{ kind=descriptor; path="takes.schema.json"; hash="{schema}"; }}
asset first {{ kind=audio; path="first.pcm"; hash="{first}"; format="pcm_f32le_interleaved/1"; rate=48000Hz; channels=1; frames=8; }}
asset second {{ kind=audio; path="second.pcm"; hash="{second}"; format="pcm_f32le_interleaved/1"; rate=48kHz; channels=1; frames=6; }}
audio early {{ asset=&first; at=0s; source=[1frame,3frame]; mode=rate; }}
audio late {{ asset=&second; at=1/24000s; source=[4frame,6frame]; mode=rate; gain=1/2; fade_in=0ms; }}
node mix {{ type="core.sum/1"; config={{channels=1;}}; }}
connect early_mix {{ from=&early:out; to=&mix:in; }}
connect late_mix {{ from=&late:out; to=&mix:in; }}
extension comp {{ namespace="maac.takes/1"; schema=&schema; render_affecting=true; data={{ groups={{ vocals={{ origin=0s; takes={{
    first_take={{asset=&first; source_origin=1frame;}};
    second_take={{asset=&second; source_origin=2frame;}};
}}; regions={{
    early_region={{take=first_take; range=[0frame,2frame]; clip=&early;}};
    late_region={{take=second_take; range=[2frame,4frame]; clip=&late;}};
}}; }}; }}; }}; }}
"#,
        schema = sha256_digest(schema),
        first = sha256_digest(&[0; 32]),
        second = sha256_digest(&[0; 24]),
    );
    let assets = BTreeMap::from([
        ("takes.schema.json".into(), schema.to_vec()),
        ("first.pcm".into(), vec![0; 32]),
        ("second.pcm".into(), vec![0; 24]),
    ]);
    (source, assets)
}

const VOCALS: &[&str] = &["data", "groups", "vocals"];

fn at(base: &'static [&'static str], rest: &[&str]) -> &'static [&'static str] {
    leak_path(&[base, rest].concat())
}

#[test]
fn take_diagnostics_are_located_in_the_source() {
    let (base, assets) = takes();
    let cases = [
        Case {
            name: "render_affecting false",
            from: "render_affecting=true",
            to: "render_affecting=false",
            code: "E_CAPABILITY",
            object: &["comp"],
            field: &["render_affecting"],
            text: "false",
        },
        Case {
            name: "second take extension",
            from: "extension comp {",
            to: "extension another { namespace=\"maac.takes/1\"; schema=&schema; render_affecting=true; data={}; }\nextension comp {",
            code: "E_CAPABILITY",
            object: &["comp"],
            field: &[],
            text: "extension",
        },
        Case {
            name: "schema is not an asset",
            from: "schema=&schema;",
            to: "schema=&mix;",
            code: "E_ASSET",
            object: &["comp"],
            field: &["schema"],
            text: "&mix",
        },
        Case {
            name: "schema asset kind",
            from: "kind=descriptor;",
            to: "kind=blob;",
            code: "E_ASSET",
            object: &["schema"],
            field: &["kind"],
            text: "blob",
        },
        Case {
            name: "unknown data field",
            from: "data={ groups",
            to: "data={ extra=1; groups",
            code: "E_UNKNOWN_FIELD",
            object: &["comp"],
            field: &["data", "extra"],
            text: "extra",
        },
        Case {
            name: "negative origin",
            from: "origin=0s",
            to: "origin=-1s",
            code: "E_RANGE",
            object: &["comp"],
            field: at(VOCALS, &["origin"]),
            text: "-1s",
        },
        Case {
            name: "origin unit",
            from: "origin=0s",
            to: "origin=0q",
            code: "E_UNIT",
            object: &["comp"],
            field: at(VOCALS, &["origin"]),
            text: "0q",
        },
        Case {
            name: "duplicate member asset",
            from: "second_take={asset=&second",
            to: "second_take={asset=&first",
            code: "E_REFERENCE",
            object: &["comp"],
            field: at(VOCALS, &["takes", "second_take", "asset"]),
            text: "&first",
        },
        Case {
            name: "member asset missing",
            from: "second_take={asset=&second",
            to: "second_take={asset=&nothing",
            code: "E_REFERENCE",
            object: &["comp"],
            field: at(VOCALS, &["takes", "second_take", "asset"]),
            text: "&nothing",
        },
        Case {
            name: "member asset format",
            from: "rate=48kHz; channels=1; frames=6;",
            to: "rate=48kHz; channels=3; frames=6;",
            code: "E_CAPABILITY",
            object: &["second"],
            field: &["channels"],
            text: "3",
        },
        Case {
            name: "member asset rate unit",
            from: "rate=48kHz;",
            to: "rate=48000;",
            code: "E_UNIT",
            object: &["second"],
            field: &["rate"],
            text: "48000",
        },
        Case {
            name: "member rates differ",
            from: "rate=48kHz;",
            to: "rate=44100Hz;",
            code: "E_ASSET",
            object: &["comp"],
            field: at(VOCALS, &["takes", "second_take", "asset"]),
            text: "&second",
        },
        Case {
            name: "source origin past the end",
            from: "source_origin=1frame",
            to: "source_origin=8frame",
            code: "E_RANGE",
            object: &["comp"],
            field: at(VOCALS, &["takes", "first_take", "source_origin"]),
            text: "8frame",
        },
        Case {
            name: "source origin unit",
            from: "source_origin=1frame",
            to: "source_origin=1",
            code: "E_UNIT",
            object: &["comp"],
            field: at(VOCALS, &["takes", "first_take", "source_origin"]),
            text: "1",
        },
        Case {
            name: "region take",
            from: "take=second_take",
            to: "take=third_take",
            code: "E_REFERENCE",
            object: &["comp"],
            field: at(VOCALS, &["regions", "late_region", "take"]),
            text: "third_take",
        },
        Case {
            name: "region range order",
            from: "range=[2frame,4frame]",
            to: "range=[4frame,2frame]",
            code: "E_RANGE",
            object: &["comp"],
            field: at(VOCALS, &["regions", "late_region", "range"]),
            text: "[4frame,2frame]",
        },
        Case {
            name: "region past the take",
            from: "range=[2frame,4frame]",
            to: "range=[2frame,5frame]",
            code: "E_RANGE",
            object: &["comp"],
            field: at(VOCALS, &["regions", "late_region", "range"]),
            text: "[2frame,5frame]",
        },
        Case {
            name: "shared clip",
            from: "clip=&late",
            to: "clip=&early",
            code: "E_REFERENCE",
            object: &["comp"],
            field: at(VOCALS, &["regions", "late_region", "clip"]),
            text: "&early",
        },
        Case {
            name: "clip is not audio",
            from: "clip=&late",
            to: "clip=&mix",
            code: "E_REFERENCE",
            object: &["comp"],
            field: at(VOCALS, &["regions", "late_region", "clip"]),
            text: "&mix",
        },
        Case {
            name: "clip asset",
            from: "audio late { asset=&second",
            to: "audio late { asset=&first",
            code: "E_REFERENCE",
            object: &["late"],
            field: &["asset"],
            text: "&first",
        },
        Case {
            name: "clip speed",
            from: "mode=rate; gain=1/2;",
            to: "mode=rate; speed=2; gain=1/2;",
            code: "E_CAPABILITY",
            object: &["late"],
            field: &["speed"],
            text: "2",
        },
        Case {
            name: "clip source",
            from: "source=[4frame,6frame]",
            to: "source=[3frame,5frame]",
            code: "E_RANGE",
            object: &["late"],
            field: &["source"],
            text: "[3frame,5frame]",
        },
        Case {
            name: "clip position",
            from: "at=1/24000s",
            to: "at=1/12000s",
            code: "E_RANGE",
            object: &["late"],
            field: &["at"],
            text: "1/12000s",
        },
        Case {
            name: "regions overlap",
            from: "range=[0frame,2frame] && source=[1frame,3frame]",
            to: "range=[0frame,3frame] && source=[1frame,4frame]",
            code: "E_RANGE",
            object: &["comp"],
            field: at(VOCALS, &["regions", "late_region", "range"]),
            text: "[2frame,4frame]",
        },
        Case {
            name: "unknown region field",
            from: "clip=&late;",
            to: "clip=&late; gain=1;",
            code: "E_UNKNOWN_FIELD",
            object: &["comp"],
            field: at(VOCALS, &["regions", "late_region", "gain"]),
            text: "gain",
        },
        Case {
            name: "required capability without an extension",
            from: "extension comp {",
            to: "extension_removed comp {",
            code: "E_CAPABILITY",
            object: &["p"],
            field: &["requires", "0"],
            text: "\"maac.takes/1\"",
        },
    ];
    run(&base, &assets, &cases);
}

#[test]
fn grouped_take_diagnostics_are_located_in_the_source() {
    let (base, mut assets) = takes();
    let schema = maac::takes::SCHEMA_V2_BYTES;
    let base = base
        .replace("maac.takes/1", "maac.takes/2")
        .replace("takes.schema.json", "takes-v2.schema.json")
        .replace(&sha256_digest(maac::takes::SCHEMA_BYTES), &sha256_digest(schema))
        .replace("first_take={asset=&first; source_origin=1frame;}", "first_take={lanes={close={asset=&first; source_origin=1frame;};room={asset=&third; source_origin=2frame;};};}")
        .replace("second_take={asset=&second; source_origin=2frame;}", "second_take={lanes={close={asset=&second; source_origin=2frame;};room={asset=&fourth; source_origin=3frame;};};}")
        .replace("clip=&early", "clips={close=&early;room=&early_room;}")
        .replace("clip=&late", "clips={close=&late;room=&late_room;}")
        + &format!(
            r#"
asset third {{ kind=audio; path="room.pcm"; hash="{hash}"; format="pcm_f32le_interleaved/1"; rate=48000Hz; channels=2; frames=8; }}
asset fourth {{ kind=audio; path="room.pcm"; hash="{hash}"; format="pcm_f32le_interleaved/1"; rate=48000Hz; channels=2; frames=8; }}
audio early_room {{ asset=&third; at=0s; source=[2frame,4frame]; mode=rate; }}
audio late_room {{ asset=&fourth; at=1/24000s; source=[5frame,7frame]; mode=rate; }}
"#,
            hash = sha256_digest(&[0; 64])
        );
    assets.remove("takes.schema.json");
    assets.insert("takes-v2.schema.json".into(), schema.to_vec());
    assets.insert("room.pcm".into(), vec![0; 64]);
    let cases = [
        Case {
            name: "lanes differ between takes",
            from: "room={asset=&fourth; source_origin=3frame;}",
            to: "ambient={asset=&fourth; source_origin=3frame;}",
            code: "E_REFERENCE",
            object: &["comp"],
            field: at(VOCALS, &["takes", "second_take"]),
            text: "{...",
        },
        Case {
            name: "region misses a lane",
            from: "clips={close=&late;room=&late_room;}",
            to: "clips={close=&late;}",
            code: "E_REFERENCE",
            object: &["comp"],
            field: at(VOCALS, &["regions", "late_region", "clips"]),
            text: "{close=&late;}",
        },
        Case {
            name: "empty lanes",
            from: "lanes={close={asset=&second; source_origin=2frame;};room={asset=&fourth; source_origin=3frame;};}",
            to: "lanes={}",
            code: "E_RESOURCE_LIMIT",
            object: &["comp"],
            field: at(VOCALS, &["takes", "second_take", "lanes"]),
            text: "{}",
        },
    ];
    run(&base, &assets, &cases);
}
