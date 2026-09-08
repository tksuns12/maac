use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle;
use maac::graph::{GraphProcessor, InstrumentProgram};
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use std::collections::BTreeMap;
use std::sync::Arc;

fn source(channels: u8, shared: bool) -> String {
    let (pan, voice_output) = if channels == 2 {
        (
            r#"node pan { type = "synth.pan/1"; params = { pan = -1/2; }; }
connect p { from = &amp:out; to = &pan:in; }"#,
            "pan",
        )
    } else {
        ("", "amp")
    };
    let filter = format!(
        r#"node hp {{ type = "synth.highpass/1"; config = {{ channels = {channels}; }}; }}"#
    );
    let (voice_body, output, shared_body, stage) = if shared {
        (
            String::new(),
            voice_output,
            format!(
                r#"shared s {{ channels = {channels}; output = &hp:out; {filter} connect f {{ from = &input:out; to = &hp:in; }} }}"#
            ),
            "s",
        )
    } else {
        (
            format!(r#"{filter} connect f {{ from = &{voice_output}:out; to = &hp:in; }}"#),
            "hp",
            String::new(),
            "v",
        )
    };
    format!(
        r#"maac 1;
project song {{ score = [0q, 1/6000q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument sound {{ channels = {channels};
voice v {{ channels = {channels}; amplitude = &amp; output = &{output}:out;
node amp {{ type = "synth.adsr/1"; }} {pan} {voice_body} }}
{shared_body}
control cutoff {{ target = &{stage}.hp.params.cutoff; default = 1000Hz; }}
}}
node synth {{ instrument = &sound; }}
pattern phrase {{ length = 1/6000q; note n {{ at = 0q; dur = 1/6000q; pitch = C4; velocity = 1; }} }}
track melody {{ target = &synth:events; }}
place play {{ pattern = &phrase; track = &melody; at = 0q; }}
"#
    )
}

fn program(channels: u8, shared: bool) -> InstrumentProgram {
    compile_bundle(&SourceBundle::new(
        "highpass.maac",
        source(channels, shared),
    ))
    .unwrap()
    .instruments
    .unwrap()
    .programs
    .remove(0)
}

fn runtime(program: &InstrumentProgram) -> InstrumentRuntime {
    InstrumentRuntime::new(
        Arc::new(CompiledInstrument::compile(program, &BTreeMap::new()).unwrap()),
        4,
        48000.0,
        &BTreeMap::new(),
    )
    .unwrap()
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 2e-14, "{actual} != {expected}");
}
fn coefficient(cutoff: f64) -> f64 {
    (-std::f64::consts::TAU * cutoff / 48000.0).exp()
}

#[test]
fn dc_response_matches_analytic_decay_in_voice_and_shared_stages() {
    let a = coefficient(1000.0);
    for shared in [false, true] {
        for channels in [1, 2] {
            let mut runtime = runtime(&program(channels, shared));
            runtime.note_on("note", 440.0, 1.0, 0).unwrap();
            for frame in 0..128 {
                let gains = if channels == 1 {
                    vec![1.0]
                } else {
                    vec![
                        (std::f64::consts::PI / 8.0).cos(),
                        (std::f64::consts::PI / 8.0).sin(),
                    ]
                };
                for (&actual, gain) in runtime.render(frame).unwrap().iter().zip(gains) {
                    close(actual, gain * a.powi(frame as i32 + 1));
                }
            }
        }
    }
}

#[test]
fn overlapping_voices_start_with_independent_filter_history() {
    let mut runtime = runtime(&program(1, false));
    let a = coefficient(1000.0);
    runtime.note_on("a", 440.0, 1.0, 0).unwrap();
    close(runtime.render(0).unwrap()[0], a);
    runtime.note_on("b", 440.0, 0.5, 1).unwrap();
    close(runtime.render(1).unwrap()[0], a * a + a * 0.5);
    close(runtime.render(2).unwrap()[0], a * a * a + a * a * 0.5);
}

#[test]
fn highpass_complements_existing_lowpass_exactly() {
    let high = program(1, true);
    let mut low = high.clone();
    low.shared.as_mut().unwrap().nodes[0].processor = GraphProcessor::OnePole { channels: 1 };
    let mut high = runtime(&high);
    let mut low = runtime(&low);
    for runtime in [&mut high, &mut low] {
        runtime.note_on("note", 440.0, 1.0, 0).unwrap();
    }
    for frame in 0..12 {
        if frame == 2 {
            for runtime in [&mut high, &mut low] {
                runtime
                    .update_controls(&[("cutoff".into(), 9000.0)].into_iter().collect())
                    .unwrap();
            }
        }
        if frame == 4 {
            for runtime in [&mut high, &mut low] {
                runtime.note_off("note", frame).unwrap();
                runtime.prune_finished(frame).unwrap();
            }
        }
        assert_eq!(
            high.render(frame).unwrap()[0],
            if frame < 4 { 1.0 } else { 0.0 } - low.render(frame).unwrap()[0]
        );
    }
}

#[test]
fn cutoff_contract_matches_lowpass_and_plan_rejects_bad_bounds_and_modulation_cycles() {
    let high =
        maac::graph::parameter_descriptor(&GraphProcessor::HighPass { channels: 1 }, "cutoff")
            .unwrap();
    let low = maac::graph::parameter_descriptor(&GraphProcessor::OnePole { channels: 1 }, "cutoff")
        .unwrap();
    assert_eq!(high, low);
    let mut program = program(1, false);
    for cutoff in ["0", "24000", "-1"] {
        program
            .voice
            .nodes
            .iter_mut()
            .find(|n| n.id == "hp")
            .unwrap()
            .params
            .insert("cutoff".into(), maac::parse_rational(cutoff).unwrap());
        assert_eq!(program.validate().unwrap_err().code, "E_RANGE");
    }
    program
        .voice
        .nodes
        .iter_mut()
        .find(|n| n.id == "hp")
        .unwrap()
        .params
        .clear();
    program.voice.modulations.push(maac::graph::Modulation {
        id: "feedback".into(),
        from: maac::plan::PortRef::new("hp", "out").unwrap(),
        to: maac::graph::ParameterTarget {
            node: "hp".into(),
            parameter: "cutoff".into(),
        },
        depth: maac::parse_rational("1").unwrap(),
    });
    assert_eq!(program.validate().unwrap_err().code, "E_ALGEBRAIC_LOOP");
}

#[test]
fn impulse_tail_survives_note_retirement_and_stereo_channels_are_independent() {
    let a = coefficient(1000.0);
    for channels in [1, 2] {
        let mut runtime = runtime(&program(channels, true));
        runtime.note_on("note", 440.0, 1.0, 0).unwrap();
        let amplitudes: Vec<f64> = if channels == 1 {
            vec![1.0]
        } else {
            vec![
                (std::f64::consts::PI / 8.0).cos(),
                (std::f64::consts::PI / 8.0).sin(),
            ]
        };
        let first = runtime.render(0).unwrap().to_vec();
        for (&actual, &amplitude) in first.iter().zip(&amplitudes) {
            close(actual, amplitude * a);
        }
        runtime.note_off("note", 1).unwrap();
        runtime.prune_finished(1).unwrap();
        assert_eq!(runtime.active_voice_count(), 0);
        for frame in 1..16 {
            for (&actual, &amplitude) in runtime.render(frame).unwrap().iter().zip(&amplitudes) {
                close(actual, -amplitude * (1.0 - a) * a.powi(frame as i32));
            }
        }
        runtime.reset(&BTreeMap::new()).unwrap();
        runtime.note_on("note", 440.0, 1.0, 0).unwrap();
        assert_eq!(runtime.render(0).unwrap(), first);
    }
}

#[test]
fn cutoff_updates_immediately_without_resetting_filter_history() {
    for shared in [false, true] {
        let mut runtime = runtime(&program(1, shared));
        runtime.note_on("note", 440.0, 1.0, 0).unwrap();
        close(runtime.render(0).unwrap()[0], coefficient(1000.0));
        runtime
            .update_controls(&[("cutoff".into(), 8000.0)].into_iter().collect())
            .unwrap();
        close(
            runtime.render(1).unwrap()[0],
            coefficient(1000.0) * coefficient(8000.0),
        );
        for invalid in [0.0, 24000.0, f64::NAN] {
            assert!(runtime
                .update_controls(&[("cutoff".into(), invalid)].into_iter().collect())
                .is_err());
        }
    }
}

#[test]
fn source_plan_roundtrip_and_repeated_render_preserve_exact_samples() {
    for channels in [1, 2] {
        let automated = source(channels, true)
            + r#"
curve brightness { clock = seconds; points = [(0s, 1000Hz, step), (1/48000s, 8000Hz, step)]; }
automation change { target = &synth.params.cutoff; curve = &brightness; at = 0s; }
"#;
        let plan = compile_bundle(&SourceBundle::new("highpass.maac", automated)).unwrap();
        let bytes = serde_json::to_vec(&plan).unwrap();
        let loaded = maac::load_plan(bytes.as_slice()).unwrap();
        let collect = |engine: &mut maac::dsp::DspEngine<'_>| {
            let mut result = Vec::new();
            engine
                .render(|frame| {
                    result.push(frame.to_vec());
                    Ok(())
                })
                .unwrap();
            result
        };
        let mut engine = maac::dsp::DspEngine::new(&plan).unwrap();
        let samples = collect(&mut engine);
        assert_eq!(samples.len(), 4);
        assert!(samples[0][0] > samples[1][0]);
        close(samples[1][0], samples[0][0] * coefficient(8000.0));
        assert_eq!(samples, collect(&mut engine));
        assert_eq!(
            samples,
            collect(&mut maac::dsp::DspEngine::new(&loaded).unwrap())
        );
    }
}

#[test]
fn source_rejects_invalid_channels_cutoff_config_and_inputs() {
    let valid = source(1, false);
    for config in [
        "",
        "config = {};",
        "config = { channels = 0; };",
        "config = { channels = 3; };",
        "config = { channels = 1/2; };",
        "config = { channels = 1; other = 0; };",
    ] {
        assert!(
            compile_bundle(&SourceBundle::new(
                "highpass.maac",
                valid.replace("config = { channels = 1; };", config)
            ))
            .is_err(),
            "{config}"
        );
    }
    for cutoff in ["0Hz", "24000Hz", "-1Hz", "1000", "24001Hz"] {
        assert!(
            compile_bundle(&SourceBundle::new(
                "highpass.maac",
                valid.replace("default = 1000Hz", &format!("default = {cutoff}"))
            ))
            .is_err(),
            "{cutoff}"
        );
    }
    for invalid in [
        valid.replace("connect f { from = &amp:out; to = &hp:in; }", ""),
        valid.replace(
            "from = &amp:out; to = &hp:in",
            "from = &hp:out; to = &hp:in",
        ),
        valid.replace("config = { channels = 1; };", "config = { channels = 2; };"),
        valid.replace("to = &hp:in", "to = &hp:wrong"),
    ] {
        assert!(compile_bundle(&SourceBundle::new("highpass.maac", invalid)).is_err());
    }
}

#[test]
fn strict_wire_requires_channels_and_rejects_unknown_fields_and_versions() {
    for valid in [
        r#"{"kind":"synth.highpass/1","channels":1}"#,
        r#"{"kind":"synth.highpass/1","channels":2}"#,
    ] {
        let processor: GraphProcessor = serde_json::from_str(valid).unwrap();
        assert_eq!(
            serde_json::to_value(&processor).unwrap(),
            serde_json::from_str::<serde_json::Value>(valid).unwrap()
        );
    }
    for invalid in [
        r#"{"kind":"synth.highpass/1"}"#,
        r#"{"kind":"synth.highpass/1","channels":1,"cutoff":1000}"#,
        r#"{"kind":"synth.highpass/1","channels":1.5}"#,
        r#"{"kind":"synth.highpass/1","channels":256}"#,
        r#"{"kind":"synth.highpass/2","channels":1}"#,
    ] {
        assert!(
            serde_json::from_str::<GraphProcessor>(invalid).is_err(),
            "{invalid}"
        );
    }
    let plan = compile_bundle(&SourceBundle::new("highpass.maac", source(1, false))).unwrap();
    for channels in [0, 3] {
        let mut wire = serde_json::to_value(&plan).unwrap();
        let nodes = wire["instruments"]["programs"][0]["voice"]["nodes"]
            .as_array_mut()
            .unwrap();
        nodes.iter_mut().find(|node| node["id"] == "hp").unwrap()["processor"]["channels"] =
            channels.into();
        assert!(maac::load_plan(serde_json::to_vec(&wire).unwrap().as_slice()).is_err());
    }
}
