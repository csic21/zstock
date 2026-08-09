use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportedMetric {
    pub name: String,
    pub value: Option<f64>,
    pub unit: String,
    pub reporting_period: String,
    pub announced_on: String,
    pub source: String,
}

impl ReportedMetric {
    pub fn available_on(&self, signal_date: &str) -> bool {
        self.value.is_some() && self.announced_on.as_str() <= signal_date
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualityGate {
    pub passed: bool,
    pub blockers: Vec<String>,
    pub unknown: Vec<String>,
}

pub fn quality_gate(metrics: &[ReportedMetric], signal_date: &str) -> QualityGate {
    let mut blockers = Vec::new();
    let mut unknown = Vec::new();
    for metric in metrics {
        if !metric.available_on(signal_date) {
            unknown.push(metric.name.clone());
            continue;
        }
        let value = metric.value.unwrap_or_default();
        match metric.name.as_str() {
            "debt_ratio_pct" if value > 85.0 => blockers.push("资产负债率过高".into()),
            "operating_cash_to_profit" if value < 0.3 => {
                blockers.push("经营现金流与利润明显背离".into())
            }
            "goodwill_ratio_pct" if value > 35.0 => blockers.push("商誉占比过高".into()),
            _ => {}
        }
    }
    QualityGate {
        passed: blockers.is_empty() && unknown.is_empty(),
        blockers,
        unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn future_announcement_is_unknown_not_passed() {
        let metric = ReportedMetric {
            name: "debt_ratio_pct".into(),
            value: Some(10.0),
            unit: "%".into(),
            reporting_period: "2026Q2".into(),
            announced_on: "2026-08-30".into(),
            source: "fixture".into(),
        };
        let gate = quality_gate(&[metric], "2026-08-01");
        assert!(!gate.passed);
        assert_eq!(gate.unknown, vec!["debt_ratio_pct"]);
    }

    #[test]
    fn value_trap_red_line_blocks_even_when_other_metrics_are_strong() {
        let metrics = vec![
            ReportedMetric {
                name: "debt_ratio_pct".into(),
                value: Some(91.0),
                unit: "%".into(),
                reporting_period: "2025FY".into(),
                announced_on: "2026-03-20".into(),
                source: "fixture".into(),
            },
            ReportedMetric {
                name: "operating_cash_to_profit".into(),
                value: Some(1.2),
                unit: "ratio".into(),
                reporting_period: "2025FY".into(),
                announced_on: "2026-03-20".into(),
                source: "fixture".into(),
            },
        ];
        let gate = quality_gate(&metrics, "2026-04-01");
        assert!(!gate.passed);
        assert!(gate.blockers.iter().any(|reason| reason.contains("负债")));
    }
}
