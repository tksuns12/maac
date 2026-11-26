//! Stage 2: Native SoundFont (`.sf2`) & SFZ (`.sfz`) sample instrument importer.
//!
//! Parses standard `.sf2` and `.sfz` instruments in pure Rust and compiles them
//! into portable, 100% bit-identical MaaC libraries using `sample` declarations
//! and `synth.sample/1` (`zones`, velocity layers, sustain loops, `synth.adsr/1`).

pub mod emit_maac;
pub mod sf2;
pub mod sfz;

#[derive(Clone, Debug)]
pub struct ImportedSample {
    pub id: String,
    pub rate_hz: u32,
    pub channels: u8,
    pub root_midi: u8,
    pub loop_range: Option<(u32, u32)>,
    /// Interleaved float32 PCM samples (`frames * channels`).
    pub samples: Vec<f32>,
}

impl ImportedSample {
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.max(1))
    }
}

#[derive(Clone, Debug)]
pub struct ImportedZone {
    pub sample_index: usize,
    pub low_key: u8,
    pub high_key: u8,
    pub low_vel: u8,
    pub high_vel: u8,
}

#[derive(Clone, Debug)]
pub struct ImportedInstrument {
    pub name: String,
    pub slug: String,
    pub channels: u8,
    pub attack_s: f32,
    pub decay_s: f32,
    pub sustain: f32,
    pub release_s: f32,
    pub samples: Vec<ImportedSample>,
    pub zones: Vec<ImportedZone>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ImportSummary {
    pub library_rel_path: String,
    pub alias: String,
    pub instrument_name: String,
    pub emitted_samples: usize,
    pub emitted_zones: usize,
    pub original_zones: usize,
    pub total_values: usize,
    pub warnings: Vec<String>,
}

pub fn sanitize_maac_ident(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    let trimmed = s.trim_matches('_').to_owned();
    let mut out = if trimmed.is_empty() {
        "imported_inst".to_owned()
    } else {
        trimmed
    };
    if out.as_bytes()[0].is_ascii_digit() {
        out.insert(0, 'i');
    }
    // Avoid MaaC reserved keywords.
    if matches!(
        out.as_str(),
        "project"
            | "node"
            | "track"
            | "pattern"
            | "note"
            | "hit"
            | "chord"
            | "place"
            | "connect"
            | "import"
            | "library"
            | "instrument"
            | "sample"
            | "asset"
            | "audio"
            | "tempo"
            | "meter"
            | "curve"
            | "automation"
            | "voice"
    ) {
        out.push_str("_inst");
    }
    out
}
