use maac::{dsp::DspEngine, PlanArtifact};
use serde_json::{json, Value};
fn wire(channels: u8, samples: &[f32]) -> Value {
    let bytes: Vec<u8> = samples
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    json!({"version":6,"output":{"score_start_q":"0/1","score_end_q":"1/6000","tail_seconds":"1/12000","sample_rate_hz":48000,"channels":channels,"total_frames":12,"output":{"node":"clip","port":"out"}},"tempo":{"points":[{"q":"0/1","bpm":"60/1","shape":"step"}]},"nodes":[{"id":"clip","processor":{"kind":"warp_rate","clip":{"asset":"sample","channels":channels,"at_q":"1/192000","source_start_frame":0,"source_end_frame":samples.len()/channels as usize,"warp":[{"q":"0/1","source_frame":0},{"q":"1/16000","source_frame":samples.len()/channels as usize}],"gain":"1/1","fade_in_seconds":"0/1","fade_out_seconds":"0/1","fade_shape":"linear","source":{"object":"clip","path":["clip"]},"start_frame":1,"end_frame":4}}}],"audio_assets":[{"id":"sample","format":"pcm_f32le_interleaved/1","rate_hz":24000,"channels":channels,"frames":samples.len()/channels as usize,"hash":maac::bundle::sha256_digest(&bytes),"bytes":bytes}]})
}
fn clip(v: &mut Value) -> &mut Value {
    &mut v["nodes"][0]["processor"]["clip"]
}
fn artifact(v: &Value) -> PlanArtifact {
    PlanArtifact::from_json(&serde_json::to_vec(v).unwrap()).unwrap()
}
fn render(p: &PlanArtifact) -> Result<Vec<f64>, maac::dsp::RenderError> {
    let mut output = vec![];
    maac::render_artifact(p, |frame| {
        output.extend_from_slice(frame);
        Ok(())
    })?;
    Ok(output)
}
#[test]
fn fractional_forward_warp_uses_authored_frames_not_asset_rate() {
    let v = wire(1, &[1., 2., 4.]);
    assert_eq!(
        &render(&artifact(&v)).unwrap()[..5],
        &[0., 1.75, 3.5, 1., 0.]
    );
}
fn source_plan(tempo: &str, body: &str) -> PlanArtifact {
    let bytes: Vec<u8> = [1f32, 2., 4.]
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let source = format!(
        r#"maac 1;
project p {{score=[0q,1/6000q];tail=1/12000s;rate=48000Hz;tempo=&clock;meter=&metre;output=&clip:out;}}
tempo clock {{points={tempo};}} meter metre {{points=[(0q,4,4)];}}
asset sample {{kind=audio;path="sample.pcm";hash="{}";format="pcm_f32le_interleaved/1";rate=24000Hz;channels=1;frames=3;}}
{body}"#,
        maac::bundle::sha256_digest(&bytes)
    );
    let mut bundle = maac::SourceBundle::new("warp.maac", source);
    bundle.assets.insert("sample.pcm".into(), bytes);
    maac::compiler::compile_bundle_artifact(&bundle).unwrap()
}
const SIMPLE:&str="audio clip {asset=&sample;at=0q;source=[0frame,3frame];mode=warp_rate;warp=[(0q,0frame),(1/6000q,3frame)];}";
fn sample_at(u: f64) -> f64 {
    if u < 1. {
        1. + u
    } else if u < 2. {
        2. + 2. * (u - 1.)
    } else if u < 3. {
        4. * (3. - u)
    } else {
        0.
    }
}
#[test]
fn ramps_and_piecewise_tempo_render_analytical_waveforms() {
    for (b, end_b) in [(60., 120.), (120., 60.)] {
        let p = source_plan(
            &format!("[(0q,{b}bpm,linear),(1/6000q,{end_b}bpm,step)]"),
            SIMPLE,
        );
        let audio = render(&p).unwrap();
        let slope = (end_b - b) * 6000.;
        for (n, value) in audio.iter().enumerate().take(6) {
            let q = b / slope * (slope * n as f64 / (60. * 48000.)).exp_m1();
            assert!((*value - sample_at(18000. * q)).abs() < 1e-12);
        }
        assert!(audio[6..].iter().all(|value| *value == 0.));
    }
    let p = source_plan(
        "[(0q,60bpm,step),(1/12000q,120bpm,step)]",
        &SIMPLE.replace("(1/6000q,3frame)", "(1/12000q,1frame),(1/6000q,3frame)"),
    );
    assert_eq!(
        &render(&p).unwrap()[..7],
        &[1., 1.25, 1.5, 1.75, 2., 4., 0.]
    );
}
#[test]
fn stereo_slice_and_overlapping_physical_fades() {
    let mut v = wire(2, &[99., 98., 1., 10., 2., 20., 4., 40., 88., 87.]);
    let c = clip(&mut v);
    c["source_start_frame"] = json!(1);
    c["source_end_frame"] = json!(4);
    c["warp"] = json!([{"q":"0/1","source_frame":1},{"q":"1/16000","source_frame":4}]);
    assert_eq!(
        &render(&artifact(&v)).unwrap()[..10],
        &[0., 0., 1.75, 17.5, 3.5, 35., 1., 10., 0., 0.]
    );
    for shape in ["linear", "equal_power"] {
        let mut v = wire(1, &[1., 2., 4.]);
        let c = clip(&mut v);
        c["gain"] = json!("1/2");
        c["fade_in_seconds"] = json!("1/24000");
        c["fade_out_seconds"] = json!("1/16000");
        c["fade_shape"] = json!(shape);
        let audio = render(&artifact(&v)).unwrap();
        for (n, raw) in [(1, 1.75), (2, 3.5), (3, 1.)] {
            let elapsed = n as f64 - 0.25;
            let mut fi = (elapsed / 2.).min(1.);
            let mut fo = (3. - elapsed) / 3.;
            if shape == "equal_power" {
                fi = (fi * std::f64::consts::FRAC_PI_2).sin();
                fo = (fo * std::f64::consts::FRAC_PI_2).sin();
            }
            assert!((audio[n] - raw * 0.5 * fi * fo).abs() < 1e-14);
        }
    }
}
#[test]
fn empty_interval_tail_future_tempo_and_natural_fade_truncation() {
    let mut v = wire(1, &[1., 2., 4.]);
    let c = clip(&mut v);
    c["warp"][1]["q"] = json!("1/192000");
    c["end_frame"] = json!(1);
    assert!(render(&artifact(&v))
        .unwrap()
        .iter()
        .all(|value| *value == 0.));
    let p = source_plan(
        "[(0q,60bpm,step),(1/6000q,120bpm,step)]",
        &SIMPLE.replace("at=0q", "at=1/8000q"),
    );
    let audio = render(&p).unwrap();
    assert_eq!(&audio[..6], &[0.; 6]);
    for (n, u) in [(6, 0.), (7, 0.375), (8, 0.75), (9, 1.5), (10, 2.25)] {
        assert!((audio[n] - sample_at(u)).abs() < 1e-12);
    }
    assert_eq!(audio[11], 0.);
    // Natural end is frame16, but render truncates at12: fade-out remains half.
    let p = source_plan(
        "[(0q,60bpm,step)]",
        &SIMPLE
            .replace("(1/6000q,3frame)", "(1/3000q,3frame)")
            .replace("mode=warp_rate;", "mode=warp_rate;fade_out=1/3000s;"),
    );
    let audio = render(&p).unwrap();
    assert!((audio[8] - sample_at(1.5) * 0.5).abs() < 1e-12);
}
#[test]
fn nonfinite_unconnected_and_zero_downstream_sources_fail() {
    let mut v = wire(1, &[f32::MAX; 3]);
    clip(&mut v)["gain"] = json!("0/1");
    assert!(render(&artifact(&v))
        .unwrap()
        .iter()
        .all(|value| *value == 0.));
    clip(&mut v)["gain"] = json!(format!("{}/1", "1".to_owned() + &"0".repeat(300)));
    assert_eq!(render(&artifact(&v)).unwrap_err().code(), "E_NONFINITE");
    v["nodes"].as_array_mut().unwrap().push(
        json!({"id":"zero","processor":{"kind":"core","processor":{"kind":"sum","channels":1}}}),
    );
    v["output"]["output"] = json!({"node":"zero","port":"out"});
    assert_eq!(render(&artifact(&v)).unwrap_err().code(), "E_NONFINITE");
    v["nodes"][1]["processor"]["processor"]["kind"] = json!("gain");
    v["nodes"][1]["params"] = json!({"gain":"0/1"});
    v["connections"] = json!([{"id":"route","from":{"node":"clip","port":"out"},"to":{"node":"zero","port":"in"}}]);
    assert_eq!(render(&artifact(&v)).unwrap_err().code(), "E_NONFINITE");
}
#[test]
fn mixed_warp_rate_kit_note_effect_selected_ports_reset_and_retained() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<DspEngine<'static>>();
    let mut v = wire(1, &[1., 2., 4.]);
    v["audio_assets"][0]["rate_hz"] = json!(48000);
    let mut second = v["nodes"][0].clone();
    second["processor"] = json!({"kind":"audio","clip":{"asset":"sample","channels":1,"at":{"kind":"seconds","seconds":"1/192000"},"source_start_frame":0,"source_end_frame":3,"speed":"1/1","reverse":false,"gain":"1/1","fade_in_seconds":"0/1","fade_out_seconds":"0/1","fade_shape":"linear","source":{"object":"second","path":["second"]},"start_frame":1,"end_frame":4}});
    second["id"] = json!("second");
    second["processor"]["clip"]["source"] = json!({"object":"second","path":["second"]});
    v["nodes"].as_array_mut().unwrap().extend([second,json!({"id":"kit","processor":{"kind":"kit","channels":1,"voices":1,"samples":[{"key":"hit","asset":"sample"}]}}),json!({"id":"sine","processor":{"kind":"core","processor":{"kind":"sine","voices":1}},"params":{"attack":"0/1","release":"0/1","level":"1/1"}}),json!({"id":"sum","processor":{"kind":"core","processor":{"kind":"sum","channels":1}}}),json!({"id":"gain","processor":{"kind":"core","processor":{"kind":"gain","channels":1}},"params":{"gain":"1/2"}})]);
    v["connections"] = json!([{"id":"a","from":{"node":"clip","port":"out"},"to":{"node":"sum","port":"in"}},{"id":"b","from":{"node":"second","port":"out"},"to":{"node":"sum","port":"in"}},{"id":"c","from":{"node":"kit","port":"out"},"to":{"node":"sum","port":"in"}},{"id":"d","from":{"node":"sine","port":"out"},"to":{"node":"sum","port":"in"}},{"id":"e","from":{"node":"sum","port":"out"},"to":{"node":"gain","port":"in"}}]);
    v["output"]["output"] = json!({"node":"gain","port":"out"});
    v["events"] = json!([{"address":"hit","source":{"object":"hit","path":["hit"]},"target":{"node":"kit","port":"events"},"kind":{"kind":"hit","key":"hit","velocity":"1/1"},"score_on_q":"0/1","onset_offset_seconds":"0/1","release_offset_seconds":"0/1","release_velocity":0.0,"on_frame":0},{"address":"note","source":{"object":"note","path":["note"]},"target":{"node":"sine","port":"events"},"kind":{"kind":"note","pitch_hz":12000.0,"velocity":"1/1"},"score_on_q":"0/1","score_off_q":"1/24000","onset_offset_seconds":"0/1","release_offset_seconds":"0/1","release_velocity":0.0,"on_frame":0,"off_frame":2}]);
    let p = artifact(&v);
    let audio = render(&p).unwrap();
    assert_eq!(&audio[..5], &[0.5, 3.25, 5.5, 1., 0.]);
    let mut engine = DspEngine::new_artifact(&p).unwrap();
    for _ in 0..2 {
        engine.reset();
        let mut repeated = vec![];
        engine
            .render(|f| {
                repeated.extend_from_slice(f);
                Ok(())
            })
            .unwrap();
        assert_eq!(repeated, audio);
    }
    assert_eq!(
        render(&PlanArtifact::from_json(&p.to_json().unwrap()).unwrap()).unwrap(),
        audio
    );
    let ports = [
        maac::plan::PortRef::new("clip", "out").unwrap(),
        maac::plan::PortRef::new("gain", "out").unwrap(),
    ];
    let mut selected = vec![];
    maac::dsp::render_ports_artifact_with_limits(&p, &Default::default(), &ports, |f| {
        selected.push(f.to_vec());
        Ok(())
    })
    .unwrap();
    assert_eq!(selected[1], vec![vec![1.75], vec![3.25]]);
}
