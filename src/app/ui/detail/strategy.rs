use crate::data::backtest::BacktestReport;

pub(super) struct EvidenceDisplay {
    pub summary: String,
    pub evidence: String,
    pub execution: String,
}

impl EvidenceDisplay {
    pub fn from_report(report: &BacktestReport, work: bool) -> Self {
        Self {
            summary: report.summary_line(work),
            evidence: report.evidence_line(work),
            execution: report.notes.first().cloned().unwrap_or_default(),
        }
    }
}
