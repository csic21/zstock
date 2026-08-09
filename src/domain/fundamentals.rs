use serde::{Deserialize, Serialize};

use super::market::Market;
use super::money::Currency;

pub const REQUIRED_QUALITY_METRICS: &[&str] = &[
    "roe_pct",
    "roic_pct",
    "operating_cash_to_profit",
    "debt_ratio_pct",
    "revenue_growth_pct",
    "profit_growth_pct",
    "goodwill_ratio_pct",
    "audit_risk_flag",
];

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FundamentalSnapshot {
    pub code: String,
    pub market: Market,
    pub currency: Currency,
    pub fetched_at: i64,
    pub source: String,
    pub metrics: Vec<ReportedMetric>,
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
    for name in REQUIRED_QUALITY_METRICS {
        let metric = metrics
            .iter()
            .filter(|metric| metric.name == *name && metric.available_on(signal_date))
            .max_by(|left, right| {
                (&left.reporting_period, &left.announced_on)
                    .cmp(&(&right.reporting_period, &right.announced_on))
            });
        let Some(metric) = metric else {
            unknown.push(format!("{}未知", metric_label(name)));
            continue;
        };
        let value = metric.value.unwrap_or_default();
        match *name {
            "roe_pct" if value < 0.0 => blockers.push("ROE 为负".into()),
            "roic_pct" if value < 0.0 => blockers.push("ROIC 为负".into()),
            "debt_ratio_pct" if value > 85.0 => blockers.push("资产负债率过高".into()),
            "operating_cash_to_profit" if value < 0.3 => {
                blockers.push("经营现金流与利润明显背离".into())
            }
            "revenue_growth_pct" if value < -30.0 => blockers.push("营收大幅下降".into()),
            "profit_growth_pct" if value < -30.0 => blockers.push("利润大幅下降".into()),
            "goodwill_ratio_pct" if value > 35.0 => blockers.push("商誉占比过高".into()),
            "audit_risk_flag" if value >= 1.0 => blockers.push("审计意见存在风险".into()),
            _ => {}
        }
    }
    QualityGate {
        passed: blockers.is_empty() && unknown.is_empty(),
        blockers,
        unknown,
    }
}

pub fn metric_label(name: &str) -> &str {
    match name {
        "roe_pct" => "ROE",
        "roic_pct" => "ROIC",
        "operating_cash_to_profit" => "经营现金流/净利润",
        "debt_ratio_pct" => "资产负债率",
        "revenue_growth_pct" => "营收同比",
        "profit_growth_pct" => "利润同比",
        "goodwill_ratio_pct" => "商誉/总资产",
        "audit_risk_flag" => "审计意见",
        _ => "基本面指标",
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
        assert!(gate.unknown.iter().any(|item| item.contains("负债率")));
    }

    #[test]
    fn value_trap_red_line_blocks_even_when_other_metrics_are_strong() {
        let mut metrics = complete_metrics("2026-03-20");
        metrics
            .iter_mut()
            .find(|metric| metric.name == "debt_ratio_pct")
            .unwrap()
            .value = Some(91.0);
        let gate = quality_gate(&metrics, "2026-04-01");
        assert!(!gate.passed);
        assert!(gate.blockers.iter().any(|reason| reason.contains("负债")));
    }

    #[test]
    fn complete_point_in_time_evidence_can_pass() {
        let gate = quality_gate(&complete_metrics("2026-03-20"), "2026-04-01");
        assert!(gate.passed);
        assert!(gate.blockers.is_empty());
        assert!(gate.unknown.is_empty());
    }

    #[test]
    fn missing_required_metric_is_unknown() {
        let mut metrics = complete_metrics("2026-03-20");
        metrics.retain(|metric| metric.name != "audit_risk_flag");
        let gate = quality_gate(&metrics, "2026-04-01");
        assert!(!gate.passed);
        assert!(gate.unknown.iter().any(|item| item.contains("审计")));
    }

    fn complete_metrics(announced_on: &str) -> Vec<ReportedMetric> {
        REQUIRED_QUALITY_METRICS
            .iter()
            .map(|name| ReportedMetric {
                name: (*name).into(),
                value: Some(match *name {
                    "roe_pct" => 12.0,
                    "roic_pct" => 10.0,
                    "operating_cash_to_profit" => 0.9,
                    "debt_ratio_pct" => 40.0,
                    "revenue_growth_pct" => 8.0,
                    "profit_growth_pct" => 6.0,
                    "goodwill_ratio_pct" => 3.0,
                    "audit_risk_flag" => 0.0,
                    _ => unreachable!(),
                }),
                unit: if name.ends_with("_pct") { "%" } else { "ratio" }.into(),
                reporting_period: "2025-12-31".into(),
                announced_on: announced_on.into(),
                source: "fixture".into(),
            })
            .collect()
    }
}
