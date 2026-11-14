//! What `git` says about a repository. Uses `git` itself (not a library): the status comes
//! out the same as in the user's terminal, with `.gitignore`, config and fsmonitor.
//!
//! Every call has a deadline, never asks for a password and runs with
//! `--no-optional-locks` (`status` does not rewrite the index: otherwise each read would
//! trigger the `.git` watcher again).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::i18n::{fmt, tr};

/// Deadline of a `status` (huge repository or slow disk).
const STATUS_TIMEOUT: Duration = Duration::from_secs(20);
/// Deadline of a `fetch` (network).
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
/// Deadline of a `push` or `pull` requested by the button (network, and the pre-push hook).
const SYNC_TIMEOUT: Duration = Duration::from_secs(120);
/// Deadline of the quick reads (log, stash, branches).
const QUICK_TIMEOUT: Duration = Duration::from_secs(10);
/// How many changed files to check to find the oldest change.
const MAX_AGE_PATHS: usize = 300;

/// A changed file: the short code of the change and the path relative to the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// `M` modified, `A` added to the index, `D` deleted, `R` renamed, `T` type changed,
    /// `U` conflicted, `?` untracked.
    pub code: char,
    pub path: String,
}

/// A commit for the detail: short hash, subject and time (seconds since 1970).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub subject: String,
    pub time: u64,
}

/// The summary of `git status --porcelain=v2 --branch --show-stash`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// Branch name; empty = detached HEAD.
    pub branch: String,
    /// Remote branch this one tracks (`origin/main`), if any.
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Changes already in the index (`git add`).
    pub staged: u32,
    /// Changes outside the index.
    pub unstaged: u32,
    pub untracked: u32,
    pub conflicts: u32,
    pub stash: u32,
    /// Changed files, for the age of the changes and the detail.
    pub files: Vec<FileChange>,
}

impl Status {
    pub fn dirty(&self) -> bool {
        self.staged + self.unstaged + self.untracked + self.conflicts > 0
    }
}

/// Reads the output of `git status --porcelain=v2 --branch --show-stash -z`.
pub fn parse_status(out: &[u8]) -> Status {
    let mut st = Status::default();
    let text = String::from_utf8_lossy(out);
    let mut records = text.split('\0').filter(|r| !r.is_empty());
    while let Some(rec) = records.next() {
        if let Some(header) = rec.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.head" if value != "(detached)" => st.branch = value.to_string(),
                "branch.upstream" => st.upstream = Some(value.to_string()),
                "branch.ab" => {
                    for part in value.split(' ') {
                        if let Some(n) = part.strip_prefix('+') {
                            st.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            st.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                "stash" => st.stash = value.parse().unwrap_or(0),
                _ => {}
            }
            continue;
        }
        // fields before the path: "1" has 8, "2" (renamed) has 9, "u" (conflict) has 10
        let (fields, kind) = match rec.as_bytes().first() {
            Some(b'1') => (8, '1'),
            Some(b'2') => (9, '2'),
            Some(b'u') => (10, 'u'),
            Some(b'?') => {
                st.untracked += 1;
                st.files.push(FileChange { code: '?', path: rec[2..].to_string() });
                continue;
            }
            _ => continue,
        };
        let parts: Vec<&str> = rec.splitn(fields + 1, ' ').collect();
        let (xy, path) = (parts.get(1).copied().unwrap_or(".."), parts.get(fields).copied().unwrap_or_default());
        let code = match kind {
            'u' => {
                st.conflicts += 1;
                'U'
            }
            _ => {
                let (x, y) = (xy.chars().next().unwrap_or('.'), xy.chars().nth(1).unwrap_or('.'));
                if x != '.' {
                    st.staged += 1;
                }
                if y != '.' {
                    st.unstaged += 1;
                }
                // the index one first; a copy counts as added
                match if x != '.' { x } else { y } {
                    _ if kind == '2' => 'R',
                    'A' | 'C' => 'A',
                    'D' => 'D',
                    'T' => 'T',
                    _ => 'M',
                }
            }
        };
        st.files.push(FileChange { code, path: path.to_string() });
        // a rename carries the old path in the next record
        if kind == '2' {
            records.next();
        }
    }
    st
}

/// Runs `git` in `dir`, with a deadline. Returns stdout, or the first line of the error.
pub fn git(dir: &Path, args: &[&str], timeout: Duration) -> Result<Vec<u8>, String> {
    run("git", dir, args, timeout)
}

/// Runs `program` (`git` or `gh`) in `dir`, with a deadline and never asking for a
/// password or confirmation. Returns stdout, or the first line of the error (which the
/// user sees).
pub fn run(program: &str, dir: &Path, args: &[&str], timeout: Duration) -> Result<Vec<u8>, String> {
    let mut cmd = miniutils_app::os::command(program);
    cmd.args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // fetch over ssh without a loaded key: fail instead of waiting for a password
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let mut child = cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => fmt("{} is not installed", &[&program]),
        _ => format!("{program}: {e}"),
    })?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(s) = stdout.as_mut() {
            let _ = s.read_to_end(&mut buf);
        }
        buf
    });
    let err = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(s) = stderr.as_mut() {
            let _ = s.read_to_string(&mut buf);
        }
        buf
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let command = format!("{program} {}", args.first().unwrap_or(&""));
                return Err(fmt("{} did not answer within {} s", &[&command, &timeout.as_secs()]));
            }
        }
    };
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    if status.success() {
        Ok(out)
    } else {
        let line = err
            .lines()
            .map(|l| l.trim_start_matches("fatal: ").trim_start_matches("error: ").trim())
            .find(|l| !l.is_empty() && !l.starts_with("hint:"));
        Err(line.map_or_else(|| fmt("{} exited with {}", &[&program, &status]), str::to_string))
    }
}

pub fn status(dir: &Path) -> Result<Status, String> {
    let out = git(dir, &["--no-optional-locks", "status", "--porcelain=v2", "--branch", "--show-stash", "-z"], STATUS_TIMEOUT)?;
    Ok(parse_status(&out))
}

/// Time (seconds since 1970) of the oldest commit not yet pushed to the upstream.
pub fn oldest_unpushed(dir: &Path) -> Option<u64> {
    let out = git(dir, &["log", "--format=%ct", "@{upstream}..HEAD"], STATUS_TIMEOUT).ok()?;
    String::from_utf8_lossy(&out).lines().filter_map(|l| l.trim().parse().ok()).min()
}

/// Time of the oldest change not committed yet (by the modification time of the changed
/// files). A deleted file has no time and is left out.
pub fn oldest_change(dir: &Path, files: &[FileChange]) -> Option<u64> {
    files
        .iter()
        .take(MAX_AGE_PATHS)
        .filter_map(|f| std::fs::symlink_metadata(dir.join(&f.path)).ok()?.modified().ok())
        .filter_map(|t| t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs()))
        .min()
}

pub fn fetch(dir: &Path) -> Result<(), String> {
    git(dir, &["fetch", "--quiet", "--no-write-fetch-head"], FETCH_TIMEOUT).map(|_| ())
}

/// Pushes the current branch to its upstream.
pub fn push(dir: &Path) -> Result<(), String> {
    git(dir, &["push", "--quiet"], SYNC_TIMEOUT).map(|_| ())
}

/// Pulls the upstream only if the branch can fast-forward (never creates a merge or rebase).
pub fn pull(dir: &Path) -> Result<(), String> {
    git(dir, &["pull", "--ff-only", "--quiet"], SYNC_TIMEOUT).map(|_| ())
}

/// The last `n` commits of the current branch (empty in a repository without commits).
pub fn recent_commits(dir: &Path, n: usize) -> Vec<Commit> {
    let count = format!("-{n}");
    let Ok(out) = git(dir, &["log", &count, "--format=%h%x1f%s%x1f%ct"], QUICK_TIMEOUT) else { return Vec::new() };
    parse_commits(&out)
}

pub fn parse_commits(out: &[u8]) -> Vec<Commit> {
    String::from_utf8_lossy(out)
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\x1f');
            let (hash, subject, time) = (f.next()?, f.next()?, f.next()?);
            Some(Commit { hash: hash.to_string(), subject: subject.to_string(), time: time.trim().parse().unwrap_or(0) })
        })
        .collect()
}

/// The stash entries: "stash@{0}: message".
pub fn stashes(dir: &Path) -> Vec<String> {
    let Ok(out) = git(dir, &["stash", "list", "--format=%gd: %s"], QUICK_TIMEOUT) else { return Vec::new() };
    String::from_utf8_lossy(&out).lines().map(str::to_string).filter(|l| !l.is_empty()).collect()
}

/// The repository's common folder: in a worktree, the main repository's (where the
/// `config` and the refs live).
pub fn common_dir(git_dir: &Path) -> PathBuf {
    std::fs::read_to_string(git_dir.join("commondir")).map(|c| git_dir.join(c.trim())).unwrap_or_else(|_| git_dir.to_path_buf())
}

/// The repository's `config` (empty if it cannot be read).
pub fn config_text(git_dir: &Path) -> String {
    std::fs::read_to_string(common_dir(git_dir).join("config")).unwrap_or_default()
}

/// The main branch, read from disk (without running git): the one `origin/HEAD` points to
/// ("origin/main") or, without it, the local `main` or `master`.
pub fn default_branch(git_dir: &Path) -> Option<String> {
    let common = common_dir(git_dir);
    if let Ok(head) = std::fs::read_to_string(common.join("refs/remotes/origin/HEAD"))
        && let Some(target) = head.trim().strip_prefix("ref: refs/remotes/")
    {
        return Some(target.to_string());
    }
    let packed = std::fs::read_to_string(common.join("packed-refs")).unwrap_or_default();
    ["main", "master"]
        .into_iter()
        .find(|b| common.join("refs/heads").join(b).is_file() || packed.lines().any(|l| l.ends_with(&format!(" refs/heads/{b}"))))
        .map(str::to_string)
}

/// The local name of a branch ("origin/main" -> "main").
pub fn short_branch(branch: &str) -> &str {
    branch.split_once('/').map_or(branch, |(_, b)| b)
}

/// Local branches already merged into `base`, except the current one and the main one.
pub fn merged_branches(dir: &Path, base: &str, current: &str) -> Vec<String> {
    let Ok(out) = git(dir, &["branch", "--merged", base, "--format=%(refname:short)"], QUICK_TIMEOUT) else { return Vec::new() };
    let keep = |b: &str| !b.is_empty() && b != current && b != short_branch(base) && b != base && !matches!(b, "main" | "master");
    String::from_utf8_lossy(&out).lines().map(str::trim).filter(|b| keep(b)).map(str::to_string).collect()
}

/// Deletes local branches with `-d`: git refuses one that is not merged yet.
pub fn delete_branches(dir: &Path, branches: &[String]) -> Result<(), String> {
    if branches.is_empty() {
        return Ok(());
    }
    let mut args = vec!["branch", "-d", "--"];
    args.extend(branches.iter().map(String::as_str));
    git(dir, &args, QUICK_TIMEOUT).map(|_| ())
}

/// The repository's git folder: `.git`, or the target of a `.git` file (worktree,
/// submodule: "gitdir: …").
pub fn git_dir(repo: &Path) -> PathBuf {
    let dot = repo.join(".git");
    if dot.is_file()
        && let Some(target) = std::fs::read_to_string(&dot).ok().and_then(|t| t.trim().strip_prefix("gitdir:").map(|p| p.trim().to_string()))
    {
        let p = PathBuf::from(target);
        return if p.is_absolute() { p } else { repo.join(p) };
    }
    dot
}

/// Half-done operation (merge, rebase…) that needs the user's attention, as the row shows it.
pub fn operation(git_dir: &Path) -> Option<&'static str> {
    let has = |name: &str| git_dir.join(name).exists();
    if has("rebase-merge") || has("rebase-apply") {
        Some(tr("rebase in progress"))
    } else if has("MERGE_HEAD") {
        Some(tr("merge in progress"))
    } else if has("CHERRY_PICK_HEAD") {
        Some(tr("cherry-pick in progress"))
    } else if has("REVERT_HEAD") {
        Some(tr("revert in progress"))
    } else if has("BISECT_LOG") {
        Some(tr("bisect in progress"))
    } else {
        None
    }
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_porcelain_v2_status() {
        let out = [
            "# branch.oid 1234",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -1",
            "# stash 3",
            "1 M. N... 100644 100644 100644 aaa bbb src/main.rs",
            "1 .M N... 100644 100644 100644 aaa bbb name with space.txt",
            "1 MM N... 100644 100644 100644 aaa bbb two.rs",
            "2 R. N... 100644 100644 100644 aaa bbb R100 new.rs",
            "old.rs",
            "u UU N... 100644 100644 100644 100644 aaa bbb ccc conflict.rs",
            "? loose.txt",
            "! ignored/",
            "",
        ]
        .join("\0");
        let st = parse_status(out.as_bytes());
        assert_eq!(st.branch, "main");
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!((st.ahead, st.behind, st.stash), (2, 1, 3));
        assert_eq!((st.staged, st.unstaged, st.untracked, st.conflicts), (3, 2, 1, 1));
        let files: Vec<(char, &str)> = st.files.iter().map(|f| (f.code, f.path.as_str())).collect();
        assert_eq!(
            files,
            [('M', "src/main.rs"), ('M', "name with space.txt"), ('M', "two.rs"), ('R', "new.rs"), ('U', "conflict.rs"), ('?', "loose.txt")]
        );
        assert!(st.dirty());
        let st = parse_status(b"1 A. N... 0 100644 100644 0 bbb new.rs\x001 .D N... 100644 100644 0 aaa bbb gone.rs\0");
        let codes: Vec<char> = st.files.iter().map(|f| f.code).collect();
        assert_eq!(codes, ['A', 'D']);
    }

    #[test]
    fn reads_commits_and_short_branch_name() {
        let c = parse_commits(b"abc1234\x1ffix: something | with a bar\x1f1700000000\ndef5678\x1f\x1f1\n");
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].hash.as_str(), c[0].subject.as_str(), c[0].time), ("abc1234", "fix: something | with a bar", 1_700_000_000));
        assert_eq!(c[1].subject, "");
        assert_eq!(short_branch("origin/main"), "main");
        assert_eq!(short_branch("main"), "main");
    }

    #[test]
    fn detached_head_and_new_repository() {
        let st = parse_status(b"# branch.oid (initial)\0# branch.head (detached)\0");
        assert_eq!(st, Status::default());
        assert!(!st.dirty());
    }

    /// A real repository: changed, committed and with a half-done merge.
    #[test]
    fn status_of_a_real_repository() {
        let dir = std::env::temp_dir().join(format!("repos-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| git(&dir, args, STATUS_TIMEOUT).unwrap();
        if git(&dir, &["init", "-q", "-b", "main"], STATUS_TIMEOUT).is_err() {
            return; // no git on the test machine
        }
        std::fs::write(dir.join("a.txt"), "1").unwrap();
        let st = status(&dir).unwrap();
        assert_eq!((st.branch.as_str(), st.untracked), ("main", 1));
        assert!(oldest_change(&dir, &st.files).is_some_and(|t| t <= now()));
        assert!(recent_commits(&dir, 3).is_empty(), "no commit yet");
        run(&["add", "a.txt"]);
        let commit = |msg: &str| run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", msg]);
        commit("x");
        let st = status(&dir).unwrap();
        assert!(!st.dirty() && st.upstream.is_none());
        assert_eq!(git_dir(&dir), dir.join(".git"));
        assert_eq!(operation(&git_dir(&dir)), None);
        // main branch from disk, and a branch already merged into it
        assert_eq!(default_branch(&git_dir(&dir)).as_deref(), Some("main"));
        run(&["branch", "done"]);
        run(&["checkout", "-q", "-b", "fresh"]);
        std::fs::write(dir.join("b.txt"), "2").unwrap();
        run(&["add", "b.txt"]);
        commit("y");
        assert_eq!(merged_branches(&dir, "main", "fresh"), ["done"]);
        let log = recent_commits(&dir, 3);
        assert_eq!(log.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["y", "x"]);
        run(&["checkout", "-q", "main"]);
        // "fresh" is not in main: -d refuses and nothing is lost
        assert!(delete_branches(&dir, &["fresh".to_string()]).is_err());
        delete_branches(&dir, &["done".to_string()]).unwrap();
        assert_eq!(merged_branches(&dir, "main", "main"), Vec::<String>::new());
        assert!(stashes(&dir).is_empty());
        std::fs::write(dir.join(".git/MERGE_HEAD"), "x").unwrap();
        assert_eq!(operation(&git_dir(&dir)), Some("merge in progress"));
        assert!(git(&dir, &["rev-parse", "does-not-exist"], STATUS_TIMEOUT).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
