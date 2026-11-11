//! O que o `git` diz de um repositório. Usa o próprio `git` (e não uma biblioteca): o
//! status sai igual ao do terminal do usuário, com `.gitignore`, config e fsmonitor.
//!
//! Toda chamada tem prazo, roda sem pedir senha e com `--no-optional-locks` (o `status`
//! não regrava o índice: senão cada leitura dispararia o vigia do `.git` de novo).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Prazo de um `status` (repositório enorme ou disco lento).
const STATUS_TIMEOUT: Duration = Duration::from_secs(20);
/// Prazo de um `fetch` (rede).
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
/// Quantos arquivos alterados conferir para achar a mudança mais antiga.
const MAX_AGE_PATHS: usize = 300;

/// O resumo do `git status --porcelain=v2 --branch --show-stash`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// Nome da branch; vazio = HEAD solto (detached).
    pub branch: String,
    /// Branch remota que esta acompanha (`origin/main`), se houver.
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Alterações já no índice (`git add`).
    pub staged: u32,
    /// Alterações fora do índice.
    pub unstaged: u32,
    pub untracked: u32,
    pub conflicts: u32,
    pub stash: u32,
    /// Caminhos alterados (relativos ao repositório), para a idade das mudanças.
    pub paths: Vec<String>,
}

impl Status {
    pub fn dirty(&self) -> bool {
        self.staged + self.unstaged + self.untracked + self.conflicts > 0
    }
}

/// Lê a saída de `git status --porcelain=v2 --branch --show-stash -z`.
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
        // campos antes do caminho: "1" tem 8, "2" (renomeado) tem 9, "u" (conflito) tem 10
        let (fields, kind) = match rec.as_bytes().first() {
            Some(b'1') => (8, '1'),
            Some(b'2') => (9, '2'),
            Some(b'u') => (10, 'u'),
            Some(b'?') => {
                st.untracked += 1;
                st.paths.push(rec[2..].to_string());
                continue;
            }
            _ => continue,
        };
        let parts: Vec<&str> = rec.splitn(fields + 1, ' ').collect();
        let (xy, path) = (parts.get(1).copied().unwrap_or(".."), parts.get(fields).copied().unwrap_or_default());
        if kind == 'u' {
            st.conflicts += 1;
        } else {
            let mut xy = xy.chars();
            if xy.next().is_some_and(|c| c != '.') {
                st.staged += 1;
            }
            if xy.next().is_some_and(|c| c != '.') {
                st.unstaged += 1;
            }
        }
        st.paths.push(path.to_string());
        // o renomeado traz o caminho antigo no registro seguinte
        if kind == '2' {
            records.next();
        }
    }
    st
}

/// Roda o `git` em `dir`, com prazo. Devolve o stdout, ou a primeira linha do erro.
pub fn git(dir: &Path, args: &[&str], timeout: Duration) -> Result<Vec<u8>, String> {
    let mut cmd = miniutils_app::os::command("git");
    cmd.args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // fetch por ssh sem chave carregada: falha em vez de esperar senha
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let mut child = cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "o git não está instalado".to_string(),
        _ => format!("git: {e}"),
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
                return Err(format!("git {} não respondeu em {} s", args.first().unwrap_or(&""), timeout.as_secs()));
            }
        }
    };
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    if status.success() {
        Ok(out)
    } else {
        let line = err.lines().map(|l| l.trim_start_matches("fatal: ").trim()).find(|l| !l.is_empty());
        Err(line.map_or_else(|| format!("git saiu com {status}"), str::to_string))
    }
}

pub fn status(dir: &Path) -> Result<Status, String> {
    let out = git(dir, &["--no-optional-locks", "status", "--porcelain=v2", "--branch", "--show-stash", "-z"], STATUS_TIMEOUT)?;
    Ok(parse_status(&out))
}

/// Data (segundos desde 1970) do commit mais antigo ainda não enviado ao upstream.
pub fn oldest_unpushed(dir: &Path) -> Option<u64> {
    let out = git(dir, &["log", "--format=%ct", "@{upstream}..HEAD"], STATUS_TIMEOUT).ok()?;
    String::from_utf8_lossy(&out).lines().filter_map(|l| l.trim().parse().ok()).min()
}

/// Data da mudança mais antiga ainda não commitada (pela data dos arquivos alterados).
/// Arquivo apagado não tem data e fica de fora.
pub fn oldest_change(dir: &Path, paths: &[String]) -> Option<u64> {
    paths
        .iter()
        .take(MAX_AGE_PATHS)
        .filter_map(|p| std::fs::symlink_metadata(dir.join(p)).ok()?.modified().ok())
        .filter_map(|t| t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs()))
        .min()
}

pub fn fetch(dir: &Path) -> Result<(), String> {
    git(dir, &["fetch", "--quiet", "--no-write-fetch-head"], FETCH_TIMEOUT).map(|_| ())
}

/// A pasta do git do repositório: `.git`, ou o destino de um arquivo `.git` (worktree,
/// submódulo: "gitdir: …").
pub fn git_dir(repo: &Path) -> PathBuf {
    let dot = repo.join(".git");
    if dot.is_file() {
        if let Some(target) = std::fs::read_to_string(&dot).ok().and_then(|t| t.trim().strip_prefix("gitdir:").map(|p| p.trim().to_string())) {
            let p = PathBuf::from(target);
            return if p.is_absolute() { p } else { repo.join(p) };
        }
    }
    dot
}

/// Operação pela metade (merge, rebase…), que pede a atenção do usuário.
pub fn operation(git_dir: &Path) -> Option<&'static str> {
    let has = |name: &str| git_dir.join(name).exists();
    if has("rebase-merge") || has("rebase-apply") {
        Some("rebase em andamento")
    } else if has("MERGE_HEAD") {
        Some("merge em andamento")
    } else if has("CHERRY_PICK_HEAD") {
        Some("cherry-pick em andamento")
    } else if has("REVERT_HEAD") {
        Some("revert em andamento")
    } else if has("BISECT_LOG") {
        Some("bisect em andamento")
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
    fn le_o_status_porcelain_v2() {
        let out = [
            "# branch.oid 1234",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -1",
            "# stash 3",
            "1 M. N... 100644 100644 100644 aaa bbb src/main.rs",
            "1 .M N... 100644 100644 100644 aaa bbb nome com espaço.txt",
            "1 MM N... 100644 100644 100644 aaa bbb dois.rs",
            "2 R. N... 100644 100644 100644 aaa bbb R100 novo.rs",
            "antigo.rs",
            "u UU N... 100644 100644 100644 100644 aaa bbb ccc conflito.rs",
            "? solto.txt",
            "! ignorado/",
            "",
        ]
        .join("\0");
        let st = parse_status(out.as_bytes());
        assert_eq!(st.branch, "main");
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!((st.ahead, st.behind, st.stash), (2, 1, 3));
        assert_eq!((st.staged, st.unstaged, st.untracked, st.conflicts), (3, 2, 1, 1));
        assert_eq!(st.paths, ["src/main.rs", "nome com espaço.txt", "dois.rs", "novo.rs", "conflito.rs", "solto.txt"]);
        assert!(st.dirty());
    }

    #[test]
    fn head_solto_e_repositorio_novo() {
        let st = parse_status(b"# branch.oid (initial)\0# branch.head (detached)\0");
        assert_eq!(st, Status::default());
        assert!(!st.dirty());
    }

    /// Um repositório de verdade: alterado, commitado e com merge pela metade.
    #[test]
    fn status_de_um_repositorio_de_verdade() {
        let dir = std::env::temp_dir().join(format!("repos-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| git(&dir, args, STATUS_TIMEOUT).unwrap();
        if git(&dir, &["init", "-q", "-b", "main"], STATUS_TIMEOUT).is_err() {
            return; // sem git na máquina de teste
        }
        std::fs::write(dir.join("a.txt"), "1").unwrap();
        let st = status(&dir).unwrap();
        assert_eq!((st.branch.as_str(), st.untracked), ("main", 1));
        assert!(oldest_change(&dir, &st.paths).is_some_and(|t| t <= now()));
        run(&["add", "a.txt"]);
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "x"]);
        let st = status(&dir).unwrap();
        assert!(!st.dirty() && st.upstream.is_none());
        assert_eq!(git_dir(&dir), dir.join(".git"));
        assert_eq!(operation(&git_dir(&dir)), None);
        std::fs::write(dir.join(".git/MERGE_HEAD"), "x").unwrap();
        assert_eq!(operation(&git_dir(&dir)), Some("merge em andamento"));
        assert!(git(&dir, &["rev-parse", "nao-existe"], STATUS_TIMEOUT).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
