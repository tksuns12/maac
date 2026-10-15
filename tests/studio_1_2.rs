//! `std/studio/1.2.0`: frozen bytes, 1.1.0 with only the two hats changed,
//! and hats that hiss instead of ringing at a pitch, as loud as before.

use std::f64::consts::TAU;

use maac::analyze::{analyze_artifact, AnalyzeOptions, SectionMode, SourceReport};
use maac::bundle::{sha256_digest, SourceBundle};
use maac::compiler::compile_bundle_artifact_with_limits;
use maac::plan::PlanLimits;
use maac::stdlib::{self, STUDIO_1_1_ID, STUDIO_1_1_SOURCE, STUDIO_1_2_ID, STUDIO_1_2_SOURCE};
use maac::{parse_rational, render_artifact, PlanArtifact};

const HATS: [(&str, &str); 2] = [("closed_hat", "hat"), ("open_hat", "open_hat")];

fn composition(library: &str, body: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 16q]; rate = 48000Hz; tail = 2s; tempo = &clock; meter = &metre; output = &out:out; }}
tempo clock {{ points = [(0q, 90bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
import studio {{ builtin = "{library}"; }}
node out {{ instrument = &studio.drums; }}
track t {{ target = &out:events; }}
pattern phrase {{ length = 16q;
{body}
}}
place once {{ pattern = &phrase; track = &t; at = 0q; }}
"#
    )
}

fn plan(source: &str) -> PlanArtifact {
    compile_bundle_artifact_with_limits(
        &SourceBundle::new("main.maac", source),
        &PlanLimits::song(),
    )
    .unwrap_or_else(|error| panic!("{error:?}\n{source}"))
}

fn render(plan: &PlanArtifact) -> Vec<f64> {
    let mut samples = Vec::new();
    render_artifact(plan, |frame| {
        samples.extend_from_slice(frame);
        Ok(())
    })
    .unwrap();
    samples
}

fn analysis(source: &str) -> SourceReport {
    let report = analyze_artifact(
        &plan(source),
        None,
        &AnalyzeOptions {
            sections: SectionMode::Whole,
            ..AnalyzeOptions::default()
        },
        &PlanLimits::song(),
    )
    .unwrap();
    report.sources.into_iter().next().unwrap()
}

fn hit(library: &str, key: &str, velocity: &str) -> Vec<f64> {
    render(&plan(&composition(
        library,
        &format!(r#"hit h {{ at = 0q; key = "{key}"; velocity = {velocity}; }}"#),
    )))
}

/// Each instrument's source by name, without comments.
fn instruments(source: &str) -> Vec<(String, String)> {
    let uncommented: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .map(|line| format!("{line}\n"))
        .collect();
    uncommented
        .split("\ninstrument ")
        .map(|block| {
            let name = block.split_once(' ').map_or("", |(name, _)| name);
            (name.to_owned(), block.to_owned())
        })
        .collect()
}

#[test]
fn published_version_keeps_its_exact_source_bytes() {
    assert_eq!(
        sha256_digest(STUDIO_1_2_SOURCE.as_bytes()),
        include_str!("../stdlib/studio/1.2.0.sha256").trim()
    );
}

#[test]
fn only_the_hats_and_their_kit_levels_change() {
    let old =
        instruments(&STUDIO_1_1_SOURCE.replacen("version = \"1.1.0\";", "version = \"1.2.0\";", 1));
    let new = instruments(STUDIO_1_2_SOURCE);
    assert_eq!(
        old.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        new.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    for ((name, before), (_, after)) in old.iter().zip(&new) {
        match name.as_str() {
            "closed_hat" | "open_hat" => assert_ne!(before, after),
            "drums" => assert_eq!(
                before
                    .replace(
                        "hat_level { target = &hat.params.level; default = 7.32;",
                        "hat_level { target = &hat.params.level; default = 4.59;"
                    )
                    .replace(
                        "open_hat_level { target = &open_hat.params.level; default = 2.78;",
                        "open_hat_level { target = &open_hat.params.level; default = 1.73;"
                    ),
                *after
            ),
            _ => assert_eq!(before, after, "{name}"),
        }
    }
}

#[test]
fn every_other_catalog_entry_is_unchanged() {
    let old = stdlib::catalog_for(STUDIO_1_1_ID).unwrap();
    let new = stdlib::catalog_for(STUDIO_1_2_ID).unwrap();
    assert_eq!(
        old.instruments
            .iter()
            .map(|item| &item.name)
            .collect::<Vec<_>>(),
        new.instruments
            .iter()
            .map(|item| &item.name)
            .collect::<Vec<_>>()
    );
    for (before, after) in old.instruments.iter().zip(&new.instruments) {
        let mut expected = before.clone();
        expected.usage = before.usage.replace(STUDIO_1_1_ID, STUDIO_1_2_ID);
        if HATS.iter().any(|(name, _)| *name == before.name) {
            assert_ne!(after.description, before.description);
            expected.description = after.description.clone();
            expected.controls.get_mut("level").unwrap().default =
                after.controls["level"].default.clone();
        }
        assert_eq!(*after, expected, "{}", before.name);
    }
    let (old, new) = (&old.kits[0], &new.kits[0]);
    assert_eq!(new.pieces, old.pieces);
    let mut controls = old.controls.clone();
    controls.get_mut("hat_level").unwrap().default = parse_rational("4.59").unwrap();
    controls.get_mut("open_hat_level").unwrap().default = parse_rational("1.73").unwrap();
    assert_eq!(new.controls, controls);
}

#[test]
fn every_other_sound_renders_bit_identically() {
    let catalog = stdlib::catalog_for(STUDIO_1_1_ID).unwrap();
    for item in &catalog.instruments {
        if HATS.iter().any(|(name, _)| *name == item.name) {
            continue;
        }
        let before = render(&plan(&item.usage));
        let after = render(&plan(&item.usage.replace(STUDIO_1_1_ID, STUDIO_1_2_ID)));
        assert_eq!(before, after, "{}", item.name);
    }
    for piece in &catalog.kits[0].pieces {
        if HATS.iter().any(|(_, key)| *key == piece.key) {
            continue;
        }
        assert_eq!(
            hit(STUDIO_1_1_ID, &piece.key, "0.7"),
            hit(STUDIO_1_2_ID, &piece.key, "0.7"),
            "{}",
            piece.key
        );
    }
}

/// The share of a hit's 2–16 kHz energy within two bins of its ten
/// strongest spectral peaks, over its first `frames` frames: about a tenth
/// for noise through the hats' filters, and far more when partials ring.
fn ringing(stereo: &[f64], frames: usize) -> f64 {
    let windowed: Vec<f64> = stereo
        .chunks(2)
        .take(frames)
        .enumerate()
        .map(|(index, frame)| {
            (frame[0] + frame[1]) * (0.5 - 0.5 * (TAU * index as f64 / frames as f64).cos())
        })
        .collect();
    let bin = 48_000.0 / frames as f64;
    let (low, high) = ((2_000.0 / bin) as usize, (16_000.0 / bin) as usize);
    let power: Vec<f64> = (low..high)
        .map(|k| {
            let (mut re, mut im) = (0.0, 0.0);
            for (index, sample) in windowed.iter().enumerate() {
                let angle = TAU * ((k * index) % frames) as f64 / frames as f64;
                re += sample * angle.cos();
                im -= sample * angle.sin();
            }
            re * re + im * im
        })
        .collect();
    let mut peaks: Vec<usize> = (1..power.len() - 1)
        .filter(|&k| power[k] > power[k - 1] && power[k] >= power[k + 1])
        .collect();
    peaks.sort_by(|a, b| power[*b].total_cmp(&power[*a]));
    let near: f64 = peaks[..10]
        .iter()
        .map(|&k| {
            power[k.saturating_sub(2)..(k + 3).min(power.len())]
                .iter()
                .sum::<f64>()
        })
        .sum();
    near / power.iter().sum::<f64>()
}

#[test]
fn the_hats_hiss_instead_of_ringing_at_a_pitch() {
    for ((_, key), frames, limit) in [(HATS[0], 2048, 0.3), (HATS[1], 8192, 0.15)] {
        let old = ringing(&hit(STUDIO_1_1_ID, key, "0.5"), frames);
        let new = ringing(&hit(STUDIO_1_2_ID, key, "0.5"), frames);
        assert!(
            old > 0.4 && new < limit,
            "{key}: {old:.3} before, {new:.3} after"
        );
    }
}

#[test]
fn the_hats_keep_their_loudness_and_get_brighter_with_velocity() {
    let beats = |velocity: &str, key: &str| {
        (0..16)
            .map(|beat| {
                format!(r#"hit b{beat} {{ at = {beat}q; key = "{key}"; velocity = {velocity}; }}"#)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    for (_, key) in HATS {
        let loudness = |library: &str| {
            analysis(&composition(library, &beats("0.7", key)))
                .whole
                .integrated_lufs
                .unwrap()
        };
        let (old, new) = (loudness(STUDIO_1_1_ID), loudness(STUDIO_1_2_ID));
        assert!(
            (new - old).abs() <= 0.1,
            "{key}: {old:.2} LUFS before, {new:.2} after"
        );
        let brightness = |velocity: &str| {
            let bands = analysis(&composition(STUDIO_1_2_ID, &beats(velocity, key)))
                .whole
                .bands_db;
            let shares: Vec<f64> = bands
                .iter()
                .map(|db| db.map_or(0.0, |db| 10f64.powf(db / 10.0)))
                .collect();
            shares
                .iter()
                .enumerate()
                .map(|(octave, share)| octave as f64 * share)
                .sum::<f64>()
                / shares.iter().sum::<f64>()
        };
        let (soft, hard) = (brightness("0.3"), brightness("1"));
        assert!(hard > soft, "{key}: soft {soft:.4}, hard {hard:.4}");
    }
}
