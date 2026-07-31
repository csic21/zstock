//! Free A-share data via Tencent Finance public HTTP APIs (no API key).
//!
//! - Quotes: `qt.gtimg.cn/q=sh600519,sz000001` (GBK text, `~` fields)
//! - Daily K: `web.ifzq.gtimg.cn/.../newfqkline/get` 前复权 (`qfq`；旧 `fqkline` 兜底)
//! - Search: `smartbox.gtimg.cn/s3/?q=…&t=all`
//!
//! Used as failover when Eastmoney is unavailable. No SLA; rate-limit politely.
//! K-line window is capped (~640 bars on this endpoint).

use anyhow::{Context, Result, anyhow};
use serde_json::Value;

use crate::model::{
    Candle, MinutePeriod, MinutePoint, MinuteSeries, Symbol, board_for_code, shared,
};

use super::eastmoney::QuoteTick;

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";
/// Practical max bars returned by Tencent day K endpoints.
const KLINE_CAP: usize = 640;

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(8))
        .timeout_read(std::time::Duration::from_secs(15))
        .build()
}

/// `sh600519` / `sz000001` / `bj830799`
pub fn tencent_symbol(code: &str) -> String {
    let code = code.trim();
    if is_sh_market(code) {
        format!("sh{code}")
    } else if is_bj_market(code) {
        format!("bj{code}")
    } else {
        format!("sz{code}")
    }
}

fn is_sh_market(code: &str) -> bool {
    code.starts_with('6') || code.starts_with('5') || code.starts_with('9')
}

fn is_bj_market(code: &str) -> bool {
    // 北交所常见 4xxxxx / 8xxxxx
    code.starts_with('4') || code.starts_with('8')
}

fn decode_body_bytes(bytes: &[u8]) -> String {
    // qt.gtimg.cn is classically GBK; JSON endpoints are UTF-8. Prefer UTF-8 when valid.
    if let Ok(s) = std::str::from_utf8(bytes) {
        // Valid UTF-8 that still has replacement chars is rare; accept clean UTF-8.
        if !s.contains('\u{FFFD}') {
            return s.to_string();
        }
    }
    let (cow, _, _) = encoding_rs::GBK.decode(bytes);
    cow.into_owned()
}

fn fetch_bytes(url: &str, referer: &str) -> Result<Vec<u8>> {
    let resp = agent()
        .get(url)
        .set("User-Agent", UA)
        .set("Referer", referer)
        .call()
        .map_err(|e| anyhow!("腾讯请求失败: {e}"))?;
    let mut reader = resp.into_reader();
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut buf).context("read tencent body")?;
    Ok(buf)
}

fn fetch_text(url: &str, referer: &str) -> Result<String> {
    let bytes = fetch_bytes(url, referer)?;
    Ok(decode_body_bytes(&bytes))
}

fn fetch_json(url: &str) -> Result<Value> {
    let body = fetch_text(url, "https://finance.qq.com")?;
    // newfqkline is pure JSON; fqkline with _var may be `kline_dayqfq={...}`
    let json_str = body
        .find('{')
        .map(|i| &body[i..])
        .unwrap_or(body.as_str())
        .trim();
    serde_json::from_str(json_str).context("parse tencent json")
}

/// Batch quotes for pure codes (`600519`, `000001`, …).
pub fn fetch_quotes(codes: &[String]) -> Result<Vec<QuoteTick>> {
    if codes.is_empty() {
        return Ok(vec![]);
    }
    let mut out = Vec::with_capacity(codes.len());
    for chunk in codes.chunks(50) {
        let list: Vec<String> = chunk.iter().map(|c| tencent_symbol(c)).collect();
        let list = list.join(",");
        let url = format!("https://qt.gtimg.cn/q={list}");
        let body = fetch_text(&url, "https://finance.qq.com")?;
        out.extend(parse_quote_body(&body)?);
    }
    if out.is_empty() && !codes.is_empty() {
        return Err(anyhow!("腾讯行情为空"));
    }
    Ok(out)
}

/// Parse `v_sh600519="1~贵州茅台~600519~…";` lines.
fn parse_quote_body(body: &str) -> Result<Vec<QuoteTick>> {
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(eq) = line.find('=') else {
            continue;
        };
        let payload = line[eq + 1..].trim();
        let payload = payload
            .trim_start_matches('"')
            .trim_end_matches(';')
            .trim_end_matches('"');
        if payload.is_empty() {
            continue;
        }
        let f: Vec<&str> = payload.split('~').collect();
        // Need at least high/low (index 34) for a usable tick.
        if f.len() < 35 {
            continue;
        }
        let code = f[2].trim().to_string();
        if code.is_empty() {
            continue;
        }
        let parse_f = |i: usize| -> f64 { f.get(i).and_then(|s| s.parse().ok()).unwrap_or(0.0) };
        out.push(QuoteTick {
            code,
            name: f[1].trim().to_string(),
            last: parse_f(3),
            prev_close: parse_f(4),
            open: parse_f(5),
            volume: parse_f(6) as u64,
            change_pct: parse_f(32),
            high: parse_f(33),
            low: parse_f(34),
        });
    }
    Ok(out)
}

/// Daily K-line (前复权 `qfq`), last `limit` bars (capped at ~640).
pub fn fetch_klines(code: &str, limit: usize) -> Result<(String, String, Vec<Candle>)> {
    let code = code.trim();
    let limit = limit.clamp(5, KLINE_CAP);
    let symbol = tencent_symbol(code);

    let primary = format!(
        "https://web.ifzq.gtimg.cn/appstock/app/newfqkline/get?param={symbol},day,,,{limit},qfq"
    );
    match parse_klines_response(code, &symbol, fetch_json(&primary)?) {
        Ok(ok) if !ok.2.is_empty() => return Ok(ok),
        Ok(_) => {}
        Err(_) => {}
    }

    let fallback = format!(
        "https://web.ifzq.gtimg.cn/appstock/app/fqkline/get?param={symbol},day,,,{limit},qfq"
    );
    parse_klines_response(code, &symbol, fetch_json(&fallback)?)
}

fn parse_klines_response(
    code: &str,
    symbol: &str,
    v: Value,
) -> Result<(String, String, Vec<Candle>)> {
    let node = v
        .pointer(&format!("/data/{symbol}"))
        .ok_or_else(|| anyhow!("腾讯K线无 data/{symbol}"))?;

    let name = node
        .pointer(&format!("/qt/{symbol}/1"))
        .and_then(|x| x.as_str())
        .unwrap_or(code)
        .to_string();

    // Prefer 前复权 series; indices may only expose `day`.
    let series = node
        .get("qfqday")
        .or_else(|| node.get("day"))
        .and_then(|x| x.as_array())
        .ok_or_else(|| anyhow!("腾讯K线序列为空 ({code})"))?;

    let mut candles = Vec::with_capacity(series.len());
    for row in series {
        let parts = match row {
            Value::Array(a) => a,
            _ => continue,
        };
        if parts.len() < 6 {
            continue;
        }
        let day = parts[0].as_str().unwrap_or_default();
        if day.is_empty() {
            continue;
        }
        let label = if day.len() >= 10 {
            day[..10].to_string()
        } else {
            day.to_string()
        };
        let num = |i: usize| -> f64 {
            parts
                .get(i)
                .and_then(|x| {
                    x.as_f64()
                        .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
                })
                .unwrap_or(0.0)
        };
        // [date, open, close, high, low, volume, …]
        candles.push(Candle {
            date: shared(label),
            open: num(1),
            close: num(2),
            high: num(3),
            low: num(4),
            volume: num(5) as u64,
        });
    }
    if candles.is_empty() {
        return Err(anyhow!("腾讯K线为空 ({code})"));
    }
    Ok((code.to_string(), name, candles))
}

/// 分时（当日分钟线），腾讯 `minute/query`。
///
/// 每行 `HHMM price cum_volume cum_amount`，其中 volume 为累计手数、amount 为累计成交额。
pub fn fetch_minute_series(code: &str) -> Result<MinuteSeries> {
    let code = code.trim();
    let symbol = tencent_symbol(code);
    let url = format!(
        "https://web.ifzq.gtimg.cn/appstock/app/minute/query?code={symbol}"
    );
    let v = fetch_json(&url)?;
    let node = v
        .pointer(&format!("/data/{symbol}/data"))
        .ok_or_else(|| anyhow!("腾讯分时无 data/{symbol}/data"))?;

    let name = v
        .pointer(&format!("/data/{symbol}/qt/{symbol}/1"))
        .and_then(|x| x.as_str())
        .unwrap_or(code)
        .to_string();
    let prev_close = v
        .pointer(&format!("/data/{symbol}/qt/{symbol}/4"))
        .and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(0.0);
    let date = node
        .get("date")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let rows = node
        .get("data")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();

    let points = parse_minute_rows(&rows);
    if points.is_empty() {
        return Err(anyhow!("腾讯分时为空 ({code})"));
    }
    Ok(MinuteSeries {
        date,
        points,
        prev_close,
        name,
    })
}

/// `HHMM price cum_volume cum_amount` → points. Volume 为累计手数，amount 为累计成交额。
fn parse_minute_rows(rows: &[Value]) -> Vec<MinutePoint> {
    let mut points = Vec::with_capacity(rows.len());
    for row in rows {
        let s = row.as_str().unwrap_or_default();
        let mut parts = s.split_whitespace();
        let (Some(hhmm), Some(price), Some(cum_vol), Some(cum_amt)) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            continue;
        };
        let (Ok(price), Ok(cum_vol), Ok(cum_amt)) = (
            price.parse::<f64>(),
            cum_vol.parse::<f64>(),
            cum_amt.parse::<f64>(),
        ) else {
            continue;
        };
        let time = if hhmm.len() == 4 {
            format!("{}:{}", &hhmm[..2], &hhmm[2..])
        } else {
            hhmm.to_string()
        };
        points.push(MinutePoint {
            time: shared(time),
            price,
            cum_volume: cum_vol as u64,
            cum_amount: cum_amt,
        });
    }
    points
}

/// 分钟 K（`mkline`）：m1/m5/m15/m30/m60，单次上限约 320/800 根。
pub fn fetch_minute_klines(
    code: &str,
    period: MinutePeriod,
    limit: usize,
) -> Result<Vec<Candle>> {
    let code = code.trim();
    let symbol = tencent_symbol(code);
    let limit = limit.clamp(5, period.bars());
    let url = format!(
        "https://ifzq.gtimg.cn/appstock/app/kline/mkline?param={symbol},{},,{limit}",
        period.param()
    );
    let v = fetch_json(&url)?;
    let series = v
        .pointer(&format!("/data/{symbol}/{}", period.param()))
        .and_then(|x| x.as_array())
        .ok_or_else(|| anyhow!("腾讯分钟K无 data/{symbol}/{}", period.param()))?;

    let mut candles = Vec::with_capacity(series.len());
    for row in series {
        let parts = match row {
            Value::Array(a) => a,
            _ => continue,
        };
        if parts.len() < 6 {
            continue;
        }
        let ts = parts[0].as_str().unwrap_or_default();
        if ts.is_empty() {
            continue;
        }
        let num = |i: usize| -> f64 {
            parts
                .get(i)
                .and_then(|x| {
                    x.as_f64()
                        .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
                })
                .unwrap_or(0.0)
        };
        // `202607311500` → `2026-07-31 15:00`
        candles.push(Candle {
            date: shared(format_minute_label(ts)),
            open: num(1),
            close: num(2),
            high: num(3),
            low: num(4),
            volume: num(5) as u64,
        });
    }
    if candles.is_empty() {
        return Err(anyhow!("腾讯分钟K为空 ({code} {})", period.param()));
    }
    Ok(candles)
}

fn format_minute_label(ts: &str) -> String {
    let b = ts.as_bytes();
    if b.len() >= 12 && b.iter().all(|c| c.is_ascii_digit()) {
        format!(
            "{}-{}-{} {}:{}",
            &ts[..4],
            &ts[4..6],
            &ts[6..8],
            &ts[8..10],
            &ts[10..12]
        )
    } else {
        ts.to_string()
    }
}

/// SmartBox search (name / pinyin / code). Filters to A-share style 6-digit codes.
pub fn search_symbols(query: &str, limit: usize) -> Result<Vec<Symbol>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let limit = limit.clamp(1, 20);
    let url = format!(
        "https://smartbox.gtimg.cn/s3/?q={}&t=all",
        urlencoding_minimal(q)
    );
    let body = fetch_text(&url, "https://finance.qq.com")?;
    let mut out = parse_smartbox(&body, limit);

    // Fallback: pure 6-digit code
    if out.is_empty() && q.chars().all(|c| c.is_ascii_digit()) && q.len() == 6 {
        out.push(Symbol {
            code: q.to_string(),
            name: shared(q),
            last: 0.0,
            change_pct: 0.0,
            volume: 0,
            board: board_for_code(q),
        });
    }
    Ok(out)
}

fn parse_smartbox(body: &str, limit: usize) -> Vec<Symbol> {
    // v_hint="sh~600519~\u8d35\u5dde\u8305\u53f0~gzmt~GP-A^sz~000001~…"
    let payload = body
        .find("=\"")
        .map(|i| &body[i + 2..])
        .or_else(|| body.find('=').map(|i| &body[i + 1..]))
        .unwrap_or(body);
    let payload = payload
        .trim()
        .trim_start_matches('"')
        .trim_end_matches(';')
        .trim_end_matches('"');
    let payload = decode_json_unicode_escapes(payload);

    let mut out = Vec::new();
    for entry in payload.split('^') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let parts: Vec<&str> = entry.split('~').collect();
        if parts.len() < 3 {
            continue;
        }
        let market = parts[0].trim().to_ascii_lowercase();
        if market != "sh" && market != "sz" && market != "bj" {
            continue;
        }
        let code = parts[1].trim().to_string();
        if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let typ = parts.get(4).map(|s| s.trim()).unwrap_or("");
        // Drop pure indices / open-end funds; keep A-shares and common listed funds.
        if typ == "ZS" || typ == "KJ" {
            continue;
        }
        let name = decode_json_unicode_escapes(parts[2].trim());
        if name.is_empty() {
            continue;
        }
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
    out
}

/// SmartBox returns literal `\uXXXX` sequences (not real UTF-8 Chinese).
fn decode_json_unicode_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' && it.peek() == Some(&'u') {
            it.next(); // consume 'u'
            let mut hex = String::with_capacity(4);
            for _ in 0..4 {
                if let Some(h) = it.peek().copied() {
                    if h.is_ascii_hexdigit() {
                        hex.push(h);
                        it.next();
                        continue;
                    }
                }
                break;
            }
            if hex.len() == 4 {
                if let Ok(cp) = u16::from_str_radix(&hex, 16) {
                    if let Some(ch) = char::from_u32(cp as u32) {
                        out.push(ch);
                        continue;
                    }
                }
            }
            // Incomplete escape — keep raw.
            out.push('\\');
            out.push('u');
            out.push_str(&hex);
            continue;
        }
        out.push(c);
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires public market-data network"]
    fn quotes_smoke() {
        let codes = vec!["600519".into(), "000001".into()];
        let r = fetch_quotes(&codes).expect("tencent quotes");
        assert!(r.iter().any(|t| t.last > 0.0));
        assert!(
            r.iter()
                .any(|t| t.name.contains('茅') || t.code == "000001")
        );
        eprintln!(
            "tencent quotes: {:?}",
            r.iter()
                .map(|t| format!("{} {} {}", t.code, t.name, t.last))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    #[ignore = "requires public market-data network"]
    fn klines_smoke() {
        let r = fetch_klines("600519", 20).expect("tencent klines");
        assert!(r.2.len() >= 5);
        eprintln!(
            "tencent klines n={} name={} last={}",
            r.2.len(),
            r.1,
            r.2.last().unwrap().close
        );
    }

    #[test]
    #[ignore = "requires public market-data network"]
    fn search_smoke() {
        let r = search_symbols("茅台", 8).expect("search");
        assert!(r.iter().any(|s| s.code == "600519"), "{r:?}");
    }

    #[test]
    #[ignore = "requires public market-data network"]
    fn minute_series_smoke() {
        let s = fetch_minute_series("600519").expect("tencent minute series");
        assert!(!s.points.is_empty());
        assert!(s.prev_close > 0.0);
        assert!(s.points[0].cum_volume > 0);
        assert!(!s.points[0].time.is_empty());
        eprintln!(
            "tencent minute: date={} pts={} prev_close={} last={}",
            s.date,
            s.points.len(),
            s.prev_close,
            s.points.last().unwrap().price
        );
    }

    #[test]
    #[ignore = "requires public market-data network"]
    fn minute_klines_smoke() {
        for p in [
            MinutePeriod::M1,
            MinutePeriod::M5,
            MinutePeriod::M15,
            MinutePeriod::M30,
            MinutePeriod::M60,
        ] {
            let c = fetch_minute_klines("600519", p, 30).expect("tencent minute klines");
            assert!(c.len() >= 5, "{p:?} n={}", c.len());
            let last = c.last().unwrap();
            assert!(last.high >= last.low && last.close > 0.0, "{p:?}");
            assert!(last.date.as_ref().contains(':'), "{}", last.date);
        }
        eprintln!("tencent minute klines ok");
    }

    #[test]
    fn parse_minute_rows_offline() {
        let rows: Vec<Value> = serde_json::from_str(
            r#"["0930 1330.03 1191 158406573.03","0931 1327.77 3547 471549408.00","bad row"]"#,
        )
        .unwrap();
        let points = parse_minute_rows(&rows);
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].time.as_ref(), "09:30");
        assert!((points[0].price - 1330.03).abs() < 1e-6);
        assert_eq!(points[0].cum_volume, 1191);
        assert!((points[1].avg_price() - 471549408.0 / (3547.0 * 100.0)).abs() < 1e-6);
        assert_eq!(points[1].minute_volume(Some(&points[0])), 3547 - 1191);
    }

    #[test]
    fn parse_quote_line() {
        let body = r#"v_sh600519="1~贵州茅台~600519~1420.97~1422.35~1423.05~18655~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~~20250808161419~-1.38~-0.10~1426.50~1418.00~x";"#;
        let ticks = parse_quote_body(body).unwrap();
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].code, "600519");
        assert!((ticks[0].last - 1420.97).abs() < 1e-6);
        assert!((ticks[0].change_pct - (-0.10)).abs() < 1e-6);
        assert!((ticks[0].high - 1426.50).abs() < 1e-6);
    }
}
