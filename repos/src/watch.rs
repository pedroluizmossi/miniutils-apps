//! Vigia o `.git` de cada repositório: commit, add, checkout, pull e fetch mexem no índice,
//! no HEAD ou nas refs, e o app relê aquele repositório na hora.
//!
//! A árvore de trabalho não é vigiada (um `node_modules` gastaria milhares de vigias do
//! inotify); editar um arquivo aparece na releitura periódica ou quando a janela abre.

use std::path::{Path, PathBuf};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

pub struct Watch {
    watcher: RecommendedWatcher,
    /// O que está sendo vigiado agora (para tirar ao trocar a lista).
    watched: Vec<PathBuf>,
}

impl Watch {
    /// `on` recebe o caminho de cada mudança (na thread do vigia).
    pub fn new(on: impl Fn(PathBuf) + Send + 'static) -> Result<Watch, String> {
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            if !matches!(ev.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)) {
                return;
            }
            for p in ev.paths {
                // arquivos de trava aparecem e somem a cada comando: o arquivo de verdade avisa
                if p.extension().is_none_or(|e| e != "lock") {
                    on(p);
                }
            }
        })
        .map_err(|e| e.to_string())?;
        Ok(Watch { watcher, watched: Vec::new() })
    }

    /// Passa a vigiar estas pastas do git (e só elas).
    pub fn set(&mut self, git_dirs: &[PathBuf]) {
        for p in std::mem::take(&mut self.watched) {
            let _ = self.watcher.unwatch(&p);
        }
        for gd in git_dirs {
            // a pasta em si (index, HEAD, packed-refs) e as refs (branches e remotos)
            let targets = [(gd.clone(), RecursiveMode::NonRecursive), (gd.join("refs"), RecursiveMode::Recursive)];
            for (path, mode) in targets {
                if path.is_dir() {
                    match self.watcher.watch(&path, mode) {
                        Ok(()) => self.watched.push(path),
                        Err(e) => eprintln!("não consegui vigiar {}: {e}", path.display()),
                    }
                }
            }
        }
    }
}

/// O repositório (pela pasta do git) a que pertence uma mudança em `path`.
pub fn owner<'a>(path: &Path, repos: impl IntoIterator<Item = (&'a PathBuf, &'a PathBuf)>) -> Option<PathBuf> {
    repos.into_iter().find(|(_, git_dir)| path.starts_with(git_dir)).map(|(repo, _)| repo.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    #[test]
    fn mudanca_no_git_avisa_o_repositorio_certo() {
        let root = std::env::temp_dir().join(format!("repos-vigia-{}", std::process::id()));
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
        // o aviso da escrita chega e aponta o repositório b. No Windows podem vir avisos a
        // mais (de outro repositório, ou repetidos): só fazem o app reler, então o teste os
        // ignora
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
        assert!(found, "sem aviso da escrita em b; chegaram: {seen:?}");
        // arquivo de trava não avisa
        std::fs::write(a.join(".git/index.lock"), "x").unwrap();
        let later: Vec<PathBuf> = std::iter::from_fn(|| rx.recv_timeout(Duration::from_millis(500)).ok()).take(50).collect();
        assert!(later.iter().all(|p| p.extension().is_none_or(|e| e != "lock")), "{later:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
