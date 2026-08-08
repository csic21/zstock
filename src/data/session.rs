//! Exchange session windows for quote polling.
//!
//! A 股（北京时间）：含集合竞价
//! - 上午 09:15–11:30（09:15–09:25 开盘集合竞价，09:30 起连续竞价）
//! - 下午 13:00–15:00（含临近收盘集合竞价）
//!
//! 港股（香港时间，与北京时间同为 UTC+8）：
//! - 上午 09:00–12:00（含开市前竞价）
//! - 下午 13:00–16:10（连续竞价至 16:00，收市竞价至约 16:10）
//!
//! 节假日未内置日历：周末休市；工作日按上述时段轮询。盘外仅在应用启动时
//! 由 `refresh_all` 拉一次快照，轮询循环不再打行情接口。

use chrono::{Datelike, Local, NaiveTime, Weekday};

use crate::model::{is_a_share_code, is_hk_code};

/// Which exchange a pure code belongs to (for session gating).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketId {
    /// 沪 / 深 / 北 A 股与场内基金
    CnA,
    /// 港股主板等（5 位代码）
    Hk,
}

impl MarketId {
    pub fn of_code(code: &str) -> Option<Self> {
        if is_hk_code(code) {
            Some(Self::Hk)
        } else if is_a_share_code(code) {
            Some(Self::CnA)
        } else {
            None
        }
    }

    pub fn is_open_at(self, now: chrono::DateTime<Local>) -> bool {
        if !is_weekday(now) {
            return false;
        }
        let t = now.time();
        match self {
            Self::CnA => a_share_session(t),
            Self::Hk => hk_session(t),
        }
    }
}

fn is_weekday(now: chrono::DateTime<Local>) -> bool {
    !matches!(now.weekday(), Weekday::Sat | Weekday::Sun)
}

/// Inclusive start, exclusive end on wall-clock `NaiveTime`.
fn in_window(t: NaiveTime, start: NaiveTime, end: NaiveTime) -> bool {
    t >= start && t < end
}

fn hm(h: u32, m: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).expect("valid clock")
}

/// A 股：09:15–11:30、13:00–15:00（含竞价）。
fn a_share_session(t: NaiveTime) -> bool {
    // 11:30 这一分钟仍属交易时段 → 结束用 11:31
    in_window(t, hm(9, 15), hm(11, 31)) || in_window(t, hm(13, 0), hm(15, 1))
}

/// 港股：09:00–12:00、13:00–16:10（含开市前 / 收市竞价）。
fn hk_session(t: NaiveTime) -> bool {
    in_window(t, hm(9, 0), hm(12, 1)) || in_window(t, hm(13, 0), hm(16, 11))
}

/// Presence of markets in a code list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MarketSet {
    pub a: bool,
    pub hk: bool,
}

impl MarketSet {
    pub fn from_codes<I, S>(codes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut s = Self::default();
        for c in codes {
            match MarketId::of_code(c.as_ref()) {
                Some(MarketId::CnA) => s.a = true,
                Some(MarketId::Hk) => s.hk = true,
                None => {}
            }
            if s.a && s.hk {
                break;
            }
        }
        s
    }

    pub fn is_empty(self) -> bool {
        !self.a && !self.hk
    }
}

/// Which markets among `present` are currently open.
pub fn open_markets_now(present: MarketSet) -> MarketSet {
    open_markets_at(present, Local::now())
}

pub fn open_markets_at(present: MarketSet, now: chrono::DateTime<Local>) -> MarketSet {
    MarketSet {
        a: present.a && MarketId::CnA.is_open_at(now),
        hk: present.hk && MarketId::Hk.is_open_at(now),
    }
}

/// Keep only codes whose exchange is in session.
pub fn filter_codes_in_session(codes: &[String], open: MarketSet) -> Vec<String> {
    codes
        .iter()
        .filter(|c| match MarketId::of_code(c) {
            Some(MarketId::CnA) => open.a,
            Some(MarketId::Hk) => open.hk,
            None => false,
        })
        .cloned()
        .collect()
}

/// Whether any quote polling should run for the watchlist right now.
pub fn should_poll_quotes(present: MarketSet) -> bool {
    let open = open_markets_now(present);
    open.a || open.hk
}

/// A 股当前是否不在连续交易时段（含周末）——适合静默更新长线榜。
///
/// 盘中不抢带宽；盘后 / 午休边缘 / 周末可以预扫。
pub fn is_a_share_quiet_now() -> bool {
    is_a_share_quiet_at(Local::now())
}

pub fn is_a_share_quiet_at(now: chrono::DateTime<Local>) -> bool {
    !MarketId::CnA.is_open_at(now)
}

/// 是否适合启动后台长线预扫（A 股休市窗口）。
pub fn should_background_long_rescan() -> bool {
    is_a_share_quiet_now()
}

/// Sleep while closed so we wake near the next open without hammering the network.
///
/// Caps at `max_secs` so clock skew / DST edge cases still recover.
pub fn idle_delay_secs(present: MarketSet, max_secs: u64) -> u64 {
    idle_delay_secs_at(present, Local::now(), max_secs)
}

pub fn idle_delay_secs_at(
    present: MarketSet,
    now: chrono::DateTime<Local>,
    max_secs: u64,
) -> u64 {
    let max_secs = max_secs.clamp(5, 300);
    if present.is_empty() {
        return max_secs;
    }
    if let Some(secs) = secs_until_next_open(present, now) {
        secs.clamp(1, max_secs)
    } else {
        max_secs
    }
}

fn secs_until_next_open(present: MarketSet, now: chrono::DateTime<Local>) -> Option<u64> {
    // Scan forward up to 8 days in 1-minute steps is heavy; jump by session anchors.
    let anchors = session_anchors(present);
    if anchors.is_empty() {
        return None;
    }
    let today = now.date_naive();
    for day_off in 0..8 {
        let day = today + chrono::Duration::days(day_off);
        let weekday = day.weekday();
        if matches!(weekday, Weekday::Sat | Weekday::Sun) {
            continue;
        }
        for &anchor in &anchors {
            let candidate = day.and_time(anchor).and_local_timezone(Local).single()?;
            if candidate > now {
                let d = candidate.signed_duration_since(now);
                return Some(d.num_seconds().max(0) as u64);
            }
        }
    }
    None
}

/// Next open clock times we care about (start of each session window).
fn session_anchors(present: MarketSet) -> Vec<NaiveTime> {
    let mut v = Vec::new();
    if present.a {
        v.push(hm(9, 15));
        v.push(hm(13, 0));
    }
    if present.hk {
        v.push(hm(9, 0));
        v.push(hm(13, 0));
    }
    v.sort();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> chrono::DateTime<Local> {
        Local
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .expect("local time")
    }

    #[test]
    fn a_share_includes_call_auction() {
        // 2026-08-03 is Monday
        let mon = at(2026, 8, 3, 9, 15);
        assert!(MarketId::CnA.is_open_at(mon));
        assert!(MarketId::CnA.is_open_at(at(2026, 8, 3, 9, 20)));
        assert!(MarketId::CnA.is_open_at(at(2026, 8, 3, 10, 0)));
        assert!(MarketId::CnA.is_open_at(at(2026, 8, 3, 11, 30)));
        assert!(!MarketId::CnA.is_open_at(at(2026, 8, 3, 11, 31)));
        assert!(!MarketId::CnA.is_open_at(at(2026, 8, 3, 9, 14)));
        assert!(MarketId::CnA.is_open_at(at(2026, 8, 3, 13, 0)));
        assert!(MarketId::CnA.is_open_at(at(2026, 8, 3, 15, 0)));
        assert!(!MarketId::CnA.is_open_at(at(2026, 8, 3, 15, 1)));
        assert!(!MarketId::CnA.is_open_at(at(2026, 8, 1, 10, 0))); // Saturday
    }

    #[test]
    fn hk_includes_preopen_and_close_auction() {
        assert!(MarketId::Hk.is_open_at(at(2026, 8, 3, 9, 0)));
        assert!(MarketId::Hk.is_open_at(at(2026, 8, 3, 9, 10)));
        assert!(MarketId::Hk.is_open_at(at(2026, 8, 3, 12, 0)));
        assert!(!MarketId::Hk.is_open_at(at(2026, 8, 3, 12, 1)));
        assert!(MarketId::Hk.is_open_at(at(2026, 8, 3, 16, 10)));
        assert!(!MarketId::Hk.is_open_at(at(2026, 8, 3, 16, 11)));
        assert!(!MarketId::Hk.is_open_at(at(2026, 8, 3, 8, 59)));
    }

    #[test]
    fn filter_respects_open_set() {
        let codes = vec![
            "600519".into(),
            "00700".into(),
            "000001".into(),
        ];
        let only_a = filter_codes_in_session(&codes, MarketSet { a: true, hk: false });
        assert_eq!(only_a, vec!["600519", "000001"]);
        let only_hk = filter_codes_in_session(&codes, MarketSet { a: false, hk: true });
        assert_eq!(only_hk, vec!["00700"]);
        let both = filter_codes_in_session(&codes, MarketSet { a: true, hk: true });
        assert_eq!(both.len(), 3);
    }

    #[test]
    fn market_set_from_codes() {
        let s = MarketSet::from_codes(["600519", "00700"]);
        assert!(s.a && s.hk);
        let a_only = MarketSet::from_codes(["510300"]);
        assert!(a_only.a && !a_only.hk);
    }

    #[test]
    fn idle_delay_jumps_toward_open() {
        // Monday 08:00, A-only → next open 09:15 ≈ 75 min, capped at 60
        let present = MarketSet { a: true, hk: false };
        let d = idle_delay_secs_at(present, at(2026, 8, 3, 8, 0), 60);
        assert_eq!(d, 60);
        // 09:14 → 1 minute to 09:15 (under cap)
        let d2 = idle_delay_secs_at(present, at(2026, 8, 3, 9, 14), 60);
        assert_eq!(d2, 60); // 60s until 09:15
        // With a higher cap, exact remaining is returned
        let d3 = idle_delay_secs_at(present, at(2026, 8, 3, 9, 14), 300);
        assert_eq!(d3, 60);
    }
}
