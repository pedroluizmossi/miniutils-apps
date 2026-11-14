//! Do status de cada repositório ao que a tela e a mascote mostram. Tudo puro (sem disco
//! nem processo), para testar.

use std::path::PathBuf;

use miniutils_app::Mood;
use serde_json::{Value, json};

use crate::agents::Seen;
use crate::gh::{Ci, GhInfo};
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
    /// O remoto é do github.com (o `gh` sabe ler o CI e os PRs).
    pub github: bool,
    /// A branch principal ("origin/main"), se dá para saber.
    pub base: Option<String>,
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
            github: false,
            base: None,
            oldest_change: None,
            oldest_unpushed: None,
        }
    }
}

impl RepoInfo {
    /// Tem alterações sem commit?
    pub fn dirty(&self) -> bool {
        self.status.as_ref().is_ok_and(Status::dirty)
    }

    /// Commits sem enviar ao upstream.
    pub fn unpushed(&self) -> u32 {
        self.status.as_ref().map_or(0, |s| if s.upstream.is_some() { s.ahead } else { 0 })
    }

    /// Dá para enviar com um clique: tem upstream, commits à frente e nada pela metade.
    pub fn can_push(&self) -> bool {
        self.status.as_ref().is_ok_and(|s| s.upstream.is_some() && s.ahead > 0 && s.conflicts == 0) && self.op.is_none()
    }

    /// Dá para puxar com um clique: atrás do upstream e sem nada local que o pull pudesse
    /// misturar (sem alteração, sem commit à frente: o `--ff-only` só avança).
    pub fn can_pull(&self) -> bool {
        self.status.as_ref().is_ok_and(|s| s.upstream.is_some() && s.behind > 0 && s.ahead == 0 && !s.dirty()) && self.op.is_none()
    }
}

/// A situação de um repositório, da mais urgente para a mais calma (a ordem da lista).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    /// Conflito, ou merge/rebase pela metade.
    Conflict,
    /// O CI da branch principal falhou.
    Broken,
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
            State::Broken => "broken",
            State::Stale => "stale",
            State::Work => "work",
            State::Push => "push",
            State::Error => "error",
            State::Clean => "clean",
        }
    }

    pub fn pending(self) -> bool {
        matches!(self, State::Conflict | State::Broken | State::Stale | State::Work | State::Push)
    }
}

/// O que, além do git, muda a situação de um repositório.
#[derive(Debug, Clone, Copy, Default)]
pub struct Flags {
    /// O CI da branch principal falhou.
    pub ci_broken: bool,
    /// Um agente está trabalhando nele (ou esperando o usuário): a pendência é do agente,
    /// e ainda não conta como velha.
    pub agent_active: bool,
}

pub fn classify(info: &RepoInfo, now: u64, stale_secs: u64, flags: Flags) -> State {
    let st = match &info.status {
        Ok(st) => st,
        Err(_) => return State::Error,
    };
    if st.conflicts > 0 || info.op.is_some() {
        return State::Conflict;
    }
    if flags.ci_broken {
        return State::Broken;
    }
    let old = |since: Option<u64>| !flags.agent_active && since.is_some_and(|t| now.saturating_sub(t) >= stale_secs);
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

/// O que a linha mostra além do git.
#[derive(Debug, Clone, Copy, Default)]
pub struct Extra<'a> {
    /// O agente trabalhando (ou esperando) no repositório.
    pub agent: Option<&'a Seen>,
    /// O agente que terminou ali há pouco ("Claude"), se deixou alteração sem commit.
    pub finished: Option<&'a str>,
    pub gh: Option<&'a GhInfo>,
    /// Ação em curso: "enviando…", "puxando…", "apagando branches…".
    pub busy: Option<&'a str>,
    pub pinned: bool,
    pub hidden: bool,
}

/// O CI que a linha mostra: o da branch atual (que pode ser a principal).
fn shown_ci(st: &Status, gh: &GhInfo) -> Option<Ci> {
    if !gh.base.is_empty() && st.branch == gh.base { gh.base_ci } else { gh.branch_ci }
}

/// Uma linha da lista da tela (os campos do `struct Repo` do `view.slint`).
pub fn row(info: &RepoInfo, name: &str, state: State, now: u64, extra: Extra) -> Value {
    let path = info.path.display().to_string();
    let mut base = json!({
        "name": name, "path": path, "branch": "", "changes": "erro", "ahead": 0, "behind": 0,
        "age": "", "state": state.key(), "note": "",
        "agent": "", "agent-state": "", "can-push": false, "can-pull": false,
        "busy": extra.busy.unwrap_or_default(), "ci": "", "ci-state": "",
        "pinned": extra.pinned, "hidden": extra.hidden,
    });
    let st = match &info.status {
        Ok(st) => st,
        Err(e) => {
            base["note"] = json!(e);
            return base;
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
    let (mut ci, mut ci_state) = ("", "");
    if let Some(gh) = extra.gh {
        if gh.base_ci == Some(Ci::Fail) && st.branch != gh.base {
            notes.push(format!("CI de {} falhou", gh.base));
        }
        if let Some(c) = shown_ci(st, gh) {
            (ci, ci_state) = (c.label(), c.key());
        }
        if let Some(s) = count(gh.reviews, "PR espera sua revisão", "PRs esperam sua revisão") {
            notes.push(s);
        }
    }
    let (agent, agent_state) = match (extra.agent.filter(|a| a.activity.active()), extra.finished) {
        (Some(a), _) => (a.label(), a.key()),
        (None, Some(tool)) if st.dirty() => (format!("{tool} terminou · sem commit"), "finished"),
        _ => (String::new(), ""),
    };
    let busy = extra.busy.is_some();
    base["branch"] = json!(if st.branch.is_empty() { "(solto)" } else { st.branch.as_str() });
    base["changes"] = json!(if changes.is_empty() { "limpo".to_string() } else { changes.join(" · ") });
    base["ahead"] = json!(st.ahead);
    base["behind"] = json!(st.behind);
    // "há 5 h" (ou "agora"): pronto para a tela
    base["age"] = json!(since.map(|t| age(now.saturating_sub(t))).map(|a| if a == "agora" { a } else { format!("há {a}") }).unwrap_or_default());
    base["note"] = json!(notes.join(" · "));
    base["agent"] = json!(agent);
    base["agent-state"] = json!(agent_state);
    base["can-push"] = json!(!busy && info.can_push());
    base["can-pull"] = json!(!busy && info.can_pull());
    base["ci"] = json!(ci);
    base["ci-state"] = json!(ci_state);
    base
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
    let (conflict, broken, stale) = (with(State::Conflict), with(State::Broken), with(State::Stale));
    let mood = if !conflict.is_empty() || !broken.is_empty() {
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
    } else if !broken.is_empty() {
        format!("CI quebrado: {}", list(&broken))
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
    use crate::agents::Activity;

    const H: u64 = 3600;
    const NADA: Flags = Flags { ci_broken: false, agent_active: false };

    fn info(st: Status) -> RepoInfo {
        RepoInfo { path: PathBuf::from("/p/api"), status: Ok(st), ..Default::default() }
    }

    #[test]
    fn classifica_pela_pendencia_mais_velha() {
        let now = 100 * H;
        let limpo = Status { branch: "main".into(), upstream: Some("origin/main".into()), ..Default::default() };
        assert_eq!(classify(&info(limpo.clone()), now, 24 * H, NADA), State::Clean);
        // atrás do remoto não é pendência do usuário
        assert_eq!(classify(&info(Status { behind: 3, ..limpo.clone() }), now, 24 * H, NADA), State::Clean);
        let sujo = Status { unstaged: 2, ..limpo.clone() };
        let mut i = info(sujo);
        i.oldest_change = Some(now - H);
        assert_eq!(classify(&i, now, 24 * H, NADA), State::Work);
        i.oldest_change = Some(now - 30 * H);
        assert_eq!(classify(&i, now, 24 * H, NADA), State::Stale);
        // com um agente trabalhando nele, a pendência ainda não é velha
        assert_eq!(classify(&i, now, 24 * H, Flags { agent_active: true, ..NADA }), State::Work);
        let mut i = info(Status { ahead: 1, ..limpo.clone() });
        i.oldest_unpushed = Some(now - 2 * H);
        assert_eq!(classify(&i, now, 24 * H, NADA), State::Push);
        assert_eq!(classify(&i, now, H, NADA), State::Stale);
        // CI quebrado passa na frente das pendências, mas não do conflito
        assert_eq!(classify(&i, now, H, Flags { ci_broken: true, ..NADA }), State::Broken);
        let mut i = info(limpo.clone());
        i.op = Some("rebase em andamento");
        assert_eq!(classify(&i, now, 24 * H, Flags { ci_broken: true, ..NADA }), State::Conflict);
        assert_eq!(classify(&info(Status { conflicts: 1, ..limpo }), now, 24 * H, NADA), State::Conflict);
        let erro = RepoInfo { status: Err("dubious ownership".into()), ..Default::default() };
        assert_eq!(classify(&erro, now, 24 * H, NADA), State::Error);
    }

    #[test]
    fn linha_da_tela() {
        let now = 100 * H;
        let st = Status { branch: "main".into(), staged: 1, unstaged: 2, untracked: 1, ahead: 2, stash: 1, ..Default::default() };
        let mut i = info(st);
        i.has_remote = true;
        i.oldest_change = Some(now - 5 * H);
        let r = row(&i, "api", State::Work, now, Extra::default());
        assert_eq!(r["changes"], "1 preparado · 2 alterados · 1 novo");
        assert_eq!(r["age"], "há 5 h");
        assert_eq!(r["note"], "branch sem upstream · 1 no stash");
        assert_eq!((r["ahead"].as_u64(), r["state"].as_str()), (Some(2), Some("work")));
        // sem upstream não dá para enviar com um clique
        assert_eq!(r["can-push"], false);
        let r = row(&info(Status::default()), "x", State::Clean, now, Extra::default());
        assert_eq!((r["changes"].as_str(), r["branch"].as_str(), r["note"].as_str()), (Some("limpo"), Some("(solto)"), Some("HEAD solto")));
        let r = row(&RepoInfo { status: Err("ruim".into()), ..Default::default() }, "e", State::Error, now, Extra::default());
        assert_eq!((r["note"].as_str(), r["can-pull"].as_bool()), (Some("ruim"), Some(false)));
        assert_eq!(age(30), "agora");
        assert_eq!(age(3 * 86_400 + 5), "3 d");
    }

    #[test]
    fn enviar_e_puxar_com_um_clique() {
        let up = Status { branch: "main".into(), upstream: Some("origin/main".into()), ..Default::default() };
        assert!(info(Status { ahead: 1, ..up.clone() }).can_push());
        assert!(!info(Status { ahead: 1, conflicts: 1, ..up.clone() }).can_push());
        assert!(info(Status { behind: 2, ..up.clone() }).can_pull());
        // alteração local ou commit à frente: o pull não é só avançar
        assert!(!info(Status { behind: 2, unstaged: 1, ..up.clone() }).can_pull());
        assert!(!info(Status { behind: 2, ahead: 1, ..up.clone() }).can_pull());
        let i = info(Status { ahead: 1, ..up });
        let busy = row(&i, "a", State::Push, 0, Extra { busy: Some("enviando…"), ..Default::default() });
        assert_eq!((busy["can-push"].as_bool(), busy["busy"].as_str()), (Some(false), Some("enviando…")));
        assert_eq!(i.unpushed(), 1);
    }

    #[test]
    fn agente_ci_e_revisoes_na_linha() {
        let st = Status { branch: "feat".into(), upstream: Some("origin/feat".into()), unstaged: 1, ..Default::default() };
        let i = info(st);
        let working = Seen { tool: "Claude".into(), activity: Activity::Working };
        let gh = GhInfo { base: "main".into(), base_ci: Some(Ci::Fail), branch_ci: Some(Ci::Ok), reviews: 2 };
        let r = row(&i, "api", State::Broken, 0, Extra { agent: Some(&working), gh: Some(&gh), ..Default::default() });
        assert_eq!((r["agent"].as_str(), r["agent-state"].as_str()), (Some("Claude trabalhando"), Some("working")));
        assert_eq!((r["ci"].as_str(), r["ci-state"].as_str()), (Some("CI ✓"), Some("ok")));
        assert_eq!(r["note"], "CI de main falhou · 2 PRs esperam sua revisão");
        // terminou e deixou alteração: a linha cobra
        let r = row(&i, "api", State::Work, 0, Extra { finished: Some("Codex"), ..Default::default() });
        assert_eq!((r["agent"].as_str(), r["agent-state"].as_str()), (Some("Codex terminou · sem commit"), Some("finished")));
        // na principal, o CI mostrado é o dela
        let main = info(Status { branch: "main".into(), upstream: Some("origin/main".into()), ..Default::default() });
        let r = row(&main, "api", State::Broken, 0, Extra { gh: Some(&gh), ..Default::default() });
        assert_eq!((r["ci"].as_str(), r["note"].as_str()), (Some("CI ✗"), Some("2 PRs esperam sua revisão")));
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
        let o = overview(&s(&[("a", State::Broken), ("b", State::Clean)]));
        assert_eq!((o.mood, o.summary.as_str()), (Mood::Alarm, "CI quebrado: a"));
        let muitos = s(&[("a", State::Push), ("b", State::Push), ("c", State::Push), ("d", State::Push), ("e", State::Push)]);
        assert_eq!(overview(&muitos).summary, "5 com pendências: a, b, c e mais 2");
    }
}
