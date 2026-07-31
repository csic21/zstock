//! Free A-share market data via Eastmoney public HTTP APIs (no API key).
//!
//! Endpoints (unofficial, same as used by many open-source tools e.g. AKShare):
//! - Quotes: `push2.eastmoney.com/api/qt/ulist.np/get`
//! - History K: `push2his.eastmoney.com/api/qt/stock/kline/get`
//! - Search: `searchapi.eastmoney.com/api/suggest/get`
//!
//! These are free for personal tooling but have no SLA; rate-limit politely.

use anyhow::{Context, Result, anyhow};
use serde_json::Value;

use crate::model::{Candle, Symbol, board_for_code, secid_for_code, shared};

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

#[derive(Debug, Clone)]
pub struct QuoteTick {
    pub code: String,
    pub name: String,
    pub last: f64,
    pub change_pct: f64,
    pub volume: u64,
    pub high: f64,
    pub low: f64,
    pub open: f64,
    pub prev_close: f64,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(8))
        .timeout_read(std::time::Duration::from_secs(15))
        .build()
}

/// Eastmoney push2 nodes (round-robin / fallback).
const PUSH2_HOSTS: &[&str] = &[
    "push2.eastmoney.com",
    "82.push2.eastmoney.com",
    "80.push2.eastmoney.com",
    "push2delay.eastmoney.com",
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
        return format!("{prefix}");
    }
    let s = msg.chars().take(80).collect::<String>();
    if msg.len() > 80 { format!("{s}…") } else { s }
}

/// Batch quotes for a list of pure codes (`600519`, `000001`, …).
pub fn fetch_quotes(codes: &[String]) -> Result<Vec<QuoteTick>> {
    if codes.is_empty() {
        return Ok(vec![]);
    }
    let secids: Vec<String> = codes.iter().map(|c| secid_for_code(c)).collect();
    fetch_quotes_by_secids(&secids)
}

/// Quotes by raw Eastmoney `secid` list (e.g. `1.000001` 上证指数).
pub fn fetch_quotes_by_secids(secids: &[String]) -> Result<Vec<QuoteTick>> {
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
    for host in PUSH2_HOSTS {
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

fn parse_quote_diff(v: Value) -> Result<Vec<QuoteTick>> {
    let diff = v
        .pointer("/data/diff")
        .and_then(|d| d.as_array())
        .ok_or_else(|| anyhow!("行情数据为空或格式异常"))?;

    let mut out = Vec::with_capacity(diff.len());
    for item in diff {
        let code = item
            .get("f12")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string();
        if code.is_empty() {
            continue;
        }
        out.push(QuoteTick {
            code,
            name: item
                .get("f14")
                .and_then(|x| x.as_str())
                .unwrap_or("--")
                .to_string(),
            last: num_f64(item.get("f2")),
            change_pct: num_f64(item.get("f3")),
            volume: num_f64(item.get("f5")) as u64,
            high: num_f64(item.get("f15")),
            low: num_f64(item.get("f16")),
            open: num_f64(item.get("f17")),
            prev_close: num_f64(item.get("f18")),
        });
    }
    Ok(out)
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
            let code = it
                .get("Code")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let typ = it
                .get("SecurityTypeName")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let mkt = it.get("MktNum").and_then(|x| x.as_str()).unwrap_or("");
            if !(typ.contains('A')
                || typ.contains("ETF")
                || typ.contains("股票")
                || typ.is_empty()
                || mkt == "0"
                || mkt == "1")
            {
                continue;
            }
            let name = it
                .get("Name")
                .and_then(|x| x.as_str())
                .unwrap_or(&code)
                .to_string();
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

    // Fallback: pure 6-digit code
    if out.is_empty() && q.chars().all(|c| c.is_ascii_digit()) && q.len() == 6 {
        return Ok(vec![Symbol {
            code: q.to_string(),
            name: shared(q),
            last: 0.0,
            change_pct: 0.0,
            volume: 0,
            board: board_for_code(q),
        }]);
    }

    Ok(out)
}

fn num_f64(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
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
    pub amount: f64,
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
    let fields = "f12,f14,f2,f3,f6,f20,f21";

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
            amount: num_f64(item.get("f6")),
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

/// Build Symbol list from codes using quote API (fills name/last).
pub fn hydrate_symbols(codes: &[String]) -> Result<Vec<Symbol>> {
    let quotes = fetch_quotes(codes)?;
    let mut map: std::collections::HashMap<String, QuoteTick> =
        quotes.into_iter().map(|q| (q.code.clone(), q)).collect();
    let mut out = Vec::with_capacity(codes.len());
    for code in codes {
        if let Some(q) = map.remove(code) {
            out.push(Symbol {
                code: code.clone(),
                name: shared(q.name),
                last: q.last,
                change_pct: q.change_pct,
                volume: q.volume,
                board: board_for_code(code),
            });
        } else {
            out.push(Symbol {
                code: code.clone(),
                name: shared(code.clone()),
                last: 0.0,
                change_pct: 0.0,
                volume: 0,
                board: board_for_code(code),
            });
        }
    }
    Ok(out)
}
