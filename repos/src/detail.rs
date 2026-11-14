//! O detalhe de um repositório, mostrado ao clicar na linha: os arquivos alterados (já
//! vêm do status), os últimos commits, o stash e as branches que já podem ser apagadas.
//! Só é lido para o repositório aberto.

use std::path::Path;

use serde_json::{Value, json};

use crate::git::{self, Commit, FileChange};
use crate::state::age;

/// Arquivos mostrados no detalhe (o resto vira "+N arquivos").
const FILES: usize = 8;
const COMMITS: usize = 3;
const STASHES: usize = 3;

/// O que o detalhe precisa ler do git (o resto vem do status).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detail {
    pub commits: Vec<Commit>,
    pub stashes: Vec<String>,
    /// Branches locais já mergeadas na principal.
    pub merged: Vec<String>,
    /// A principal em que foram mergeadas ("origin/main").
    pub base: Option<String>,
}

/// Lê o detalhe (roda o git: chame fora da thread principal).
pub fn read(repo: &Path, branch: &str, base: Option<&str>) -> Detail {
    Detail {
        commits: git::recent_commits(repo, COMMITS),
        stashes: git::stashes(repo).into_iter().take(STASHES).collect(),
        merged: base.map(|b| git::merged_branches(repo, b, branch)).unwrap_or_default(),
        base: base.map(str::to_string),
    }
}

/// As propriedades `detail-*` da tela.
pub fn props(d: &Detail, files: &[FileChange], now: u64) -> Vec<(&'static str, Value)> {
    let shown: Vec<String> = files.iter().take(FILES).map(|f| format!("{}  {}", f.code, f.path)).collect();
    let more = match files.len().saturating_sub(FILES) {
        0 => String::new(),
        1 => "+1 arquivo".into(),
        n => format!("+{n} arquivos"),
    };
    let commits: Vec<String> = d
        .commits
        .iter()
        .map(|c| {
            let when = age(now.saturating_sub(c.time));
            let when = if when == "agora" { when } else { format!("há {when}") };
            format!("{}  {} · {when}", c.hash, c.subject)
        })
        .collect();
    let merged = match d.merged.len() {
        0 => String::new(),
        1 => format!("1 branch já mergeada em {}: {}", git::short_branch(d.base.as_deref().unwrap_or("")), d.merged[0]),
        n => format!("{n} branches já mergeadas em {}: {}", git::short_branch(d.base.as_deref().unwrap_or("")), d.merged.join(", ")),
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
    fn propriedades_do_detalhe() {
        let files: Vec<FileChange> = (0..10).map(|i| FileChange { code: 'M', path: format!("f{i}.rs") }).collect();
        let d = Detail {
            commits: vec![Commit { hash: "abc1234".into(), subject: "feat: x".into(), time: 1000 }],
            stashes: vec!["stash@{0}: WIP on main".into()],
            merged: vec!["feita".into(), "velha".into()],
            base: Some("origin/main".into()),
        };
        let p: std::collections::HashMap<_, _> = props(&d, &files, 1000 + 7200).into_iter().collect();
        assert_eq!(p["detail-files"].as_array().unwrap().len(), FILES);
        assert_eq!(p["detail-files"][0], "M  f0.rs");
        assert_eq!(p["detail-more"], "+2 arquivos");
        assert_eq!(p["detail-commits"][0], "abc1234  feat: x · há 2 h");
        assert_eq!(p["detail-merged"], "2 branches já mergeadas em main: feita, velha");
        let vazio: std::collections::HashMap<_, _> = props(&Detail::default(), &[], 0).into_iter().collect();
        assert_eq!((vazio["detail-more"].as_str(), vazio["detail-merged"].as_str()), (Some(""), Some("")));
    }
}
