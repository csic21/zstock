//! Sina Finance public APIs (free, no key).
//!
//! Used for **index constituents** (`Market_Center.getHQNodeData`), which also
//! carry per-stock PE (`per`) and PB (`pb`) for financial-percentile filtering.

use anyhow::{Context, Result, anyhow};
use serde_json::Value;

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";
const HQ_NODE: &str =
    "https://vip.stock.finance.sina.com.cn/quotes_service/api/json_v2.php/Market_Center.getHQNodeData";

/// One index member with optional valuation fields.
#[derive(Debug, Clone)]
pub struct IndexMember {
    pub code: String,
    #[allow(dead_code)]
    pub name: String,
    /// PE (市盈率) from the list row, when the source provides it.
    pub pe: Option<f64>,
    /// PB (市净率) from the list row, when the source provides it.
    pub pb: Option<f64>,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(8))
        .timeout_read(std::time::Duration::from_secs(20))
        .build()
}

fn num(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Fetch every member of a Sina index node, e.g. `hs300`, `zhishu_000905`.
///
/// The endpoint caps each page at 100 rows; loop until a short page.
pub fn fetch_index_constituents(node: &str) -> Result<Vec<IndexMember>> {
    let node = node.trim();
    if node.is_empty() {
        return Err(anyhow!("指数节点为空"));
    }
    let mut out: Vec<IndexMember> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for page in 1..=30u32 {
        let url = format!(
            "{HQ_NODE}?page={page}&num=100&sort=symbol&asc=1&node={node}&symbol=&_s_r_a=init"
        );
        let body = agent()
            .get(&url)
            .set("User-Agent", UA)
            .set("Referer", "https://finance.sina.com.cn/")
            .call()
            .with_context(|| format!("sina index {node}"))?
            .into_string()
            .context("read body")?;
        let rows: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let Some(arr) = rows.as_array() else {
            // Empty page → done.
            break;
        };
        if arr.is_empty() {
            break;
        }
        for item in arr {
            let code = item
                .get("code")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            // Some rows expose `symbol` like "sh600519"; prefer `code`, else strip.
            let code = if code.is_empty() {
                item.get("symbol")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .chars()
                    .rev()
                    .take(6)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
            } else {
                code
            };
            if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            if !seen.insert(code.clone()) {
                continue;
            }
            let name = item
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let pe = num(item.get("per"));
            let pb = num(item.get("pb"));
            out.push(IndexMember {
                code,
                name,
                pe: (pe > 0.0 && pe.is_finite()).then_some(pe),
                pb: (pb > 0.0 && pb.is_finite()).then_some(pb),
            });
        }
        if arr.len() < 100 {
            break;
        }
    }
    if out.is_empty() {
        return Err(anyhow!("指数 {node} 无成分数据"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires public market-data network"]
    fn hs300_constituents_smoke() {
        let rows = fetch_index_constituents("hs300").expect("hs300");
        assert!(rows.len() >= 280, "n={}", rows.len());
        assert!(rows.iter().all(|r| r.code.len() == 6));
        let with_pe = rows.iter().filter(|r| r.pe.is_some()).count();
        eprintln!("hs300 n={} with_pe={with_pe}", rows.len());
        assert!(with_pe as f64 / rows.len() as f64 > 0.9);
    }
}
