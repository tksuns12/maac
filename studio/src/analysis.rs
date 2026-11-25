//! Background `maac::analyze` runner for the Studio analysis sidebar.

use std::time::Instant;

use maac::analyze::{analyze_artifact, AnalysisReport, AnalyzeOptions};
use maac::music::MeterMap;
use maac::plan::PlanLimits;
use maac::PlanArtifact;

#[derive(Clone, Debug)]
pub struct SectionMetric {
    pub id: String,
    pub label: Option<String>,
    pub start_frame: u64,
    pub end_frame: u64,
    pub lufs: Option<f64>,
    pub true_peak: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct SourceMetric {
    pub port: String,
    pub role: String,
    pub lufs: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct FindingItem {
    pub severity: String,
    pub kind: String,
    pub source: String,
    pub section: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct AnalysisSummary {
    pub integrated_lufs: Option<f64>,
    pub true_peak_dbtp: Option<f64>,
    pub crest_db: Option<f64>,
    pub clipped_samples: u64,
    pub sections: Vec<SectionMetric>,
    pub sources: Vec<SourceMetric>,
    pub findings: Vec<FindingItem>,
    pub elapsed_ms: u64,
}

pub fn run_analysis(
    plan: PlanArtifact,
    meter: Option<MeterMap>,
    limits: PlanLimits,
) -> Result<AnalysisSummary, String> {
    let started = Instant::now();
    let options = AnalyzeOptions::default();
    let report = analyze_artifact(&plan, meter.as_ref(), &options, &limits)
        .map_err(|err| format!("{}: {}", err.code, err.message))?;
    Ok(summarize(report, started.elapsed().as_millis() as u64))
}

fn summarize(report: AnalysisReport, elapsed_ms: u64) -> AnalysisSummary {
    let output_source = report.sources.iter().find(|s| s.role == "output");
    let (integrated_lufs, true_peak_dbtp, crest_db, clipped_samples) = match output_source {
        Some(out) => (
            out.whole.integrated_lufs,
            out.whole.true_peak_dbtp,
            out.whole.crest_db,
            out.whole.clipped_samples,
        ),
        None => (None, None, None, 0),
    };

    let sections = report
        .sections
        .iter()
        .map(|sec| {
            let meas = output_source
                .and_then(|out| out.sections.iter().find(|sm| sm.section == sec.id))
                .map(|sm| &sm.measures);
            SectionMetric {
                id: sec.id.clone(),
                label: sec.label.clone(),
                start_frame: sec.start_frame,
                end_frame: sec.end_frame,
                lufs: meas.and_then(|m| m.integrated_lufs),
                true_peak: meas.and_then(|m| m.true_peak_dbtp),
            }
        })
        .collect();

    let sources = report
        .sources
        .iter()
        .filter(|s| s.role == "output" || s.role == "stem" || s.role == "bus")
        .map(|s| SourceMetric {
            port: s.port.clone(),
            role: s.role.to_string(),
            lufs: s.whole.integrated_lufs,
        })
        .collect();

    let findings = report
        .findings
        .iter()
        .map(|f| FindingItem {
            severity: f.severity.to_string(),
            kind: f.kind.to_string(),
            source: f.source.clone(),
            section: f.section.clone(),
            message: f.message.clone(),
        })
        .collect();

    AnalysisSummary {
        integrated_lufs,
        true_peak_dbtp,
        crest_db,
        clipped_samples,
        sections,
        sources,
        findings,
        elapsed_ms,
    }
}
