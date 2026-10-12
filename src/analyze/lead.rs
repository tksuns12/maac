//! Lead parts: whether a part the author names with `--lead` sits on top of
//! its accompaniment.
//!
//! Each of a lead's notes gets a window like the note checks': from its onset
//! to its release, at most [`super::notes::NOTE_MAX_S`]. While rendering, the
//! window adds up the octave-band energy of the lead port, of its mix point,
//! and of every other lead feeding that mix point. The note's margin is the
//! lead's energy over the rest of the mix point, in the band where the lead
//! is strongest over the note. Other leads are left out of the rest, so two
//! leads playing together are each compared with the accompaniment.

use std::collections::BTreeSet;

use serde::Serialize;

use super::notes::FrameValues;
use super::round2;
use crate::production_analysis::loudness;

/// A lead note is silent, and gets no margin, at or below this loudness;
/// `silent_note` reports it.
const SILENT_LUFS: f64 = -70.0;
/// The rest of the mix point is taken as empty, and the note as clear, when
/// it holds at most this share of the mix point's energy in the lead's band.
/// The subtraction is only as exact as the parts are uncorrelated, so a
/// smaller rest is within its error.
const CLEAR_SHARE: f64 = 0.01;

/// A lead named with `--lead`, resolved against the analysed sources.
pub(super) struct Lead {
    /// `node:port`.
    pub port: String,
    pub source: usize,
    /// The mix point the lead is read at: its node, and the analysed source
    /// that is its output.
    pub mix_node: String,
    pub mix: usize,
    /// The other leads that feed the same mix point.
    pub others: Vec<usize>,
    /// The nodes at or upstream of the lead that receive notes.
    pub nodes: BTreeSet<String>,
}

/// One measured lead note.
pub(super) struct LeadWindow {
    pub address: String,
    pub lead: usize,
    pub start: u64,
    pub end: u64,
    source: usize,
    mix: usize,
    /// Each other lead at the mix point, with 1 / its channel count.
    others: Vec<(usize, f64)>,
    lead_scale: f64,
    mix_scale: f64,
    frames: u64,
    lead_k: f64,
    lead_bands: [f64; 10],
    mix_bands: [f64; 10],
    other_bands: [f64; 10],
}

impl LeadWindow {
    pub fn new(
        address: String,
        index: usize,
        lead: &Lead,
        channels: &[u8],
        start: u64,
        end: u64,
    ) -> Self {
        let scale = |source: usize| 1.0 / f64::from(channels[source]);
        Self {
            address,
            lead: index,
            start,
            end,
            source: lead.source,
            mix: lead.mix,
            others: lead
                .others
                .iter()
                .map(|&other| (other, scale(other)))
                .collect(),
            lead_scale: scale(lead.source),
            mix_scale: scale(lead.mix),
            frames: 0,
            lead_k: 0.0,
            lead_bands: [0.0; 10],
            mix_bands: [0.0; 10],
            other_bands: [0.0; 10],
        }
    }

    /// Add one frame of every source. Band energies are per channel, so that
    /// a mono lead and a stereo mix point compare as the share does.
    pub fn add(&mut self, values: &[FrameValues]) {
        let lead = &values[self.source];
        self.frames += 1;
        self.lead_k += lead.k;
        for band in 0..10 {
            self.lead_bands[band] += lead.bands[band] * self.lead_scale;
            self.mix_bands[band] += values[self.mix].bands[band] * self.mix_scale;
            for &(other, scale) in &self.others {
                self.other_bands[band] += values[other].bands[band] * scale;
            }
        }
    }

    /// The note's margin in dB, or why it has none.
    pub fn outcome(&self, limit: f64) -> (Option<f64>, &'static str) {
        let level = (self.frames > 0 && self.lead_k > 0.0)
            .then(|| loudness(self.lead_k / self.frames as f64).ok())
            .flatten();
        let band = (0..10)
            .max_by(|&a, &b| self.lead_bands[a].total_cmp(&self.lead_bands[b]))
            .expect("ten bands");
        let (lead, mix) = (self.lead_bands[band], self.mix_bands[band]);
        if level.is_none_or(|level| level <= SILENT_LUFS) || lead <= 0.0 {
            return (None, "silent");
        }
        let rest = mix - lead - self.other_bands[band];
        if rest <= CLEAR_SHARE * mix {
            return (None, "clear");
        }
        let margin = round2(10.0 * (lead / rest).log10());
        (Some(margin), if margin <= limit { "under" } else { "ok" })
    }
}

/// One lead note, as listed with `--notes`.
#[derive(Clone, Debug, Serialize)]
pub struct LeadNote {
    /// The event's source address.
    pub event: String,
    /// The lead, `node:port`.
    pub lead: String,
    pub start_frame: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// The lead's energy over the rest of its mix point in its strongest
    /// band, in dB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin_db: Option<f64>,
    /// `ok`, `under` (at or below the limit), `clear` (nothing else sounds in
    /// the lead's band), or `silent`.
    pub status: &'static str,
}

/// A lead's notes over a span: how far it sits over or under the rest of its
/// mix point.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct LeadSummary {
    pub mix_point: String,
    /// Notes measured; silent ones are left to `silent_note`.
    pub notes: usize,
    /// Notes with nothing else in the lead's band.
    pub clear: usize,
    /// The median margin, the lower middle one for an even count, with clear
    /// notes above every margin. `None` when the median note is clear.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub median_db: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_db: Option<f64>,
    /// Notes at or below the limit.
    pub under: usize,
}

/// The summary of `notes`, all of one lead; `None` when none was measured.
pub(super) fn summary<'n>(
    mix_point: &str,
    notes: impl Iterator<Item = &'n LeadNote>,
) -> Option<LeadSummary> {
    let measured: Vec<&LeadNote> = notes.filter(|note| note.status != "silent").collect();
    if measured.is_empty() {
        return None;
    }
    let mut margins: Vec<f64> = measured.iter().filter_map(|note| note.margin_db).collect();
    margins.sort_by(f64::total_cmp);
    let middle = (measured.len() - 1) / 2;
    Some(LeadSummary {
        mix_point: mix_point.to_owned(),
        notes: measured.len(),
        clear: measured.len() - margins.len(),
        median_db: margins.get(middle).copied(),
        min_db: margins.first().copied(),
        under: measured
            .iter()
            .filter(|note| note.status == "under")
            .count(),
    })
}
