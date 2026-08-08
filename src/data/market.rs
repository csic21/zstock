//! Multi-source market data with automatic failover.
//!
//! Quotes: Eastmoney → Tencent（港股 secid `116.*` / `hkxxxxx`）  
//! Daily K: Eastmoney (前复权) → Tencent (前复权, ≤~640；港股东财常空，腾讯可拉)  
//! Search: Eastmoney → Tencent SmartBox（A 股 + 港股）

use anyhow::{Result, anyhow};

use crate::model::{
    Candle, MinutePeriod, MinuteSeries, Symbol, board_for_code, is_hk_code, shared,
};

use super::eastmoney::{self, QuoteTick};
pub use super::eastmoney::SectorTick;
use super::tencent;

/// 行业板块成分股（东财）。
pub fn fetch_sector_constituents(
    sector_code: &str,
    limit: usize,
) -> Result<Sourced<Vec<QuoteTick>>> {
    match eastmoney::fetch_sector_constituents(sector_code, limit) {
        Ok(data) if !data.is_empty() => Ok(Sourced {
            data,
            source: SRC_EASTMONEY,
        }),
        Ok(_) => Err(anyhow!("板块成分为空")),
        Err(e) => Err(e),
    }
}

/// Successful fetch tagged with which backend served it.
#[derive(Debug, Clone)]
pub struct Sourced<T> {
    pub data: T,
    /// Short label for UI, e.g. `东方财富` / `腾讯财经`.
    pub source: &'static str,
}

pub const SRC_EASTMONEY: &str = "东方财富";
pub const SRC_TENCENT: &str = "腾讯财经";
pub const SRC_LABEL: &str = "东财 / 腾讯 · 自动切换";

fn quotes_usable(ticks: &[QuoteTick], requested: usize) -> bool {
    if requested == 0 {
        return true;
    }
    ticks.iter().any(|t| t.last > 0.0 || !t.name.is_empty())
}

/// 上证 / 沪深300 / 创业板 — Eastmoney only (indices).
pub fn fetch_major_indices() -> Result<Sourced<Vec<QuoteTick>>> {
    match eastmoney::fetch_major_indices() {
        Ok(data) if !data.is_empty() => Ok(Sourced {
            data,
            source: SRC_EASTMONEY,
        }),
        Ok(_) => Err(anyhow!("指数行情为空")),
        Err(e) => Err(e),
    }
}

/// Batch quotes: Eastmoney → Tencent.
///
/// 纯港股列表优先腾讯（`qt.gtimg.cn/q=hk…` 更稳）；混仓 / 纯 A 仍东财优先。
pub fn fetch_quotes(codes: &[String]) -> Result<Sourced<Vec<QuoteTick>>> {
    let n = codes.len();
    let hk_only = !codes.is_empty() && codes.iter().all(|c| is_hk_code(c));
    if hk_only {
        match tencent::fetch_quotes(codes) {
            Ok(data) if quotes_usable(&data, n) => {
                return Ok(Sourced {
                    data,
                    source: SRC_TENCENT,
                });
            }
            Ok(_) | Err(_) => {
                // fall through to Eastmoney
            }
        }
        match eastmoney::fetch_quotes(codes) {
            Ok(data) if quotes_usable(&data, n) => {
                return Ok(Sourced {
                    data,
                    source: SRC_EASTMONEY,
                });
            }
            Ok(_) => return Err(anyhow!("行情为空（腾讯与东财均无有效港股数据）")),
            Err(e) => return Err(anyhow!("港股行情失败: {e}")),
        }
    }

    match eastmoney::fetch_quotes(codes) {
        Ok(data) if quotes_usable(&data, n) => Ok(Sourced {
            data,
            source: SRC_EASTMONEY,
        }),
        Ok(empty) => match tencent::fetch_quotes(codes) {
            Ok(data) if quotes_usable(&data, n) => Ok(Sourced {
                data,
                source: SRC_TENCENT,
            }),
            Ok(_) if !empty.is_empty() => Ok(Sourced {
                data: empty,
                source: SRC_EASTMONEY,
            }),
            Ok(_) => Err(anyhow!("行情为空（东财与腾讯均无有效数据）")),
            Err(e2) => {
                if !empty.is_empty() {
                    Ok(Sourced {
                        data: empty,
                        source: SRC_EASTMONEY,
                    })
                } else {
                    Err(anyhow!("行情失败: 东财无数据; 腾讯: {e2}"))
                }
            }
        },
        Err(e1) => match tencent::fetch_quotes(codes) {
            Ok(data) if quotes_usable(&data, n) => Ok(Sourced {
                data,
                source: SRC_TENCENT,
            }),
            Ok(_) => Err(anyhow!("行情失败: 东财: {e1}; 腾讯无有效数据")),
            Err(e2) => Err(anyhow!("行情失败: 东财: {e1}; 腾讯: {e2}")),
        },
    }
}

fn try_klines_chain(
    code: &str,
    limit: usize,
    errors: &mut Vec<String>,
) -> Result<Sourced<(String, String, Vec<Candle>)>> {
    // 港股：东财 his 常 empty / 空 klines，腾讯 `hkxxxxx` 日 K 可用 → 先腾讯。
    if is_hk_code(code) {
        match tencent::fetch_klines(code, limit) {
            Ok(data) if !data.2.is_empty() => {
                return Ok(Sourced {
                    data,
                    source: SRC_TENCENT,
                });
            }
            Ok(_) => errors.push("腾讯无数据".into()),
            Err(e) => errors.push(format!("腾讯: {e}")),
        }
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
        return Err(anyhow!("K线失败: {}", errors.join("; ")));
    }

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

    match tencent::fetch_klines(code, limit) {
        Ok(data) if !data.2.is_empty() => Ok(Sourced {
            data,
            source: SRC_TENCENT,
        }),
        Ok(_) => {
            errors.push("腾讯无数据".into());
            Err(anyhow!("K线失败: {}", errors.join("; ")))
        }
        Err(e) => {
            errors.push(format!("腾讯: {e}"));
            Err(anyhow!("K线失败: {}", errors.join("; ")))
        }
    }
}

/// Daily K: Eastmoney (前复权) → Tencent (前复权).
pub fn fetch_klines(code: &str, limit: usize) -> Result<Sourced<(String, String, Vec<Candle>)>> {
    let mut errors = Vec::new();
    try_klines_chain(code, limit, &mut errors)
}

/// 分时（当日分钟线）：腾讯 `minute/query`。
pub fn fetch_minute_series(code: &str) -> Result<Sourced<MinuteSeries>> {
    let data = tencent::fetch_minute_series(code)?;
    Ok(Sourced {
        data,
        source: SRC_TENCENT,
    })
}

/// 分钟 K：腾讯 `mkline`（m1/m5/m15/m30/m60）。
pub fn fetch_minute_klines(
    code: &str,
    period: MinutePeriod,
    limit: usize,
) -> Result<Sourced<Vec<Candle>>> {
    let data = tencent::fetch_minute_klines(code, period, limit)?;
    Ok(Sourced {
        data,
        source: SRC_TENCENT,
    })
}

/// 历史低位比较专用：只走**前复权**源（东财 → 腾讯）。
///
/// 腾讯日 K 上限约 640 根；东财可到约 1000。寻宝鼠优先东财长窗。
pub fn fetch_klines_adjusted(
    code: &str,
    limit: usize,
) -> Result<Sourced<(String, String, Vec<Candle>)>> {
    // Both providers return 前复权; reuse the same chain.
    fetch_klines(code, limit)
}

/// Symbol search: Eastmoney first; Tencent SmartBox failover.
pub fn search_symbols(query: &str, limit: usize) -> Result<Sourced<Vec<Symbol>>> {
    match eastmoney::search_symbols(query, limit) {
        Ok(data) if !data.is_empty() => Ok(Sourced {
            data,
            source: SRC_EASTMONEY,
        }),
        Ok(_) | Err(_) => {
            let data = tencent::search_symbols(query, limit)?;
            if data.is_empty() {
                Ok(Sourced {
                    data: Vec::new(),
                    source: SRC_EASTMONEY,
                })
            } else {
                Ok(Sourced {
                    data,
                    source: SRC_TENCENT,
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
    #[ignore = "requires public market-data network"]
    fn quotes_failover_works() {
        let codes = vec!["600519".into(), "000001".into()];
        let r = fetch_quotes(&codes).expect("quotes");
        assert!(!r.data.is_empty(), "source={}", r.source);
        assert!(r.data.iter().any(|t| t.last > 0.0));
        eprintln!("quotes source={} n={}", r.source, r.data.len());
    }

    #[test]
    #[ignore = "requires public market-data network"]
    fn klines_failover_works() {
        let r = fetch_klines("600519", 30).expect("klines");
        assert!(!r.data.2.is_empty(), "source={}", r.source);
        eprintln!("klines source={} n={}", r.source, r.data.2.len());
    }

    #[test]
    #[ignore = "requires public market-data network"]
    fn tencent_direct_works() {
        let r = tencent::fetch_klines("600519", 15).expect("tencent");
        assert!(r.2.len() >= 5);
        eprintln!(
            "tencent direct n={} close={}",
            r.2.len(),
            r.2.last().unwrap().close
        );
    }

    #[test]
    #[ignore = "requires public market-data network"]
    fn minute_series_works() {
        let r = fetch_minute_series("600519").expect("minute series");
        assert!(!r.data.is_empty(), "source={}", r.source);
        assert_eq!(r.source, SRC_TENCENT);
        eprintln!(
            "minute series source={} pts={} prev_close={}",
            r.source,
            r.data.points.len(),
            r.data.prev_close
        );
    }

    #[test]
    #[ignore = "requires public market-data network"]
    fn minute_klines_works() {
        let r = fetch_minute_klines("600519", MinutePeriod::M5, 30).expect("minute klines");
        assert!(r.data.len() >= 5, "source={} n={}", r.source, r.data.len());
        assert_eq!(r.source, SRC_TENCENT);
        eprintln!(
            "minute klines source={} n={} last={}",
            r.source,
            r.data.len(),
            r.data.last().unwrap().close
        );
    }
}
