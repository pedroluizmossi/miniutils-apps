//! Do status de cada repositório ao que a tela e a mascote mostram. Tudo puro (sem disco
//! nem processo), para testar.

use std::path::PathBuf;

use miniutils_app::Mood;
use serde_json::{Value, json};

use crate::git::Status;

/// O que o app sabe de um repositório na última leitura.
#[derive(Debug, Clone)]
pub struct RepoInfo {
    pub path: PathBuf,
    pub status: Result<Status, String>,
    /// Operação pela metade (merge, rebase…).
    pub op: Option<&'static str>,
    /// O repositório tem algum remoto configurado.
    pub has_remote: bool,
    /// Data (segundos desde 1970) da mudança não commitada mais antiga.
    pub oldest_change: Option<u64>,
    /// Data do commit não enviado mais antigo.
    pub oldest_unpushed: Option<u64>,
}

impl Default for RepoInfo {
    fn default() -> Self {
        RepoInfo {
            path: PathBuf::new(),
            status: Ok(Status::default()),
            op: None,
            has_remote: false,
            oldest_change: None,
            oldest_unpushed: None,
        }
    }
}

/// A situação de um repositório, da mais urgente para a mais calma (a ordem da lista).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    /// Conflito, ou merge/rebase pela metade.
    Conflict,
    /// Pendência (não commitado ou não enviado) mais velha que o limite.
    Stale,
    /// Alterações não commitadas, recentes.
    Work,
    /// Commits não enviados, recentes.
    Push,
    /// O git não conseguiu ler.
    Error,
    Clean,
}

impl State {
    pub fn key(self) -> &'static str {
        match self {
            State::Conflict => "conflict",
            State::Stale => "stale",
            State::Work => "work",
            State::Push => "push",
            State::Error => "error",
            State::Clean => "clean",
        }
    }

    pub fn pending(self) -> bool {
        matches!(self, State::Conflict | State::Stale | State::Work | State::Push)
    }
}

pub fn classify(info: &RepoInfo, now: u64, stale_secs: u64) -> State {
    let st = match &info.status {
        Ok(st) => st,
        Err(_) => return State::Error,
    };
    if st.conflicts > 0 || info.op.is_some() {
        return State::Conflict;
    }
    let old = |since: Option<u64>| since.is_some_and(|t| now.saturating_sub(t) >= stale_secs);
    let dirty = st.dirty();
    let unpushed = st.ahead > 0;
    if (dirty && old(info.oldest_change)) || (unpushed && old(info.oldest_unpushed)) {
        State::Stale
    } else if dirty {
        State::Work
    } else if unpushed {
        State::Push
    } else {
        State::Clean
    }
}

/// "agora", "12 min", "5 h", "3 d".
pub fn age(secs: u64) -> String {
    match secs {
        0..60 => "agora".into(),
        60..3600 => format!("{} min", secs / 60),
        3600..86_400 => format!("{} h", secs / 3600),
        _ => format!("{} d", secs / 86_400),
    }
}

fn count(n: u32, one: &str, many: &str) -> Option<String> {
    match n {
        0 => None,
        1 => Some(format!("1 {one}")),
        _ => Some(format!("{n} {many}")),
    }
}

/// Uma linha da lista da tela (os campos do `struct Repo` do `view.slint`).
pub fn row(info: &RepoInfo, name: &str, state: State, now: u64) -> Value {
    let path = info.path.display().to_string();
    let st = match &info.status {
        Ok(st) => st,
        Err(e) => {
            return json!({
                "name": name, "path": path, "branch": "", "changes": "erro", "ahead": 0, "behind": 0,
                "age": "", "state": state.key(), "note": e,
            });
        }
    };
    let changes: Vec<String> = [
        count(st.conflicts, "em conflito", "em conflito"),
        count(st.staged, "preparado", "preparados"),
        count(st.unstaged, "alterado", "alterados"),
        count(st.untracked, "novo", "novos"),
    ]
    .into_iter()
    .flatten()
    .collect();
    let since = if st.dirty() { info.oldest_change } else if st.ahead > 0 { info.oldest_unpushed } else { None };
    let mut notes: Vec<String> = Vec::new();
    if let Some(op) = info.op {
        notes.push(op.to_string());
    }
    if st.branch.is_empty() {
        notes.push("HEAD solto".into());
    } else if st.upstream.is_none() {
        notes.push(if info.has_remote { "branch sem upstream" } else { "só local" }.into());
    }
    if let Some(s) = count(st.stash, "no stash", "no stash") {
        notes.push(s);
    }
    json!({
        "name": name,
        "path": path,
        "branch": if st.branch.is_empty() { "(solto)" } else { st.branch.as_str() },
        "changes": if changes.is_empty() { "limpo".to_string() } else { changes.join(" · ") },
        "ahead": st.ahead,
        "behind": st.behind,
        // "há 5 h" (ou "agora"): pronto para a tela
        "age": since.map(|t| age(now.saturating_sub(t))).map(|a| if a == "agora" { a } else { format!("há {a}") }).unwrap_or_default(),
        "state": state.key(),
        "note": notes.join(" · "),
    })
}

/// O resumo de todos: humor, subtítulo da tela e linha da bandeja.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overview {
    pub mood: Mood,
    pub subtitle: String,
    pub summary: String,
}

pub fn overview(states: &[(String, State)]) -> Overview {
    if states.is_empty() {
        let text = "nenhum repositório nas pastas vigiadas".to_string();
        return Overview { mood: Mood::Sleep, subtitle: text.clone(), summary: text };
    }
    let n = states.len();
    let with = |s: State| states.iter().filter(|(_, x)| *x == s).map(|(name, _)| name.as_str()).collect::<Vec<_>>();
    let pending: Vec<&str> = states.iter().filter(|(_, s)| s.pending()).map(|(name, _)| name.as_str()).collect();
    let (conflict, stale) = (with(State::Conflict), with(State::Stale));
    let mood = if !conflict.is_empty() {
        Mood::Alarm
    } else if !stale.is_empty() {
        Mood::Busy
    } else if !pending.is_empty() || !with(State::Error).is_empty() {
        Mood::Idle
    } else {
        Mood::Happy
    };
    let total = if n == 1 { "1 repositório".to_string() } else { format!("{n} repositórios") };
    let subtitle = match pending.len() {
        0 => format!("{total} · tudo commitado e enviado"),
        p => format!("{total} · {p} com pendências"),
    };
    let list = |names: &[&str]| {
        let shown: Vec<&str> = names.iter().take(3).copied().collect();
        let more = names.len().saturating_sub(3);
        if more > 0 { format!("{} e mais {more}", shown.join(", ")) } else { shown.join(", ") }
    };
    let summary = if !conflict.is_empty() {
        format!("em conflito: {}", list(&conflict))
    } else if !pending.is_empty() {
        format!("{} com pendências: {}", pending.len(), list(&pending))
    } else {
        "tudo commitado e enviado".into()
    };
    Overview { mood, subtitle, summary }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: u64 = 3600;

    fn info(st: Status) -> RepoInfo {
        RepoInfo { path: PathBuf::from("/p/api"), status: Ok(st), ..Default::default() }
    }

    #[test]
    fn classifica_pela_pendencia_mais_velha() {
        let now = 100 * H;
        let limpo = Status { branch: "main".into(), upstream: Some("origin/main".into()), ..Default::default() };
        assert_eq!(classify(&info(limpo.clone()), now, 24 * H), State::Clean);
        // atrás do remoto não é pendência do usuário
        assert_eq!(classify(&info(Status { behind: 3, ..limpo.clone() }), now, 24 * H), State::Clean);
        let sujo = Status { unstaged: 2, ..limpo.clone() };
        let mut i = info(sujo);
        i.oldest_change = Some(now - H);
        assert_eq!(classify(&i, now, 24 * H), State::Work);
        i.oldest_change = Some(now - 30 * H);
        assert_eq!(classify(&i, now, 24 * H), State::Stale);
        let mut i = info(Status { ahead: 1, ..limpo.clone() });
        i.oldest_unpushed = Some(now - 2 * H);
        assert_eq!(classify(&i, now, 24 * H), State::Push);
        assert_eq!(classify(&i, now, H), State::Stale);
        let mut i = info(limpo.clone());
        i.op = Some("rebase em andamento");
        assert_eq!(classify(&i, now, 24 * H), State::Conflict);
        assert_eq!(classify(&info(Status { conflicts: 1, ..limpo }), now, 24 * H), State::Conflict);
        let erro = RepoInfo { status: Err("dubious ownership".into()), ..Default::default() };
        assert_eq!(classify(&erro, now, 24 * H), State::Error);
    }

    #[test]
    fn linha_da_tela() {
        let now = 100 * H;
        let st = Status { branch: "main".into(), staged: 1, unstaged: 2, untracked: 1, ahead: 2, stash: 1, ..Default::default() };
        let mut i = info(st);
        i.has_remote = true;
        i.oldest_change = Some(now - 5 * H);
        let r = row(&i, "api", State::Work, now);
        assert_eq!(r["changes"], "1 preparado · 2 alterados · 1 novo");
        assert_eq!(r["age"], "há 5 h");
        assert_eq!(r["note"], "branch sem upstream · 1 no stash");
        assert_eq!((r["ahead"].as_u64(), r["state"].as_str()), (Some(2), Some("work")));
        let r = row(&info(Status::default()), "x", State::Clean, now);
        assert_eq!((r["changes"].as_str(), r["branch"].as_str(), r["note"].as_str()), (Some("limpo"), Some("(solto)"), Some("HEAD solto")));
        assert_eq!(age(30), "agora");
        assert_eq!(age(3 * 86_400 + 5), "3 d");
    }

    #[test]
    fn humor_e_resumo() {
        let s = |v: &[(&str, State)]| v.iter().map(|(n, s)| (n.to_string(), *s)).collect::<Vec<_>>();
        assert_eq!(overview(&[]).mood, Mood::Sleep);
        let tudo = overview(&s(&[("a", State::Clean), ("b", State::Clean)]));
        assert_eq!((tudo.mood, tudo.subtitle.as_str()), (Mood::Happy, "2 repositórios · tudo commitado e enviado"));
        let o = overview(&s(&[("a", State::Work), ("b", State::Stale), ("c", State::Clean)]));
        assert_eq!(o.mood, Mood::Busy);
        assert_eq!(o.summary, "2 com pendências: a, b");
        let o = overview(&s(&[("a", State::Conflict), ("b", State::Stale)]));
        assert_eq!((o.mood, o.summary.as_str()), (Mood::Alarm, "em conflito: a"));
        let muitos = s(&[("a", State::Push), ("b", State::Push), ("c", State::Push), ("d", State::Push), ("e", State::Push)]);
        assert_eq!(overview(&muitos).summary, "5 com pendências: a, b, c e mais 2");
    }
}
