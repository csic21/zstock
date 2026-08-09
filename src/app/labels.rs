//! Centralized UI copy for normal vs work mode.
//!
//! Prefer `L::…(work)` over ad-hoc `if work { "…" } else { "…" }` in render paths
//! so work-mode disguise and Chinese labels stay consistent.

/// Namespace for static UI strings.
pub(crate) struct L;

impl L {
    // —— Left tabs ——
    pub(crate) fn left_watchlist(work: bool) -> &'static str {
        if work { "List" } else { "自选" }
    }

    pub(crate) fn left_portfolio(work: bool) -> &'static str {
        if work { "Book" } else { "持仓" }
    }

    pub(crate) fn left_treasure(work: bool) -> &'static str {
        if work { "Find" } else { "🔍 找" }
    }

    pub(crate) fn find_long(work: bool) -> &'static str {
        if work { "Long" } else { "长线" }
    }

    pub(crate) fn find_short(work: bool) -> &'static str {
        if work { "Short" } else { "短线" }
    }

    // —— Detail (analysis dock) tabs ——
    pub(crate) fn detail_overview(work: bool) -> &'static str {
        if work { "Overview" } else { "概览" }
    }

    pub(crate) fn detail_strategy(work: bool) -> &'static str {
        if work { "Signal" } else { "策略" }
    }

    pub(crate) fn detail_ai(_work: bool) -> &'static str {
        "AI"
    }

    pub(crate) fn detail_portfolio(work: bool) -> &'static str {
        if work { "Book" } else { "持仓" }
    }

    pub(crate) fn detail_treasure(work: bool) -> &'static str {
        if work { "Scan" } else { "机会" }
    }

    pub(crate) fn detail_indicators(work: bool) -> &'static str {
        if work { "Tech" } else { "指标" }
    }

    // —— Command palette ——
    pub(crate) fn palette_section_local(work: bool) -> &'static str {
        if work { "List" } else { "自选" }
    }

    pub(crate) fn palette_section_remote(work: bool) -> &'static str {
        if work {
            "Results · Enter to add"
        } else {
            "搜索结果 · Enter 添加"
        }
    }

    pub(crate) fn palette_empty(work: bool) -> &'static str {
        if work {
            "Type an id or name · ↑↓ · Enter"
        } else {
            "输入代码或名称 · ↑↓ 选择 · Enter 确认"
        }
    }

    pub(crate) fn palette_footer(work: bool) -> &'static str {
        if work {
            "↑↓ navigate · Enter select · Esc close"
        } else {
            "↑↓ 选择 · Enter 确认 · Esc 关闭 · ⌘K 开关"
        }
    }

    pub(crate) fn palette_add(work: bool) -> &'static str {
        if work { "attach" } else { "添加" }
    }

    // —— Chart loading ——
    pub(crate) fn chart_loading(work: bool) -> &'static str {
        if work {
            "Loading series…"
        } else {
            "K线加载中…"
        }
    }

    pub(crate) fn chart_refreshing(work: bool) -> &'static str {
        if work {
            "Refreshing…"
        } else {
            "刷新中…"
        }
    }

    pub(crate) fn chart_no_data(work: bool) -> &'static str {
        if work {
            "No series data"
        } else {
            "暂无匹配的 K 线"
        }
    }

    pub(crate) fn loading_short(work: bool) -> &'static str {
        if work { "Loading…" } else { "加载中…" }
    }

    // —— Quick links ——
    pub(crate) fn quick_links(work: bool) -> &'static str {
        if work { "Quick" } else { "快捷" }
    }

    pub(crate) fn goto_strategy(work: bool) -> &'static str {
        if work { "Signal →" } else { "策略 →" }
    }

    pub(crate) fn goto_ai(_work: bool) -> &'static str {
        "AI →"
    }

    pub(crate) fn goto_portfolio(work: bool) -> &'static str {
        if work { "Book →" } else { "持仓 →" }
    }

    pub(crate) fn goto_treasure(work: bool) -> &'static str {
        if work { "Scan →" } else { "机会 →" }
    }

    pub(crate) fn goto_indicators(work: bool) -> &'static str {
        if work { "Tech →" } else { "指标 →" }
    }
}
