//! Dynamics processors: how much each `fx.compressor/1` or `fx.limiter/1`
//! takes from each section.
//!
//! The renderer reports each processor's gain reduction after every frame: a
//! compressor's smoothed reduction before its makeup, or a limiter's leaving
//! out its drive. A span's reduction is the loudness the processor takes from
//! it: its K-weighted output power scaled back up by the reduction at each
//! frame, over the output power, in dB. Quiet frames count for little, as
//! they do in loudness, and an automated makeup or drive is the author's
//! level change, not a reduction.

use std::collections::BTreeSet;

use serde::Serialize;

use super::round2;
use crate::plan::{PlanView, Processor, ProcessorView};

/// A processor's output power and reduction over a span of frames.
#[derive(Clone, Copy, Default)]
pub(super) struct Span {
    /// The K-weighted output power.
    output: f64,
    /// The same, scaled back up by the reduction at each frame.
    input: f64,
    max: f64,
}

impl Span {
    fn add(&mut self, output: f64, input: f64, reduction: f64) {
        self.output += output;
        self.input += input;
        self.max = self.max.max(reduction);
    }

    pub fn summary(&self) -> Dynamics {
        Dynamics {
            reduction_db: (self.output > 0.0)
                .then(|| round2(10.0 * (self.input / self.output).log10())),
            max_reduction_db: round2(self.max),
        }
    }
}

/// A processor's reduction over the analysed frames and in each section.
/// Sections are summed by their exact frames, not by blocks, so a section's
/// maximum does not take in the start of the next.
pub(super) struct Tap {
    pub source: usize,
    pub whole: Span,
    pub sections: Vec<Span>,
}

impl Tap {
    pub fn new(source: usize, sections: usize) -> Self {
        Self {
            source,
            whole: Span::default(),
            sections: vec![Span::default(); sections],
        }
    }

    /// Add one frame: the output's K-weighted power, the reduction in dB, and
    /// the sections the frame lies in.
    pub fn add(&mut self, power: f64, reduction: f64, sections: &[usize]) {
        let input = if reduction > 0.0 {
            power * 10.0_f64.powf(reduction / 10.0)
        } else {
            power
        };
        self.whole.add(power, input, reduction);
        for &section in sections {
            self.sections[section].add(power, input, reduction);
        }
    }
}

/// What a compressor or limiter takes over a span.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Dynamics {
    /// The loudness the processor takes, in dB: its gain reduction weighted
    /// by the power of its input, leaving out makeup and drive. Absent when
    /// the processor's output is silent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reduction_db: Option<f64>,
    /// The largest gain reduction at any frame, in dB.
    pub max_reduction_db: f64,
}

/// Whether `node` is a compressor or a limiter.
pub(super) fn is_dynamics(view: &PlanView<'_>, node: &str) -> bool {
    view.nodes.iter().any(|candidate| {
        *candidate.id == node
            && matches!(
                candidate.processor,
                ProcessorView::Core(Processor::Compressor { .. } | Processor::Limiter { .. })
            )
    })
}

/// The compressors and limiters between the last mix point and the project
/// output, in signal order. The walk goes upstream from the output through
/// nodes with one main audio input, and stops at the first node with several
/// or none. A compressor's sidechain is not a main input.
pub(super) fn master_chain(view: &PlanView<'_>) -> Vec<String> {
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = view.output.output.node.as_str();
    while seen.insert(current) {
        if is_dynamics(view, current) {
            chain.push(current.to_owned());
        }
        let mut inputs = view.connections.iter().filter(|connection| {
            connection.to.node == current && connection.to.port != "sidechain"
        });
        match (inputs.next(), inputs.next()) {
            (Some(input), None) => current = input.from.node.as_str(),
            _ => break,
        }
    }
    chain.reverse();
    chain
}
