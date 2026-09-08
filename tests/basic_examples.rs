use std::collections::BTreeSet;
use std::io::Cursor;
use std::path::Path;

use maac::export::{write_wav, WavFormat};
use maac::plan::Processor;
use maac::{check_bundle, compile_bundle, render, SourceBundle};

const INSTRUMENTS: [&str; 24] = [
    "mellow_piano",
    "bright_piano",
    "electric_piano",
    "organ",
    "nylon_guitar",
    "steel_guitar",
    "muted_guitar",
    "finger_bass",
    "pick_bass",
    "sub_bass",
    "synth_bass",
    "kick",
    "snare",
    "clap",
    "closed_hat",
    "open_hat",
    "low_tom",
    "high_tom",
    "crash",
    "strings",
    "flute",
    "bell",
    "warm_pad",
    "synth_lead",
];

fn compile_example(name: &str) -> maac::Plan {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/basic")
        .join(format!("{name}.maac"));
    let source = std::fs::read_to_string(path).expect("checked-in example source");
    let bundle = SourceBundle::new("standalone.maac", source);
    assert_eq!(bundle.sources.len(), 1);
    assert!(bundle.assets.is_empty());
    check_bundle(&bundle).unwrap_or_else(|error| panic!("{name} checks: {error:?}"));
    let plan = compile_bundle(&bundle)
        .unwrap_or_else(|error| panic!("{name} compiles from entry text only: {error:?}"));
    assert_eq!(plan.version, 2);
    assert_eq!(plan.output.channels, 2);
    assert!(plan.instruments.as_ref().unwrap().wavetables.is_empty());
    plan
}

fn verify_audio(name: &str, plan: &maac::Plan) {
    let mut frames = 0_u64;
    let mut peak = 0.0_f64;
    let mut energy = 0.0;
    render(plan, |frame| {
        assert_eq!(frame.len(), 2, "{name} remains stereo");
        for &sample in frame {
            assert!(sample.is_finite(), "{name} produces finite samples");
            peak = peak.max(sample.abs());
            energy += sample * sample;
        }
        frames += 1;
        Ok(())
    })
    .unwrap_or_else(|error| panic!("{name} renders: {error:?}"));
    assert_eq!(frames, plan.output.total_frames);
    assert!(
        peak > 1.0e-5 && energy > 1.0e-5,
        "{name} is audible numerically"
    );
    assert!(peak < 1.0, "{name} has PCM16 headroom, peak={peak}");
    let mut wav = Cursor::new(Vec::new());
    let stats = write_wav(&mut wav, plan, WavFormat::Pcm16)
        .unwrap_or_else(|error| panic!("{name} exports PCM16 without overload: {error:?}"));
    assert_eq!(stats.frames, frames);
    assert_eq!(stats.channels, 2);
    assert!(wav.get_ref().len() > frames as usize * 4);
    eprintln!("{name}: frames={frames}, peak={peak:.9}, energy={energy:.9}");
}

#[test]
fn each_basic_instrument_has_a_standalone_pcm16_safe_audition() {
    let expected: BTreeSet<_> = INSTRUMENTS.into_iter().collect();
    let example_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/basic");
    let actual: BTreeSet<_> = std::fs::read_dir(example_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "maac")
        })
        .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
        .filter(|name| name != "full_band")
        .collect();
    assert_eq!(
        actual,
        expected.iter().map(|name| (*name).to_owned()).collect()
    );
    for name in INSTRUMENTS {
        let plan = compile_example(name);
        assert!((96_000..=192_000).contains(&plan.output.total_frames));
        let programs = &plan.instruments.as_ref().unwrap().programs;
        let selected = plan
            .nodes
            .iter()
            .find_map(|node| match &node.processor {
                Processor::Instrument { program, .. } => Some(program),
                _ => None,
            })
            .expect("audition instrument instance");
        let program = programs
            .iter()
            .find(|program| &program.id == selected)
            .expect("instance references an embedded program");
        assert_eq!(program.source.object, name);
        verify_audio(name, &plan);
    }
}

#[test]
fn full_band_is_standalone_and_routes_separate_drums_to_stereo() {
    let plan = compile_example("full_band");
    assert!((384_000..=576_000).contains(&plan.output.total_frames));
    let targets: BTreeSet<_> = plan
        .events
        .iter()
        .map(|event| event.target.node.as_str())
        .collect();
    assert_eq!(
        targets,
        BTreeSet::from(["keys", "guitar", "bass", "melody", "pad", "kick", "snare", "hat"])
    );
    verify_audio("full_band", &plan);
}
