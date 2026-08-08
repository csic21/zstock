//! 扫描结果新鲜度：让用户一眼知道「缓存能不能信」。

use chrono::{DateTime, Local, NaiveDateTime, TimeZone};

/// 新鲜度档位（驱动 UI 颜色与文案）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// 无时间戳 / 无法解析。
    Unknown,
    /// < 4h：新鲜。
    Fresh,
    /// 4h–24h：可用但建议关注。
    Aging,
    /// > 24h：过期，建议重扫。
    Stale,
}

impl Freshness {
    pub fn label(self, work: bool) -> &'static str {
        match (self, work) {
            (Self::Unknown, true) => "Unknown age",
            (Self::Unknown, false) => "时间未知",
            (Self::Fresh, true) => "Fresh",
            (Self::Fresh, false) => "刚刚更新",
            (Self::Aging, true) => "Aging",
            (Self::Aging, false) => "稍旧",
            (Self::Stale, true) => "Stale · rescan",
            (Self::Stale, false) => "已过期 · 建议重扫",
        }
    }

    pub fn from_hours(hours: f64) -> Self {
        if !hours.is_finite() || hours < 0.0 {
            Self::Unknown
        } else if hours < 4.0 {
            Self::Fresh
        } else if hours < 24.0 {
            Self::Aging
        } else {
            Self::Stale
        }
    }
}

/// 解析寻宝/雷达缓存里的本地时间串 `"YYYY-MM-DD HH:MM"`。
pub fn parse_updated_at(s: &str) -> Option<DateTime<Local>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S"))
        .ok()?;
    Local.from_local_datetime(&naive).single()
}

/// 相对现在的小时数；解析失败返回 `None`。
pub fn age_hours(updated_at: &str) -> Option<f64> {
    let t = parse_updated_at(updated_at)?;
    let secs = (Local::now() - t).num_seconds().max(0) as f64;
    Some(secs / 3600.0)
}

pub fn classify(updated_at: &str) -> Freshness {
    match age_hours(updated_at) {
        Some(h) => Freshness::from_hours(h),
        None => {
            if updated_at.trim().is_empty() {
                Freshness::Unknown
            } else {
                // 有字但解析失败：仍展示原串，档位 Unknown
                Freshness::Unknown
            }
        }
    }
}

/// 人类可读：`更新于 08-08 15:05 · 2.1h 前` / `缓存时间未知`。
pub fn banner_text(updated_at: &str, work: bool) -> String {
    let trimmed = updated_at.trim();
    if trimmed.is_empty() {
        return if work {
            "No cache yet · run a scan".into()
        } else {
            "尚无缓存 · 点上方按钮开始找".into()
        };
    }
    let age = age_hours(trimmed);
    let fresh = classify(trimmed);
    let age_part = match age {
        Some(h) if h < 1.0 => {
            let m = (h * 60.0).round().max(1.0) as i64;
            if work {
                format!("{m}m ago")
            } else {
                format!("{m} 分钟前")
            }
        }
        Some(h) if h < 48.0 => {
            if work {
                format!("{h:.1}h ago")
            } else {
                format!("{h:.1} 小时前")
            }
        }
        Some(h) => {
            let d = (h / 24.0).round().max(1.0) as i64;
            if work {
                format!("{d}d ago")
            } else {
                format!("{d} 天前")
            }
        }
        None => String::new(),
    };
    if age_part.is_empty() {
        if work {
            format!("Updated {trimmed} · {}", fresh.label(true))
        } else {
            format!("更新于 {trimmed} · {}", fresh.label(false))
        }
    } else if work {
        format!("Updated {trimmed} · {age_part} · {}", fresh.label(true))
    } else {
        format!("更新于 {trimmed} · {age_part} · {}", fresh.label(false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_bands() {
        assert_eq!(Freshness::from_hours(0.5), Freshness::Fresh);
        assert_eq!(Freshness::from_hours(6.0), Freshness::Aging);
        assert_eq!(Freshness::from_hours(30.0), Freshness::Stale);
    }

    #[test]
    fn parse_local_stamp() {
        assert!(parse_updated_at("2026-08-08 15:05").is_some());
        assert!(parse_updated_at("").is_none());
    }
}
