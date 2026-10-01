//! Line diff computation and value types.

use std::time::{Duration, Instant};

use similar::{DiffOp, TextDiff};

/// Lines on one side above which the diff is refused as too large.
pub const MAX_DIFF_LINES: usize = 50_000;

/// Budget for a single diff; past it the result is approximated.
pub const DIFF_TIMEOUT: Duration = Duration::from_millis(200);

/// Unchanged lines kept on each side of a fold.
const FOLD_KEEP: usize = 3;

/// Line counts of a diff, as shown next to a tree row.
///
/// Within a hunk a run of deleted lines is paired with a run of added lines:
/// `min(deleted, added)` pairs become [`Self::replaced`] and whatever is left
/// over becomes [`Self::added`] or [`Self::removed`].
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct DiffStat {
    /// Lines that only exist on the current side.
    pub added: usize,
    /// Lines that only exist on the baseline side.
    pub removed: usize,
    /// Deleted/added pairs that were matched up.
    pub replaced: usize,
}

impl DiffStat {
    /// True when no line differs.
    pub fn is_empty(&self) -> bool {
        self.added == 0 && self.removed == 0 && self.replaced == 0
    }

    /// Adds the counts of `other` into `self`.
    pub fn merge(&mut self, other: &Self) {
        self.added += other.added;
        self.removed += other.removed;
        self.replaced += other.replaced;
    }

    /// The `+N -N ~N` form, omitting the parts that are zero.
    pub fn format(&self) -> String {
        let mut parts = Vec::with_capacity(3);
        if self.added > 0 {
            parts.push(format!("+{}", self.added));
        }
        if self.removed > 0 {
            parts.push(format!("-{}", self.removed));
        }
        if self.replaced > 0 {
            parts.push(format!("~{}", self.replaced));
        }
        parts.join(" ")
    }
}

/// Why a diff could not be computed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiffError {
    /// Both sides exceeded [`MAX_DIFF_LINES`].
    TooLarge,
}

/// What a row of the inline diff shows.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Row {
    /// A line both sides agree on.
    Context(String),
    /// A line only on the current side.
    Added(String),
    /// A line only on the baseline side.
    Removed(String),
    /// `usize` unchanged lines collapsed between hunks.
    Fold(usize),
}

/// What a row of the side-by-side diff shows.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SideRow {
    /// An unchanged line, shown on both sides.
    Context(String),
    /// A deleted/added pair shown opposite each other.
    Replace {
        /// The baseline side of the pair.
        baseline: String,
        /// The current side of the pair.
        current: String,
    },
    /// A baseline line with no counterpart.
    Removed(String),
    /// A current line with no counterpart.
    Added(String),
    /// `usize` unchanged lines collapsed on both sides.
    Fold(usize),
}

/// A computed diff, ready to render.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Diff {
    /// Rows to draw, already folded around the hunks.
    pub rows: Vec<Row>,
    /// Index into [`Self::rows`] of the first changed row of every hunk.
    pub hunks: Vec<usize>,
    /// Line counts of the whole file.
    pub stat: DiffStat,
    /// True when the run went past [`DIFF_TIMEOUT`] and is only approximate.
    pub approximated: bool,
}

impl Diff {
    /// Diffs `baseline` against `current` line by line.
    ///
    /// CRLF becomes LF and a missing trailing newline never counts as a
    /// change. Long unchanged stretches between hunks come back as
    /// [`Row::Fold`].
    pub fn compute(baseline: &str, current: &str) -> Result<Diff, DiffError> {
        let old: Vec<&str> = baseline.lines().collect();
        let new: Vec<&str> = current.lines().collect();
        if old.len() > MAX_DIFF_LINES && new.len() > MAX_DIFF_LINES {
            return Err(DiffError::TooLarge);
        }

        let started = Instant::now();
        let diff = TextDiff::configure()
            .timeout(DIFF_TIMEOUT)
            .diff_slices(&old, &new);
        // A run that used up its whole budget handed back an approximation,
        // and any slower run should warn the user all the same.
        let approximated = started.elapsed() >= DIFF_TIMEOUT;

        let stat = stat_of(diff.ops());
        let (rows, hunks) = rows_of(diff.ops(), &old, &new);
        Ok(Diff {
            rows,
            hunks,
            stat,
            approximated,
        })
    }

    /// Re-pairs [`Diff::rows`] into side-by-side rows: within every change
    /// block the deletion and insertion runs are matched from the top, and
    /// whatever one side has too much of stays on its own side.
    pub fn side_rows(&self) -> Vec<SideRow> {
        let mut out = Vec::with_capacity(self.rows.len());
        let mut i = 0;
        while i < self.rows.len() {
            match &self.rows[i] {
                Row::Context(line) => {
                    out.push(SideRow::Context(line.clone()));
                    i += 1;
                }
                Row::Fold(n) => {
                    out.push(SideRow::Fold(*n));
                    i += 1;
                }
                Row::Removed(_) | Row::Added(_) => {
                    let start = i;
                    while i < self.rows.len()
                        && matches!(self.rows[i], Row::Removed(_) | Row::Added(_))
                    {
                        i += 1;
                    }
                    out.extend(side_block(&self.rows[start..i]));
                }
            }
        }
        out
    }
}

/// Pairs one change block into side-by-side rows.
fn side_block(block: &[Row]) -> Vec<SideRow> {
    let mut removed = Vec::new();
    let mut added = Vec::new();
    for row in block {
        match row {
            Row::Removed(line) => removed.push(line.clone()),
            Row::Added(line) => added.push(line.clone()),
            _ => {}
        }
    }
    let paired = removed.len().min(added.len());
    let mut out = Vec::with_capacity(removed.len().max(added.len()));
    for (baseline, current) in removed.drain(..paired).zip(added.drain(..paired)) {
        out.push(SideRow::Replace { baseline, current });
    }
    out.extend(removed.into_iter().map(SideRow::Removed));
    out.extend(added.into_iter().map(SideRow::Added));
    out
}

/// Counts the whole file from the diff ops, pairing each deletion block with
/// the insertion block the diff matched it against.
fn stat_of(ops: &[DiffOp]) -> DiffStat {
    let mut stat = DiffStat::default();
    for op in ops {
        match *op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete { old_len, .. } => stat.removed += old_len,
            DiffOp::Insert { new_len, .. } => stat.added += new_len,
            DiffOp::Replace {
                old_len, new_len, ..
            } => {
                let paired = old_len.min(new_len);
                stat.replaced += paired;
                stat.removed += old_len - paired;
                stat.added += new_len - paired;
            }
        }
    }
    stat
}

/// Drains buffered unchanged lines into rows, collapsing a run that is longer
/// than twice [`FOLD_KEEP`] down to its edges plus a fold marker.
fn push_context(rows: &mut Vec<Row>, context: &mut Vec<&str>, fold: bool) {
    if fold && context.len() > FOLD_KEEP * 2 {
        for line in context.drain(..FOLD_KEEP) {
            rows.push(Row::Context(line.to_string()));
        }
        let folded = context.len() - FOLD_KEEP;
        rows.push(Row::Fold(folded));
        context.drain(..folded);
    }
    for line in context.drain(..) {
        rows.push(Row::Context(line.to_string()));
    }
}

/// Turns the diff ops into display rows, folding unchanged runs longer than
/// twice [`FOLD_KEEP`] down to a `⋯ N lines` marker.
fn rows_of(ops: &[DiffOp], old: &[&str], new: &[&str]) -> (Vec<Row>, Vec<usize>) {
    let mut rows = Vec::new();
    let mut hunks = Vec::new();
    let mut context: Vec<&str> = Vec::new();
    let mut in_hunk = false;

    for op in ops {
        match *op {
            DiffOp::Equal { old_index, len, .. } => {
                context.extend_from_slice(&old[old_index..old_index + len]);
                in_hunk = false;
            }
            DiffOp::Delete {
                old_index, old_len, ..
            } => {
                push_context(&mut rows, &mut context, true);
                if !in_hunk {
                    hunks.push(rows.len());
                    in_hunk = true;
                }
                rows.extend(
                    old[old_index..old_index + old_len]
                        .iter()
                        .map(|line| Row::Removed(line.to_string())),
                );
            }
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                push_context(&mut rows, &mut context, true);
                if !in_hunk {
                    hunks.push(rows.len());
                    in_hunk = true;
                }
                rows.extend(
                    new[new_index..new_index + new_len]
                        .iter()
                        .map(|line| Row::Added(line.to_string())),
                );
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
                ..
            } => {
                push_context(&mut rows, &mut context, true);
                if !in_hunk {
                    hunks.push(rows.len());
                    in_hunk = true;
                }
                rows.extend(
                    old[old_index..old_index + old_len]
                        .iter()
                        .map(|line| Row::Removed(line.to_string())),
                );
                rows.extend(
                    new[new_index..new_index + new_len]
                        .iter()
                        .map(|line| Row::Added(line.to_string())),
                );
            }
        }
    }
    push_context(&mut rows, &mut context, !hunks.is_empty());

    (rows, hunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders rows the way a unified diff would: ` `/`+`/`-` prefix, folds
    /// spelled out.
    fn render(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Context(line) => format!(" {line}"),
                Row::Added(line) => format!("+{line}"),
                Row::Removed(line) => format!("-{line}"),
                Row::Fold(n) => format!("⋯ {n} lines"),
            })
            .collect()
    }

    #[test]
    fn formats_only_non_zero_parts_in_spec_order() {
        let stat = DiffStat {
            added: 12,
            removed: 3,
            replaced: 4,
        };
        assert_eq!(stat.format(), "+12 -3 ~4");
        assert_eq!(DiffStat::default().format(), "");
    }

    #[test]
    fn reports_emptiness() {
        assert!(DiffStat::default().is_empty());
        assert!(
            !DiffStat {
                replaced: 1,
                ..Default::default()
            }
            .is_empty()
        );
    }

    #[test]
    fn merges_counts() {
        let mut a = DiffStat {
            added: 1,
            removed: 2,
            replaced: 3,
        };
        a.merge(&DiffStat {
            added: 1,
            removed: 0,
            replaced: 0,
        });
        assert_eq!(
            a,
            DiffStat {
                added: 2,
                removed: 2,
                replaced: 3
            }
        );
    }

    #[test]
    fn counts_paired_and_excess_lines() {
        let replaced = Diff::compute("keep\na\nkeep\n", "keep\nb\nkeep\n").unwrap();
        assert_eq!(
            replaced.stat,
            DiffStat {
                replaced: 1,
                ..Default::default()
            }
        );

        let over_removed = Diff::compute("a\nb\nc\nd\ne\n", "a\nx\ne\n").unwrap();
        assert_eq!(
            over_removed.stat,
            DiffStat {
                removed: 2,
                replaced: 1,
                ..Default::default()
            }
        );

        let over_added = Diff::compute("a\ne\n", "a\nx\ny\nz\ne\n").unwrap();
        assert_eq!(
            over_added.stat,
            DiffStat {
                added: 3,
                ..Default::default()
            }
        );
    }

    #[test]
    fn ignores_crlf_and_a_missing_trailing_newline() {
        let diff = Diff::compute("a\r\nb\r\n", "a\nb\n").unwrap();
        assert!(diff.stat.is_empty());
        assert!(diff.hunks.is_empty());

        let diff = Diff::compute("a\nb", "a\nb\n").unwrap();
        assert!(diff.stat.is_empty());
        assert!(diff.hunks.is_empty());
    }

    #[test]
    fn identical_files_are_all_context_with_no_hunks() {
        let diff = Diff::compute("one\ntwo\nthree\n", "one\ntwo\nthree\n").unwrap();
        assert_eq!(
            render(&diff.rows),
            vec![" one", " two", " three"],
            "an identical file must not be folded"
        );
        assert!(diff.hunks.is_empty());
        assert!(!diff.approximated);
    }

    #[test]
    fn builds_rows_and_a_hunk_around_a_change() {
        let diff = Diff::compute("one\ntwo\nthree\n", "one\nTWO\nthree\n").unwrap();
        assert_eq!(render(&diff.rows), vec![" one", "-two", "+TWO", " three"]);
        assert_eq!(diff.hunks, vec![1]);
    }

    #[test]
    fn folds_long_unchanged_runs_but_keeps_the_edges() {
        let baseline = (1..=13)
            .map(|n| format!("line{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let current = baseline.replace("line5", "changed");
        let diff = Diff::compute(&baseline, &current).unwrap();
        assert_eq!(
            render(&diff.rows),
            vec![
                " line1",
                " line2",
                " line3",
                " line4",
                "-line5",
                "+changed",
                " line6",
                " line7",
                " line8",
                "⋯ 2 lines",
                " line11",
                " line12",
                " line13",
            ]
        );
        assert_eq!(diff.hunks, vec![4]);
    }

    #[test]
    fn keeps_short_unchanged_runs_whole() {
        let diff = Diff::compute("a\nb\nc\nd\n", "a\nB\nc\nd\n").unwrap();
        assert_eq!(render(&diff.rows), vec![" a", "-b", "+B", " c", " d"]);
    }

    #[test]
    fn pairs_side_by_side_rows_from_the_top() {
        let diff = Diff::compute("x1\nx2\nkeep\n", "y1\ny2\ny3\nkeep\n").unwrap();
        assert_eq!(
            diff.stat,
            DiffStat {
                added: 1,
                replaced: 2,
                ..Default::default()
            }
        );
        let sides = diff.side_rows();
        assert_eq!(
            sides,
            vec![
                SideRow::Replace {
                    baseline: "x1".into(),
                    current: "y1".into(),
                },
                SideRow::Replace {
                    baseline: "x2".into(),
                    current: "y2".into(),
                },
                SideRow::Added("y3".into()),
                SideRow::Context("keep".into()),
            ]
        );
    }

    #[test]
    fn keeps_an_excess_on_its_own_side_by_side() {
        let diff = Diff::compute("x1\nx2\nx3\nkeep\n", "y1\nkeep\n").unwrap();
        assert_eq!(
            diff.stat,
            DiffStat {
                removed: 2,
                replaced: 1,
                ..Default::default()
            }
        );
        assert_eq!(
            diff.side_rows(),
            vec![
                SideRow::Replace {
                    baseline: "x1".into(),
                    current: "y1".into(),
                },
                SideRow::Removed("x2".into()),
                SideRow::Removed("x3".into()),
                SideRow::Context("keep".into()),
            ]
        );
    }

    #[test]
    fn refuses_when_both_sides_are_huge() {
        let huge = "line\n".repeat(MAX_DIFF_LINES + 1);
        assert_eq!(Diff::compute(&huge, &huge), Err(DiffError::TooLarge));

        let small = "line\n";
        assert!(Diff::compute(&huge, small).is_ok());
    }
}
