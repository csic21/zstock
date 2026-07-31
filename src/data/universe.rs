//! 寻宝鼠候选宇宙：自选 + 东财扩大池（失败则回落内置龙头表）。
//!
//! 全市场逐只拉多年 K 线不现实；策略是：
//! 1. 从东财 clist 按总市值取流动性较好的一批（默认约 400）
//! 2. 合并自选
//! 3. 深评后只保留分数 Top N（默认 100）

use std::collections::BTreeSet;

use anyhow::Result;

use super::eastmoney;

/// 扩大扫描时最多深评的只数（拉多年 K 线）。
pub const TREASURE_SCAN_CAP: usize = 400;
/// 最终入榜只数。
pub const TREASURE_TOP_N: usize = 100;

/// 内置扩展池（东财列表失败时的兜底，约 70+ 只行业代表）。
pub fn extended_universe_codes() -> Vec<String> {
    const CODES: &[&str] = &[
        // 金融
        "600036", "601318", "601166", "600000", "000001", "601398", "601288", "601988", "601328",
        "600016", "601601", "601628", "600030", "300059", "601939", "601288",
        // 消费
        "600519", "000858", "000568", "600809", "002304", "000596", "603288", "600887", "000333",
        "000651", "600690", "002415", "601888", "600132", "002714", "603369",
        // 医药
        "600276", "000661", "300015", "603259", "300347", "600196", "000538", "002422",
        // 制造 / 新能源 / 汽车
        "300750", "002594", "601012", "002460", "300274", "002129", "601633", "000625", "600104",
        "002050", "300124", "002371", "601127", "002920",
        // 科技 / 半导体 / 通信
        "688981", "603501", "002049", "603986", "000063", "600050", "002230", "300014", "688012",
        "603019", "000977", "002415", "300308", // 周期 / 资源 / 基建
        "601899", "600019", "600585", "000825", "601668", "601390", "601186", "600900", "601985",
        "600886", "000157", "601225", "600547", // 地产 / 交运
        "000002", "001979", "600048", "601111", "600009", "601021", "601006",
        // 传媒 / 服务
        "002027", "300413", "603444", "002739",
    ];

    let mut out = Vec::new();
    for c in CODES {
        let code = c.trim();
        if code.len() == 6 && code.chars().all(|ch| ch.is_ascii_digit()) {
            out.push(code.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// 离线兜底：自选 ∪ 内置扩展（不访问网络）。
pub fn build_scan_universe_offline(watchlist: &[String]) -> Vec<String> {
    merge_watchlist_and_extra(watchlist, &extended_universe_codes(), TREASURE_SCAN_CAP)
}

/// 在线扩大：东财按市值取一批 + 自选，上限 `TREASURE_SCAN_CAP`。
///
/// 成功时返回 `(codes, "eastmoney-cap")`；失败回落离线池。
pub fn build_scan_universe_expanded(watchlist: &[String]) -> (Vec<String>, &'static str) {
    match fetch_expanded_codes(TREASURE_SCAN_CAP) {
        Ok(online) if online.len() >= 50 => {
            let codes = merge_watchlist_and_extra(watchlist, &online, TREASURE_SCAN_CAP);
            (codes, "eastmoney-mcap")
        }
        Ok(_) | Err(_) => (build_scan_universe_offline(watchlist), "offline-fallback"),
    }
}

fn fetch_expanded_codes(limit: usize) -> Result<Vec<String>> {
    let rows = eastmoney::fetch_liquid_a_shares(limit)?;
    Ok(rows.into_iter().map(|r| r.code).collect())
}

fn merge_watchlist_and_extra(watchlist: &[String], extra: &[String], cap: usize) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for c in watchlist {
        let c = c.trim();
        if c.len() == 6 && c.chars().all(|ch| ch.is_ascii_digit()) && seen.insert(c.to_string()) {
            out.push(c.to_string());
        }
    }
    for c in extra {
        if out.len() >= cap {
            break;
        }
        if seen.insert(c.clone()) {
            out.push(c.clone());
        }
    }
    out
}

/// 兼容旧名。
pub fn build_scan_universe(watchlist: &[String]) -> Vec<String> {
    build_scan_universe_offline(watchlist)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn universe_is_unique_six_digit() {
        let u = extended_universe_codes();
        assert!(u.len() >= 40);
        assert!(u.iter().all(|c| c.len() == 6));
        let mut s = u.clone();
        s.sort();
        s.dedup();
        assert_eq!(s.len(), u.len());
    }

    #[test]
    fn watchlist_comes_first() {
        let u = build_scan_universe_offline(&["600519".into(), "000001".into()]);
        assert_eq!(u[0], "600519");
        assert_eq!(u[1], "000001");
        assert!(u.len() > 2);
    }

    #[test]
    fn merge_respects_cap() {
        let extra: Vec<String> = (0..500).map(|i| format!("{:06}", i)).collect();
        let u = merge_watchlist_and_extra(&["600519".into()], &extra, 100);
        assert_eq!(u.len(), 100);
        assert_eq!(u[0], "600519");
    }
}
