//! Line diff value types.

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
        if self.replaced > 0 {
            parts.push(format!("~{}", self.replaced));
        }
        if self.added > 0 {
            parts.push(format!("+{}", self.added));
        }
        if self.removed > 0 {
            parts.push(format!("-{}", self.removed));
        }
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_only_non_zero_parts() {
        let stat = DiffStat {
            added: 12,
            removed: 3,
            replaced: 4,
        };
        assert_eq!(stat.format(), "~4 +12 -3");
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
}
