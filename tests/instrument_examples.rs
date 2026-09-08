use std::collections::BTreeSet;
use std::path::Path;

use maac::bundle::sha256_digest;
use maac::bundle_fs::load_bundle;
use maac::plan::Processor;
use maac::{check_bundle, compile_bundle, render};

fn render_signature(plan: &maac::Plan) -> (u64, f64, f64, u64) {
    let mut frames = 0_u64;
    let mut peak = 0.0_f64;
    let mut energy = 0.0_f64;
    let mut checksum = 0xcbf29ce484222325_u64;
    render(plan, |frame| {
        assert_eq!(frame.len(), 2);
        for &sample in frame {
            assert!(sample.is_finite());
            peak = peak.max(sample.abs());
            energy += sample * sample;
            checksum ^= sample.to_bits();
            checksum = checksum.wrapping_mul(0x100000001b3);
        }
        frames += 1;
        Ok(())
    })
    .expect("starter composition renders");
    (frames, peak, energy, checksum)
}

#[test]
fn reusable_starter_sounds_load_compile_and_render_deterministically() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let bundle = load_bundle(Path::new("examples/reusable.maac"), root)
        .expect("filesystem example bundle loads with valid pins");
    let library_source = &bundle.sources["examples/sounds/studio.maac"];
    let table_bytes = &bundle.assets["examples/sounds/colors.wav"];
    assert!(library_source.contains(&sha256_digest(table_bytes)));
    assert!(bundle.sources["examples/reusable.maac"]
        .contains(&sha256_digest(library_source.as_bytes())));
    let resolved = bundle.resolve().expect("example dependencies resolve");
    let preset_exports = resolved.documents["examples/sounds/studio.maac"]
        .objects
        .values()
        .filter(|object| object.kind == "preset")
        .map(|object| object.id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        preset_exports,
        BTreeSet::from([
            "bright_pad",
            "deep_bass",
            "glass_bell",
            "pluck_bass",
            "soft_bell",
            "warm_pad",
        ])
    );
    check_bundle(&bundle).expect("starter example checks");
    let plan = compile_bundle(&bundle).expect("starter example compiles");

    assert_eq!(plan.version, 2);
    assert_eq!(plan.output.channels, 2);
    assert_eq!(plan.output.total_frames, 480_000);
    assert_eq!(plan.events.len(), 31);
    let resources = plan
        .instruments
        .as_ref()
        .expect("embedded instrument resources");
    assert_eq!(resources.wavetables.len(), 1);
    assert_eq!(resources.wavetables[0].cycle_length, 512);
    assert_eq!(resources.wavetables[0].samples.len(), 512 * 3);
    let exports = resources
        .programs
        .iter()
        .map(|program| program.source.object.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(exports, BTreeSet::from(["bass", "fm_bell", "pad"]));
    assert!(resources
        .programs
        .iter()
        .find(|program| program.source.object == "fm_bell")
        .expect("FM bell program")
        .shared
        .is_some());
    assert!(resources
        .programs
        .iter()
        .find(|program| program.source.object == "pad")
        .expect("pad program")
        .controls
        .contains_key("position"));

    let instruments = plan
        .nodes
        .iter()
        .filter_map(|node| match &node.processor {
            Processor::Instrument { program, .. } => Some((node, program)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(instruments.len(), 4);
    let fm_program = resources
        .programs
        .iter()
        .find(|program| program.source.object == "fm_bell")
        .expect("FM bell program")
        .id
        .clone();
    let bell_instances = instruments
        .iter()
        .filter(|(_, program)| program.as_str() == fm_program)
        .collect::<Vec<_>>();
    assert_eq!(bell_instances.len(), 2);
    assert_ne!(
        bell_instances[0].0.params["release"],
        bell_instances[1].0.params["release"]
    );
    let event_targets = plan
        .events
        .iter()
        .map(|event| event.target.node.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        event_targets,
        BTreeSet::from(["bass", "bell_glass", "bell_soft", "pad"])
    );
    assert!(plan
        .automation
        .iter()
        .any(|automation| automation.target.node == "pad" && automation.target.port == "position"));

    let first = render_signature(&plan);
    let second = render_signature(&plan);
    assert_eq!(first, second);
    assert_eq!(first.0, plan.output.total_frames);
    assert!(first.1 > 1.0e-6, "render must be nonsilent");
    assert!(first.2 > 1.0e-6, "render must carry finite energy");
    assert!(first.1 <= 1.0, "starter mix should remain PCM-safe");
}
