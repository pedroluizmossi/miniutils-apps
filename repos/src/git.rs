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
/// Prazo de um `push` ou `pull` pedido pelo botão (rede, e o hook de pre-push).
const SYNC_TIMEOUT: Duration = Duration::from_secs(120);
/// Prazo das leituras rápidas (log, stash, branches).
const QUICK_TIMEOUT: Duration = Duration::from_secs(10);
/// Quantos arquivos alterados conferir para achar a mudança mais antiga.
const MAX_AGE_PATHS: usize = 300;

/// Um arquivo alterado: o código curto da mudança e o caminho relativo ao repositório.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// `M` alterado, `A` novo no índice, `D` apagado, `R` renomeado, `T` mudou de tipo,
    /// `U` em conflito, `?` fora do git.
    pub code: char,
    pub path: String,
}

/// Um commit para o detalhe: hash curto, assunto e data (segundos desde 1970).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub subject: String,
    pub time: u64,
}

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
    /// Arquivos alterados, para a idade das mudanças e o detalhe.
    pub files: Vec<FileChange>,
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
                // o do índice primeiro; cópia conta como novo
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
        // o renomeado traz o caminho antigo no registro seguinte
        if kind == '2' {
            records.next();
        }
    }
    st
}

/// Roda o `git` em `dir`, com prazo. Devolve o stdout, ou a primeira linha do erro.
pub fn git(dir: &Path, args: &[&str], timeout: Duration) -> Result<Vec<u8>, String> {
    run("git", dir, args, timeout)
}

/// Roda `program` (o `git` ou o `gh`) em `dir`, com prazo e sem nunca pedir senha nem
/// confirmação. Devolve o stdout, ou a primeira linha do erro.
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
    // fetch por ssh sem chave carregada: falha em vez de esperar senha
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let mut child = cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("o {program} não está instalado"),
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
                return Err(format!("{program} {} não respondeu em {} s", args.first().unwrap_or(&""), timeout.as_secs()));
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
        Err(line.map_or_else(|| format!("{program} saiu com {status}"), str::to_string))
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

/// Envia a branch atual para o upstream dela.
pub fn push(dir: &Path) -> Result<(), String> {
    git(dir, &["push", "--quiet"], SYNC_TIMEOUT).map(|_| ())
}

/// Traz o upstream só se der para avançar a branch (nunca cria merge nem rebase).
pub fn pull(dir: &Path) -> Result<(), String> {
    git(dir, &["pull", "--ff-only", "--quiet"], SYNC_TIMEOUT).map(|_| ())
}

/// Os últimos `n` commits da branch atual (vazio num repositório sem commit).
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

/// As entradas do stash: "stash@{0}: mensagem".
pub fn stashes(dir: &Path) -> Vec<String> {
    let Ok(out) = git(dir, &["stash", "list", "--format=%gd: %s"], QUICK_TIMEOUT) else { return Vec::new() };
    String::from_utf8_lossy(&out).lines().map(str::to_string).filter(|l| !l.is_empty()).collect()
}

/// A pasta comum do repositório: numa worktree, a do repositório principal (onde ficam
/// o `config` e as refs).
pub fn common_dir(git_dir: &Path) -> PathBuf {
    std::fs::read_to_string(git_dir.join("commondir")).map(|c| git_dir.join(c.trim())).unwrap_or_else(|_| git_dir.to_path_buf())
}

/// O `config` do repositório (vazio se não der para ler).
pub fn config_text(git_dir: &Path) -> String {
    std::fs::read_to_string(common_dir(git_dir).join("config")).unwrap_or_default()
}

/// A branch principal, lida do disco (sem rodar o git): a do `origin/HEAD` ("origin/main")
/// ou, sem ela, a `main` ou a `master` locais.
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

/// O nome local de uma branch ("origin/main" -> "main").
pub fn short_branch(branch: &str) -> &str {
    branch.split_once('/').map_or(branch, |(_, b)| b)
}

/// Branches locais já mergeadas em `base`, tirando a atual e a própria principal.
pub fn merged_branches(dir: &Path, base: &str, current: &str) -> Vec<String> {
    let Ok(out) = git(dir, &["branch", "--merged", base, "--format=%(refname:short)"], QUICK_TIMEOUT) else { return Vec::new() };
    let keep = |b: &str| !b.is_empty() && b != current && b != short_branch(base) && b != base && !matches!(b, "main" | "master");
    String::from_utf8_lossy(&out).lines().map(str::trim).filter(|b| keep(b)).map(str::to_string).collect()
}

/// Apaga branches locais com `-d`: o git recusa a que ainda não foi mergeada.
pub fn delete_branches(dir: &Path, branches: &[String]) -> Result<(), String> {
    if branches.is_empty() {
        return Ok(());
    }
    let mut args = vec!["branch", "-d", "--"];
    args.extend(branches.iter().map(String::as_str));
    git(dir, &args, QUICK_TIMEOUT).map(|_| ())
}

/// A pasta do git do repositório: `.git`, ou o destino de um arquivo `.git` (worktree,
/// submódulo: "gitdir: …").
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
        let files: Vec<(char, &str)> = st.files.iter().map(|f| (f.code, f.path.as_str())).collect();
        assert_eq!(
            files,
            [('M', "src/main.rs"), ('M', "nome com espaço.txt"), ('M', "dois.rs"), ('R', "novo.rs"), ('U', "conflito.rs"), ('?', "solto.txt")]
        );
        assert!(st.dirty());
        let st = parse_status(b"1 A. N... 0 100644 100644 0 bbb novo.rs\x001 .D N... 100644 100644 0 aaa bbb foi.rs\0");
        let codes: Vec<char> = st.files.iter().map(|f| f.code).collect();
        assert_eq!(codes, ['A', 'D']);
    }

    #[test]
    fn le_os_commits_e_o_nome_curto_da_branch() {
        let c = parse_commits(b"abc1234\x1ffix: algo | com barra\x1f1700000000\ndef5678\x1f\x1f1\n");
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].hash.as_str(), c[0].subject.as_str(), c[0].time), ("abc1234", "fix: algo | com barra", 1_700_000_000));
        assert_eq!(c[1].subject, "");
        assert_eq!(short_branch("origin/main"), "main");
        assert_eq!(short_branch("main"), "main");
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
        assert!(oldest_change(&dir, &st.files).is_some_and(|t| t <= now()));
        assert!(recent_commits(&dir, 3).is_empty(), "sem commit ainda");
        run(&["add", "a.txt"]);
        let commit = |msg: &str| run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", msg]);
        commit("x");
        let st = status(&dir).unwrap();
        assert!(!st.dirty() && st.upstream.is_none());
        assert_eq!(git_dir(&dir), dir.join(".git"));
        assert_eq!(operation(&git_dir(&dir)), None);
        // branch principal pelo disco, e uma branch já mergeada nela
        assert_eq!(default_branch(&git_dir(&dir)).as_deref(), Some("main"));
        run(&["branch", "feita"]);
        run(&["checkout", "-q", "-b", "nova"]);
        std::fs::write(dir.join("b.txt"), "2").unwrap();
        run(&["add", "b.txt"]);
        commit("y");
        assert_eq!(merged_branches(&dir, "main", "nova"), ["feita"]);
        let log = recent_commits(&dir, 3);
        assert_eq!(log.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["y", "x"]);
        run(&["checkout", "-q", "main"]);
        // "nova" não está em main: o -d recusa e nada some
        assert!(delete_branches(&dir, &["nova".to_string()]).is_err());
        delete_branches(&dir, &["feita".to_string()]).unwrap();
        assert_eq!(merged_branches(&dir, "main", "main"), Vec::<String>::new());
        assert!(stashes(&dir).is_empty());
        std::fs::write(dir.join(".git/MERGE_HEAD"), "x").unwrap();
        assert_eq!(operation(&git_dir(&dir)), Some("merge em andamento"));
        assert!(git(&dir, &["rev-parse", "nao-existe"], STATUS_TIMEOUT).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
