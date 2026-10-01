//! Where each pane goes.
//!
//! The computation is a pure function of the terminal size and whether the
//! tree is visible, so the reducer can keep scroll offsets in step with the
//! layout without the renderer touching the state (invariant I7).

use ratatui::layout::Rect;

/// Smallest terminal that can show the whole layout.
pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 15;

const STATUS_BAR_HEIGHT: u16 = 1;
const PREVIEW_HEADER_HEIGHT: u16 = 1;
const MIN_TREE_WIDTH: u16 = 24;
const MAX_TREE_WIDTH: u16 = 60;
const MIN_PREVIEW_WIDTH: u16 = 20;
/// Share of the width the tree takes when it fits between the two limits.
const TREE_WIDTH_NUMERATOR: u16 = 2;
const TREE_WIDTH_DENOMINATOR: u16 = 5;

/// The rectangles one frame is drawn into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Areas {
    pub tree: Rect,
    pub header: Rect,
    pub preview: Rect,
    pub status: Rect,
}

/// True when the terminal is smaller than the supported minimum.
pub fn too_small(area: Rect) -> bool {
    area.width < MIN_WIDTH || area.height < MIN_HEIGHT
}

/// Splits the terminal into the tree, the preview header, the preview body and
/// the status bar.
pub fn compute(area: Rect, tree_visible: bool) -> Areas {
    let status = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(STATUS_BAR_HEIGHT),
        area.width,
        STATUS_BAR_HEIGHT,
    );
    let body_height = area.height.saturating_sub(STATUS_BAR_HEIGHT);
    if !tree_visible || body_height == 0 {
        return Areas {
            tree: Rect::ZERO,
            header: Rect::new(
                area.x,
                area.y,
                area.width,
                PREVIEW_HEADER_HEIGHT.min(body_height),
            ),
            preview: Rect::new(
                area.x,
                area.y + PREVIEW_HEADER_HEIGHT.min(body_height),
                area.width,
                body_height.saturating_sub(PREVIEW_HEADER_HEIGHT),
            ),
            status,
        };
    }

    let tree_width = tree_width(area.width);
    let tree = Rect::new(area.x, area.y, tree_width, body_height);
    let preview_x = area.x + tree_width;
    let preview_width = area.width.saturating_sub(tree_width);
    let header_height = PREVIEW_HEADER_HEIGHT.min(body_height);
    Areas {
        tree,
        header: Rect::new(preview_x, area.y, preview_width, header_height),
        preview: Rect::new(
            preview_x,
            area.y + header_height,
            preview_width,
            body_height.saturating_sub(header_height),
        ),
        status,
    }
}

/// Width of the tree pane, kept between the two minimums when possible.
pub fn tree_width(total: u16) -> u16 {
    let wanted = total.saturating_mul(TREE_WIDTH_NUMERATOR) / TREE_WIDTH_DENOMINATOR;
    let upper = MAX_TREE_WIDTH.min(total.saturating_sub(MIN_PREVIEW_WIDTH));
    wanted.clamp(
        MIN_TREE_WIDTH.min(total),
        upper.max(MIN_TREE_WIDTH.min(total)),
    )
}

/// Rows available to the tree, which is what scroll offsets are clamped to.
pub fn tree_height(area: Rect, tree_visible: bool) -> u16 {
    if !tree_visible {
        return 0;
    }
    let outer = compute(area, tree_visible).tree;
    // Two rows go to the border.
    outer.height.saturating_sub(2)
}

/// Rows available to the preview body.
pub fn preview_height(area: Rect) -> u16 {
    compute(area, true).preview.height
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_terminals_below_the_minimum() {
        assert!(!too_small(Rect::new(0, 0, 80, 24)));
        assert!(too_small(Rect::new(0, 0, 59, 24)));
        assert!(too_small(Rect::new(0, 0, 80, 14)));
        assert!(!too_small(Rect::new(0, 0, MIN_WIDTH, MIN_HEIGHT)));
    }

    #[test]
    fn panes_tile_the_terminal_without_gaps() {
        let area = Rect::new(0, 0, 80, 24);
        let areas = compute(area, true);
        assert_eq!(areas.status, Rect::new(0, 23, 80, 1));
        assert_eq!(areas.tree.x, 0);
        assert_eq!(areas.header.x + areas.header.width, 80);
        assert_eq!(areas.preview.x, areas.tree.width);
        assert_eq!(areas.preview.height + 1, areas.tree.height);
        assert_eq!(areas.preview.y + areas.preview.height, 23);
    }

    #[test]
    fn hiding_the_tree_gives_the_whole_width_to_the_preview() {
        let areas = compute(Rect::new(0, 0, 80, 24), false);
        assert_eq!(areas.tree, Rect::ZERO);
        assert_eq!(areas.header.width, 80);
        assert_eq!(areas.preview.width, 80);
        assert_eq!(areas.preview.height, 22);
    }

    #[test]
    fn the_tree_keeps_a_usable_width_at_the_minimum_size() {
        let narrow = compute(Rect::new(0, 0, MIN_WIDTH, MIN_HEIGHT), true);
        assert!(narrow.tree.width >= MIN_TREE_WIDTH);
        assert!(narrow.preview.width >= MIN_PREVIEW_WIDTH);
    }

    #[test]
    fn the_tree_does_not_grow_without_bound() {
        let wide = compute(Rect::new(0, 0, 400, 50), true);
        assert_eq!(wide.tree.width, MAX_TREE_WIDTH);
    }

    #[test]
    fn heights_shrink_with_the_border() {
        let area = Rect::new(0, 0, 80, 24);
        assert_eq!(tree_height(area, true), 21);
        assert_eq!(tree_height(area, false), 0);
        assert_eq!(preview_height(area), 22);
    }
}
