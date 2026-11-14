//! End-of-day reminder: one notification a day, from the chosen hour on, if there are
//! unpushed commits. All pure: the local hour and day come from the caller.

use crate::i18n::{fmt, tr};

/// Time to remind? From `hour` on, once a day (`reminded` is the day of the last one).
pub fn due(today: &str, hour_now: u32, hour: u32, reminded: Option<&str>) -> bool {
    hour_now >= hour && reminded != Some(today)
}

/// The notification's text, or `None` if there is nothing to push.
/// `unpushed`: unpushed commits per repository; `dirty`: how many have uncommitted changes.
pub fn text(unpushed: &[u32], dirty: usize) -> Option<String> {
    let commits: u32 = unpushed.iter().sum();
    let repos = unpushed.iter().filter(|n| **n > 0).count();
    if commits == 0 {
        return None;
    }
    let mut s = match (commits, repos) {
        (1, _) => tr("You have 1 unpushed commit in 1 repository.").to_string(),
        (n, 1) => fmt("You have {} unpushed commits in 1 repository.", &[&n]),
        (n, r) => fmt("You have {} unpushed commits in {} repositories.", &[&n, &r]),
    };
    match dirty {
        0 => {}
        1 => {
            s.push(' ');
            s.push_str(tr("And 1 repository with uncommitted changes."));
        }
        n => {
            s.push(' ');
            s.push_str(&fmt("And {} repositories with uncommitted changes.", &[&n]));
        }
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn once_a_day_from_the_hour_on() {
        assert!(!due("2026-11-14", 17, 18, None));
        assert!(due("2026-11-14", 18, 18, None));
        assert!(due("2026-11-14", 23, 18, Some("2026-11-13")));
        assert!(!due("2026-11-14", 23, 18, Some("2026-11-14")));
    }

    #[test]
    fn text_only_with_unpushed_commits() {
        assert_eq!(text(&[0, 0], 3), None);
        assert_eq!(text(&[1], 0).unwrap(), "You have 1 unpushed commit in 1 repository.");
        assert_eq!(text(&[0, 2], 1).unwrap(), "You have 2 unpushed commits in 1 repository. And 1 repository with uncommitted changes.");
        assert_eq!(
            text(&[3, 0, 1], 2).unwrap(),
            "You have 4 unpushed commits in 2 repositories. And 2 repositories with uncommitted changes."
        );
    }
}
