//! Free A-share data via Sina Finance public HTTP APIs (no API key).
//!
//! - Quotes: `hq.sinajs.cn/list=sh600519,sz000001` (GBK body)
//! - Daily K: `money.finance.sina.com.cn/.../CN_MarketData.getKLineData` (JSON UTF-8)
//!
//! Used as failover when Eastmoney is unavailable. No SLA; rate-limit politely.

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

use crate::model::{board_for_code, shared, Candle, Symbol};

use super::eastmoney::QuoteTick;

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(8))
        .timeout_read(std::time::Duration::from_secs(15))
        .build()
}

/// `sh600519` / `sz000001`
pub fn sina_symbol(code: &str) -> String {
    let code = code.trim();
    if is_sh_market(code) {
        format!("sh{code}")
    } else {
        format!("sz{code}")
    }
}

fn is_sh_market(code: &str) -> bool {
    code.starts_with('6')
        || code.starts_with('5')
        || code.starts_with('9')
}

fn decode_body_bytes(bytes: &[u8]) -> String {
    // Sina quotes are classically GBK; kline JSON is UTF-8. Try UTF-8 first.
    if let Ok(s) = std::str::from_utf8(bytes) {
        if !s.contains('\u{FFFD}') {
            return s.to_string();
        }
    }
    let (cow, _, _) = encoding_rs::GBK.decode(bytes);
    cow.into_owned()
}

/// Batch quotes for pure codes (`600519`, `000001`, …).
pub fn fetch_quotes(codes: &[String]) -> Result<Vec<QuoteTick>> {
    if codes.is_empty() {
        return Ok(vec![]);
    }
    // Sina list length is practical; chunk to stay polite.
    let mut out = Vec::with_capacity(codes.len());
    for chunk in codes.chunks(50) {
        let list: Vec<String> = chunk.iter().map(|c| sina_symbol(c)).collect();
        let list = list.join(",");
        let url = format!("https://hq.sinajs.cn/list={list}");
        let bytes = fetch_bytes(&url)?;
        let body = decode_body_bytes(&bytes);
        out.extend(parse_quote_body(&body)?);
    }
    if out.is_empty() && !codes.is_empty() {
        return Err(anyhow!("新浪行情为空"));
    }
    Ok(out)
}

fn fetch_bytes(url: &str) -> Result<Vec<u8>> {
    let resp = agent()
        .get(url)
        .set("User-Agent", UA)
        .set("Referer", "https://finance.sina.com.cn")
        .call()
        .map_err(|e| anyhow!("新浪请求失败: {e}"))?;
    let mut reader = resp.into_reader();
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut buf).context("read sina body")?;
    Ok(buf)
}

fn parse_quote_body(body: &str) -> Result<Vec<QuoteTick>> {
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // var hq_str_sh600519="name,open,prev,last,high,low,...";
        let Some(eq) = line.find('=') else {
            continue;
        };
        let left = &line[..eq];
        let right = line[eq + 1..].trim().trim_end_matches(';');
        let payload = right.trim_matches('"');
        if payload.is_empty() {
            continue;
        }
        // left ends with `sh600519` / `sz000001`
        let Some(sym) = left.rsplit('_').next() else {
            continue;
        };
        let sym = sym.trim();
        if sym.len() < 8 {
            continue;
        }
        let code = sym[2..].to_string();
        if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let parts: Vec<&str> = payload.split(',').collect();
        if parts.len() < 9 {
            continue;
        }
        let name = parts[0].trim().to_string();
        let open = parts[1].parse().unwrap_or(0.0);
        let prev_close = parts[2].parse().unwrap_or(0.0);
        let last = parts[3].parse().unwrap_or(0.0);
        let high = parts[4].parse().unwrap_or(0.0);
        let low = parts[5].parse().unwrap_or(0.0);
        let volume = parts[8].parse::<f64>().unwrap_or(0.0) as u64;
        let change_pct = if prev_close > 0.0 && last > 0.0 {
            (last - prev_close) / prev_close * 100.0
        } else {
            0.0
        };
        out.push(QuoteTick {
            code,
            name: if name.is_empty() { "--".into() } else { name },
            last,
            change_pct,
            volume,
            high,
            low,
            open,
            prev_close,
        });
    }
    Ok(out)
}

/// Daily K-line (not adjusted on this free endpoint), latest `limit` bars.
pub fn fetch_klines(code: &str, limit: usize) -> Result<(String, String, Vec<Candle>)> {
    let code = code.trim();
    let limit = limit.clamp(5, 1000);
    let symbol = sina_symbol(code);
    let url = format!(
        "https://money.finance.sina.com.cn/quotes_service/api/json_v2.php/\
         CN_MarketData.getKLineData?symbol={symbol}&scale=240&ma=no&datalen={limit}"
    );
    let body = agent()
        .get(&url)
        .set("User-Agent", UA)
        .set("Referer", "https://finance.sina.com.cn")
        .call()
        .map_err(|e| anyhow!("新浪K线: {e}"))?
        .into_string()
        .context("read sina kline")?;

    let v: Value = serde_json::from_str(body.trim()).context("parse sina kline json")?;
    let arr = v
        .as_array()
        .ok_or_else(|| anyhow!("新浪K线格式异常 ({code})"))?;

    let mut candles = Vec::with_capacity(arr.len());
    for row in arr {
        let day = row
            .get("day")
            .and_then(|x| x.as_str())
            .unwrap_or_default();
        if day.is_empty() {
            continue;
        }
        let label = if day.len() >= 10 {
            day[..10].to_string()
        } else {
            day.to_string()
        };
        let num = |k: &str| -> f64 {
            row.get(k)
                .and_then(|x| {
                    x.as_f64()
                        .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
                })
                .unwrap_or(0.0)
        };
        candles.push(Candle {
            date: shared(label),
            open: num("open"),
            high: num("high"),
            low: num("low"),
            close: num("close"),
            volume: num("volume") as u64,
        });
    }
    if candles.is_empty() {
        return Err(anyhow!("新浪K线为空 ({code})"));
    }

    // Name is not in kline payload; leave empty so caller keeps existing / quote name.
    Ok((code.to_string(), String::new(), candles))
}

/// Very light search: 6-digit code → single hit; otherwise empty (Eastmoney owns search).
pub fn search_symbols(query: &str, _limit: usize) -> Result<Vec<Symbol>> {
    let q = query.trim();
    if q.chars().all(|c| c.is_ascii_digit()) && q.len() == 6 {
        return Ok(vec![Symbol {
            code: q.to_string(),
            name: shared(q),
            last: 0.0,
            change_pct: 0.0,
            volume: 0,
            board: board_for_code(q),
        }]);
    }
    Ok(vec![])
}
