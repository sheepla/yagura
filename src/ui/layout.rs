//! Where each pane goes.
//!
//! The computation is a pure function of the terminal size and whether the
//! tree is visible, so the reducer can keep scroll offsets in step with the
//! layout without the renderer touching the state (invariant I7).

use ratatui::layout::{Constraint, Layout, Margin, Rect};

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
/// Share of the width a finder overlay gives to its list (§13.2).
const FINDER_LIST_NUMERATOR: u16 = 3;
const FINDER_LIST_DENOMINATOR: u16 = 5;
/// How much of what is left of the status bar a finder overlay covers.
const FINDER_WIDTH_PERCENT: u16 = 90;
const FINDER_HEIGHT_PERCENT: u16 = 85;

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

/// The rectangles a finder overlay is drawn into (§13.2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FinderAreas {
    /// The whole overlay, border and title included.
    pub popup: Rect,
    /// The first row inside the border, which holds the query box.
    pub query: Rect,
    /// The list pane.
    pub list: Rect,
    /// The line above the preview body inside the overlay.
    pub header: Rect,
    /// The preview pane.
    pub preview: Rect,
}

/// A finder overlay, centered on what the status bar leaves above it so that
/// the mode of the moment stays readable while the overlay is up (§10).
///
/// The status bar is a row the overlay never covers: it says `FILES`, and it
/// is where a message goes while the overlay is open.
pub fn finder(area: Rect) -> FinderAreas {
    let body = Rect::new(
        area.x,
        area.y,
        area.width,
        area.height.saturating_sub(STATUS_BAR_HEIGHT),
    );
    let popup = centered(body, FINDER_WIDTH_PERCENT, FINDER_HEIGHT_PERCENT);
    let inner = popup.inner(Margin::new(1, 1));
    if inner.width == 0 || inner.height == 0 {
        return FinderAreas {
            popup,
            query: Rect::ZERO,
            list: Rect::ZERO,
            header: Rect::ZERO,
            preview: Rect::ZERO,
        };
    }
    let query = Rect::new(inner.x, inner.y, inner.width, 1);
    let rest = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
    let list_width = (rest.width * FINDER_LIST_NUMERATOR / FINDER_LIST_DENOMINATOR)
        .max(1)
        .min(rest.width.saturating_sub(1));
    let list = Rect::new(rest.x, rest.y, list_width, rest.height);
    let right = Rect::new(
        rest.x + list_width,
        rest.y,
        rest.width - list_width,
        rest.height,
    );
    let header_height = PREVIEW_HEADER_HEIGHT.min(right.height);
    FinderAreas {
        popup,
        query,
        list,
        header: Rect::new(right.x, right.y, right.width, header_height),
        preview: Rect::new(
            right.x,
            right.y + header_height,
            right.width,
            right.height.saturating_sub(header_height),
        ),
    }
}

/// Rows a finder list has for entries, which is what a page of it is.
pub fn list_rows(area: Rect) -> u16 {
    finder(area).list.inner(Margin::new(1, 1)).height
}

/// A rectangle of a percentage of `area`, centered in it.
fn centered(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let rows = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Percentage(percent_y),
        Constraint::Fill(1),
    ])
    .split(area);
    let columns = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Percentage(percent_x),
        Constraint::Fill(1),
    ])
    .split(rows[1]);
    columns[1]
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

    #[test]
    fn a_finder_overlay_covers_everything_but_the_status_bar() {
        let area = Rect::new(0, 0, 80, 24);
        let finder = finder(area);
        assert!(
            finder.popup.y + finder.popup.height < area.height,
            "the status bar is free"
        );
        assert!(finder.popup.x + finder.popup.width <= area.width);
        // The query box is the first row inside the border.
        assert_eq!(finder.query.y, finder.popup.y + 1);
        assert_eq!(finder.query.height, 1);
        // The list and the preview tile the row below it without a gap.
        assert_eq!(
            finder.list.x + finder.list.width,
            finder.preview.x,
            "the panes meet"
        );
        // The list pane and the preview's title line share the top row, and
        // the body follows the title, as it does behind the overlay.
        assert_eq!(finder.list.y, finder.header.y);
        assert_eq!(finder.preview.y, finder.header.y + finder.header.height);
    }

    #[test]
    fn the_list_of_a_finder_is_the_wider_of_the_two_panes() {
        let finder = finder(Rect::new(0, 0, 80, 24));
        assert!(finder.list.width > finder.preview.width);
    }

    #[test]
    fn a_page_of_the_list_is_what_the_border_leaves() {
        let area = Rect::new(0, 0, 80, 24);
        let finder = finder(area);
        assert_eq!(list_rows(area), finder.list.height.saturating_sub(2));
        assert!(list_rows(area) > 0);
    }

    #[test]
    fn a_terminal_no_bigger_than_the_status_bar_gives_a_finder_nothing() {
        let finder = finder(Rect::new(0, 0, 80, 1));
        assert_eq!(finder.list, Rect::ZERO);
        assert_eq!(finder.query, Rect::ZERO);
    }
}
