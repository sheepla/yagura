//! How long ago something changed, in the words a tree row has room for.
//!
//! The label, the color and the frame that repaints all read this one
//! function, so a row never says one thing and the paint decision another.

use std::time::{Duration, SystemTime};

use crate::ui::theme::Role;

/// The three ways the age of a change is drawn (spec, tree rows).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Freshness {
    /// Seconds old: drawn to be noticed.
    Fresh,
    /// Minutes old: drawn plainly.
    Recent,
    /// Older: drawn out of the way.
    Stale,
}

/// A change is highlighted for this long.
const FRESH_FOR: Duration = Duration::from_secs(10);
/// A change is drawn plainly for this long.
const RECENT_FOR: Duration = Duration::from_secs(600);

/// The time between a change and `now`, which a clock that jumped backwards
/// reports as nothing at all rather than as a negative age.
pub fn age(from: SystemTime, now: SystemTime) -> Duration {
    now.duration_since(from).unwrap_or_default()
}

/// `12s`, `3m`, `2h`, `5d`: the unit is the largest one the age fits in.
pub fn label(from: SystemTime, now: SystemTime) -> String {
    let seconds = age(from, now).as_secs();
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3_599 => format!("{}m", seconds / 60),
        3_600..=86_399 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// Which of the three ways the age is drawn (spec, tree rows).
///
/// Both steps of this fall where the label steps as well, so a color only
/// changes on a frame the label already asked for.
pub fn freshness(from: SystemTime, now: SystemTime) -> Freshness {
    let age = age(from, now);
    if age < FRESH_FOR {
        Freshness::Fresh
    } else if age < RECENT_FOR {
        Freshness::Recent
    } else {
        Freshness::Stale
    }
}

/// The role a freshness is drawn in.
pub fn role(freshness: Freshness) -> Role {
    match freshness {
        Freshness::Fresh => Role::TreeFresh,
        Freshness::Recent => Role::TreeText,
        Freshness::Stale => Role::StatusMuted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn an_age_is_given_the_largest_unit_it_fits_in() {
        assert_eq!(label(at(0), at(0)), "0s");
        assert_eq!(label(at(0), at(12)), "12s");
        assert_eq!(label(at(0), at(59)), "59s");
        assert_eq!(label(at(0), at(60)), "1m");
        assert_eq!(label(at(0), at(3 * 60 + 40)), "3m");
        assert_eq!(label(at(0), at(59 * 60)), "59m");
        assert_eq!(label(at(0), at(60 * 60)), "1h");
        assert_eq!(label(at(0), at(23 * 60 * 60)), "23h");
        assert_eq!(label(at(0), at(24 * 60 * 60)), "1d");
        assert_eq!(label(at(0), at(5 * 24 * 60 * 60)), "5d");
    }

    #[test]
    fn a_change_in_the_future_is_no_older_than_now() {
        assert_eq!(label(at(100), at(0)), "0s");
        assert_eq!(freshness(at(100), at(0)), Freshness::Fresh);
    }

    #[test]
    fn the_color_only_changes_where_the_label_changes() {
        // A frame the color alone would need never comes: the two steps of
        // the color, 10s and 600s, are steps of the label as well.
        assert_eq!(freshness(at(0), at(9)), Freshness::Fresh);
        assert_eq!(freshness(at(0), at(10)), Freshness::Recent);
        assert_eq!(freshness(at(0), at(599)), Freshness::Recent);
        assert_eq!(freshness(at(0), at(600)), Freshness::Stale);
        for second in 1..=100_000u64 {
            let color_changes = freshness(at(0), at(second - 1)) != freshness(at(0), at(second));
            if color_changes {
                assert_ne!(
                    label(at(0), at(second - 1)),
                    label(at(0), at(second)),
                    "the color changed alone at {second}s"
                );
            }
        }
    }

    #[test]
    fn each_freshness_has_its_own_role() {
        let roles = [
            role(Freshness::Fresh),
            role(Freshness::Recent),
            role(Freshness::Stale),
        ];
        assert_ne!(roles[0], roles[1]);
        assert_ne!(roles[1], roles[2]);
    }
}
