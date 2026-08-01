//! 寻宝鼠候选宇宙：自选 + 指数成分（动态拉取）或东财市值池，可加财务分位过滤。
//!
//! 全市场逐只拉多年 K 线不现实；策略是：
//! 1. 选择候选池：指数成分（沪深300 / 中证500 / 上证50 / 创业板指 / 科创50，
//!    新浪 `getHQNodeData` 动态拉取，含 PE/PB）或东财按总市值取一批（默认约 400）
//! 2. 可选按 PE / PB 分位过滤（池内横截面分位）
//! 3. 合并自选 → 深评后只保留分数 Top N（默认 100）

use std::collections::BTreeSet;

use anyhow::Result;

use super::{eastmoney, sina};

/// 扩大扫描时最多深评的只数（拉多年 K 线）。
pub const TREASURE_SCAN_CAP: usize = 400;
/// 最终入榜只数。
pub const TREASURE_TOP_N: usize = 100;

/// 寻宝候选池来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreasurePool {
    /// 东财按总市值取沪深 A（默认）。
    Mcap,
    /// 沪深300 成分。
    Hs300,
    /// 中证500 成分。
    Zz500,
    /// 上证50 成分。
    Sh50,
    /// 创业板指成分。
    Cyb,
    /// 科创50 成分。
    Kc50,
}

impl TreasurePool {
    pub fn id(self) -> &'static str {
        match self {
            Self::Mcap => "mcap",
            Self::Hs300 => "hs300",
            Self::Zz500 => "zz500",
            Self::Sh50 => "sh50",
            Self::Cyb => "cyb",
            Self::Kc50 => "kc50",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mcap => "市值",
            Self::Hs300 => "沪深300",
            Self::Zz500 => "中证500",
            Self::Sh50 => "上证50",
            Self::Cyb => "创业板指",
            Self::Kc50 => "科创50",
        }
    }

    pub fn all() -> [Self; 6] {
        [Self::Mcap, Self::Hs300, Self::Zz500, Self::Sh50, Self::Cyb, Self::Kc50]
    }

    pub fn from_id(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "hs300" => Self::Hs300,
            "zz500" => Self::Zz500,
            "sh50" => Self::Sh50,
            "cyb" => Self::Cyb,
            "kc50" => Self::Kc50,
            _ => Self::Mcap,
        }
    }

    /// Sina `getHQNodeData` node, when the pool is index-constituent based.
    fn sina_node(self) -> Option<&'static str> {
        match self {
            Self::Mcap => None,
            Self::Hs300 => Some("hs300"),
            Self::Zz500 => Some("zhishu_000905"),
            Self::Sh50 => Some("zhishu_000016"),
            Self::Cyb => Some("zhishu_399006"),
            Self::Kc50 => Some("zhishu_000688"),
        }
    }
}

/// 财务分位过滤模式（在候选池内按 PE / PB 横截面分位过滤）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinFilter {
    /// 不过滤。
    Off,
    /// 保留 PE 分位 ≤ 50% 的标的。
    Pe,
    /// 保留 PB 分位 ≤ 50% 的标的。
    Pb,
    /// 同时满足 PE 与 PB 分位 ≤ 50%。
    Value,
}

impl FinFilter {
    pub fn id(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Pe => "pe",
            Self::Pb => "pb",
            Self::Value => "value",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "不限",
            Self::Pe => "PE低",
            Self::Pb => "PB低",
            Self::Value => "PE+PB双低",
        }
    }

    pub fn all() -> [Self; 4] {
        [Self::Off, Self::Pe, Self::Pb, Self::Value]
    }

    pub fn from_id(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "pe" => Self::Pe,
            "pb" => Self::Pb,
            "value" => Self::Value,
            _ => Self::Off,
        }
    }
}

/// 候选池构建结果：代码 + 来源标签 + 过滤说明。
pub struct PoolBuild {
    pub codes: Vec<String>,
    pub source: &'static str,
    pub filter_note: String,
}

/// 带估值的一行候选（用于财务分位过滤）。
#[derive(Clone)]
struct PoolRow {
    code: String,
    pe: Option<f64>,
    pb: Option<f64>,
}

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

/// 按指定候选池 + 财务分位过滤构建扫描宇宙。
///
/// - 指数成分：新浪动态拉取（沪深300 / 中证500 / 上证50 / 创业板指 / 科创50）。
/// - 市值池：东财 clist 按总市值取一批。
/// - 任一来源失败时回落内置龙头表（无财务数据时跳过过滤）。
pub fn build_scan_universe_for_pool(
    watchlist: &[String],
    pool: TreasurePool,
    fin: FinFilter,
) -> PoolBuild {
    match fetch_pool_rows(pool) {
        Ok(rows) => {
            let (rows, note) = apply_fin_filter(rows, fin);
            let source = match pool {
                TreasurePool::Mcap => "eastmoney-mcap",
                _ => "sina-index",
            };
            let codes: Vec<String> = rows.into_iter().map(|r| r.code).collect();
            PoolBuild {
                codes: merge_watchlist_and_extra(watchlist, &codes, TREASURE_SCAN_CAP),
                source,
                filter_note: note,
            }
        }
        Err(_) => PoolBuild {
            codes: build_scan_universe_offline(watchlist),
            source: "offline-fallback",
            filter_note: "数据源不可用 · 回落内置池（跳过财务过滤）".into(),
        },
    }
}

/// 兼容旧入口：东财市值池、不过滤。
#[allow(dead_code)]
pub fn build_scan_universe_expanded(watchlist: &[String]) -> (Vec<String>, &'static str) {
    let build = build_scan_universe_for_pool(watchlist, TreasurePool::Mcap, FinFilter::Off);
    (build.codes, build.source)
}

fn fetch_pool_rows(pool: TreasurePool) -> Result<Vec<PoolRow>> {
    match pool {
        TreasurePool::Mcap => {
            let rows = eastmoney::fetch_liquid_a_shares(TREASURE_SCAN_CAP)?;
            Ok(rows
                .into_iter()
                .map(|r| PoolRow {
                    code: r.code,
                    pe: r.pe,
                    pb: r.pb,
                })
                .collect())
        }
        _ => {
            let node = pool.sina_node().ok_or_else(|| anyhow::anyhow!("无指数节点"))?;
            let rows = sina::fetch_index_constituents(node)?;
            Ok(rows
                .into_iter()
                .map(|r| PoolRow {
                    code: r.code,
                    pe: r.pe,
                    pb: r.pb,
                })
                .collect())
        }
    }
}

/// 池内 PE / PB 横截面分位过滤。返回过滤后的行与说明文本。
fn apply_fin_filter(rows: Vec<PoolRow>, fin: FinFilter) -> (Vec<PoolRow>, String) {
    if fin == FinFilter::Off || rows.is_empty() {
        return (rows, format!("{}", fin.label()));
    }
    let coverage_pe = rows.iter().filter(|r| r.pe.is_some()).count();
    let coverage_pb = rows.iter().filter(|r| r.pb.is_some()).count();
    if coverage_pe == 0 && coverage_pb == 0 {
        return (rows, format!("{} · 无财务数据", fin.label()));
    }
    let (pe_rank, pb_rank) = (percentile_ranks(rows.iter().map(|r| r.pe)), percentile_ranks(rows.iter().map(|r| r.pb)));
    let before = rows.len();
    let keep = |i: usize| -> bool {
        match fin {
            FinFilter::Off => true,
            FinFilter::Pe => pe_rank.get(i).copied().unwrap_or(0.5) <= 0.5,
            FinFilter::Pb => pb_rank.get(i).copied().unwrap_or(0.5) <= 0.5,
            FinFilter::Value => {
                pe_rank.get(i).copied().unwrap_or(0.5) <= 0.5
                    && pb_rank.get(i).copied().unwrap_or(0.5) <= 0.5
            }
        }
    };
    let kept: Vec<PoolRow> = rows
        .into_iter()
        .enumerate()
        .filter_map(|(i, r)| keep(i).then_some(r))
        .collect();
    let dropped = before.saturating_sub(kept.len());
    (
        kept,
        format!(
            "{} · 过滤 {dropped}（PE样本 {coverage_pe} · PB样本 {coverage_pb}）",
            fin.label()
        ),
    )
}

/// 每行在全体有效值中的分位（0..1）。无值行为 0.5（视为中性，不因缺失被过滤）。
fn percentile_ranks(values: impl Iterator<Item = Option<f64>>) -> Vec<f64> {
    let vals: Vec<Option<f64>> = values.collect();
    let mut present: Vec<f64> = vals.iter().flatten().copied().collect();
    if present.is_empty() {
        return vec![0.5; vals.len()];
    }
    present.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = present.len() as f64;
    vals.iter()
        .map(|v| match v {
            Some(x) => {
                let below = present.iter().filter(|p| **p < *x).count() as f64;
                let equal = present.iter().filter(|p| **p == *x).count() as f64;
                (below + equal * 0.5) / n
            }
            None => 0.5,
        })
        .collect()
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
#[allow(dead_code)]
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

    #[test]
    fn fin_filter_keeps_low_percentiles() {
        let rows: Vec<PoolRow> = (0..10)
            .map(|i| PoolRow {
                code: format!("{:06}", i),
                pe: Some(i as f64),
                pb: Some(i as f64),
            })
            .collect();
        let (kept, note) = apply_fin_filter(rows.clone(), FinFilter::Pe);
        assert_eq!(kept.len(), 5, "{note}");
        let (kept, note) = apply_fin_filter(rows.clone(), FinFilter::Value);
        assert_eq!(kept.len(), 5, "{note}");
        assert!(note.contains("过滤 5"));
        let (kept, _) = apply_fin_filter(rows.clone(), FinFilter::Off);
        assert_eq!(kept.len(), 10);
    }

    #[test]
    fn fin_filter_skips_when_no_data() {
        let rows: Vec<PoolRow> = (0..6)
            .map(|i| PoolRow {
                code: format!("{:06}", i),
                pe: None,
                pb: None,
            })
            .collect();
        let (kept, note) = apply_fin_filter(rows.clone(), FinFilter::Value);
        assert_eq!(kept.len(), 6);
        assert!(note.contains("无财务数据"));
    }

    #[test]
    fn pool_ids_roundtrip() {
        for p in TreasurePool::all() {
            assert_eq!(TreasurePool::from_id(p.id()), p);
        }
        assert_eq!(TreasurePool::from_id("bogus"), TreasurePool::Mcap);
        for f in FinFilter::all() {
            assert_eq!(FinFilter::from_id(f.id()), f);
        }
        assert_eq!(FinFilter::from_id("bogus"), FinFilter::Off);
    }
}
