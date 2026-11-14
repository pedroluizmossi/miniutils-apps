//! Agentes de IA trabalhando nos repositórios, pelo provedor `agents` do hub (o mesmo do
//! app Agentes: Claude Code, Codex e OpenCode). O hub manda a lista de sessões com a pasta
//! de cada uma; aqui ela vira "quem está em qual repositório".
//!
//! Tudo puro (sem disco nem processo), para testar.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

/// O que o agente está fazendo, do mais calmo ao que mais pede atenção.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Activity {
    /// Terminou o turno e espera o próximo pedido.
    Idle,
    Working,
    /// Parado esperando o usuário (permissão, pergunta).
    Waiting,
}

impl Activity {
    /// Está no meio de alguma coisa (trabalhando ou esperando resposta).
    pub fn active(self) -> bool {
        self != Activity::Idle
    }
}

/// O agente mais relevante de um repositório.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// "Claude", "Codex", "OpenCode".
    pub tool: String,
    pub activity: Activity,
}

impl Seen {
    /// O texto da linha: "Claude trabalhando", "Codex espera você"; parado não aparece.
    pub fn label(&self) -> String {
        match self.activity {
            Activity::Working => format!("{} trabalhando", self.tool),
            Activity::Waiting => format!("{} espera você", self.tool),
            Activity::Idle => String::new(),
        }
    }

    /// "working", "waiting" ou "" (para a cor na tela).
    pub fn key(&self) -> &'static str {
        match self.activity {
            Activity::Working => "working",
            Activity::Waiting => "waiting",
            Activity::Idle => "",
        }
    }
}

#[derive(Deserialize)]
struct Snapshot {
    #[serde(default)]
    agents: Vec<Session>,
}

#[derive(Debug, Clone, Deserialize)]
struct Session {
    #[serde(default)]
    tool: String,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    state: String,
    /// Subagente: segue o agente que o disparou, que já conta.
    #[serde(default)]
    parent: Option<String>,
}

fn tool_name(key: &str) -> String {
    match key {
        "claude" => "Claude".into(),
        "codex" => "Codex".into(),
        "opencode" => "OpenCode".into(),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
    }
}

/// Caminho para comparar: barras normais, sem barra no fim e, no Windows, sem diferença
/// de maiúsculas (lá `C:\Projetos` e `c:\projetos` são a mesma pasta).
fn norm(p: &str) -> String {
    let s = p.replace('\\', "/");
    let s = s.trim_end_matches('/');
    if cfg!(windows) { s.to_lowercase() } else { s.to_string() }
}

/// Os agentes de cada repositório, pela pasta de cada sessão (a sessão numa subpasta conta
/// para o repositório; com repositórios aninhados, o mais de dentro). Com vários agentes
/// no mesmo repositório, fica o que mais pede atenção.
pub fn by_repo(data: &Value, repos: &[PathBuf]) -> HashMap<PathBuf, Seen> {
    let Ok(snap) = Snapshot::deserialize(data) else { return HashMap::new() };
    let keys: Vec<(String, &PathBuf)> = repos.iter().map(|r| (norm(&r.to_string_lossy()), r)).collect();
    let mut out: HashMap<PathBuf, Seen> = HashMap::new();
    for s in snap.agents.iter().filter(|s| s.parent.is_none() && !s.cwd.is_empty()) {
        let activity = match s.state.as_str() {
            "working" => Activity::Working,
            "waiting" => Activity::Waiting,
            _ => Activity::Idle,
        };
        let cwd = norm(&s.cwd);
        let inside = |k: &str| cwd == k || cwd.strip_prefix(k).is_some_and(|rest| rest.starts_with('/'));
        let Some((_, repo)) = keys.iter().filter(|(k, _)| inside(k)).max_by_key(|(k, _)| k.len()) else { continue };
        let seen = Seen { tool: tool_name(&s.tool), activity };
        match out.get(*repo) {
            Some(old) if old.activity >= activity => {}
            _ => {
                out.insert((*repo).clone(), seen);
            }
        }
    }
    out
}

/// Repositórios em que um agente estava ativo e parou (terminou o turno ou fechou).
pub fn finished<'a>(before: &'a HashMap<PathBuf, Seen>, now: &HashMap<PathBuf, Seen>) -> Vec<(&'a Path, &'a str)> {
    before
        .iter()
        .filter(|(repo, s)| s.activity == Activity::Working && !now.get(*repo).is_some_and(|n| n.activity.active()))
        .map(|(repo, s)| (repo.as_path(), s.tool.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn path(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn cada_sessao_vai_para_o_repositorio_dela() {
        let repos = [path("/p/api"), path("/p/api/vendor/lib"), path("/p/site"), path("/p/apix")];
        let data = json!({"agents": [
            {"tool": "claude", "cwd": "/p/api/src", "state": "working", "parent": null},
            {"tool": "codex", "cwd": "/p/api/vendor/lib", "state": "idle"},
            {"tool": "opencode", "cwd": "/p/site/", "state": "waiting"},
            // subagente: o pai já conta
            {"tool": "claude", "cwd": "/p/apix", "state": "working", "parent": "abc"},
            {"tool": "claude", "cwd": "/fora", "state": "working"},
        ], "found": ["claude"], "errors": []});
        let m = by_repo(&data, &repos);
        assert_eq!(m.len(), 3);
        assert_eq!(m[&path("/p/api")].label(), "Claude trabalhando");
        assert_eq!(m[&path("/p/api/vendor/lib")].activity, Activity::Idle);
        assert_eq!(m[&path("/p/site")].label(), "OpenCode espera você");
        assert!(!m.contains_key(&path("/p/apix")), "/p/apix não é /p/api nem conta o subagente");
    }

    #[test]
    fn no_mesmo_repositorio_fica_quem_pede_atencao() {
        let repos = [path("/p/api")];
        let data = json!({"agents": [
            {"tool": "codex", "cwd": "/p/api", "state": "working"},
            {"tool": "claude", "cwd": "/p/api", "state": "waiting"},
            {"tool": "opencode", "cwd": "/p/api", "state": "idle"},
        ]});
        assert_eq!(by_repo(&data, &repos)[&path("/p/api")].tool, "Claude");
        assert!(by_repo(&json!({"novidade": 1}), &repos).is_empty());
        assert!(by_repo(&Value::Null, &repos).is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn no_windows_compara_sem_maiusculas_nem_barra() {
        let repos = [path(r"C:\Projetos\api")];
        let data = json!({"agents": [{"tool": "claude", "cwd": "c:/projetos/API/src", "state": "working"}]});
        assert_eq!(by_repo(&data, &repos).len(), 1);
    }

    #[test]
    fn agente_que_parou_de_trabalhar() {
        let w = |t: &str, a| Seen { tool: t.into(), activity: a };
        let before = HashMap::from([
            (path("/a"), w("Claude", Activity::Working)),
            (path("/b"), w("Codex", Activity::Working)),
            (path("/c"), w("Claude", Activity::Working)),
            (path("/d"), w("Claude", Activity::Waiting)),
        ]);
        let now = HashMap::from([
            (path("/a"), w("Claude", Activity::Idle)),
            (path("/c"), w("Claude", Activity::Waiting)),
        ]);
        let mut done: Vec<_> = finished(&before, &now).into_iter().map(|(p, t)| (p.to_path_buf(), t)).collect();
        done.sort();
        // /c passou a esperar o usuário (não terminou); /d já não trabalhava
        assert_eq!(done, [(path("/a"), "Claude"), (path("/b"), "Codex")]);
    }
}
