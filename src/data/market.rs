//! Multi-source market data with automatic failover.
//!
//! Quotes: Eastmoney → Sina  
//! Daily K: Eastmoney (前复权) → Sina → BaoStock (前复权)  
//! Search: Eastmoney (6-digit Sina fallback)

use anyhow::{anyhow, Result};

use crate::model::{board_for_code, shared, Candle, Symbol};

use super::baostock;
use super::eastmoney::{self, QuoteTick};
use super::sina;

/// Successful fetch tagged with which backend served it.
#[derive(Debug, Clone)]
pub struct Sourced<T> {
    pub data: T,
    /// Short label for UI, e.g. `东方财富` / `新浪财经` / `BaoStock`.
    pub source: &'static str,
}

pub const SRC_EASTMONEY: &str = "东方财富";
pub const SRC_SINA: &str = "新浪财经";
pub const SRC_BAOSTOCK: &str = "BaoStock";
pub const SRC_LABEL: &str = "东财 / 新浪 / BaoStock · 自动切换";

fn quotes_usable(ticks: &[QuoteTick], requested: usize) -> bool {
    if requested == 0 {
        return true;
    }
    ticks.iter().any(|t| t.last > 0.0 || !t.name.is_empty())
}

/// Batch quotes: Eastmoney → Sina.
pub fn fetch_quotes(codes: &[String]) -> Result<Sourced<Vec<QuoteTick>>> {
    let n = codes.len();
    match eastmoney::fetch_quotes(codes) {
        Ok(data) if quotes_usable(&data, n) => Ok(Sourced {
            data,
            source: SRC_EASTMONEY,
        }),
        Ok(empty) => match sina::fetch_quotes(codes) {
            Ok(data) if quotes_usable(&data, n) => Ok(Sourced {
                data,
                source: SRC_SINA,
            }),
            Ok(_) if !empty.is_empty() => Ok(Sourced {
                data: empty,
                source: SRC_EASTMONEY,
            }),
            Ok(_) => Err(anyhow!("行情为空（东财与新浪均无有效数据）")),
            Err(e2) => {
                if !empty.is_empty() {
                    Ok(Sourced {
                        data: empty,
                        source: SRC_EASTMONEY,
                    })
                } else {
                    Err(anyhow!("行情失败: 东财无数据; 新浪: {e2}"))
                }
            }
        },
        Err(e1) => match sina::fetch_quotes(codes) {
            Ok(data) if quotes_usable(&data, n) => Ok(Sourced {
                data,
                source: SRC_SINA,
            }),
            Ok(_) => Err(anyhow!("行情失败: 东财: {e1}; 新浪无有效数据")),
            Err(e2) => Err(anyhow!("行情失败: 东财: {e1}; 新浪: {e2}")),
        },
    }
}

fn try_klines_chain(
    code: &str,
    limit: usize,
    errors: &mut Vec<String>,
) -> Result<Sourced<(String, String, Vec<Candle>)>> {
    match eastmoney::fetch_klines(code, limit) {
        Ok(data) if !data.2.is_empty() => {
            return Ok(Sourced {
                data,
                source: SRC_EASTMONEY,
            });
        }
        Ok(_) => errors.push("东财无数据".into()),
        Err(e) => errors.push(format!("东财: {e}")),
    }

    match sina::fetch_klines(code, limit) {
        Ok(data) if !data.2.is_empty() => {
            return Ok(Sourced {
                data,
                source: SRC_SINA,
            });
        }
        Ok(_) => errors.push("新浪无数据".into()),
        Err(e) => errors.push(format!("新浪: {e}")),
    }

    match baostock::fetch_klines(code, limit) {
        Ok(data) if !data.2.is_empty() => Ok(Sourced {
            data,
            source: SRC_BAOSTOCK,
        }),
        Ok(_) => {
            errors.push("BaoStock无数据".into());
            Err(anyhow!("K线失败: {}", errors.join("; ")))
        }
        Err(e) => {
            errors.push(format!("BaoStock: {e}"));
            Err(anyhow!("K线失败: {}", errors.join("; ")))
        }
    }
}

/// Daily K: Eastmoney (前复权) → Sina → BaoStock (前复权).
pub fn fetch_klines(code: &str, limit: usize) -> Result<Sourced<(String, String, Vec<Candle>)>> {
    let mut errors = Vec::new();
    try_klines_chain(code, limit, &mut errors)
}

/// Symbol search: Eastmoney first; 6-digit fallback via Sina helper.
pub fn search_symbols(query: &str, limit: usize) -> Result<Sourced<Vec<Symbol>>> {
    match eastmoney::search_symbols(query, limit) {
        Ok(data) if !data.is_empty() => Ok(Sourced {
            data,
            source: SRC_EASTMONEY,
        }),
        Ok(_) | Err(_) => {
            let data = sina::search_symbols(query, limit)?;
            if data.is_empty() {
                Ok(Sourced {
                    data: Vec::new(),
                    source: SRC_EASTMONEY,
                })
            } else {
                Ok(Sourced {
                    data,
                    source: SRC_SINA,
                })
            }
        }
    }
}

/// Hydrate watchlist codes with names/prices (failover quotes).
pub fn hydrate_symbols(codes: &[String]) -> Result<Sourced<Vec<Symbol>>> {
    let sourced = fetch_quotes(codes)?;
    let mut map: std::collections::HashMap<String, QuoteTick> = sourced
        .data
        .into_iter()
        .map(|q| (q.code.clone(), q))
        .collect();
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
    Ok(Sourced {
        data: out,
        source: sourced.source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_failover_works() {
        let codes = vec!["600519".into(), "000001".into()];
        let r = fetch_quotes(&codes).expect("quotes");
        assert!(!r.data.is_empty(), "source={}", r.source);
        assert!(r.data.iter().any(|t| t.last > 0.0));
        eprintln!("quotes source={} n={}", r.source, r.data.len());
    }

    #[test]
    fn klines_failover_works() {
        let r = fetch_klines("600519", 30).expect("klines");
        assert!(!r.data.2.is_empty(), "source={}", r.source);
        eprintln!("klines source={} n={}", r.source, r.data.2.len());
    }

    #[test]
    fn baostock_direct_works() {
        let r = baostock::fetch_klines("600519", 15).expect("baostock");
        assert!(r.2.len() >= 5);
        eprintln!("baostock direct n={} close={}", r.2.len(), r.2.last().unwrap().close);
    }
}
