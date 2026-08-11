//! Deterministic squarified-treemap layout for market heatmaps.
//!
//! The layout is UI-framework agnostic: callers provide weights and receive
//! rectangles normalized to a 0..1 coordinate space. Keeping the geometry
//! pure makes it cheap to test and lets GPUI own rendering and interaction.

const MIN_WEIGHT_RATIO: f64 = 0.0005;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TreemapRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl TreemapRect {
    #[cfg(test)]
    fn area(self) -> f64 {
        f64::from(self.width) * f64::from(self.height)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TreemapCell {
    /// Index into the caller's weight/item slice.
    pub index: usize,
    pub rect: TreemapRect,
}

#[derive(Debug, Clone, Copy)]
struct WorkRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Debug, Clone, Copy)]
struct AreaItem {
    index: usize,
    area: f64,
}

/// Lay out `weights` using a squarified treemap.
///
/// `target_aspect_ratio` should approximate the rendered width / height. The
/// returned rectangles are normalized, so callers can use relative GPUI
/// positioning and remain responsive when the window is resized.
pub(crate) fn squarified_treemap(weights: &[f64], target_aspect_ratio: f64) -> Vec<TreemapCell> {
    if weights.is_empty() {
        return Vec::new();
    }

    let layout_width = if target_aspect_ratio.is_finite() {
        target_aspect_ratio.clamp(1.0, 4.0)
    } else {
        1.0
    };
    let max_weight = weights
        .iter()
        .copied()
        .filter(|weight| weight.is_finite() && *weight > 0.0)
        .fold(0.0_f64, f64::max);
    let floor = max_weight * MIN_WEIGHT_RATIO;
    let normalized_weights: Vec<f64> = if max_weight > 0.0 {
        weights
            .iter()
            .map(|weight| {
                if weight.is_finite() && *weight > 0.0 {
                    weight.max(floor)
                } else {
                    floor
                }
            })
            .collect()
    } else {
        vec![1.0; weights.len()]
    };
    let total_weight: f64 = normalized_weights.iter().sum();
    let total_area = layout_width;
    let mut items: Vec<AreaItem> = normalized_weights
        .into_iter()
        .enumerate()
        .map(|(index, weight)| AreaItem {
            index,
            area: weight / total_weight * total_area,
        })
        .collect();

    let mut remaining = WorkRect {
        x: 0.0,
        y: 0.0,
        width: layout_width,
        height: 1.0,
    };
    let mut row = Vec::new();
    let mut cells = Vec::with_capacity(items.len());

    for item in items.drain(..) {
        let short_side = remaining.width.min(remaining.height).max(f64::EPSILON);
        if row.is_empty()
            || worst_ratio_with(&row, item, short_side) <= worst_ratio(&row, short_side)
        {
            row.push(item);
        } else {
            layout_row(&row, &mut remaining, &mut cells, layout_width);
            row.clear();
            row.push(item);
        }
    }
    if !row.is_empty() {
        layout_row(&row, &mut remaining, &mut cells, layout_width);
    }

    cells.sort_by_key(|cell| cell.index);
    cells
}

/// Return how many leading, descending weights should remain visible before
/// the long tail is collapsed into one aggregate cell.
pub(crate) fn primary_item_count(
    descending_weights: &[f64],
    minimum_items: usize,
    maximum_items: usize,
    minimum_share: f64,
) -> usize {
    if descending_weights.is_empty() || maximum_items == 0 {
        return 0;
    }
    let cap = maximum_items.min(descending_weights.len());
    let floor = minimum_items.min(cap);
    let total: f64 = descending_weights
        .iter()
        .copied()
        .filter(|weight| weight.is_finite() && *weight > 0.0)
        .sum();
    if total <= 0.0 {
        return cap;
    }
    let minimum_share = minimum_share.clamp(0.0, 1.0);
    let mut keep = floor;
    for (index, weight) in descending_weights
        .iter()
        .copied()
        .enumerate()
        .take(cap)
        .skip(floor)
    {
        if weight.is_finite() && weight > 0.0 && weight / total >= minimum_share {
            keep = index + 1;
        } else {
            break;
        }
    }
    keep
}

fn worst_ratio(row: &[AreaItem], side: f64) -> f64 {
    if row.is_empty() {
        return f64::INFINITY;
    }
    let sum: f64 = row.iter().map(|item| item.area).sum();
    let min = row
        .iter()
        .map(|item| item.area)
        .fold(f64::INFINITY, f64::min);
    let max = row.iter().map(|item| item.area).fold(0.0_f64, f64::max);
    let side_squared = side * side;
    ((side_squared * max) / (sum * sum)).max((sum * sum) / (side_squared * min))
}

fn worst_ratio_with(row: &[AreaItem], item: AreaItem, side: f64) -> f64 {
    let mut candidate = Vec::with_capacity(row.len() + 1);
    candidate.extend_from_slice(row);
    candidate.push(item);
    worst_ratio(&candidate, side)
}

fn layout_row(
    row: &[AreaItem],
    remaining: &mut WorkRect,
    cells: &mut Vec<TreemapCell>,
    layout_width: f64,
) {
    let row_area: f64 = row.iter().map(|item| item.area).sum();
    if remaining.width >= remaining.height {
        let column_width = (row_area / remaining.height).min(remaining.width);
        let mut y = remaining.y;
        for (position, item) in row.iter().enumerate() {
            let height = if position + 1 == row.len() {
                remaining.y + remaining.height - y
            } else {
                item.area / column_width
            };
            push_cell(
                cells,
                item.index,
                remaining.x,
                y,
                column_width,
                height,
                layout_width,
            );
            y += height;
        }
        remaining.x += column_width;
        remaining.width = (remaining.width - column_width).max(0.0);
    } else {
        let row_height = (row_area / remaining.width).min(remaining.height);
        let mut x = remaining.x;
        for (position, item) in row.iter().enumerate() {
            let width = if position + 1 == row.len() {
                remaining.x + remaining.width - x
            } else {
                item.area / row_height
            };
            push_cell(
                cells,
                item.index,
                x,
                remaining.y,
                width,
                row_height,
                layout_width,
            );
            x += width;
        }
        remaining.y += row_height;
        remaining.height = (remaining.height - row_height).max(0.0);
    }
}

fn push_cell(
    cells: &mut Vec<TreemapCell>,
    index: usize,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    layout_width: f64,
) {
    cells.push(TreemapCell {
        index,
        rect: TreemapRect {
            x: (x / layout_width) as f32,
            y: y as f32,
            width: (width / layout_width) as f32,
            height: height as f32,
        },
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_fills_bounds_without_overlap() {
        let cells = squarified_treemap(&[50.0, 30.0, 12.0, 8.0], 2.7);
        let total_area: f64 = cells.iter().map(|cell| cell.rect.area()).sum();
        assert!((total_area - 1.0).abs() < 1e-5, "area={total_area}");

        for cell in &cells {
            let rect = cell.rect;
            assert!(rect.x >= 0.0 && rect.y >= 0.0);
            assert!(rect.width > 0.0 && rect.height > 0.0);
            assert!(rect.x + rect.width <= 1.000_01);
            assert!(rect.y + rect.height <= 1.000_01);
        }
        for left in 0..cells.len() {
            for right in left + 1..cells.len() {
                assert!(!overlaps(cells[left].rect, cells[right].rect));
            }
        }
    }

    #[test]
    fn cell_areas_follow_positive_weights() {
        let weights = [60.0, 25.0, 10.0, 5.0];
        let cells = squarified_treemap(&weights, 2.7);
        for (cell, weight) in cells.iter().zip(weights) {
            assert!((cell.rect.area() - weight / 100.0).abs() < 1e-5);
        }
    }

    #[test]
    fn invalid_or_zero_weights_still_get_cells() {
        let cells = squarified_treemap(&[0.0, f64::NAN, -4.0], 2.7);
        assert_eq!(cells.len(), 3);
        assert!(cells.iter().all(|cell| cell.rect.area() > 0.0));
        let total_area: f64 = cells.iter().map(|cell| cell.rect.area()).sum();
        assert!((total_area - 1.0).abs() < 1e-5);
    }

    #[test]
    fn layout_is_deterministic() {
        let weights = [41.0, 27.0, 19.0, 8.0, 5.0];
        assert_eq!(
            squarified_treemap(&weights, 2.7),
            squarified_treemap(&weights, 2.7)
        );
    }

    #[test]
    fn long_tail_partition_keeps_minimum_and_respects_share() {
        let weights = [50.0, 25.0, 10.0, 8.0, 4.0, 2.0, 0.6, 0.4];
        assert_eq!(primary_item_count(&weights, 3, 6, 0.03), 5);
        assert_eq!(primary_item_count(&weights, 3, 4, 0.0), 4);
    }

    #[test]
    fn zero_weight_partition_uses_the_display_cap() {
        assert_eq!(primary_item_count(&[0.0; 50], 20, 40, 0.004), 40);
    }

    fn overlaps(a: TreemapRect, b: TreemapRect) -> bool {
        let epsilon = 1e-6;
        a.x + a.width > b.x + epsilon
            && b.x + b.width > a.x + epsilon
            && a.y + a.height > b.y + epsilon
            && b.y + b.height > a.y + epsilon
    }
}
