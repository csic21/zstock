//! Free A 股 + 港股 market data via Eastmoney public HTTP APIs (no API key).
//!
//! Endpoints (unofficial, same as used by many open-source tools e.g. AKShare):
//! - Quotes: `push2.eastmoney.com/api/qt/ulist.np/get`（港股 secid `116.xxxxx`，delay 节点更稳）
//! - History K: `push2his.eastmoney.com/api/qt/stock/kline/get`（港股 K 常空，靠腾讯备源）
//! - Search: `searchapi.eastmoney.com/api/suggest/get`（含港股）
//!
//! These are free for personal tooling but have no SLA; rate-limit politely.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{
    LazyLock,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::domain::market::{Availability, Freshness};
use crate::domain::money::Currency;
use crate::model::{
    Candle, Symbol, board_for_code, is_hk_code, normalize_code, secid_for_code, shared,
};

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

#[derive(Debug, Clone)]
pub struct QuoteTick {
    pub code: String,
    pub name: String,
    pub last: f64,
    pub change_pct: f64,
    pub volume: u64,
    /// Turnover amount when supplied by the quote backend.
    pub amount: f64,
    pub currency: Currency,
    pub source: String,
    pub fetched_at: i64,
    pub market_time: Option<String>,
    pub availability: Availability,
    pub freshness: Freshness,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FundamentalReportRow {
    pub reporting_period: String,
    pub announced_on: String,
    pub is_annual: bool,
    pub currency: Currency,
    pub roe_pct: Option<f64>,
    pub roic_pct: Option<f64>,
    pub operating_cash_to_profit: Option<f64>,
    pub debt_ratio_pct: Option<f64>,
    pub revenue_growth_pct: Option<f64>,
    pub profit_growth_pct: Option<f64>,
    pub goodwill_ratio_pct: Option<f64>,
    pub audit_risk_flag: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DividendContinuityPoint {
    pub fiscal_year: i32,
    pub announced_on: String,
    pub consecutive_years: Option<u32>,
}

/// A 股行业板块的实时快照。
///
/// Eastmoney 的板块列表接口同时返回涨跌幅、成交额和涨跌家数，足够支撑
/// 市场分析页的热度榜与市场宽度概览。
#[derive(Debug, Clone)]
pub struct SectorTick {
    pub code: String,
    pub name: String,
    pub change_pct: f64,
    pub amount: f64,
    pub advances: u64,
    pub declines: u64,
    pub unchanged: u64,
}

/// One mutually-exclusive Shenwan level-2 industry inside a level-1 sector.
#[derive(Debug, Clone)]
pub struct IndustryStockGroup {
    pub name: String,
    pub amount: f64,
    pub stocks: Vec<QuoteTick>,
}

/// Complete, paginated stock membership for one Shenwan level-1 industry.
#[derive(Debug, Clone)]
pub struct IndustryHeatmapSector {
    pub sector: SectorTick,
    pub industries: Vec<IndustryStockGroup>,
}

/// Process-wide agent so keep-alive sockets are reused across quote polls.
static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(15))
        .build()
});

fn agent() -> ureq::Agent {
    AGENT.clone()
}

/// Eastmoney push2 nodes (round-robin / fallback).
const PUSH2_HOSTS: &[&str] = &[
    "push2.eastmoney.com",
    "82.push2.eastmoney.com",
    "80.push2.eastmoney.com",
    "push2delay.eastmoney.com",
];

const CLIST_PAGE_SIZE: usize = 100;
const MAX_SECTOR_CONSTITUENTS: usize = 1000;
const HEATMAP_FETCH_CONCURRENCY: usize = 6;
/// Eastmoney's own first-level industry taxonomy. Keeping one level avoids
/// counting parent and child boards as siblings in a treemap.
const EASTMONEY_LEVEL_ONE_INDUSTRIES: &str = "m:90%2Bs:2%2Bf:!50";
const EASTMONEY_LEVEL_TWO_INDUSTRIES: &str = "m:90%2Bs:4%2Bf:!50";
const EASTMONEY_A_SHARE_UNIVERSE: &str = "m:0%2Bt:6,m:0%2Bt:80,m:1%2Bt:2,m:1%2Bt:23";

/// Prefer delay node first when the request includes 港股（主节点对 116.* 常 empty-reply）。
const PUSH2_HOSTS_HK: &[&str] = &[
    "push2delay.eastmoney.com",
    "push2.eastmoney.com",
    "82.push2.eastmoney.com",
    "80.push2.eastmoney.com",
];

fn get_json(url: &str) -> Result<Value> {
    let body = agent()
        .get(url)
        .set("User-Agent", UA)
        .set("Referer", "https://quote.eastmoney.com/")
        .call()
        .map_err(|e| anyhow!("{}", short_http_err(&e.to_string())))?
        .into_string()
        .context("read body")?;
    serde_json::from_str(&body).context("parse json")
}

/// Point-in-time financial quality reports for A shares.
///
/// The main-finance and balance-sheet responses both carry `REPORT_DATE` and
/// `NOTICE_DATE`. The later announcement date is retained when the two tables
/// disagree, so no balance-sheet field can leak into an earlier signal date.
pub fn fetch_fundamental_reports(
    code: &str,
    report_limit: usize,
) -> Result<Vec<FundamentalReportRow>> {
    let secu_code = finance_secu_code(code)?;
    let limit = report_limit.clamp(1, 20);
    let main = fetch_finance_table(
        &secu_code,
        "RPT_F10_FINANCE_MAINFINADATA",
        "APP_F10_MAINFINADATA",
        limit,
    )?;
    let balance = fetch_finance_table(
        &secu_code,
        "RPT_F10_FINANCE_GBALANCE",
        "SECUCODE,REPORT_DATE,NOTICE_DATE,CURRENCY,GOODWILL,TOTAL_ASSETS,OPINION_TYPE,OSOPINION_TYPE",
        limit,
    )?;
    parse_fundamental_reports(&main, &balance)
}

fn fetch_finance_table(secu_code: &str, report: &str, style: &str, limit: usize) -> Result<Value> {
    let filter = urlencoding_minimal(&format!("(SECUCODE=\"{secu_code}\")"));
    let url = format!(
        "https://datacenter.eastmoney.com/securities/api/data/get?type={report}\
         &sty={}&filter={filter}&p=1&ps={limit}&sr=-1&st=REPORT_DATE&source=HSF10&client=PC",
        urlencoding_minimal(style)
    );
    let value = get_json(&url)?;
    if !value
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        bail!(
            "financial endpoint failed: {}",
            value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown response")
        );
    }
    Ok(value)
}

fn parse_fundamental_reports(main: &Value, balance: &Value) -> Result<Vec<FundamentalReportRow>> {
    let balance_rows = response_rows(balance)?;
    let mut balance_by_period = HashMap::new();
    for row in balance_rows {
        let period = iso_date(row.get("REPORT_DATE"), "balance REPORT_DATE")?;
        let announced = iso_date(row.get("NOTICE_DATE"), "balance NOTICE_DATE")?;
        let goodwill = optional_f64(row.get("GOODWILL"));
        let total_assets = optional_f64(row.get("TOTAL_ASSETS"));
        let goodwill_ratio_pct = goodwill
            .zip(total_assets)
            .and_then(|(goodwill, assets)| (assets > 0.0).then_some(goodwill / assets * 100.0));
        let audit_opinion = row
            .get("OPINION_TYPE")
            .or_else(|| row.get("OSOPINION_TYPE"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty());
        let audit_risk_flag = audit_opinion.map(|opinion| {
            if opinion.contains("标准无保留") {
                0.0
            } else {
                1.0
            }
        });
        balance_by_period.insert(period, (announced, goodwill_ratio_pct, audit_risk_flag));
    }

    let mut reports = Vec::new();
    for row in response_rows(main)? {
        let reporting_period = iso_date(row.get("REPORT_DATE"), "main REPORT_DATE")?;
        let main_announced = iso_date(row.get("NOTICE_DATE"), "main NOTICE_DATE")?;
        let (announced_on, goodwill_ratio_pct, audit_risk_flag) = balance_by_period
            .get(&reporting_period)
            .map(|(balance_announced, goodwill, audit)| {
                (
                    main_announced.clone().max(balance_announced.clone()),
                    *goodwill,
                    *audit,
                )
            })
            .unwrap_or((main_announced, None, None));
        let currency = match row.get("CURRENCY").and_then(Value::as_str) {
            Some("CNY") | None => Currency::Cny,
            Some(other) => bail!("unsupported financial currency {other}"),
        };
        let is_annual = reporting_period.ends_with("-12-31");
        reports.push(FundamentalReportRow {
            reporting_period,
            announced_on,
            is_annual,
            currency,
            roe_pct: optional_f64(row.get("ROEJQ")),
            roic_pct: optional_f64(row.get("ROIC")),
            operating_cash_to_profit: optional_f64(row.get("NCO_NETPROFIT")),
            debt_ratio_pct: optional_f64(row.get("ZCFZL")),
            revenue_growth_pct: optional_f64(row.get("TOTALOPERATEREVETZ")),
            profit_growth_pct: optional_f64(row.get("PARENTNETPROFITTZ")),
            goodwill_ratio_pct,
            audit_risk_flag,
        });
    }
    if reports.is_empty() {
        bail!("financial endpoint returned no reports");
    }
    Ok(reports)
}

/// Point-in-time financial quality reports for Hong Kong shares.
///
/// Eastmoney's Hong Kong indicator table has reporting periods but no release
/// date. We therefore match every period to the first HK announcement carrying
/// the corresponding final/interim/quarterly-results category. Rows without a
/// trustworthy match are omitted instead of borrowing the reporting date.
pub fn fetch_hk_fundamental_reports(
    code: &str,
    report_limit: usize,
) -> Result<Vec<FundamentalReportRow>> {
    let normalized = normalize_code(code)
        .filter(|code| is_hk_code(code))
        .ok_or_else(|| anyhow!("Hong Kong financial provider requires a 5-digit code"))?;
    let secu_code = format!("{normalized}.HK");
    let limit = report_limit.clamp(1, 20);
    let filter = format!("(SECUCODE=\"{secu_code}\")");
    let main = fetch_hk_finance_table(
        "RPT_HKF10_FN_MAININDICATOR",
        "HKF10_FN_MAININDICATOR",
        &filter,
        limit,
        "-1",
        "REPORT_DATE",
    )?;
    let periods = response_rows(&main)?
        .iter()
        .filter_map(|row| {
            row.get("REPORT_DATE")
                .and_then(Value::as_str)
                .and_then(|date| date.get(..10))
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    let Some(earliest_period) = periods.iter().min().cloned() else {
        bail!("Hong Kong financial endpoint returned no reports");
    };
    let quoted_periods = periods
        .iter()
        .map(|period| format!("'{period}'"))
        .collect::<Vec<_>>()
        .join(",");
    let balance_filter = format!("(SECUCODE=\"{secu_code}\")(REPORT_DATE in ({quoted_periods}))");
    let balance = fetch_hk_finance_table(
        "RPT_HKF10_FN_BALANCE_PC",
        "SECUCODE,REPORT_DATE,STD_ITEM_CODE,STD_ITEM_NAME,AMOUNT",
        &balance_filter,
        5_000,
        "-1,1",
        "REPORT_DATE,STD_ITEM_CODE",
    )?;
    let notices = fetch_hk_result_notices(&normalized, &earliest_period)?;
    parse_hk_fundamental_reports(&main, &balance, &notices)
}

fn fetch_hk_finance_table(
    report_name: &str,
    columns: &str,
    filter: &str,
    page_size: usize,
    sort_types: &str,
    sort_columns: &str,
) -> Result<Value> {
    let url = format!(
        "https://datacenter.eastmoney.com/securities/api/data/v1/get?\
         reportName={}&columns={}&quoteColumns=&filter={}&pageNumber=1&pageSize={page_size}\
         &sortTypes={}&sortColumns={}&source=F10&client=PC",
        urlencoding_minimal(report_name),
        urlencoding_minimal(columns),
        urlencoding_minimal(filter),
        urlencoding_minimal(sort_types),
        urlencoding_minimal(sort_columns),
    );
    let value = get_json(&url)?;
    if !value
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        bail!(
            "Hong Kong financial endpoint failed: {}",
            value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown response")
        );
    }
    Ok(value)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HkResultNotice {
    announced_on: String,
    category_codes: Vec<String>,
}

fn fetch_hk_result_notices(code: &str, begin_time: &str) -> Result<Vec<HkResultNotice>> {
    const PAGE_SIZE: usize = 100;
    let end_time = chrono::Utc::now().date_naive().to_string();
    let mut notices = Vec::new();
    let mut page = 1usize;
    let mut pages = 1usize;
    while page <= pages {
        let url = format!(
            "https://np-anotice-stock.eastmoney.com/api/security/ann?sr=-1&page_size={PAGE_SIZE}\
             &page_index={page}&ann_type=H&client_source=web&stock_list={}\
             &begin_time={}&end_time={}",
            urlencoding_minimal(code),
            urlencoding_minimal(begin_time),
            urlencoding_minimal(&end_time),
        );
        let value = get_json(&url)?;
        if value.get("success").and_then(Value::as_i64) != Some(1) {
            bail!(
                "Hong Kong announcement endpoint failed: {}",
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown response")
            );
        }
        let total_hits = value
            .pointer("/data/total_hits")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        pages = total_hits.div_ceil(PAGE_SIZE).max(1);
        notices.extend(parse_hk_result_notices(&value)?);
        page += 1;
    }
    Ok(notices)
}

fn parse_hk_result_notices(value: &Value) -> Result<Vec<HkResultNotice>> {
    let rows = value
        .pointer("/data/list")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Hong Kong announcement response missing data.list"))?;
    let mut notices = Vec::new();
    for row in rows {
        let category_codes = row
            .get("columns")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|column| column.get("column_code").and_then(Value::as_str))
            .filter(|code| matches!(*code, "011001003005" | "011001003007" | "011001003011"))
            .map(str::to_string)
            .collect::<Vec<_>>();
        if category_codes.is_empty() {
            continue;
        }
        notices.push(HkResultNotice {
            announced_on: iso_date(row.get("notice_date"), "notice_date")?,
            category_codes,
        });
    }
    Ok(notices)
}

fn parse_hk_fundamental_reports(
    main: &Value,
    balance: &Value,
    notices: &[HkResultNotice],
) -> Result<Vec<FundamentalReportRow>> {
    let mut balance_by_period: HashMap<String, (Option<f64>, Option<f64>)> = HashMap::new();
    for row in response_rows(balance)? {
        let period = iso_date(row.get("REPORT_DATE"), "HK balance REPORT_DATE")?;
        let entry = balance_by_period.entry(period).or_default();
        match row.get("STD_ITEM_CODE").and_then(Value::as_str) {
            Some("004001005") => entry.0 = optional_f64(row.get("AMOUNT")),
            Some("004009999") => entry.1 = optional_f64(row.get("AMOUNT")),
            _ => {}
        }
    }

    let mut reports = Vec::new();
    for row in response_rows(main)? {
        let reporting_period = iso_date(row.get("REPORT_DATE"), "HK main REPORT_DATE")?;
        let date_type = row
            .get("DATE_TYPE_CODE")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Hong Kong financial response missing DATE_TYPE_CODE"))?;
        let Some(announced_on) = match_hk_result_notice(&reporting_period, date_type, notices)?
        else {
            continue;
        };
        let currency = match row.get("CURRENCY").and_then(Value::as_str) {
            Some("HKD") | None => Currency::Hkd,
            Some("CNY") | Some("RMB") => Currency::Cny,
            Some(other) => bail!("unsupported Hong Kong financial currency {other}"),
        };
        let operating_cash_to_profit = optional_f64(row.get("PER_NETCASH_OPERATE"))
            .zip(optional_f64(row.get("BASIC_EPS")))
            .and_then(|(cash, earnings)| {
                (earnings.abs() > f64::EPSILON).then_some(cash / earnings)
            });
        let goodwill_ratio_pct = balance_by_period
            .get(&reporting_period)
            .and_then(|(goodwill, assets)| goodwill.zip(*assets))
            .and_then(|(goodwill, assets)| (assets > 0.0).then_some(goodwill / assets * 100.0));
        reports.push(FundamentalReportRow {
            reporting_period,
            announced_on,
            is_annual: date_type == "001",
            currency,
            roe_pct: optional_f64(row.get("ROE_YEARLY")),
            roic_pct: optional_f64(row.get("ROIC_YEARLY")),
            operating_cash_to_profit,
            debt_ratio_pct: optional_f64(row.get("DEBT_ASSET_RATIO")),
            revenue_growth_pct: optional_f64(row.get("OPERATE_INCOME_YOY")),
            profit_growth_pct: optional_f64(row.get("HOLDER_PROFIT_YOY")),
            goodwill_ratio_pct,
            // The free structured endpoint does not carry the auditor's opinion.
            // Keeping this absent is safer than inferring it from the auditor name.
            audit_risk_flag: None,
        });
    }
    if reports.is_empty() {
        bail!("no Hong Kong report had a traceable results announcement date");
    }
    Ok(reports)
}

fn match_hk_result_notice(
    reporting_period: &str,
    date_type: &str,
    notices: &[HkResultNotice],
) -> Result<Option<String>> {
    let expected_category = match date_type {
        "001" => "011001003005",         // final results
        "002" => "011001003007",         // interim results
        "003" | "004" => "011001003011", // quarterly results
        _ => return Ok(None),
    };
    let period = chrono::NaiveDate::parse_from_str(reporting_period, "%Y-%m-%d")
        .context("invalid Hong Kong reporting period")?;
    let latest = (period + chrono::Duration::days(210)).to_string();
    Ok(notices
        .iter()
        .filter(|notice| {
            notice
                .category_codes
                .iter()
                .any(|code| code == expected_category)
                && notice.announced_on.as_str() > reporting_period
                && notice.announced_on <= latest
        })
        .map(|notice| notice.announced_on.clone())
        .min())
}

pub fn fetch_dividend_continuity(
    code: &str,
    annual_reports: &[FundamentalReportRow],
) -> Result<Vec<DividendContinuityPoint>> {
    let normalized = normalize_code(code)
        .ok_or_else(|| anyhow!("dividend provider requires a valid stock code"))?;
    let value = if is_hk_code(&normalized) {
        fetch_hk_dividends(&normalized)?
    } else {
        fetch_a_share_dividends(&normalized)?
    };
    parse_dividend_continuity(&value, is_hk_code(&normalized), annual_reports)
}

fn fetch_hk_dividends(code: &str) -> Result<Value> {
    let filter = format!("(SECURITY_CODE=\"{code}\")(IS_BFP=\"0\")");
    fetch_hk_finance_table(
        "RPT_HKF10_MAIN_DIVBASIC",
        "SECURITY_CODE,UPDATE_DATE,NOTICE_DATE,REPORT_TYPE,YEAR,PLAN_EXPLAIN,IS_BFP",
        &filter,
        500,
        "-1,-1",
        "NOTICE_DATE,EX_DIVIDEND_DATE",
    )
}

fn fetch_a_share_dividends(code: &str) -> Result<Value> {
    let filter = urlencoding_minimal(&format!("(SECURITY_CODE=\"{code}\")"));
    let url = format!(
        "https://datacenter-web.eastmoney.com/api/data/v1/get?sortColumns=REPORT_DATE\
         &sortTypes=-1&pageSize=500&pageNumber=1&reportName=RPT_SHAREBONUS_DET\
         &columns=ALL&quoteColumns=&source=WEB&client=WEB&filter={filter}"
    );
    let value = get_json(&url)?;
    if !value
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        bail!("A-share dividend endpoint failed");
    }
    Ok(value)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DividendStateEvent {
    fiscal_year: i32,
    announced_on: String,
    paid: Option<bool>,
}

fn parse_dividend_continuity(
    value: &Value,
    is_hong_kong: bool,
    annual_reports: &[FundamentalReportRow],
) -> Result<Vec<DividendContinuityPoint>> {
    let mut events = Vec::<DividendStateEvent>::new();
    for row in response_rows(value)? {
        let event = if is_hong_kong {
            parse_hk_dividend_event(row)?
        } else {
            parse_a_share_dividend_event(row)?
        };
        if let Some(event) = event {
            events.push(event);
        }
    }
    for report in annual_reports.iter().filter(|report| report.is_annual) {
        let fiscal_year = report
            .reporting_period
            .get(..4)
            .and_then(|year| year.parse::<i32>().ok())
            .ok_or_else(|| anyhow!("invalid annual reporting period"))?;
        let known_at_report = events.iter().any(|event| {
            event.fiscal_year == fiscal_year && event.announced_on <= report.announced_on
        });
        if !known_at_report {
            events.push(DividendStateEvent {
                fiscal_year,
                announced_on: report.announced_on.clone(),
                paid: None,
            });
        }
    }
    events.sort_by(|left, right| {
        (&left.announced_on, left.fiscal_year, left.paid).cmp(&(
            &right.announced_on,
            right.fiscal_year,
            right.paid,
        ))
    });
    events.dedup();

    let mut state = HashMap::<i32, Option<bool>>::new();
    let mut points = Vec::with_capacity(events.len());
    for event in events {
        state.insert(event.fiscal_year, event.paid);
        let consecutive_years = event.paid.map(|_| {
            let mut count = 0u32;
            let mut year = event.fiscal_year;
            while state.get(&year).copied().flatten() == Some(true) {
                count += 1;
                year -= 1;
            }
            count
        });
        points.push(DividendContinuityPoint {
            fiscal_year: event.fiscal_year,
            announced_on: event.announced_on,
            consecutive_years,
        });
    }
    Ok(points)
}

fn parse_hk_dividend_event(row: &Value) -> Result<Option<DividendStateEvent>> {
    if !row
        .get("REPORT_TYPE")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.contains("年度"))
    {
        return Ok(None);
    }
    let fiscal_year = row
        .get("YEAR")
        .and_then(Value::as_str)
        .and_then(|year| year.parse::<i32>().ok())
        .ok_or_else(|| anyhow!("Hong Kong dividend response missing fiscal year"))?;
    let plan = row
        .get("PLAN_EXPLAIN")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(Some(DividendStateEvent {
        fiscal_year,
        announced_on: iso_date(row.get("NOTICE_DATE"), "HK dividend NOTICE_DATE")?,
        paid: Some(
            !plan.trim().is_empty()
                && !plan.contains("不分红")
                && !plan.contains("不派")
                && !plan.contains("无分配"),
        ),
    }))
}

fn parse_a_share_dividend_event(row: &Value) -> Result<Option<DividendStateEvent>> {
    let reporting_period = iso_date(row.get("REPORT_DATE"), "dividend REPORT_DATE")?;
    if !reporting_period.ends_with("-12-31") {
        return Ok(None);
    }
    let fiscal_year = reporting_period
        .get(..4)
        .and_then(|year| year.parse::<i32>().ok())
        .ok_or_else(|| anyhow!("A-share dividend response missing fiscal year"))?;
    let announced_on = row
        .get("PLAN_NOTICE_DATE")
        .or_else(|| row.get("PUBLISH_DATE"));
    Ok(Some(DividendStateEvent {
        fiscal_year,
        announced_on: iso_date(announced_on, "dividend PLAN_NOTICE_DATE")?,
        paid: Some(optional_f64(row.get("PRETAX_BONUS_RMB")).is_some_and(|value| value > 0.0)),
    }))
}

fn response_rows(value: &Value) -> Result<&Vec<Value>> {
    value
        .pointer("/result/data")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("financial response missing result.data"))
}

fn finance_secu_code(code: &str) -> Result<String> {
    let normalized = normalize_code(code)
        .ok_or_else(|| anyhow!("point-in-time financial provider requires a valid code"))?;
    if normalized.len() != 6 || !normalized.chars().all(|ch| ch.is_ascii_digit()) {
        bail!("point-in-time financial provider currently supports A shares only");
    }
    let exchange = if normalized.starts_with('6') {
        "SH"
    } else {
        "SZ"
    };
    Ok(format!("{normalized}.{exchange}"))
}

fn iso_date(value: Option<&Value>, field: &str) -> Result<String> {
    let raw = value
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("financial response missing {field}"))?;
    let date = raw.get(..10).unwrap_or(raw);
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .with_context(|| format!("invalid {field}"))?;
    Ok(date.to_string())
}

fn optional_f64(value: Option<&Value>) -> Option<f64> {
    let number = match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(number) => number.parse().ok(),
        _ => None,
    }?;
    number.is_finite().then_some(number)
}

/// Compact network errors for UI (avoid dumping full query strings).
pub fn short_http_err(msg: &str) -> String {
    let lower = msg.to_lowercase();
    if lower.contains("timed out") || lower.contains("timeout") {
        return "请求超时".into();
    }
    if lower.contains("connection") || lower.contains("connect") {
        return "网络连接失败".into();
    }
    if lower.contains("dns") || lower.contains("resolve") {
        return "DNS 解析失败".into();
    }
    if lower.contains("429") || lower.contains("too many") {
        return "请求过于频繁（限流）".into();
    }
    // Strip long URLs from ureq/anyhow messages
    if let Some(idx) = msg.find("http") {
        let prefix = msg[..idx].trim().trim_end_matches(':').trim();
        if prefix.is_empty() {
            return "HTTP 请求失败".into();
        }
        return prefix.to_string();
    }
    let s = msg.chars().take(80).collect::<String>();
    if msg.len() > 80 { format!("{s}…") } else { s }
}

/// Batch quotes for a list of pure codes (`600519`, `000001`, `00700`, …).
pub fn fetch_quotes(codes: &[String]) -> Result<Vec<QuoteTick>> {
    if codes.is_empty() {
        return Ok(vec![]);
    }
    let secids: Vec<String> = codes.iter().map(|c| secid_for_code(c)).collect();
    let prefer_hk = codes.iter().any(|c| is_hk_code(c));
    fetch_quotes_by_secids_with_hosts(
        &secids,
        if prefer_hk {
            PUSH2_HOSTS_HK
        } else {
            PUSH2_HOSTS
        },
    )
}

/// Quotes by raw Eastmoney `secid` list (e.g. `1.000001` 上证指数).
pub fn fetch_quotes_by_secids(secids: &[String]) -> Result<Vec<QuoteTick>> {
    let prefer_hk = secids.iter().any(|s| s.starts_with("116."));
    fetch_quotes_by_secids_with_hosts(
        secids,
        if prefer_hk {
            PUSH2_HOSTS_HK
        } else {
            PUSH2_HOSTS
        },
    )
}

fn fetch_quotes_by_secids_with_hosts(secids: &[String], hosts: &[&str]) -> Result<Vec<QuoteTick>> {
    if secids.is_empty() {
        return Ok(vec![]);
    }
    let secids = secids.join(",");
    let path = format!(
        "/api/qt/ulist.np/get?\
         fltt=2&np=1&ut=bd1d9ddb04089700cf9c27f6f7426281\
         &secids={secids}\
         &fields=f12,f13,f14,f2,f3,f4,f5,f6,f15,f16,f17,f18"
    );

    let mut last_err = anyhow!("no host tried");
    for host in hosts {
        let url = format!("https://{host}{path}");
        match get_json(&url) {
            Ok(v) => {
                return parse_quote_diff(v);
            }
            Err(e) => {
                last_err = e;
            }
        }
    }
    Err(anyhow!("行情接口不可用: {last_err}"))
}

/// 上证综指 / 沪深300 / 创业板指.
pub fn fetch_major_indices() -> Result<Vec<QuoteTick>> {
    let secids = vec![
        "1.000001".into(), // 上证综指
        "1.000300".into(), // 沪深300
        "0.399006".into(), // 创业板指
    ];
    fetch_quotes_by_secids(&secids)
}

/// A 股行业板块涨跌榜（东财行业口径）。
pub fn fetch_a_share_industry_sectors() -> Result<Vec<SectorTick>> {
    // Eastmoney exposes levels 1/2/3 separately. The broad `m:90+t:2`
    // filter mixes all levels and silently caps responses at 100 rows.
    fetch_complete_industry_sectors(EASTMONEY_LEVEL_ONE_INDUSTRIES, "申万一级")
}

fn fetch_complete_industry_sectors(filter: &str, level_label: &str) -> Result<Vec<SectorTick>> {
    let mut page = 1;
    let mut raw_rows = 0usize;
    let mut expected_total = None;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    loop {
        let path = format!(
            "/api/qt/clist/get?\
             pn={page}&pz={CLIST_PAGE_SIZE}&po=1&np=1&ut=bd1d9ddb04089700cf9c27f6f7426281\
             &fltt=2&invt=2&fid=f6&fs={filter}\
             &fields=f12,f14,f2,f3,f4,f5,f6,f104,f105,f106"
        );
        let mut last_err = anyhow!("{level_label}行业接口未返回数据");
        let mut response = None;
        for host in PUSH2_HOSTS {
            let url = format!("https://{host}{path}");
            match get_json(&url) {
                Ok(value) => {
                    response = Some(value);
                    break;
                }
                Err(error) => last_err = error,
            }
        }
        let value = response.ok_or_else(|| anyhow!("{level_label}行业不可用: {last_err}"))?;
        let total = clist_total(&value).ok_or_else(|| anyhow!("{level_label}行业缺少总数"))?;
        if let Some(expected) = expected_total {
            if total != expected {
                bail!("{level_label}行业分页总数变化：{expected} -> {total}");
            }
        } else {
            expected_total = Some(total);
        }
        let received_count = clist_row_count(&value);
        raw_rows += received_count;
        for sector in parse_sector_diff(value)? {
            if seen.insert(sector.code.clone()) {
                out.push(sector);
            }
        }
        if raw_rows >= total || received_count < CLIST_PAGE_SIZE {
            if raw_rows < total {
                bail!(
                    "{level_label}行业数据不完整：原始 {raw_rows}，去重 {}，应有 {total}",
                    out.len()
                );
            }
            // Eastmoney `total` can count duplicate rows across pages; unique
            // codes may be slightly lower once de-duplicated by board code.
            if out.is_empty() && total > 0 {
                bail!("{level_label}行业解析为空：接口总数 {total}");
            }
            break;
        }
        page += 1;
    }
    Ok(out)
}

fn fetch_a_share_stock_total() -> Result<usize> {
    let path = format!(
        "/api/qt/clist/get?\
         pn=1&pz=1&po=1&np=1&ut=bd1d9ddb04089700cf9c27f6f7426281\
         &fltt=2&invt=2&fid=f6&fs={EASTMONEY_A_SHARE_UNIVERSE}&fields=f12"
    );
    let mut last_err = anyhow!("全 A 股接口未返回数据");
    for host in PUSH2_HOSTS {
        let url = format!("https://{host}{path}");
        match get_json(&url)
            .and_then(|value| clist_total(&value).ok_or_else(|| anyhow!("全 A 股接口缺少总数")))
        {
            Ok(total) if total > 0 => return Ok(total),
            Ok(_) => last_err = anyhow!("全 A 股总数为 0"),
            Err(error) => last_err = error,
        }
    }
    Err(anyhow!("全 A 股总数不可用: {last_err}"))
}

/// Complete A-share heatmap hierarchy using Shenwan's mutually-exclusive
/// level-1 -> level-2 -> stock classification.
///
/// Eastmoney caps every `clist` response at 100 rows even when a larger `pz`
/// is requested. Each of the 31 level-1 boards is therefore paginated to its
/// reported `data.total`; `f100` supplies the level-2 industry for every
/// constituent. A small fixed worker pool keeps the full-market refresh fast
/// without sending an unbounded burst of requests.
pub fn fetch_a_share_industry_heatmap() -> Result<Vec<IndustryHeatmapSector>> {
    let mut sectors = fetch_a_share_industry_sectors()?;
    sectors.sort_by(|left, right| {
        right
            .amount
            .total_cmp(&left.amount)
            .then_with(|| left.code.cmp(&right.code))
    });
    if sectors.is_empty() {
        bail!("一级行业为空");
    }

    let next = AtomicUsize::new(0);
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker_count = HEATMAP_FETCH_CONCURRENCY.min(sectors.len());
    let groups = std::thread::scope(|scope| -> Result<Vec<IndustryHeatmapSector>> {
        for _ in 0..worker_count {
            let sender = sender.clone();
            let sectors = &sectors;
            let next = &next;
            scope.spawn(move || {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(sector) = sectors.get(index).cloned() else {
                        break;
                    };
                    let result = fetch_complete_sector_heatmap(sector);
                    if sender.send((index, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);

        let mut ordered = vec![None; sectors.len()];
        for _ in 0..sectors.len() {
            let (index, result) = receiver
                .recv()
                .map_err(|_| anyhow!("行业热力图任务提前结束"))?;
            ordered[index] = Some(result?);
        }
        ordered
            .into_iter()
            .map(|group| group.ok_or_else(|| anyhow!("行业热力图缺少分组")))
            .collect()
    })?;

    let level_two = fetch_complete_industry_sectors(EASTMONEY_LEVEL_TWO_INDUSTRIES, "申万二级")?;
    let expected_industries = level_two
        .into_iter()
        .map(|sector| sector.name)
        .collect::<HashSet<_>>();
    let expected_stock_total = fetch_a_share_stock_total()?;
    validate_heatmap_coverage(&groups, &expected_industries, expected_stock_total)?;
    Ok(groups)
}

fn validate_heatmap_coverage(
    groups: &[IndustryHeatmapSector],
    expected_industries: &HashSet<String>,
    expected_stock_total: usize,
) -> Result<()> {
    let mut owners = HashMap::<String, String>::new();
    let mut industry_owners = HashMap::<String, String>::new();
    for group in groups {
        for industry in &group.industries {
            if let Some(previous) =
                industry_owners.insert(industry.name.clone(), group.sector.name.clone())
                && previous != group.sector.name
            {
                bail!(
                    "申万二级行业归属重叠：{} 同时属于 {} / {}",
                    industry.name,
                    previous,
                    group.sector.name
                );
            }
            for stock in &industry.stocks {
                if let Some(previous) = owners.insert(stock.code.clone(), group.sector.name.clone())
                    && previous != group.sector.name
                {
                    bail!(
                        "申万一级行业成分重叠：{} 同时属于 {} / {}",
                        stock.code,
                        previous,
                        group.sector.name
                    );
                }
            }
        }
    }
    let actual_industries = industry_owners.keys().cloned().collect::<HashSet<_>>();
    if *expected_industries != actual_industries {
        let mut missing = expected_industries
            .difference(&actual_industries)
            .cloned()
            .collect::<Vec<_>>();
        let mut unexpected = actual_industries
            .difference(expected_industries)
            .cloned()
            .collect::<Vec<_>>();
        missing.sort_unstable();
        unexpected.sort_unstable();
        bail!(
            "申万二级行业集合不完整：缺少 [{}]，额外 [{}]",
            missing.join(", "),
            unexpected.join(", ")
        );
    }
    // Shenwan board membership and the A-share universe filter are not an
    // identical set (boards can include extras; universe totals lag listings).
    // Require near-complete coverage rather than exact equality so one API
    // mismatch does not blank the entire heatmap.
    let min_stocks = expected_stock_total.saturating_mul(95) / 100;
    if owners.len() < min_stocks {
        bail!(
            "全 A 股覆盖不完整：行业成分 {} / 全市场 {expected_stock_total}（至少需要 {min_stocks}）",
            owners.len()
        );
    }
    Ok(())
}

fn fetch_complete_sector_heatmap(sector: SectorTick) -> Result<IndustryHeatmapSector> {
    let code = sector.code.trim();
    if code.is_empty() {
        bail!("板块代码为空");
    }
    let mut page = 1;
    let mut raw_rows = 0usize;
    let mut expected_total = None;
    let mut seen = HashSet::new();
    let mut classified = Vec::new();

    loop {
        let path = format!(
            "/api/qt/clist/get?pn={page}&pz={CLIST_PAGE_SIZE}&po=1&np=1\
             &ut=bd1d9ddb04089700cf9c27f6f7426281&fltt=2&invt=2\
             &fid=f6&fs=b:{code}&fields=f12,f14,f2,f3,f5,f6,f15,f16,f17,f18,f100"
        );
        let mut last_err = anyhow!("板块成分接口未返回");
        let mut response = None;
        for host in PUSH2_HOSTS {
            let url = format!("https://{host}{path}");
            match get_json(&url) {
                Ok(value) => {
                    response = Some(value);
                    break;
                }
                Err(error) => last_err = error,
            }
        }
        let value = response.ok_or_else(|| anyhow!("{} 成分不可用: {last_err}", sector.name))?;
        let total = clist_total(&value).ok_or_else(|| anyhow!("{} 缺少成分总数", sector.name))?;
        if let Some(expected) = expected_total {
            if total != expected {
                bail!("{} 分页总数变化：{expected} -> {total}", sector.name);
            }
        } else {
            expected_total = Some(total);
        }
        let received_count = clist_row_count(&value);
        raw_rows += received_count;
        for (industry, quote) in parse_classified_quote_diff(value)? {
            if seen.insert(quote.code.clone()) {
                classified.push((industry, quote));
            }
        }

        if raw_rows >= total || received_count < CLIST_PAGE_SIZE {
            if raw_rows < total {
                bail!("{} 成分数据不完整：返回 {raw_rows} / {total}", sector.name);
            }
            // Eastmoney `data.total` is a row count, not a unique-code count.
            // Large boards (e.g. 医药生物) occasionally emit the same code on
            // adjacent pages; accepting the de-duplicated set keeps the full
            // heatmap usable instead of failing the whole refresh for one dup.
            if classified.is_empty() && total > 0 {
                bail!("{} 成分解析为空：接口总数 {total}", sector.name);
            }
            break;
        }
        page += 1;
    }

    let industries = group_classified_quotes(classified);
    Ok(IndustryHeatmapSector { sector, industries })
}

fn group_classified_quotes(classified: Vec<(String, QuoteTick)>) -> Vec<IndustryStockGroup> {
    let mut by_industry = BTreeMap::<String, Vec<QuoteTick>>::new();
    for (industry, quote) in classified {
        by_industry.entry(industry).or_default().push(quote);
    }
    let mut industries = by_industry
        .into_iter()
        .map(|(name, mut stocks)| {
            stocks.sort_by(|left, right| {
                right
                    .amount
                    .total_cmp(&left.amount)
                    .then_with(|| left.code.cmp(&right.code))
            });
            IndustryStockGroup {
                amount: stocks.iter().map(|stock| stock.amount.max(0.0)).sum(),
                name,
                stocks,
            }
        })
        .collect::<Vec<_>>();
    industries.sort_by(|left, right| {
        right
            .amount
            .total_cmp(&left.amount)
            .then_with(|| left.name.cmp(&right.name))
    });
    industries
}

/// 行业板块成分股（按成交额降序，东财 `fs=b:BKxxxx`）。
///
/// 用于市场分析页「点板块 → 看成分」下钻。
pub fn fetch_sector_constituents(sector_code: &str, limit: usize) -> Result<Vec<QuoteTick>> {
    let code = sector_code.trim();
    if code.is_empty() {
        bail!("板块代码为空");
    }
    let limit = limit.clamp(5, MAX_SECTOR_CONSTITUENTS);
    let mut page = 1;
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    loop {
        let path = format!(
            "/api/qt/clist/get?pn={page}&pz={CLIST_PAGE_SIZE}&po=1&np=1\
             &ut=bd1d9ddb04089700cf9c27f6f7426281&fltt=2&invt=2\
             &fid=f6&fs=b:{code}&fields=f12,f14,f2,f3,f5,f6,f15,f16,f17,f18"
        );
        let mut last_err = anyhow!("板块成分接口未返回");
        let mut response = None;
        for host in PUSH2_HOSTS {
            let url = format!("https://{host}{path}");
            match get_json(&url) {
                Ok(value) => {
                    response = Some(value);
                    break;
                }
                Err(error) => last_err = error,
            }
        }
        let value = response.ok_or_else(|| anyhow!("板块成分不可用: {last_err}"))?;
        let total = clist_total(&value);
        let received_count = clist_row_count(&value);
        let rows = parse_quote_diff(value)?;
        for row in rows {
            if seen.insert(row.code.clone()) {
                out.push(row);
            }
        }

        if out.len() >= limit
            || total.is_some_and(|total| out.len() >= total)
            || received_count < CLIST_PAGE_SIZE
        {
            break;
        }
        page += 1;
    }

    if out.is_empty() {
        bail!("板块成分为空");
    }
    out.sort_by(|left, right| {
        right
            .amount
            .total_cmp(&left.amount)
            .then_with(|| left.code.cmp(&right.code))
    });
    out.truncate(limit);
    Ok(out)
}

fn clist_total(value: &Value) -> Option<usize> {
    value
        .pointer("/data/total")
        .and_then(Value::as_u64)
        .and_then(|total| usize::try_from(total).ok())
}

fn clist_row_count(value: &Value) -> usize {
    value
        .pointer("/data/diff")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

fn parse_sector_diff(v: Value) -> Result<Vec<SectorTick>> {
    let diff = v
        .pointer("/data/diff")
        .and_then(|d| d.as_array())
        .ok_or_else(|| anyhow!("板块数据为空或格式异常"))?;

    let mut out = Vec::with_capacity(diff.len());
    for item in diff {
        let code = item
            .get("f12")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let name = item
            .get("f14")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if code.is_empty() || name.is_empty() {
            continue;
        }
        out.push(SectorTick {
            code,
            name,
            change_pct: num_f64(item.get("f3")),
            amount: num_f64(item.get("f6")),
            advances: num_f64(item.get("f104")).max(0.0) as u64,
            declines: num_f64(item.get("f105")).max(0.0) as u64,
            unchanged: num_f64(item.get("f106")).max(0.0) as u64,
        });
    }
    Ok(out)
}

fn parse_quote_diff(v: Value) -> Result<Vec<QuoteTick>> {
    let diff = v
        .pointer("/data/diff")
        .and_then(|d| d.as_array())
        .ok_or_else(|| anyhow!("行情数据为空或格式异常"))?;

    Ok(diff.iter().filter_map(parse_quote_item).collect())
}

fn parse_classified_quote_diff(v: Value) -> Result<Vec<(String, QuoteTick)>> {
    let diff = v
        .pointer("/data/diff")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("行情数据为空或格式异常"))?;
    Ok(diff
        .iter()
        .filter_map(|item| {
            let quote = parse_quote_item(item)?;
            let industry = item
                .get("f100")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty() && *name != "-")
                .unwrap_or("其他")
                .to_string();
            Some((industry, quote))
        })
        .collect())
}

fn parse_quote_item(item: &Value) -> Option<QuoteTick> {
    let code = item
        .get("f12")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    if code.is_empty() {
        return None;
    }
    let currency = Currency::for_code(&code)?;
    let last = num_f64(item.get("f2"));
    Some(QuoteTick {
        code,
        name: item
            .get("f14")
            .and_then(|x| x.as_str())
            .unwrap_or("--")
            .to_string(),
        last,
        change_pct: num_f64(item.get("f3")),
        volume: num_f64(item.get("f5")) as u64,
        amount: num_f64(item.get("f6")),
        currency,
        source: "东方财富".into(),
        fetched_at: chrono::Utc::now().timestamp_millis(),
        market_time: None,
        availability: if last > 0.0 {
            Availability::Available
        } else {
            Availability::Invalid
        },
        freshness: Freshness::Live,
    })
}

/// Daily K-line, end at latest, `limit` bars. `fqt=1` 前复权.
/// Returns `(code, name, candles)` so callers can discard stale responses.
pub fn fetch_klines(code: &str, limit: usize) -> Result<(String, String, Vec<Candle>)> {
    let secid = secid_for_code(code);
    let limit = limit.clamp(5, 1000);
    let url = format!(
        "https://push2his.eastmoney.com/api/qt/stock/kline/get?\
         secid={secid}\
         &fields1=f1,f2,f3,f4,f5,f6\
         &fields2=f51,f52,f53,f54,f55,f56,f57,f58,f59,f60,f61\
         &klt=101&fqt=1&end=20500101&lmt={limit}"
    );
    let v =
        get_json(&url).map_err(|e| anyhow!("K线请求失败: {}", short_http_err(&e.to_string())))?;
    let name = v
        .pointer("/data/name")
        .and_then(|x| x.as_str())
        .unwrap_or(code)
        .to_string();
    let resp_code = v
        .pointer("/data/code")
        .and_then(|x| x.as_str())
        .unwrap_or(code)
        .to_string();
    let klines = v
        .pointer("/data/klines")
        .and_then(|x| x.as_array())
        .ok_or_else(|| anyhow!("无 K 线数据 ({code})"))?;

    let mut candles = Vec::with_capacity(klines.len());
    for row in klines {
        let s = row.as_str().unwrap_or_default();
        // date,open,close,high,low,volume,amount,amp,chg_pct,chg,turnover
        let parts: Vec<&str> = s.split(',').collect();
        if parts.len() < 6 {
            continue;
        }
        let date = parts[0].trim();
        // Keep full YYYY-MM-DD for hover / axis labels
        let label = if date.len() >= 10 {
            date[..10].to_string()
        } else {
            date.to_string()
        };
        candles.push(Candle {
            date: shared(label),
            open: parts[1].parse().unwrap_or(0.0),
            close: parts[2].parse().unwrap_or(0.0),
            high: parts[3].parse().unwrap_or(0.0),
            low: parts[4].parse().unwrap_or(0.0),
            volume: parts[5].parse::<f64>().unwrap_or(0.0) as u64,
        });
    }
    if candles.is_empty() {
        return Err(anyhow!("K线为空 ({code})"));
    }
    Ok((resp_code, name, candles))
}

/// Lightweight symbol search (name / pinyin / code).
pub fn search_symbols(query: &str, limit: usize) -> Result<Vec<Symbol>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let url = format!(
        "https://searchapi.eastmoney.com/api/suggest/get?\
         input={}&type=14&token=D43BF722C8E33BDC906FB84D85E326E8&count={}",
        urlencoding_minimal(q),
        limit.clamp(1, 20)
    );

    let body = agent()
        .get(&url)
        .set("User-Agent", UA)
        .call()
        .with_context(|| format!("search {q}"))?
        .into_string()?;

    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let mut out = Vec::new();

    if let Some(arr) = v
        .pointer("/QuotationCodeTable/Data")
        .and_then(|x| x.as_array())
    {
        for it in arr {
            let raw_code = it
                .get("Code")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            let typ = it
                .get("SecurityTypeName")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let mkt = mkt_num_str(it.get("MktNum"));
            let classify = it.get("Classify").and_then(|x| x.as_str()).unwrap_or("");
            let jys = it.get("JYS").and_then(|x| x.as_str()).unwrap_or("");
            let is_hk = mkt == "116"
                || classify.eq_ignore_ascii_case("HK")
                || jys.eq_ignore_ascii_case("HK")
                || typ.contains("港股");
            let is_a = raw_code.len() == 6
                && raw_code.chars().all(|c| c.is_ascii_digit())
                && (typ.contains('A')
                    || typ.contains("ETF")
                    || typ.contains("股票")
                    || typ.is_empty()
                    || mkt == "0"
                    || mkt == "1"
                    || classify.eq_ignore_ascii_case("AStock"));
            let code = if is_hk {
                match pad_hk_code(&raw_code) {
                    Some(c) => c,
                    None => continue,
                }
            } else if is_a {
                raw_code.clone()
            } else {
                continue;
            };
            let name = it
                .get("Name")
                .and_then(|x| x.as_str())
                .unwrap_or(&code)
                .to_string();
            if out.iter().any(|s: &Symbol| s.code == code) {
                continue;
            }
            out.push(Symbol {
                code: code.clone(),
                name: shared(name),
                last: 0.0,
                change_pct: 0.0,
                volume: 0,
                board: board_for_code(&code),
            });
            if out.len() >= limit {
                break;
            }
        }
    }

    // Fallback: pure 6-digit A / 5-digit HK / hk-prefixed
    if out.is_empty()
        && let Some(code) = normalize_code(q)
    {
        return Ok(vec![Symbol {
            code: code.clone(),
            name: shared(code.clone()),
            last: 0.0,
            change_pct: 0.0,
            volume: 0,
            board: board_for_code(&code),
        }]);
    }

    Ok(out)
}

fn mkt_num_str(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn pad_hk_code(raw: &str) -> Option<String> {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.len() > 5 {
        return None;
    }
    Some(format!("{digits:0>5}"))
}

fn num_f64(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Positive finite financial value → `Some`, else `None` (missing / negative).
fn pos_fin(v: f64) -> Option<f64> {
    (v > 0.0 && v.is_finite()).then_some(v)
}

fn urlencoding_minimal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 沪深 A 股榜单行（用于扩大寻宝候选池）。
#[derive(Debug, Clone)]
pub struct UniverseRow {
    pub code: String,
    pub name: String,
    /// 总市值（元），接口字段 f20。
    pub market_cap: f64,
    /// 成交额（元），接口字段 f6；休市时常为 0。
    /// 市盈率（动态，f9），可能为 0 / 缺失。
    pub pe: Option<f64>,
    /// 市净率（f23），可能为 0 / 缺失。
    pub pb: Option<f64>,
}

/// 按总市值降序拉取沪深 A 股（含创业板/科创板），过滤 ST / 代码异常。
///
/// 使用东财 `clist/get`（与 AKShare 同源公开接口）。`limit` 为过滤后最多返回只数。
pub fn fetch_liquid_a_shares(limit: usize) -> Result<Vec<UniverseRow>> {
    let limit = limit.clamp(20, 2000);
    let page_size = 100usize;
    let mut out: Vec<UniverseRow> = Vec::with_capacity(limit);
    let mut page = 1u32;
    // 深A + 创业板 + 沪A + 科创板
    let fs = "m:0+t:6,m:0+t:80,m:1+t:2,m:1+t:23";
    // 按总市值 f20 排序（休市时成交额 f6 常为空，市值更稳）
    let fields = "f12,f14,f2,f3,f6,f9,f20,f21,f23";

    while out.len() < limit && page <= 40 {
        let path = format!(
            "/api/qt/clist/get?pn={page}&pz={page_size}&po=1&np=1\
             &ut=bd1d9ddb04089700cf9c27f6f7426281&fltt=2&invt=2\
             &fid=f20&fs={fs}&fields={fields}"
        );
        let mut page_rows: Option<Vec<UniverseRow>> = None;
        let mut last_err = anyhow!("no host");
        for host in PUSH2_HOSTS
            .iter()
            .chain(std::iter::once(&"push2delay.eastmoney.com"))
        {
            let url = format!("https://{host}{path}");
            match get_json(&url) {
                Ok(v) => {
                    page_rows = Some(parse_clist_universe(&v)?);
                    break;
                }
                Err(e) => last_err = e,
            }
        }
        let rows = page_rows.ok_or_else(|| anyhow!("A股列表失败: {last_err}"))?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            if !is_scan_eligible(&row.code, &row.name) {
                continue;
            }
            // 过小市值噪音多（默认约 30 亿以上）
            if row.market_cap > 0.0 && row.market_cap < 3.0e9 {
                continue;
            }
            if out.iter().any(|x| x.code == row.code) {
                continue;
            }
            out.push(row);
            if out.len() >= limit {
                break;
            }
        }
        page += 1;
    }

    if out.is_empty() {
        return Err(anyhow!("A股列表为空"));
    }
    Ok(out)
}

fn parse_clist_universe(v: &Value) -> Result<Vec<UniverseRow>> {
    let diff = v
        .pointer("/data/diff")
        .and_then(|d| d.as_array())
        .ok_or_else(|| anyhow!("clist 无 diff"))?;
    let mut out = Vec::with_capacity(diff.len());
    for item in diff {
        let code = item
            .get("f12")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let name = item
            .get("f14")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        out.push(UniverseRow {
            code,
            name,
            market_cap: num_f64(item.get("f20")),
            pe: pos_fin(num_f64(item.get("f9"))),
            pb: pos_fin(num_f64(item.get("f23"))),
        });
    }
    Ok(out)
}

fn is_scan_eligible(code: &str, name: &str) -> bool {
    if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let u = name.to_ascii_uppercase();
    // ST / *ST / SST / 退市整理
    if u.contains("ST") || name.contains("退") {
        return false;
    }
    // 未开板次新等前缀
    if name.starts_with('C') || name.starts_with('N') || name.starts_with('c') {
        return false;
    }
    true
}

#[cfg(test)]
mod universe_list_tests {
    use super::*;

    #[test]
    #[ignore = "requires public market-data network"]
    fn liquid_a_shares_smoke() {
        let rows = fetch_liquid_a_shares(30).expect("clist");
        assert!(rows.len() >= 10, "n={}", rows.len());
        assert!(rows.iter().all(|r| r.code.len() == 6));
        eprintln!(
            "universe sample: {} {} mv={:.0}",
            rows[0].code, rows[0].name, rows[0].market_cap
        );
    }
}

#[cfg(test)]
mod market_board_tests {
    use super::*;

    #[test]
    fn level_one_filter_does_not_mix_industry_hierarchies() {
        assert_eq!(EASTMONEY_LEVEL_ONE_INDUSTRIES, "m:90%2Bs:2%2Bf:!50");
        assert_eq!(EASTMONEY_LEVEL_TWO_INDUSTRIES, "m:90%2Bs:4%2Bf:!50");
        assert!(!EASTMONEY_LEVEL_ONE_INDUSTRIES.contains("t:2"));
        assert!(!EASTMONEY_LEVEL_TWO_INDUSTRIES.contains("t:2"));
        assert_eq!(
            EASTMONEY_A_SHARE_UNIVERSE,
            "m:0%2Bt:6,m:0%2Bt:80,m:1%2Bt:2,m:1%2Bt:23"
        );
    }

    #[test]
    fn sector_parser_preserves_declines_and_breadth() {
        let value = serde_json::json!({
            "data": {
                "total": 1,
                "diff": [{
                    "f12": "BK1201",
                    "f14": "电子",
                    "f3": -1.25,
                    "f6": 123_000_000.0,
                    "f104": 20,
                    "f105": 80,
                    "f106": 3
                }]
            }
        });
        assert_eq!(clist_total(&value), Some(1));
        assert_eq!(clist_row_count(&value), 1);
        let sectors = parse_sector_diff(value).expect("sector fixture");
        assert_eq!(sectors.len(), 1);
        assert_eq!(sectors[0].name, "电子");
        assert_eq!(sectors[0].change_pct, -1.25);
        assert_eq!(sectors[0].advances, 20);
        assert_eq!(sectors[0].declines, 80);
        assert_eq!(sectors[0].unchanged, 3);
    }

    #[test]
    fn classified_quotes_preserve_every_stock_and_form_level_two_groups() {
        let value = serde_json::json!({
            "data": {
                "total": 3,
                "diff": [
                    {"f12":"600001","f14":"甲","f2":10.0,"f3":1.0,"f5":100,"f6":300.0,"f100":"半导体"},
                    {"f12":"600002","f14":"乙","f2":20.0,"f3":-1.0,"f5":200,"f6":100.0,"f100":"元件"},
                    {"f12":"600003","f14":"丙","f2":30.0,"f3":0.0,"f5":300,"f6":200.0,"f100":"半导体"}
                ]
            }
        });
        let classified = parse_classified_quote_diff(value).expect("classified fixture");
        let groups = group_classified_quotes(classified);
        assert_eq!(groups.len(), 2);
        assert_eq!(
            groups.iter().map(|group| group.stocks.len()).sum::<usize>(),
            3
        );
        assert_eq!(groups[0].name, "半导体");
        assert_eq!(groups[0].amount, 500.0);
        assert_eq!(groups[0].stocks[0].code, "600001");
        assert_eq!(groups[0].stocks[1].code, "600003");

        let heatmap = vec![IndustryHeatmapSector {
            sector: SectorTick {
                code: "BK1201".into(),
                name: "电子".into(),
                change_pct: 0.0,
                amount: 600.0,
                advances: 1,
                declines: 1,
                unchanged: 1,
            },
            industries: groups,
        }];
        let expected = HashSet::from(["半导体".to_string(), "元件".to_string()]);
        validate_heatmap_coverage(&heatmap, &expected, 3).expect("complete fixture");
        let expected_with_missing = HashSet::from([
            "半导体".to_string(),
            "元件".to_string(),
            "银行Ⅱ".to_string(),
        ]);
        let error = validate_heatmap_coverage(&heatmap, &expected_with_missing, 3)
            .expect_err("missing industry must fail");
        assert!(error.to_string().contains("缺少 [银行Ⅱ]"));
        // 95% of 100 is 95; three stocks must fail the soft floor.
        let error = validate_heatmap_coverage(&heatmap, &expected, 100)
            .expect_err("far-below-universe must fail");
        assert!(
            error.to_string().contains("行业成分 3 / 全市场 100"),
            "error={error}"
        );
        // Industry count above universe total is fine (filter mismatch).
        validate_heatmap_coverage(&heatmap, &expected, 2).expect("extra industry stocks ok");
    }

    #[test]
    #[ignore = "requires public market-data network"]
    fn level_one_industries_smoke() {
        let sectors = fetch_a_share_industry_sectors().expect("level-one industries");
        assert_eq!(sectors.len(), 31);
        assert!(sectors.iter().all(|sector| sector.amount > 0.0));
    }

    #[test]
    #[ignore = "requires public market-data network"]
    fn large_sector_constituents_are_paginated_and_amount_sorted() {
        let rows = fetch_sector_constituents("BK1201", 1000).expect("electronics constituents");
        assert!(rows.len() > CLIST_PAGE_SIZE, "rows={}", rows.len());
        assert!(rows.windows(2).all(|pair| pair[0].amount >= pair[1].amount));
    }

    #[test]
    #[ignore = "requires public market-data network"]
    fn complete_heatmap_contains_all_levels_and_unique_stocks() {
        let groups = fetch_a_share_industry_heatmap().expect("complete heatmap");
        assert_eq!(groups.len(), 31);
        let industry_count: usize = groups.iter().map(|group| group.industries.len()).sum();
        let stock_count: usize = groups
            .iter()
            .flat_map(|group| &group.industries)
            .map(|industry| industry.stocks.len())
            .sum();
        let codes = groups
            .iter()
            .flat_map(|group| &group.industries)
            .flat_map(|industry| &industry.stocks)
            .map(|stock| stock.code.as_str())
            .collect::<HashSet<_>>();
        eprintln!(
            "complete heatmap: level1={} level2={industry_count} stocks={stock_count}",
            groups.len()
        );
        assert!(industry_count >= 120, "level-2 industries={industry_count}");
        assert!(stock_count >= 5_000, "stocks={stock_count}");
        assert_eq!(codes.len(), stock_count, "stocks must be globally unique");
    }
}

#[cfg(test)]
mod fundamental_tests {
    use super::*;

    #[test]
    fn point_in_time_fixture_keeps_later_notice_date_and_units() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/fundamentals-main.json"))
                .unwrap();
        let reports = parse_fundamental_reports(&fixture["main"], &fixture["balance"]).unwrap();
        assert_eq!(reports.len(), 1);
        let report = &reports[0];
        assert_eq!(report.reporting_period, "2025-12-31");
        assert_eq!(report.announced_on, "2026-04-30");
        assert_eq!(report.currency, Currency::Cny);
        assert_eq!(report.audit_risk_flag, Some(0.0));
        assert!(report.goodwill_ratio_pct.is_some_and(|value| value < 1.0));
    }

    #[test]
    fn hong_kong_financials_do_not_fall_through_to_a_share_endpoint() {
        let error = finance_secu_code("00700").unwrap_err();
        assert!(error.to_string().contains("A shares only"));
    }

    #[test]
    fn hong_kong_fixture_matches_real_result_categories_point_in_time() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/fundamentals-hk.json"))
                .unwrap();
        let notices = parse_hk_result_notices(&fixture["notices"]).unwrap();
        let reports =
            parse_hk_fundamental_reports(&fixture["main"], &fixture["balance"], &notices).unwrap();

        assert_eq!(reports.len(), 2, "unannounced interim row must be omitted");
        let annual = reports
            .iter()
            .find(|report| report.reporting_period == "2025-12-31")
            .unwrap();
        assert_eq!(annual.announced_on, "2026-03-18");
        assert!(annual.is_annual);
        assert_eq!(annual.operating_cash_to_profit, Some(1.2));
        assert_eq!(annual.goodwill_ratio_pct, Some(5.0));
        assert_eq!(annual.audit_risk_flag, None);

        let first_quarter = reports
            .iter()
            .find(|report| report.reporting_period == "2026-03-31")
            .unwrap();
        assert_eq!(first_quarter.announced_on, "2026-05-13");
        assert!(!first_quarter.is_annual);
    }

    #[test]
    fn dividend_continuity_breaks_on_a_missing_fiscal_year() {
        let value = serde_json::json!({
            "result": {"data": [
                {
                    "REPORT_TYPE": "年度分配",
                    "YEAR": "2025",
                    "NOTICE_DATE": "2026-03-18 00:00:00",
                    "PLAN_EXPLAIN": "每股派港币5.3元"
                },
                {
                    "REPORT_TYPE": "年度分配",
                    "YEAR": "2024",
                    "NOTICE_DATE": "2025-03-19 00:00:00",
                    "PLAN_EXPLAIN": "每股派港币4.5元"
                },
                {
                    "REPORT_TYPE": "年度分配",
                    "YEAR": "2022",
                    "NOTICE_DATE": "2023-03-22 00:00:00",
                    "PLAN_EXPLAIN": "每股派港币2.4元"
                },
                {
                    "REPORT_TYPE": "特别分配",
                    "YEAR": "2023",
                    "NOTICE_DATE": "2023-12-01 00:00:00",
                    "PLAN_EXPLAIN": "特别分配"
                }
            ]}
        });
        let points = parse_dividend_continuity(&value, true, &[]).unwrap();
        let latest = points.last().unwrap();
        assert_eq!(latest.fiscal_year, 2025);
        assert_eq!(latest.consecutive_years, Some(2));
    }

    #[test]
    fn absent_dividend_record_is_unknown_at_annual_results_date() {
        let value = serde_json::json!({"result": {"data": []}});
        let annual_report = FundamentalReportRow {
            reporting_period: "2025-12-31".into(),
            announced_on: "2026-03-18".into(),
            is_annual: true,
            currency: Currency::Hkd,
            roe_pct: None,
            roic_pct: None,
            operating_cash_to_profit: None,
            debt_ratio_pct: None,
            revenue_growth_pct: None,
            profit_growth_pct: None,
            goodwill_ratio_pct: None,
            audit_risk_flag: None,
        };
        let points = parse_dividend_continuity(&value, true, &[annual_report]).unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].consecutive_years, None);
        assert_eq!(points[0].announced_on, "2026-03-18");
    }

    #[test]
    #[ignore = "requires public financial-data network"]
    fn fundamental_reports_smoke() {
        let reports = fetch_fundamental_reports("600519", 2).expect("financial reports");
        assert!(!reports.is_empty());
        assert!(reports.iter().all(|report| {
            !report.reporting_period.is_empty() && !report.announced_on.is_empty()
        }));
    }

    #[test]
    #[ignore = "requires public financial-data network"]
    fn hong_kong_fundamental_reports_smoke() {
        let reports = fetch_hk_fundamental_reports("00700", 4).expect("HK financial reports");
        assert!(!reports.is_empty());
        assert!(reports.iter().all(|report| {
            !report.reporting_period.is_empty() && report.announced_on > report.reporting_period
        }));
    }
}
