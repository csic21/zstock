//! Free A-share daily K-lines via BaoStock TCP API (no registration).
//!
//! Protocol reverse-engineered from the official Python client (`baostock` 0.9.x):
//! - Host: `public-api.baostock.com:10030`
//! - Anonymous login (`anonymous` / `123456`)
//! - Daily K with 前复权 (`adjustflag=2`)
//!
//! Used only as a K-line fallback when Eastmoney / Sina fail.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use flate2::read::ZlibDecoder;

use crate::model::{shared, Candle};

const HOST: &str = "public-api.baostock.com";
const PORT: u16 = 10030;
const VERSION: &str = "00.9.30";
const SPLIT: char = '\u{0001}';
const HEADER_LEN: usize = 21;
const END_MARK: &[u8] = b"<![CDATA[]]>\n";
const USER: &str = "anonymous";
const PASS: &str = "123456";
/// 前复权（与东财 `fqt=1` 对齐）
const ADJUST_QFQ: &str = "2";
const MSG_LOGIN: &str = "00";
const MSG_LOGOUT: &str = "02";
const MSG_KLINE_REQ: &str = "95";
const MSG_KLINE_RESP: &str = "96";

struct Session {
    stream: TcpStream,
    user_id: String,
}

impl Session {
    fn connect() -> Result<Self> {
        use std::net::ToSocketAddrs;
        let addr = format!("{HOST}:{PORT}");
        let sock_addr = addr
            .to_socket_addrs()
            .with_context(|| format!("dns {addr}"))?
            .next()
            .ok_or_else(|| anyhow!("无可用地址: {addr}"))?;
        let stream = TcpStream::connect_timeout(&sock_addr, Duration::from_secs(8))
            .with_context(|| format!("connect {addr}"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .ok();
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .ok();
        let mut s = Self {
            stream,
            user_id: USER.into(),
        };
        s.login()?;
        Ok(s)
    }

    fn login(&mut self) -> Result<()> {
        let body = format!(
            "login{s}{user}{s}{pass}{s}0",
            s = SPLIT,
            user = USER,
            pass = PASS
        );
        let resp = self.request(MSG_LOGIN, &body)?;
        let parts = split_body(&resp);
        let code = parts.first().map(|s| s.as_str()).unwrap_or("");
        if code != "0" {
            let msg = parts.get(1).map(|s| s.as_str()).unwrap_or("login failed");
            return Err(anyhow!("BaoStock 登录失败: {msg} ({code})"));
        }
        if let Some(uid) = parts.get(3) {
            if !uid.is_empty() {
                self.user_id = uid.clone();
            }
        }
        Ok(())
    }

    fn logout(&mut self) {
        let now = chrono::Local::now().format("%Y%m%d%H%M%S");
        let body = format!(
            "logout{s}{uid}{s}{now}",
            s = SPLIT,
            uid = self.user_id,
            now = now
        );
        let _ = self.request(MSG_LOGOUT, &body);
    }

    fn request(&mut self, msg_type: &str, body: &str) -> Result<String> {
        let header = format!(
            "{VERSION}{s}{msg_type}{s}{len:0>10}",
            s = SPLIT,
            msg_type = msg_type,
            len = body.len()
        );
        let head_body = format!("{header}{body}");
        let crc = crc32fast::hash(head_body.as_bytes());
        let packet = format!("{head_body}{s}{crc}\n", s = SPLIT);

        self.stream
            .write_all(packet.as_bytes())
            .context("BaoStock 发送失败")?;
        self.stream.flush().ok();

        let raw = read_until_mark(&mut self.stream)?;
        parse_response(&raw)
    }

    fn fetch_klines_page(
        &mut self,
        bs_code: &str,
        start: &str,
        end: &str,
        page: u32,
    ) -> Result<(Vec<Vec<String>>, bool)> {
        let fields = "date,open,high,low,close,volume";
        let body = format!(
            "query_history_k_data_plus{s}{uid}{s}{page}{s}{per}{s}{code}{s}{fields}{s}{start}{s}{end}{s}d{s}{adj}",
            s = SPLIT,
            uid = self.user_id,
            page = page,
            per = 2000,
            code = bs_code,
            fields = fields,
            start = start,
            end = end,
            adj = ADJUST_QFQ,
        );
        let resp = self.request(MSG_KLINE_REQ, &body)?;
        let parts = split_body(&resp);
        let code = parts.first().map(|s| s.as_str()).unwrap_or("");
        if code != "0" {
            let msg = parts.get(1).map(|s| s.as_str()).unwrap_or("query failed");
            return Err(anyhow!("BaoStock K线失败: {msg} ({code})"));
        }
        let data_json = parts.get(6).map(|s| s.as_str()).unwrap_or("");
        let records = parse_records(data_json)?;
        // full page → maybe more
        let more = records.len() >= 2000;
        Ok((records, more))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.logout();
    }
}

fn read_until_mark(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(8192);
    let mut chunk = [0u8; 8192];
    loop {
        let n = stream
            .read(&mut chunk)
            .context("BaoStock 接收失败")?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.ends_with(END_MARK) {
            break;
        }
        // Safety: avoid unbounded growth on protocol glitch
        if buf.len() > 16 * 1024 * 1024 {
            return Err(anyhow!("BaoStock 响应过大"));
        }
    }
    if buf.is_empty() {
        return Err(anyhow!("BaoStock 空响应"));
    }
    Ok(buf)
}

fn parse_response(raw: &[u8]) -> Result<String> {
    if raw.len() < HEADER_LEN {
        return Err(anyhow!("BaoStock 响应过短"));
    }
    let header = std::str::from_utf8(&raw[..HEADER_LEN]).context("BaoStock 头非 UTF-8")?;
    let parts: Vec<&str> = header.split(SPLIT).collect();
    if parts.len() < 3 {
        return Err(anyhow!("BaoStock 消息头异常"));
    }
    let msg_type = parts[1];
    let body_len: usize = parts[2].parse().unwrap_or(0);

    if msg_type == MSG_KLINE_RESP || msg_type == "99" || msg_type == "9B" || msg_type == "9D" {
        let start = HEADER_LEN;
        let end = start.saturating_add(body_len).min(raw.len());
        let compressed = &raw[start..end];
        let mut decoder = ZlibDecoder::new(compressed);
        let mut body = String::new();
        decoder
            .read_to_string(&mut body)
            .context("BaoStock zlib 解压失败")?;
        Ok(body)
    } else {
        // Strip trailing CDATA / newline if present
        let mut text = String::from_utf8_lossy(raw).into_owned();
        if let Some(idx) = text.find("<![CDATA[]]>") {
            text.truncate(idx);
        }
        text = text.trim_end_matches(['\n', '\r']).to_string();
        if text.len() >= HEADER_LEN {
            Ok(text[HEADER_LEN..].to_string())
        } else {
            Ok(text)
        }
    }
}

fn split_body(body: &str) -> Vec<String> {
    body.split(SPLIT).map(|s| s.to_string()).collect()
}

fn parse_records(data_json: &str) -> Result<Vec<Vec<String>>> {
    let data_json = data_json.split_whitespace().collect::<String>();
    if data_json.is_empty() {
        return Ok(vec![]);
    }
    let v: serde_json::Value =
        serde_json::from_str(&data_json).context("BaoStock JSON 解析失败")?;
    let arr = v
        .get("record")
        .and_then(|x| x.as_array())
        .ok_or_else(|| anyhow!("BaoStock 无 record 字段"))?;
    let mut out = Vec::with_capacity(arr.len());
    for row in arr {
        if let Some(a) = row.as_array() {
            out.push(
                a.iter()
                    .map(|c| match c {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string().trim_matches('"').to_string(),
                    })
                    .collect(),
            );
        }
    }
    Ok(out)
}

/// `sh.600519` / `sz.000001`
pub fn baostock_code(code: &str) -> String {
    let code = code.trim();
    if code.starts_with('6') || code.starts_with('5') || code.starts_with('9') {
        format!("sh.{code}")
    } else {
        format!("sz.{code}")
    }
}

/// Daily K-line (前复权), last `limit` bars.
pub fn fetch_klines(code: &str, limit: usize) -> Result<(String, String, Vec<Candle>)> {
    let code = code.trim();
    let limit = limit.clamp(5, 1000);
    let bs_code = baostock_code(code);

    let end = chrono::Local::now().date_naive();
    // ~2 calendar days per bar covers weekends/holidays; floor 90 days
    let lookback = ((limit as i64) * 2).max(90);
    let start = end - chrono::Duration::days(lookback);
    let start_s = start.format("%Y-%m-%d").to_string();
    let end_s = end.format("%Y-%m-%d").to_string();

    let mut session = Session::connect()?;
    let mut all = Vec::new();
    let mut page = 1u32;
    loop {
        let (rows, more) = session.fetch_klines_page(&bs_code, &start_s, &end_s, page)?;
        let n = rows.len();
        all.extend(rows);
        if !more || n == 0 {
            break;
        }
        page += 1;
        if page > 20 {
            break;
        }
    }
    drop(session);

    let mut candles = Vec::with_capacity(all.len());
    for row in all {
        // date, open, high, low, close, volume
        if row.len() < 6 {
            continue;
        }
        let day = row[0].trim();
        if day.is_empty() {
            continue;
        }
        let label = if day.len() >= 10 {
            day[..10].to_string()
        } else {
            day.to_string()
        };
        let parse = |s: &str| s.trim().parse::<f64>().unwrap_or(0.0);
        candles.push(Candle {
            date: shared(label),
            open: parse(&row[1]),
            high: parse(&row[2]),
            low: parse(&row[3]),
            close: parse(&row[4]),
            volume: parse(&row[5]) as u64,
        });
    }

    if candles.len() > limit {
        candles = candles.split_off(candles.len() - limit);
    }
    if candles.is_empty() {
        return Err(anyhow!("BaoStock K线为空 ({code})"));
    }
    Ok((code.to_string(), String::new(), candles))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baostock_klines_smoke() {
        let r = fetch_klines("600519", 20).expect("baostock klines");
        assert!(!r.2.is_empty());
        assert!(r.2.last().unwrap().close > 0.0);
        eprintln!("baostock n={} last_close={}", r.2.len(), r.2.last().unwrap().close);
    }
}
