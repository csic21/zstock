//! 本地持仓：交易流水、平均成本、浮动盈亏与组合汇总。
//!
//! - **平均成本法**（A 股常见）：买入加权成本，卖出按当前均价结转已实现盈亏。
//! - 不追踪融资/融券；股数为 `f64` 以便兼容 ETF 等非整手场景。
//! - 全部本地 JSON 持久化，不上传。

use serde::{Deserialize, Serialize};

/// 买卖方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TradeSide {
    Buy,
    Sell,
}

impl TradeSide {
    pub fn label(self) -> &'static str {
        match self {
            Self::Buy => "买入",
            Self::Sell => "卖出",
        }
    }

    pub fn label_work(self) -> &'static str {
        match self {
            Self::Buy => "Buy",
            Self::Sell => "Sell",
        }
    }
}

/// 一笔成交。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    /// 稳定 id（时间戳 + 随机后缀），用于撤销。
    pub id: String,
    pub code: String,
    pub name: String,
    pub side: TradeSide,
    /// 股数（> 0）。
    pub shares: f64,
    /// 成交价（元）。
    pub price: f64,
    /// 手续费（元，可 0）。
    #[serde(default)]
    pub fee: f64,
    /// 本地时间 `YYYY-MM-DD HH:MM:SS`。
    pub time: String,
    #[serde(default)]
    pub note: String,
}

impl Trade {
    /// 成交金额（不含费）：股数 × 价。
    pub fn notional(&self) -> f64 {
        self.shares * self.price
    }

    /// 买入现金流出 / 卖出现金流入（含费）。
    pub fn cash_delta(&self) -> f64 {
        match self.side {
            TradeSide::Buy => -(self.notional() + self.fee),
            TradeSide::Sell => self.notional() - self.fee,
        }
    }
}

/// 用户投资组合：全部成交流水 + 可选现金。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Portfolio {
    #[serde(default)]
    pub trades: Vec<Trade>,
    /// 现金余额（元）。`track_cash` 为 false 时仅作展示用，不强制校验。
    #[serde(default)]
    pub cash: f64,
    /// 是否启用现金约束（买入时检查余额，卖出回补）。
    #[serde(default)]
    pub track_cash: bool,
}

/// 单标的当前持仓（由流水重放得出）。
#[derive(Debug, Clone)]
pub struct Position {
    pub code: String,
    pub name: String,
    /// 当前持股。
    pub shares: f64,
    /// 每股平均成本（含买入手续费摊入）。
    pub avg_cost: f64,
    /// 成本市值 = avg_cost × shares。
    pub total_cost: f64,
    /// 该标的历史已实现盈亏累计。
    pub realized_pnl: f64,
    /// 参与过该标的的成交笔数。
    pub trade_count: usize,
}

impl Position {
    pub fn is_open(&self) -> bool {
        self.shares > 1e-9
    }
}

/// 持仓 + 最新价标记。
#[derive(Debug, Clone)]
pub struct PositionMark {
    pub position: Position,
    pub last: f64,
    /// 当日涨跌幅（来自行情，非持仓盈亏）。
    pub day_change_pct: f64,
    pub market_value: f64,
    pub unrealized_pnl: f64,
    pub unrealized_pnl_pct: f64,
}

impl PositionMark {
    pub fn from_position(pos: Position, last: f64, day_change_pct: f64) -> Self {
        let last = if last.is_finite() && last > 0.0 {
            last
        } else {
            pos.avg_cost
        };
        let market_value = pos.shares * last;
        let unrealized_pnl = market_value - pos.total_cost;
        let unrealized_pnl_pct = if pos.total_cost > 1e-9 {
            unrealized_pnl / pos.total_cost * 100.0
        } else {
            0.0
        };
        Self {
            position: pos,
            last,
            day_change_pct,
            market_value,
            unrealized_pnl,
            unrealized_pnl_pct,
        }
    }
}

/// 组合汇总。
#[derive(Debug, Clone, Default)]
pub struct PortfolioSummary {
    pub positions: Vec<PositionMark>,
    pub total_cost: f64,
    pub total_market_value: f64,
    pub total_unrealized_pnl: f64,
    pub total_unrealized_pnl_pct: f64,
    pub total_realized_pnl: f64,
    pub cash: f64,
    pub track_cash: bool,
    /// 开仓标的只数。
    pub open_count: usize,
}

/// 录单错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TradeError {
    InvalidShares,
    InvalidPrice,
    InvalidFee,
    InsufficientShares { have: String, want: String },
    InsufficientCash { have: String, need: String },
    EmptyCode,
}

impl std::fmt::Display for TradeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidShares => write!(f, "股数须大于 0"),
            Self::InvalidPrice => write!(f, "价格须大于 0"),
            Self::InvalidFee => write!(f, "手续费不能为负"),
            Self::InsufficientShares { have, want } => {
                write!(f, "可卖不足：持有 {have} 股，尝试卖出 {want} 股")
            }
            Self::InsufficientCash { have, need } => {
                write!(f, "现金不足：余额 {have} 元，需要 {need} 元")
            }
            Self::EmptyCode => write!(f, "代码不能为空"),
        }
    }
}

impl std::error::Error for TradeError {}

/// 新建交易 id。
pub fn new_trade_id() -> String {
    let ts = chrono::Local::now().format("%Y%m%d%H%M%S%3f");
    let r: u32 = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
        ^ 0xA5A5_5A5A)
        % 10_000;
    format!("{ts}-{r:04}")
}

/// 本地时间戳字符串。
pub fn now_time_string() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

impl Portfolio {
    /// 所有出现过的代码（含已清仓）。
    pub fn all_codes(&self) -> Vec<String> {
        let mut out = Vec::new();
        for t in &self.trades {
            if !out.iter().any(|c| c == &t.code) {
                out.push(t.code.clone());
            }
        }
        out
    }

    /// 当前有持仓的代码。
    pub fn open_codes(&self) -> Vec<String> {
        self.positions()
            .into_iter()
            .filter(|p| p.is_open())
            .map(|p| p.code)
            .collect()
    }

    /// 按代码重放流水 → 持仓列表（仅开仓，按 code 排序）。
    pub fn positions(&self) -> Vec<Position> {
        let mut map: std::collections::BTreeMap<String, PositionState> =
            std::collections::BTreeMap::new();
        for t in &self.trades {
            let st = map.entry(t.code.clone()).or_insert_with(|| PositionState {
                name: t.name.clone(),
                shares: 0.0,
                cost_basis: 0.0,
                realized_pnl: 0.0,
                trade_count: 0,
            });
            if !t.name.is_empty() && t.name != t.code {
                st.name = t.name.clone();
            }
            st.apply(t);
        }
        map.into_iter()
            .filter_map(|(code, st)| {
                if st.shares <= 1e-9 {
                    return None;
                }
                let avg = if st.shares > 1e-9 {
                    st.cost_basis / st.shares
                } else {
                    0.0
                };
                Some(Position {
                    code,
                    name: st.name,
                    shares: st.shares,
                    avg_cost: avg,
                    total_cost: st.cost_basis,
                    realized_pnl: st.realized_pnl,
                    trade_count: st.trade_count,
                })
            })
            .collect()
    }

    /// 单标的持仓（含已清仓则 None）。
    pub fn position_of(&self, code: &str) -> Option<Position> {
        self.positions().into_iter().find(|p| p.code == code)
    }

    /// 单标的完整状态（含已清仓的已实现盈亏）。
    pub fn position_state_of(&self, code: &str) -> Option<Position> {
        let mut st = PositionState::default();
        let mut found = false;
        for t in &self.trades {
            if t.code != code {
                continue;
            }
            found = true;
            if !t.name.is_empty() {
                st.name = t.name.clone();
            }
            st.apply(t);
        }
        if !found {
            return None;
        }
        let avg = if st.shares > 1e-9 {
            st.cost_basis / st.shares
        } else {
            0.0
        };
        Some(Position {
            code: code.to_string(),
            name: st.name,
            shares: st.shares,
            avg_cost: avg,
            total_cost: st.cost_basis,
            realized_pnl: st.realized_pnl,
            trade_count: st.trade_count,
        })
    }

    /// 某标的全部成交（时间正序）。
    pub fn trades_for(&self, code: &str) -> Vec<&Trade> {
        self.trades.iter().filter(|t| t.code == code).collect()
    }

    /// 用最新价标记组合。
    ///
    /// `quote_fn(code) -> (last, day_change_pct, name_hint)`
    pub fn summarize_with<F>(&self, mut quote_fn: F) -> PortfolioSummary
    where
        F: FnMut(&str) -> (f64, f64, String),
    {
        let positions = self.positions();
        let mut marks = Vec::with_capacity(positions.len());
        let mut total_cost = 0.0;
        let mut total_mv = 0.0;
        let mut total_realized = 0.0;

        for mut pos in positions {
            let (last, day_chg, name_hint) = quote_fn(&pos.code);
            if pos.name.is_empty() || pos.name == pos.code {
                if !name_hint.is_empty() {
                    pos.name = name_hint;
                }
            }
            let mark = PositionMark::from_position(pos, last, day_chg);
            total_cost += mark.position.total_cost;
            total_mv += mark.market_value;
            total_realized += mark.position.realized_pnl;
            marks.push(mark);
        }

        // 按浮动盈亏比例降序，便于扫一眼风险。
        marks.sort_by(|a, b| {
            b.unrealized_pnl_pct
                .partial_cmp(&a.unrealized_pnl_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let total_unrealized = total_mv - total_cost;
        let total_unrealized_pct = if total_cost > 1e-9 {
            total_unrealized / total_cost * 100.0
        } else {
            0.0
        };
        let open_count = marks.len();

        PortfolioSummary {
            positions: marks,
            total_cost,
            total_market_value: total_mv,
            total_unrealized_pnl: total_unrealized,
            total_unrealized_pnl_pct: total_unrealized_pct,
            total_realized_pnl: total_realized,
            cash: self.cash,
            track_cash: self.track_cash,
            open_count,
        }
    }

    /// 录一笔买卖。成功返回 trade id。
    pub fn record_trade(
        &mut self,
        code: &str,
        name: &str,
        side: TradeSide,
        shares: f64,
        price: f64,
        fee: f64,
        note: &str,
        time: Option<String>,
    ) -> Result<String, TradeError> {
        let code = code.trim();
        if code.is_empty() {
            return Err(TradeError::EmptyCode);
        }
        if !shares.is_finite() || shares <= 0.0 {
            return Err(TradeError::InvalidShares);
        }
        if !price.is_finite() || price <= 0.0 {
            return Err(TradeError::InvalidPrice);
        }
        if !fee.is_finite() || fee < 0.0 {
            return Err(TradeError::InvalidFee);
        }

        if side == TradeSide::Sell {
            let have = self.position_of(code).map(|p| p.shares).unwrap_or(0.0);
            if shares > have + 1e-6 {
                return Err(TradeError::InsufficientShares {
                    have: format_shares(have),
                    want: format_shares(shares),
                });
            }
        }

        if self.track_cash && side == TradeSide::Buy {
            let need = shares * price + fee;
            if need > self.cash + 1e-6 {
                return Err(TradeError::InsufficientCash {
                    have: format!("{:.2}", self.cash),
                    need: format!("{need:.2}"),
                });
            }
        }

        let id = new_trade_id();
        let trade = Trade {
            id: id.clone(),
            code: code.to_string(),
            name: name.trim().to_string(),
            side,
            shares,
            price,
            fee,
            time: time.unwrap_or_else(now_time_string),
            note: note.trim().to_string(),
        };

        if self.track_cash {
            self.cash += trade.cash_delta();
            if self.cash < 0.0 && self.cash.abs() < 1e-6 {
                self.cash = 0.0;
            }
        }

        self.trades.push(trade);
        Ok(id)
    }

    /// 按 id 撤销一笔成交（并回滚现金，若启用）。
    pub fn remove_trade(&mut self, id: &str) -> bool {
        let Some(ix) = self.trades.iter().position(|t| t.id == id) else {
            return false;
        };
        let trade = self.trades.remove(ix);
        if self.track_cash {
            // 撤销 = 反向现金流
            self.cash -= trade.cash_delta();
        }
        // 撤销后可能出现「卖出 > 持仓」的历史状态；由调用方负责校验或允许负持仓回放失败。
        // 这里做一次一致性检查：若某标的重放后股数为负，拒绝撤销并还原。
        if self.has_negative_position() {
            if self.track_cash {
                self.cash += trade.cash_delta();
            }
            self.trades.insert(ix, trade);
            return false;
        }
        true
    }

    fn has_negative_position(&self) -> bool {
        let mut map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        for t in &self.trades {
            let e = map.entry(t.code.clone()).or_insert(0.0);
            match t.side {
                TradeSide::Buy => *e += t.shares,
                TradeSide::Sell => *e -= t.shares,
            }
            if *e < -1e-6 {
                return true;
            }
        }
        false
    }

    /// 清仓：按给定价卖出全部持股。
    pub fn close_position(
        &mut self,
        code: &str,
        price: f64,
        fee: f64,
        note: &str,
    ) -> Result<Option<String>, TradeError> {
        let Some(pos) = self.position_of(code) else {
            return Ok(None);
        };
        if pos.shares <= 1e-9 {
            return Ok(None);
        }
        let id = self.record_trade(
            code,
            &pos.name,
            TradeSide::Sell,
            pos.shares,
            price,
            fee,
            if note.is_empty() { "清仓" } else { note },
            None,
        )?;
        Ok(Some(id))
    }
}

#[derive(Default)]
struct PositionState {
    name: String,
    shares: f64,
    /// 当前持仓总成本（含买入费摊入）。
    cost_basis: f64,
    realized_pnl: f64,
    trade_count: usize,
}

impl PositionState {
    fn apply(&mut self, t: &Trade) {
        self.trade_count += 1;
        match t.side {
            TradeSide::Buy => {
                self.cost_basis += t.shares * t.price + t.fee;
                self.shares += t.shares;
            }
            TradeSide::Sell => {
                let sell = t.shares.min(self.shares.max(0.0));
                if sell <= 0.0 {
                    return;
                }
                let avg = if self.shares > 1e-9 {
                    self.cost_basis / self.shares
                } else {
                    0.0
                };
                let proceeds = sell * t.price - t.fee * (sell / t.shares).clamp(0.0, 1.0);
                let cost = avg * sell;
                self.realized_pnl += proceeds - cost;
                self.shares -= sell;
                if self.shares <= 1e-9 {
                    self.shares = 0.0;
                    self.cost_basis = 0.0;
                } else {
                    self.cost_basis = avg * self.shares;
                }
            }
        }
    }
}

/// 格式化股数：整数不带小数，否则最多 2 位。
pub fn format_shares(v: f64) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.2}")
    }
}

/// 格式化盈亏金额（带符号）。
pub fn format_money(v: f64) -> String {
    if v >= 0.0 {
        format!("+{:.2}", v)
    } else {
        format!("{v:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buy(p: &mut Portfolio, code: &str, shares: f64, price: f64, fee: f64) {
        p.record_trade(code, "测试", TradeSide::Buy, shares, price, fee, "", None)
            .unwrap();
    }

    fn sell(p: &mut Portfolio, code: &str, shares: f64, price: f64, fee: f64) {
        p.record_trade(code, "测试", TradeSide::Sell, shares, price, fee, "", None)
            .unwrap();
    }

    #[test]
    fn average_cost_on_multiple_buys() {
        let mut p = Portfolio::default();
        buy(&mut p, "600519", 100.0, 10.0, 0.0); // cost 1000
        buy(&mut p, "600519", 100.0, 20.0, 0.0); // cost +2000 → avg 15
        let pos = p.position_of("600519").unwrap();
        assert!((pos.shares - 200.0).abs() < 1e-9);
        assert!((pos.avg_cost - 15.0).abs() < 1e-9);
        assert!((pos.total_cost - 3000.0).abs() < 1e-9);
    }

    #[test]
    fn buy_fee_is_included_in_cost() {
        let mut p = Portfolio::default();
        buy(&mut p, "000001", 100.0, 10.0, 5.0);
        let pos = p.position_of("000001").unwrap();
        assert!((pos.avg_cost - 10.05).abs() < 1e-9);
    }

    #[test]
    fn sell_realizes_pnl_and_keeps_avg() {
        let mut p = Portfolio::default();
        buy(&mut p, "600519", 200.0, 10.0, 0.0);
        sell(&mut p, "600519", 100.0, 12.0, 0.0); // +200 realized
        let pos = p.position_of("600519").unwrap();
        assert!((pos.shares - 100.0).abs() < 1e-9);
        assert!((pos.avg_cost - 10.0).abs() < 1e-9);
        assert!((pos.realized_pnl - 200.0).abs() < 1e-9);
    }

    #[test]
    fn cannot_sell_more_than_held() {
        let mut p = Portfolio::default();
        buy(&mut p, "600519", 100.0, 10.0, 0.0);
        let err = p
            .record_trade("600519", "x", TradeSide::Sell, 150.0, 11.0, 0.0, "", None)
            .unwrap_err();
        assert!(matches!(err, TradeError::InsufficientShares { .. }));
    }

    #[test]
    fn close_position_sells_all() {
        let mut p = Portfolio::default();
        buy(&mut p, "600519", 100.0, 10.0, 0.0);
        let id = p.close_position("600519", 11.0, 0.0, "").unwrap();
        assert!(id.is_some());
        assert!(p.position_of("600519").is_none());
        let st = p.position_state_of("600519").unwrap();
        assert!((st.realized_pnl - 100.0).abs() < 1e-9);
    }

    #[test]
    fn mark_to_market_summary() {
        let mut p = Portfolio::default();
        buy(&mut p, "600519", 100.0, 10.0, 0.0);
        buy(&mut p, "000001", 200.0, 5.0, 0.0);
        let sum = p.summarize_with(|code| match code {
            "600519" => (12.0, 1.0, "茅台".into()),
            "000001" => (4.0, -1.0, "平安".into()),
            _ => (0.0, 0.0, String::new()),
        });
        assert_eq!(sum.open_count, 2);
        assert!((sum.total_cost - 2000.0).abs() < 1e-9);
        // 600519: 1200, 000001: 800 → 2000
        assert!((sum.total_market_value - 2000.0).abs() < 1e-9);
        assert!((sum.total_unrealized_pnl).abs() < 1e-9);
    }

    #[test]
    fn cash_tracking_on_buy_sell() {
        let mut p = Portfolio {
            cash: 10_000.0,
            track_cash: true,
            ..Default::default()
        };
        buy(&mut p, "600519", 100.0, 10.0, 5.0); // -1005
        assert!((p.cash - 8995.0).abs() < 1e-6);
        sell(&mut p, "600519", 50.0, 12.0, 2.0); // +598
        assert!((p.cash - 9593.0).abs() < 1e-6);
    }

    #[test]
    fn cash_blocks_overbuy() {
        let mut p = Portfolio {
            cash: 100.0,
            track_cash: true,
            ..Default::default()
        };
        let err = p
            .record_trade("600519", "x", TradeSide::Buy, 100.0, 10.0, 0.0, "", None)
            .unwrap_err();
        assert!(matches!(err, TradeError::InsufficientCash { .. }));
    }

    #[test]
    fn remove_trade_rolls_back() {
        let mut p = Portfolio::default();
        let id = p
            .record_trade("600519", "x", TradeSide::Buy, 100.0, 10.0, 0.0, "", None)
            .unwrap();
        assert!(p.remove_trade(&id));
        assert!(p.position_of("600519").is_none());
        assert!(p.trades.is_empty());
    }

    #[test]
    fn remove_trade_rejects_breaking_history() {
        let mut p = Portfolio::default();
        let buy_id = p
            .record_trade("600519", "x", TradeSide::Buy, 100.0, 10.0, 0.0, "", None)
            .unwrap();
        p.record_trade("600519", "x", TradeSide::Sell, 100.0, 11.0, 0.0, "", None)
            .unwrap();
        // 撤销买入会使卖出变成裸卖 → 拒绝
        assert!(!p.remove_trade(&buy_id));
        assert_eq!(p.trades.len(), 2);
    }

    #[test]
    fn serde_roundtrip() {
        let mut p = Portfolio {
            cash: 2000.0,
            track_cash: true,
            ..Default::default()
        };
        buy(&mut p, "600519", 100.0, 10.0, 1.0);
        let s = serde_json::to_string(&p).unwrap();
        let p2: Portfolio = serde_json::from_str(&s).unwrap();
        assert_eq!(p2.trades.len(), 1);
        assert!((p2.cash - p.cash).abs() < 1e-9);
        assert!((p2.cash - 999.0).abs() < 1e-9);
    }
}
