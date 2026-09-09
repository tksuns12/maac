use maac::{dsp::DspEngine, PlanArtifact};
use serde_json::{json, Value};
fn wire(rate: u32, channels: u8, samples: &[f32]) -> Value {
    let bytes: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
    json!({"version":5,"output":{"score_start_q":"0/1","score_end_q":"1/3000","tail_seconds":"1/12000","sample_rate_hz":48000,"channels":channels,"total_frames":12,"output":{"node":"clip","port":"out"}},"tempo":{"points":[{"q":"0/1","bpm":"120/1","shape":"step"}]},"nodes":[{"id":"clip","processor":{"kind":"audio","clip":{"asset":"sample","channels":channels,"at":{"kind":"seconds","seconds":"1/192000"},"source_start_frame":0,"source_end_frame":samples.len()/channels as usize,"speed":"1/1","reverse":false,"gain":"1/1","fade_in_seconds":"0/1","fade_out_seconds":"0/1","fade_shape":"linear","source":{"object":"clip","path":["clip"]},"start_frame":1,"end_frame":4}}}],"audio_assets":[{"id":"sample","format":"pcm_f32le_interleaved/1","rate_hz":rate,"channels":channels,"frames":samples.len()/channels as usize,"hash":maac::bundle::sha256_digest(&bytes),"bytes":bytes}]})
}
fn clip(v: &mut Value) -> &mut Value {
    &mut v["nodes"][0]["processor"]["clip"]
}
fn artifact(v: &Value) -> PlanArtifact {
    PlanArtifact::from_json(&serde_json::to_vec(v).unwrap()).unwrap()
}
fn render(p: &PlanArtifact) -> Result<Vec<f64>, maac::dsp::RenderError> {
    let mut out = vec![];
    maac::render_artifact(p, |f| {
        out.extend_from_slice(f);
        Ok(())
    })?;
    Ok(out)
}
#[test]
fn fractional_start_retains_phase_and_reverse() {
    let mut v = wire(48000, 1, &[1., 2., 4.]);
    assert_eq!(
        &render(&artifact(&v)).unwrap()[..5],
        &[0., 1.75, 3.5, 1., 0.]
    );
    clip(&mut v)["reverse"] = json!(true);
    assert_eq!(
        &render(&artifact(&v)).unwrap()[..5],
        &[0., 2.5, 1.25, 0.25, 0.]
    );
}
#[test]
fn slices_channels_rates_and_fractional_speed() {
    for (rate, speed, end, expected) in [
        (
            24000,
            "1/1",
            7,
            vec![0., 1.375, 1.875, 2.75, 3.75, 2.5, 0.5, 0.],
        ),
        (
            48000,
            "1/2",
            7,
            vec![0., 1.375, 1.875, 2.75, 3.75, 2.5, 0.5, 0.],
        ),
        (24000, "2/1", 4, vec![0., 1.75, 3.5, 1., 0.]),
    ] {
        let mut v = wire(rate, 1, &[99., 1., 2., 4., 88.]);
        let c = clip(&mut v);
        c["source_start_frame"] = json!(1);
        c["source_end_frame"] = json!(4);
        c["end_frame"] = json!(end);
        c["speed"] = json!(speed);
        assert_eq!(&render(&artifact(&v)).unwrap()[..expected.len()], expected);
    }
    let mut v = wire(48000, 2, &[99., 98., 1., 10., 2., 20., 4., 40., 88., 87.]);
    let c = clip(&mut v);
    c["source_start_frame"] = json!(1);
    c["source_end_frame"] = json!(4);
    c["reverse"] = json!(true);
    assert_eq!(
        &render(&artifact(&v)).unwrap()[..10],
        &[0., 0., 2.5, 25., 1.25, 12.5, 0.25, 2.5, 0., 0.]
    );
}
#[test]
fn fades_gain_equal_power_and_overlap() {
    for shape in ["linear", "equal_power"] {
        let mut v = wire(48000, 1, &[1., 2., 4.]);
        let c = clip(&mut v);
        c["gain"] = json!("1/2");
        c["fade_in_seconds"] = json!("1/24000");
        c["fade_out_seconds"] = json!("1/16000");
        c["fade_shape"] = json!(shape);
        let audio = render(&artifact(&v)).unwrap();
        for (n, raw) in [(1, 1.75), (2, 3.5), (3, 1.)] {
            let elapsed = n as f64 - 0.25;
            let mut incoming = (elapsed / 2.).clamp(0., 1.);
            let mut outgoing = ((3. - elapsed) / 3.).clamp(0., 1.);
            if shape == "equal_power" {
                incoming = (incoming * std::f64::consts::FRAC_PI_2).sin();
                outgoing = (outgoing * std::f64::consts::FRAC_PI_2).sin();
            }
            assert!((audio[n] - raw * 0.5 * incoming * outgoing).abs() < 1e-14);
        }
    }
    let mut v = wire(48000, 1, &[1., 2., 4.]);
    let c = clip(&mut v);
    c["gain"] = json!("1/2");
    c["fade_in_seconds"] = json!("1/48000");
    c["fade_out_seconds"] = json!("1/24000");
    assert_eq!(
        &render(&artifact(&v)).unwrap()[..5],
        &[0., 0.65625, 1.09375, 0.0625, 0.]
    );
}
#[test]
fn empty_interval_and_tail_first_sample() {
    for (speed, end, expected) in [
        ("20/1", 8, vec![0.; 12]),
        (
            "1/1",
            11,
            vec![0., 0., 0., 0., 0., 0., 0., 0., 1.25, 2.5, 3., 0.],
        ),
    ] {
        let mut v = wire(48000, 1, &[1., 2., 4.]);
        let c = clip(&mut v);
        c["at"] = json!({"kind":"seconds","seconds":"31/192000"});
        c["speed"] = json!(speed);
        c["start_frame"] = json!(8);
        c["end_frame"] = json!(end);
        assert_eq!(render(&artifact(&v)).unwrap(), expected);
    }
}
#[test]
fn overlapping_clips_kit_note_effect_selected_ports_reset_and_retained() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<DspEngine<'static>>();
    let mut v = wire(48000, 1, &[1., 2., 4.]);
    let mut second = v["nodes"][0].clone();
    second["id"] = json!("second");
    second["processor"]["clip"]["source"] = json!({"object":"second","path":["second"]});
    v["nodes"].as_array_mut().unwrap().extend([second,json!({"id":"kit","processor":{"kind":"kit","channels":1,"voices":1,"samples":[{"key":"hit","asset":"sample"}]}}),json!({"id":"sine","processor":{"kind":"core","processor":{"kind":"sine","voices":1}},"params":{"attack":"0/1","release":"0/1","level":"1/1"}}),json!({"id":"sum","processor":{"kind":"core","processor":{"kind":"sum","channels":1}}}),json!({"id":"gain","processor":{"kind":"core","processor":{"kind":"gain","channels":1}},"params":{"gain":"1/2"}})]);
    v["connections"] = json!([{"id":"a","from":{"node":"clip","port":"out"},"to":{"node":"sum","port":"in"}},{"id":"b","from":{"node":"second","port":"out"},"to":{"node":"sum","port":"in"}},{"id":"c","from":{"node":"kit","port":"out"},"to":{"node":"sum","port":"in"}},{"id":"d","from":{"node":"sine","port":"out"},"to":{"node":"sum","port":"in"}},{"id":"e","from":{"node":"sum","port":"out"},"to":{"node":"gain","port":"in"}}]);
    v["output"]["output"] = json!({"node":"gain","port":"out"});
    v["events"] = json!([{"address":"hit","source":{"object":"hit","path":["hit"]},"target":{"node":"kit","port":"events"},"kind":{"kind":"hit","key":"hit","velocity":"1/1"},"score_on_q":"0/1","onset_offset_seconds":"0/1","release_offset_seconds":"0/1","release_velocity":0.0,"on_frame":0},{"address":"note","source":{"object":"note","path":["note"]},"target":{"node":"sine","port":"events"},"kind":{"kind":"note","pitch_hz":12000.0,"velocity":"1/1"},"score_on_q":"0/1","score_off_q":"1/12000","onset_offset_seconds":"0/1","release_offset_seconds":"0/1","release_velocity":0.0,"on_frame":0,"off_frame":2}]);
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
#[test]
fn gain_zero_and_runtime_overflow_are_explicit() {
    let mut v = wire(48000, 1, &[f32::MAX; 3]);
    clip(&mut v)["gain"] = json!("0/1");
    assert!(render(&artifact(&v)).unwrap().iter().all(|x| *x == 0.));
    clip(&mut v)["gain"] = json!(format!("{}/1", "1".to_owned() + &"0".repeat(300)));
    assert_eq!(render(&artifact(&v)).unwrap_err().code(), "E_NONFINITE");
    v["nodes"].as_array_mut().unwrap().push(
        json!({"id":"zero","processor":{"kind":"core","processor":{"kind":"sum","channels":1}}}),
    );
    v["output"]["output"] = json!({"node":"zero","port":"out"});
    // An unused source is still evaluated and must fail explicitly.
    assert_eq!(render(&artifact(&v)).unwrap_err().code(), "E_NONFINITE");
    v["nodes"][1]["processor"]["processor"]["kind"] = json!("gain");
    v["nodes"][1]["params"] = json!({"gain":"0/1"});
    v["connections"] = json!([{"id":"route","from":{"node":"clip","port":"out"},"to":{"node":"zero","port":"in"}}]);
    assert_eq!(render(&artifact(&v)).unwrap_err().code(), "E_NONFINITE");
}
#[test]
fn negative_reset_origin_and_long_absolute_frame_phase() {
    let mut v = wire(48000, 1, &[1., 2., 4.]);
    v["output"]["score_start_q"] = json!("-1/3000");
    v["output"]["score_end_q"] = json!("0/1");
    clip(&mut v)["at"] = json!({"kind":"seconds","seconds":"-31/192000"});
    assert_eq!(
        &render(&artifact(&v)).unwrap()[..5],
        &[0., 1.75, 3.5, 1., 0.]
    );
    let mut v = wire(48000, 1, &[1., 2., 4.]);
    v["output"]["score_end_q"] = json!("4/1");
    v["output"]["tail_seconds"] = json!("0/1");
    v["output"]["total_frames"] = json!(96000);
    let c = clip(&mut v);
    c["at"] = json!({"kind":"score","q":"0/1"});
    c["speed"] = json!("1/40000");
    c["start_frame"] = json!(0);
    c["end_frame"] = json!(96000);
    let audio = render(&artifact(&v)).unwrap();
    for n in [0usize, 10001, 39999, 60001, 79999, 95003] {
        let u = n as f64 / 40000.;
        let expected = if u < 1. {
            1. + u
        } else if u < 2. {
            2. + 2. * (u - 1.)
        } else {
            4. * (3. - u)
        };
        assert!((audio[n] - expected).abs() < 2e-15);
    }
}
#[test]
fn ramp_score_anchor_retains_nonrational_fractional_phase_after_source_drop() {
    let bytes: Vec<u8> = [1f32, 2., 4.]
        .iter()
        .flat_map(|x| x.to_le_bytes())
        .collect();
    let source = format!(
        r#"maac 1;
project p {{ score=[0q,1/3000q]; tail=1/12000s; rate=48000Hz; tempo=&clock; meter=&metre; output=&clip:out; }}
tempo clock {{ points=[(0q,60bpm,linear),(1/3000q,120bpm,step)]; }}
meter metre {{ points=[(0q,4,4)]; }}
asset sample {{ kind=audio; path="sample.pcm"; hash="{}"; format="pcm_f32le_interleaved/1"; rate=48000Hz; channels=1; frames=3; }}
audio clip {{ asset=&sample; at=1/192000q; source=[0frame,3frame]; mode=rate; }}"#,
        maac::bundle::sha256_digest(&bytes)
    );
    let mut bundle = maac::SourceBundle::new("score.maac", source);
    bundle.assets.insert("sample.pcm".into(), bytes);
    let p = maac::compiler::compile_bundle_artifact(&bundle).unwrap();
    let saved = p.to_json().unwrap();
    drop(bundle);
    let audio = render(&p).unwrap();
    let start = 16. * (65f64 / 64.).ln();
    for (n, sample) in audio.iter().enumerate().take(4).skip(1) {
        let u = n as f64 - start;
        let expected = if u < 1. {
            1. + u
        } else if u < 2. {
            2. + 2. * (u - 1.)
        } else {
            4. * (3. - u)
        };
        assert!((*sample - expected).abs() < 1e-13);
    }
    assert_eq!(
        audio,
        render(&PlanArtifact::from_json(&saved).unwrap()).unwrap()
    );
}
