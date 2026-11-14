//! CI e PRs do GitHub pelo `gh` (a CLI oficial), quando ele está instalado e autenticado.
//! Só para repositórios com remoto no github.com. Lido junto com o fetch, a cada 30 min:
//! o `gh` gasta a cota da API do GitHub, e o CI não muda a cada segundo.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::git;

/// Prazo de uma consulta do `gh` (rede).
const TIMEOUT: Duration = Duration::from_secs(20);

/// Situação da última execução do CI numa branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ci {
    Ok,
    Fail,
    Running,
}

impl Ci {
    /// O texto curto da linha: "CI ✓", "CI ✗", "CI …".
    pub fn label(self) -> &'static str {
        match self {
            Ci::Ok => "CI ✓",
            Ci::Fail => "CI ✗",
            Ci::Running => "CI …",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Ci::Ok => "ok",
            Ci::Fail => "fail",
            Ci::Running => "running",
        }
    }
}

/// O que o GitHub diz de um repositório.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GhInfo {
    /// Nome local da branch principal ("main"), se conhecida.
    pub base: String,
    /// CI da branch principal.
    pub base_ci: Option<Ci>,
    /// CI da branch atual, quando ela não é a principal.
    pub branch_ci: Option<Ci>,
    /// PRs abertos que pedem a sua revisão.
    pub reviews: u32,
}

/// O `gh` está instalado e autenticado? (sem login, cada consulta só daria erro)
pub fn ready(dir: &Path) -> bool {
    miniutils_app::os::which("gh").is_some() && git::run("gh", dir, &["auth", "status"], TIMEOUT).is_ok()
}

/// O remoto do repositório é do github.com? (pelo `config`, sem rodar nada)
pub fn is_github(config: &str) -> bool {
    config.lines().any(|l| {
        let l = l.trim();
        l.starts_with("url") && l.contains("github.com")
    })
}

#[derive(Deserialize)]
struct Run {
    #[serde(default)]
    status: String,
    #[serde(default)]
    conclusion: String,
}

/// A saída de `gh run list --limit 1 --json status,conclusion`. Cancelado ou pulado não
/// diz nada sobre o código: fica sem situação.
pub fn parse_run(out: &[u8]) -> Option<Ci> {
    let runs: Vec<Run> = serde_json::from_slice(out).ok()?;
    let run = runs.first()?;
    if run.status != "completed" {
        return Some(Ci::Running);
    }
    match run.conclusion.as_str() {
        "success" => Some(Ci::Ok),
        "failure" | "timed_out" | "startup_failure" => Some(Ci::Fail),
        _ => None,
    }
}

/// Quantos itens tem a lista JSON (`gh pr list --json number`).
pub fn parse_count(out: &[u8]) -> u32 {
    serde_json::from_slice::<Vec<serde_json::Value>>(out).map(|v| v.len() as u32).unwrap_or(0)
}

fn ci_of(dir: &Path, branch: &str) -> Option<Ci> {
    match git::run("gh", dir, &["run", "list", "--branch", branch, "--limit", "1", "--json", "status,conclusion"], TIMEOUT) {
        Ok(out) => parse_run(&out),
        Err(e) => {
            eprintln!("gh run list em {}: {e}", dir.display());
            None
        }
    }
}

/// Lê o CI da principal (e o da branch atual, se for outra) e os pedidos de revisão.
pub fn read(dir: &Path, branch: &str, base: Option<&str>) -> GhInfo {
    let base = base.map(git::short_branch).unwrap_or_default().to_string();
    let base_ci = (!base.is_empty()).then(|| ci_of(dir, &base)).flatten();
    let branch_ci = (!branch.is_empty() && branch != base).then(|| ci_of(dir, branch)).flatten();
    let reviews = git::run(
        "gh",
        dir,
        &["pr", "list", "--state", "open", "--search", "review-requested:@me", "--json", "number", "--limit", "100"],
        TIMEOUT,
    )
    .map(|o| parse_count(&o))
    .unwrap_or(0);
    GhInfo { base, base_ci, branch_ci, reviews }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_a_ultima_execucao_do_ci() {
        assert_eq!(parse_run(br#"[{"status":"completed","conclusion":"success"}]"#), Some(Ci::Ok));
        assert_eq!(parse_run(br#"[{"status":"completed","conclusion":"failure"}]"#), Some(Ci::Fail));
        assert_eq!(parse_run(br#"[{"status":"in_progress","conclusion":""}]"#), Some(Ci::Running));
        assert_eq!(parse_run(br#"[{"status":"queued"}]"#), Some(Ci::Running));
        assert_eq!(parse_run(br#"[{"status":"completed","conclusion":"cancelled"}]"#), None);
        assert_eq!(parse_run(b"[]"), None);
        assert_eq!(parse_run(b"erro"), None);
        assert_eq!(parse_count(br#"[{"number":1},{"number":7}]"#), 2);
        assert_eq!(parse_count(b""), 0);
    }

    #[test]
    fn so_remoto_do_github() {
        let gh = "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = git@github.com:eu/x.git\n";
        let gl = "[remote \"origin\"]\n\turl = https://gitlab.com/eu/x.git\n";
        assert!(is_github(gh));
        assert!(!is_github(gl));
        assert!(!is_github(""));
    }
}
