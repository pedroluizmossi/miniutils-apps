//! Watches each repository's `.git`: commit, add, checkout, pull and fetch touch the index,
//! HEAD or the refs, and the app rereads that repository right away.
//!
//! The working tree is not watched (a `node_modules` would use up thousands of inotify
//! watches); editing a file shows up on the periodic reread or when the window opens.

use std::path::{Path, PathBuf};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

pub struct Watch {
    watcher: RecommendedWatcher,
    /// What is being watched now (to remove it when the list changes).
    watched: Vec<PathBuf>,
}

impl Watch {
    /// `on` gets the path of each change (on the watcher's thread).
    pub fn new(on: impl Fn(PathBuf) + Send + 'static) -> Result<Watch, String> {
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            if !matches!(ev.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)) {
                return;
            }
            for p in ev.paths {
                // lock files come and go with every command: the real file reports the change
                if p.extension().is_none_or(|e| e != "lock") {
                    on(p);
                }
            }
        })
        .map_err(|e| e.to_string())?;
        Ok(Watch { watcher, watched: Vec::new() })
    }

    /// Starts watching these git folders (and only them).
    pub fn set(&mut self, git_dirs: &[PathBuf]) {
        for p in std::mem::take(&mut self.watched) {
            let _ = self.watcher.unwatch(&p);
        }
        for gd in git_dirs {
            // the folder itself (index, HEAD, packed-refs) and the refs (branches and remotes)
            let targets = [(gd.clone(), RecursiveMode::NonRecursive), (gd.join("refs"), RecursiveMode::Recursive)];
            for (path, mode) in targets {
                if path.is_dir() {
                    match self.watcher.watch(&path, mode) {
                        Ok(()) => self.watched.push(path),
                        Err(e) => eprintln!("could not watch {}: {e}", path.display()),
                    }
                }
            }
        }
    }
}

/// The repository (by its git folder) that a change in `path` belongs to.
pub fn owner<'a>(path: &Path, repos: impl IntoIterator<Item = (&'a PathBuf, &'a PathBuf)>) -> Option<PathBuf> {
    repos.into_iter().find(|(_, git_dir)| path.starts_with(git_dir)).map(|(repo, _)| repo.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    #[test]
    fn change_in_git_reports_the_right_repository() {
        let root = std::env::temp_dir().join(format!("repos-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (a, b) = (root.join("a"), root.join("b"));
        for r in [&a, &b] {
            std::fs::create_dir_all(r.join(".git/refs/heads")).unwrap();
        }
        let (tx, rx) = channel();
        let mut w = Watch::new(move |p| {
            let _ = tx.send(p);
        })
        .unwrap();
        w.set(&[a.join(".git"), b.join(".git")]);
        std::fs::write(b.join(".git/refs/heads/main"), "x").unwrap();
        // the write is reported and points to repository b. On Windows extra events may come
        // (from the other repository, or repeated): they only make the app reread, so the
        // test ignores them
        let pairs = [(a.clone(), a.join(".git")), (b.clone(), b.join(".git"))];
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut seen = Vec::new();
        let found = loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let Ok(p) = rx.recv_timeout(left) else { break false };
            let is_b = owner(&p, pairs.iter().map(|(r, g)| (r, g))) == Some(b.clone()) && p.ends_with("main");
            seen.push(p);
            if is_b {
                break true;
            }
        };
        assert!(found, "no event for the write in b; got: {seen:?}");
        // a lock file does not report
        std::fs::write(a.join(".git/index.lock"), "x").unwrap();
        let later: Vec<PathBuf> = std::iter::from_fn(|| rx.recv_timeout(Duration::from_millis(500)).ok()).take(50).collect();
        assert!(later.iter().all(|p| p.extension().is_none_or(|e| e != "lock")), "{later:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
