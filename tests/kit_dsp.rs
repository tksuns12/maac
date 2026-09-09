use maac::{dsp::DspEngine, PlanArtifact};
use serde_json::{json, Value};

fn rat(n: i64, d: i64) -> String {
    let r = maac::Rational::new(n.into(), d.into());
    format!("{}/{}", r.numer(), r.denom())
}
fn wire(rate: u32, channels: u8, samples: &[f32]) -> Value {
    let bytes: Vec<u8> = samples.iter().flat_map(|x| x.to_le_bytes()).collect();
    json!({"version":4,"output":{"score_start_q":"0/1","score_end_q":"1/3000","tail_seconds":"1/12000","sample_rate_hz":48000,"channels":channels,"total_frames":12,"output":{"node":"kit","port":"out"}},"tempo":{"points":[{"q":"0/1","bpm":"120/1","shape":"step"}]},"nodes":[{"id":"kit","processor":{"kind":"kit","channels":channels,"voices":2,"samples":[{"key":"key","asset":"sample"}]},"params":{}}],"audio_assets":[{"id":"sample","format":"pcm_f32le_interleaved/1","rate_hz":rate,"channels":channels,"frames":samples.len()/channels as usize,"hash":maac::bundle::sha256_digest(&bytes),"bytes":bytes}],"events":[hit(0,0,"1/1")]})
}
fn hit(id: usize, frame: i64, velocity: &str) -> Value {
    json!({"address":format!("main/h{id}"),"source":{"object":format!("h{id}"),"path":["main",format!("h{id}")]},"target":{"node":"kit","port":"events"},"kind":{"kind":"hit","key":"key","velocity":velocity},"score_on_q":rat(frame,24000),"onset_offset_seconds":"0/1","release_offset_seconds":"0/1","release_velocity":0.0,"on_frame":frame})
}
fn artifact(v: &Value) -> PlanArtifact {
    PlanArtifact::from_json(&serde_json::to_vec(v).unwrap()).unwrap()
}
fn render(p: &PlanArtifact) -> Result<Vec<f64>, maac::dsp::RenderError> {
    let mut out = vec![];
    maac::render_artifact(p, |frame| {
        out.extend_from_slice(frame);
        Ok(())
    })?;
    Ok(out)
}
#[test]
fn equal_unequal_stereo_samples_and_zero_extension() {
    for (rate, channels, samples, expected) in [
        (48000, 1, vec![2., 4.], vec![2., 4., 0., 0., 0.]),
        (24000, 1, vec![2., 4.], vec![2., 3., 4., 2., 0.]),
        (72000, 1, vec![2., 4., 8.], vec![2., 6., 0., 0., 0.]),
        (
            24000,
            2,
            vec![2., -2., 4., -4.],
            vec![2., -2., 3., -3., 4., -4., 2., -2., 0., 0.],
        ),
    ] {
        let audio = render(&artifact(&wire(rate, channels, &samples))).unwrap();
        assert_eq!(&audio[..expected.len()], expected);
        assert!(audio[expected.len()..].iter().all(|x| *x == 0.));
    }
}
#[test]
fn expiry_overlap_zero_assets_and_silent_voice_capacity() {
    let mut v = wire(24000, 1, &[2.]);
    v["nodes"][0]["processor"]["voices"] = json!(1);
    v["events"] = json!([hit(0, 0, "1/1"), hit(1, 2, "1/1")]);
    assert_eq!(&render(&artifact(&v)).unwrap()[..5], &[2., 1., 2., 1., 0.]);
    v["events"] = json!([hit(0, 0, "0/1"), hit(1, 1, "1/1")]);
    assert_eq!(render(&artifact(&v)).unwrap_err().code(), "E_VOICE_LIMIT");
    v["nodes"][0]["processor"]["voices"] = json!(2);
    v["events"] = json!([hit(0, 0, "1/1"), hit(1, 1, "1/1")]);
    assert_eq!(&render(&artifact(&v)).unwrap()[..4], &[2., 3., 1., 0.]);
    let mut v = wire(48000, 1, &[]);
    v["nodes"][0]["processor"]["voices"] = json!(1);
    v["events"] = json!([hit(0, 0, "1/1"), hit(1, 0, "1/1")]);
    assert!(render(&artifact(&v)).unwrap().iter().all(|x| *x == 0.));
}
#[test]
fn reset_retained_bytes_and_multiport_mixed_graph() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<DspEngine<'static>>();
    let mut v = wire(24000, 1, &[2., 4.]);
    v["nodes"].as_array_mut().unwrap().push(json!({"id":"gain","processor":{"kind":"core","processor":{"kind":"gain","channels":1}},"params":{"gain":"1/2"}}));
    v["connections"] =
        json!([{"id":"c","from":{"node":"kit","port":"out"},"to":{"node":"gain","port":"in"}}]);
    v["output"]["output"] = json!({"node":"gain","port":"out"});
    let p = artifact(&v);
    let mut engine = DspEngine::new_artifact(&p).unwrap();
    let mut first = vec![];
    engine
        .render(|f| {
            first.extend_from_slice(f);
            Ok(())
        })
        .unwrap();
    engine.reset();
    let mut second = vec![];
    engine
        .render(|f| {
            second.extend_from_slice(f);
            Ok(())
        })
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(
        first,
        render(&PlanArtifact::from_json(&p.to_json().unwrap()).unwrap()).unwrap()
    );
    let ports = [
        maac::plan::PortRef::new("kit", "out").unwrap(),
        maac::plan::PortRef::new("gain", "out").unwrap(),
    ];
    let mut frames = vec![];
    maac::dsp::render_ports_artifact_with_limits(&p, &Default::default(), &ports, |frame| {
        frames.push(frame.to_vec());
        Ok(())
    })
    .unwrap();
    assert_eq!(frames[0], vec![vec![2.], vec![1.]]);
}
#[test]
fn level_automation_applies_before_onset_and_freezes_in_tail() {
    let mut v = wire(48000, 1, &[1.; 20]);
    v["automation"] = json!([{"id":"level","target":{"node":"kit","port":"level"},"clock":"score","at":{"kind":"score","q":"0/1"},"points":[{"position":"0/1","value":"0/1","shape":"linear"},{"position":"1/1500","value":"2/1","shape":"step"}]}]);
    let audio = render(&artifact(&v)).unwrap();
    for (i, x) in audio.iter().enumerate() {
        assert!((x - (i.min(8) as f64 / 8.)).abs() < 1e-12, "{i} {x}");
    }
    v["automation"][0]["clock"] = json!("seconds");
    v["automation"][0]["at"] = json!({"kind":"seconds","seconds":"0/1"});
    v["automation"][0]["points"][1]["position"] = json!("1/3000");
    let audio = render(&artifact(&v)).unwrap();
    for (i, x) in audio.iter().enumerate() {
        assert!((x - i.min(8) as f64 / 8.).abs() < 1e-12, "{i} {x}");
    }
}

#[test]
fn ramp_inverse_clock_knots_and_nonrational_tail_freeze() {
    let mut v = wire(48000, 1, &[1.; 20]);
    v["tempo"]["points"] = json!([{"q":"0/1","bpm":"120/1","shape":"linear"},{"q":"1/3000","bpm":"240/1","shape":"step"}]);
    v["output"]["total_frames"] = json!(10);
    v["automation"] = json!([{"id":"level","target":{"node":"kit","port":"level"},"clock":"score","at":{"kind":"score","q":"0/1"},"points":[{"position":"0/1","value":"0/1","shape":"linear"},{"position":"1/6000","value":"1/1","shape":"step"}]}]);
    let audio = render(&artifact(&v)).unwrap();
    for (frame, sample) in audio.iter().enumerate() {
        let expected = (2.0 * (frame as f64 / 8.0).exp_m1()).min(1.0);
        assert!(
            (sample - expected).abs() < 1e-12,
            "{frame} {sample} {expected}"
        );
    }
    v["automation"][0]["clock"] = json!("seconds");
    v["automation"][0]["at"] = json!({"kind":"seconds","seconds":"0/1"});
    v["automation"][0]["points"][1]["position"] = json!("1/6000");
    v["automation"][0]["points"][1]["value"] = json!("2/1");
    let audio = render(&artifact(&v)).unwrap();
    for (frame, sample) in audio.iter().enumerate() {
        let expected = if frame < 6 {
            frame as f64 / 4.0
        } else {
            2.0 * 2.0f64.ln()
        };
        assert!(
            (sample - expected).abs() < 1e-12,
            "{frame} {sample} {expected}"
        );
    }
}
#[test]
fn mixed_note_and_hit_use_native_lifecycles_in_one_graph() {
    let mut v = wire(48000, 1, &[1.; 20]);
    v["nodes"].as_array_mut().unwrap().extend([json!({"id":"sine","processor":{"kind":"core","processor":{"kind":"sine","voices":1}},"params":{"attack":"0/1","release":"0/1","level":"1/1"}}),json!({"id":"sum","processor":{"kind":"core","processor":{"kind":"sum","channels":1}},"params":{}})]);
    v["connections"] = json!([{"id":"a","from":{"node":"kit","port":"out"},"to":{"node":"sum","port":"in"}},{"id":"b","from":{"node":"sine","port":"out"},"to":{"node":"sum","port":"in"}}]);
    v["output"]["output"] = json!({"node":"sum","port":"out"});
    v["events"].as_array_mut().unwrap().push(json!({"address":"main/note","source":{"object":"note","path":["main","note"]},"target":{"node":"sine","port":"events"},"kind":{"kind":"note","pitch_hz":12000.0,"velocity":"1/1"},"score_on_q":"0/1","score_off_q":"1/12000","onset_offset_seconds":"0/1","release_offset_seconds":"0/1","release_velocity":0.0,"on_frame":0,"off_frame":2}));
    let audio = render(&artifact(&v)).unwrap();
    assert_eq!(&audio[..4], &[1., 2., 1., 1.]);
}
#[test]
fn asset_diagnostic_codes_survive_render_boundary_mapping() {
    for code in ["E_ASSET", "E_HASH"] {
        let error = maac::dsp::RenderError::Plan(maac::plan::PlanError {
            code: code.into(),
            path: "audio_assets".into(),
            message: "invalid".into(),
            span: None,
        });
        assert_eq!(error.code(), code);
    }
}

#[test]
fn compiled_bundle_renders_after_source_and_assets_are_dropped() {
    let bytes: Vec<u8> = [2f32, 4.].iter().flat_map(|v| v.to_le_bytes()).collect();
    let source = format!(
        r#"maac 1;
project p {{ score=[0q,1/3000q]; tail=1/12000s; rate=48000Hz; tempo=&clock; meter=&metre; output=&kit:out; }}
tempo clock {{ points=[(0q,120bpm,linear),(1/3000q,240bpm,step)]; }}
meter metre {{ points=[(0q,4,4)]; }}
asset sample {{ kind=audio; path="samples/hit.pcm"; hash="{}"; format="pcm_f32le_interleaved/1"; rate=24000Hz; channels=1; frames=2; }}
node kit {{ type="core.kit/1"; config={{channels=1; samples=[{{key="key"; asset=&sample;}}];}}; }}
track track {{ target=&kit:events; }}
pattern phrase {{ length=1/3000q; hit hit {{ at=0q; key="key"; }} }}
place place {{ pattern=&phrase; track=&track; at=0q; }}
"#,
        maac::bundle::sha256_digest(&bytes)
    );
    let mut bundle = maac::SourceBundle::new("scores/main.maac", source);
    bundle.assets.insert("samples/hit.pcm".into(), bytes);
    let plan = maac::compiler::compile_bundle_artifact(&bundle).unwrap();
    let saved = plan.to_json().unwrap();
    drop(bundle);
    let audio = render(&plan).unwrap();
    drop(plan);
    let retained = PlanArtifact::from_json(&saved).unwrap();
    assert_eq!(audio, render(&retained).unwrap());
    assert_eq!(&audio[..5], &[2., 3., 4., 2., 0.]);
}
