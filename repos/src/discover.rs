//! Acha os repositórios dentro das pastas vigiadas.

use std::path::{Path, PathBuf};

/// Pastas que nunca têm repositório do usuário e podem ser enormes.
const SKIP: &[&str] = &["node_modules", "target", "vendor", "dist", "build", "__pycache__", "venv"];

/// É um repositório (tem `.git`, pasta ou arquivo de worktree/submódulo)?
pub fn is_repo(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir.join(".git")).is_ok()
}

/// Os repositórios em `roots`, descendo até `depth` níveis. Não entra num repositório
/// (submódulos e pastas internas ficam com o de cima), nem em pasta oculta, link ou
/// pasta de dependências. Em ordem, sem repetidos.
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
    fn acha_repositorios_ate_a_profundidade() {
        let root = std::env::temp_dir().join(format!("repos-acha-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["a/.git", "a/sub/.git", "grupo/b/.git", "grupo/fundo/c/.git", ".oculto/d/.git", "node_modules/e/.git", "solta"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::create_dir_all(root.join("wt")).unwrap();
        std::fs::write(root.join("wt/.git"), "gitdir: /x").unwrap();
        let rel = |v: Vec<PathBuf>| -> Vec<String> {
            v.iter().map(|p| p.strip_prefix(&root).unwrap().display().to_string().replace('\\', "/")).collect()
        };
        assert_eq!(rel(find_repos(std::slice::from_ref(&root), 2)), ["a", "grupo/b", "wt"]);
        assert_eq!(rel(find_repos(std::slice::from_ref(&root), 3)), ["a", "grupo/b", "grupo/fundo/c", "wt"]);
        // a própria pasta vigiada pode ser um repositório
        assert_eq!(find_repos(&[root.join("a")], 2), [root.join("a")]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
