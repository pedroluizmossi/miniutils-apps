//! Finds the repositories inside the watched folders.

use std::path::{Path, PathBuf};

/// Folders that never hold a user's repository and can be huge.
const SKIP: &[&str] = &["node_modules", "target", "vendor", "dist", "build", "__pycache__", "venv"];

/// Is it a repository (has a `.git`, folder or worktree/submodule file)?
pub fn is_repo(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir.join(".git")).is_ok()
}

/// The repositories in `roots`, going down up to `depth` levels. Does not go into a
/// repository (submodules and inner folders belong to the outer one), nor into hidden
/// folders, links or dependency folders. Sorted, without duplicates.
pub fn find_repos(roots: &[PathBuf], depth: u32) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in roots {
        walk(root, 0, depth, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

fn walk(dir: &Path, level: u32, depth: u32, out: &mut Vec<PathBuf>) {
    if is_repo(dir) {
        out.push(dir.to_path_buf());
        return;
    }
    if level >= depth {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        if is_dir && !name.starts_with('.') && !SKIP.contains(&name.as_str()) {
            walk(&entry.path(), level + 1, depth, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_repositories_down_to_the_depth() {
        let root = std::env::temp_dir().join(format!("repos-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["a/.git", "a/sub/.git", "group/b/.git", "group/deep/c/.git", ".hidden/d/.git", "node_modules/e/.git", "loose"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::create_dir_all(root.join("wt")).unwrap();
        std::fs::write(root.join("wt/.git"), "gitdir: /x").unwrap();
        let rel = |v: Vec<PathBuf>| -> Vec<String> {
            v.iter().map(|p| p.strip_prefix(&root).unwrap().display().to_string().replace('\\', "/")).collect()
        };
        assert_eq!(rel(find_repos(std::slice::from_ref(&root), 2)), ["a", "group/b", "wt"]);
        assert_eq!(rel(find_repos(std::slice::from_ref(&root), 3)), ["a", "group/b", "group/deep/c", "wt"]);
        // the watched folder itself can be a repository
        assert_eq!(find_repos(&[root.join("a")], 2), [root.join("a")]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
