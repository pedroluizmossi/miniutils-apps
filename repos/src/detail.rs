//! A repository's detail, shown when its row is clicked: the changed files (already in
//! the status), the latest commits, the stash and the branches that can be deleted.
//! Only read for the open repository.

use std::path::Path;

use serde_json::{Value, json};

use crate::git::{self, Commit, FileChange};
use crate::i18n::{fmt, tr};
use crate::state::ago;

/// Files shown in the detail (the rest becomes "+N files").
const FILES: usize = 8;
const COMMITS: usize = 3;
const STASHES: usize = 3;

/// What the detail needs to read from git (the rest comes from the status).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detail {
    pub commits: Vec<Commit>,
    pub stashes: Vec<String>,
    /// Local branches already merged into the main one.
    pub merged: Vec<String>,
    /// The main branch they were merged into ("origin/main").
    pub base: Option<String>,
}

/// Reads the detail (runs git: call it off the main thread).
pub fn read(repo: &Path, branch: &str, base: Option<&str>) -> Detail {
    Detail {
        commits: git::recent_commits(repo, COMMITS),
        stashes: git::stashes(repo).into_iter().take(STASHES).collect(),
        merged: base.map(|b| git::merged_branches(repo, b, branch)).unwrap_or_default(),
        base: base.map(str::to_string),
    }
}

/// The view's `detail-*` properties.
pub fn props(d: &Detail, files: &[FileChange], now: u64) -> Vec<(&'static str, Value)> {
    let shown: Vec<String> = files.iter().take(FILES).map(|f| format!("{}  {}", f.code, f.path)).collect();
    let more = match files.len().saturating_sub(FILES) {
        0 => String::new(),
        1 => tr("+1 file").into(),
        n => fmt("+{} files", &[&n]),
    };
    let commits: Vec<String> = d
        .commits
        .iter()
        .map(|c| format!("{}  {} · {}", c.hash, c.subject, ago(now.saturating_sub(c.time))))
        .collect();
    let base = git::short_branch(d.base.as_deref().unwrap_or(""));
    let merged = match d.merged.len() {
        0 => String::new(),
        1 => fmt("1 branch already merged into {}: {}", &[&base, &d.merged[0]]),
        n => fmt("{} branches already merged into {}: {}", &[&n, &base, &d.merged.join(", ")]),
    };
    vec![
        ("detail-files", json!(shown)),
        ("detail-more", json!(more)),
        ("detail-commits", json!(commits)),
        ("detail-stash", json!(d.stashes)),
        ("detail-merged", json!(merged)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_properties() {
        let files: Vec<FileChange> = (0..10).map(|i| FileChange { code: 'M', path: format!("f{i}.rs") }).collect();
        let d = Detail {
            commits: vec![Commit { hash: "abc1234".into(), subject: "feat: x".into(), time: 1000 }],
            stashes: vec!["stash@{0}: WIP on main".into()],
            merged: vec!["done".into(), "old".into()],
            base: Some("origin/main".into()),
        };
        let p: std::collections::HashMap<_, _> = props(&d, &files, 1000 + 7200).into_iter().collect();
        assert_eq!(p["detail-files"].as_array().unwrap().len(), FILES);
        assert_eq!(p["detail-files"][0], "M  f0.rs");
        assert_eq!(p["detail-more"], "+2 files");
        assert_eq!(p["detail-commits"][0], "abc1234  feat: x · 2 h ago");
        assert_eq!(p["detail-merged"], "2 branches already merged into main: done, old");
        let empty: std::collections::HashMap<_, _> = props(&Detail::default(), &[], 0).into_iter().collect();
        assert_eq!((empty["detail-more"].as_str(), empty["detail-merged"].as_str()), (Some(""), Some("")));
    }
}
