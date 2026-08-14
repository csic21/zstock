//! Adaptive presentation for the A-share market heatmap.
//!
//! Laying out every listed stock as its own cell leaves most tiles too small
//! to label, and reserved sector/industry headers eat the remaining space.
//! Given available pixels this module decides whether an industry should
//! collapse into one labeled block or show only its largest names.

/// Smallest area (px²) a stock tile should occupy before we keep it.
/// About 36×18 — enough for a 2–3 character name at compact type.
pub(crate) const MIN_STOCK_TILE_AREA: f32 = 640.0;

/// An industry smaller than this becomes a single colored block.
const MIN_EXPANDED_INDUSTRY_AREA: f32 = 2_200.0;

/// A strip thinner than this cannot hold useful stock tiles.
const MIN_EXPANDED_INDUSTRY_SIDE: f32 = 40.0;

/// Hard cap so a huge board (半导体, 化学制药) stays readable.
const MAX_STOCK_TILES: usize = 32;

/// If the hidden tail is still this share of turnover, keep adding names
/// (up to [`MAX_STOCK_TILES`]) so "其他" does not become the largest cell.
const MAX_OTHERS_SHARE: f64 = 0.32;

/// Hide the tail entirely when it is this small — the visible names expand.
const MIN_OTHERS_SHARE: f64 = 0.06;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VisibleStocks {
    /// Number of leading stocks (already sorted by amount) to draw.
    pub keep: usize,
    pub others: Option<OthersTile>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OthersTile {
    pub count: usize,
    pub amount: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TileLabelPlan {
    pub show_name: bool,
    pub show_change: bool,
    pub show_amount: bool,
    /// Pack name and change on one row when the cell is wide and short.
    pub horizontal: bool,
}

/// Amount-weighted average change. Falls back to a simple mean when every
/// amount is missing so a board of unpriced names still gets a colour.
pub(crate) fn weighted_change_pct<I>(items: I) -> f64
where
    I: IntoIterator<Item = (f64, f64)>,
{
    let mut weighted = 0.0;
    let mut weight = 0.0;
    let mut simple = 0.0;
    let mut count = 0.0;
    for (amount, change) in items {
        if !change.is_finite() {
            continue;
        }
        simple += change;
        count += 1.0;
        if amount.is_finite() && amount > 0.0 {
            weighted += change * amount;
            weight += amount;
        }
    }
    if weight > 0.0 {
        weighted / weight
    } else if count > 0.0 {
        simple / count
    } else {
        0.0
    }
}

pub(crate) fn should_expand_industry(width: f32, height: f32) -> bool {
    let width = width.max(0.0);
    let height = height.max(0.0);
    width * height >= MIN_EXPANDED_INDUSTRY_AREA && width.min(height) >= MIN_EXPANDED_INDUSTRY_SIDE
}

/// Choose how many of the largest stocks to draw inside `area_px`.
///
/// `amounts` must already be sorted descending. The returned `keep` always
/// covers a prefix so callers can slice without reshuffling.
pub(crate) fn select_visible_stocks(amounts: &[f64], area_px: f32) -> VisibleStocks {
    let n = amounts.len();
    if n == 0 {
        return VisibleStocks {
            keep: 0,
            others: None,
        };
    }
    if n == 1 || area_px <= 0.0 {
        return VisibleStocks {
            keep: 1.min(n),
            others: None,
        };
    }

    let cleaned: Vec<f64> = amounts
        .iter()
        .map(|amount| {
            if amount.is_finite() && *amount > 0.0 {
                *amount
            } else {
                0.0
            }
        })
        .collect();
    let total: f64 = cleaned.iter().sum();
    if total <= 0.0 {
        return VisibleStocks {
            keep: n.min(MAX_STOCK_TILES),
            others: None,
        };
    }

    let mut keep = max_prefix_meeting_min_area(&cleaned, f64::from(area_px)).clamp(1, n);
    keep = keep.min(MAX_STOCK_TILES);

    while keep < n && keep < MAX_STOCK_TILES {
        let hidden: f64 = cleaned[keep..].iter().sum();
        if hidden / total <= MAX_OTHERS_SHARE {
            break;
        }
        keep += 1;
    }

    if keep >= n {
        return VisibleStocks {
            keep: n,
            others: None,
        };
    }

    let hidden_amount: f64 = cleaned[keep..].iter().sum();
    let hidden_count = n - keep;
    let hidden_share = hidden_amount / total;
    if hidden_count == 0 || hidden_share < MIN_OTHERS_SHARE {
        return VisibleStocks { keep, others: None };
    }

    let shown_plus_hidden = cleaned[..keep].iter().sum::<f64>() + hidden_amount;
    let others_area = if shown_plus_hidden > 0.0 {
        hidden_amount / shown_plus_hidden * f64::from(area_px)
    } else {
        0.0
    };
    if others_area < f64::from(MIN_STOCK_TILE_AREA) * 0.45 {
        return VisibleStocks { keep, others: None };
    }

    VisibleStocks {
        keep,
        others: Some(OthersTile {
            count: hidden_count,
            amount: hidden_amount,
        }),
    }
}

fn max_prefix_meeting_min_area(amounts: &[f64], area_px: f64) -> usize {
    let min_area = f64::from(MIN_STOCK_TILE_AREA);
    let mut sum = 0.0;
    let mut keep = 0;
    for (index, amount) in amounts.iter().copied().enumerate() {
        let next = sum + amount.max(0.0);
        if next <= 0.0 {
            break;
        }
        let smallest = amount.max(0.0) / next * area_px;
        if index > 0 && smallest < min_area {
            break;
        }
        sum = next;
        keep = index + 1;
    }
    keep.max(1).min(amounts.len())
}

pub(crate) fn tile_label_plan(width: f32, height: f32, emphasize: bool) -> TileLabelPlan {
    let width = width.max(0.0);
    let height = height.max(0.0);
    let name_w = if emphasize { 28.0 } else { 34.0 };
    let name_h = if emphasize { 12.0 } else { 14.0 };
    let show_name = width >= name_w && height >= name_h;
    let can_stack_change = width >= 40.0 && height >= 26.0;
    let can_inline_change = width >= 56.0 && (14.0..28.0).contains(&height);
    let show_change = show_name && (can_stack_change || can_inline_change);
    TileLabelPlan {
        show_name,
        show_change,
        show_amount: width >= 86.0 && height >= 46.0 && show_change,
        horizontal: show_change && can_inline_change && !can_stack_change,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiny_industry_collapses() {
        assert!(!should_expand_industry(36.0, 30.0));
        assert!(!should_expand_industry(120.0, 18.0));
        assert!(should_expand_industry(80.0, 50.0));
    }

    #[test]
    fn empty_or_single_stock_has_no_others() {
        assert_eq!(
            select_visible_stocks(&[], 8_000.0),
            VisibleStocks {
                keep: 0,
                others: None
            }
        );
        assert_eq!(
            select_visible_stocks(&[10.0], 8_000.0),
            VisibleStocks {
                keep: 1,
                others: None
            }
        );
    }

    #[test]
    fn large_equal_board_keeps_every_name() {
        let amounts = vec![1_000.0; 6];
        let visible = select_visible_stocks(&amounts, 20_000.0);
        assert_eq!(visible.keep, 6);
        assert!(visible.others.is_none());
    }

    #[test]
    fn huge_board_caps_named_tiles_and_merges_the_tail() {
        let mut amounts: Vec<f64> = (0..200)
            .map(|index| 10_000.0 / f64::from(index + 1))
            .collect();
        amounts.sort_by(|left, right| right.total_cmp(left));
        let visible = select_visible_stocks(&amounts, 80_000.0);
        assert!(visible.keep <= MAX_STOCK_TILES);
        assert!(visible.keep >= 8, "leaders should remain visible");
        let others = visible.others.expect("tail should merge into 其他");
        assert_eq!(others.count, 200 - visible.keep);
        assert!(others.amount > 0.0);
    }

    #[test]
    fn tiny_tail_is_dropped_instead_of_drawing_a_speck() {
        let amounts = [100.0, 80.0, 70.0, 1.0];
        let visible = select_visible_stocks(&amounts, 8_000.0);
        assert_eq!(visible.keep, 3);
        assert!(visible.others.is_none());
    }

    #[test]
    fn weighted_change_follows_turnover() {
        let change = weighted_change_pct([(90.0, 2.0), (10.0, -8.0)]);
        assert!((change - 1.0).abs() < 1e-9);
        assert_eq!(weighted_change_pct([(0.0, 3.0), (0.0, 1.0)]), 2.0);
        assert_eq!(weighted_change_pct([]), 0.0);
    }

    #[test]
    fn compact_tiles_prefer_a_name_over_empty_color() {
        let labeled = tile_label_plan(40.0, 18.0, false);
        assert!(labeled.show_name);
        assert!(!labeled.show_change);

        let inline = tile_label_plan(72.0, 16.0, false);
        assert!(inline.show_name && inline.show_change && inline.horizontal);

        let stacked = tile_label_plan(90.0, 48.0, false);
        assert!(stacked.show_name && stacked.show_change && stacked.show_amount);
        assert!(!stacked.horizontal);

        let industry = tile_label_plan(32.0, 18.0, true);
        assert!(industry.show_name);
    }
}
