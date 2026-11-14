//! Escolhas do usuário que não são opções do manifesto: repositórios fixados no topo,
//! repositórios ocultos e o dia do último lembrete. Ficam em `preferencias.json`, na pasta
//! de dados do app.
//!
//! O arquivo é novo e só acrescenta: sem ele (instalação anterior), nada fica fixado nem
//! oculto. Por isso o formato dos dados não muda de versão.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE: &str = "preferencias.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prefs {
    #[serde(default)]
    pub pinned: Vec<PathBuf>,
    #[serde(default)]
    pub hidden: Vec<PathBuf>,
    /// Dia ("2026-11-14") em que o lembrete do fim do dia já saiu.
    #[serde(default)]
    pub reminded: Option<String>,
}

impl Prefs {
    /// Lê o arquivo; ausente ou ilegível vira o padrão (e o ilegível vai para o registro).
    pub fn load(data_dir: &Path) -> Prefs {
        let file = data_dir.join(FILE);
        match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("{} ilegível ({e}): começando do zero", file.display());
                Prefs::default()
            }),
            Err(_) => Prefs::default(),
        }
    }

    /// Grava por um arquivo temporário ao lado: uma queda no meio não estraga o anterior.
    pub fn save(&self, data_dir: &Path) {
        let file = data_dir.join(FILE);
        let tmp = data_dir.join(format!("{FILE}.tmp"));
        let json = serde_json::to_vec_pretty(self).unwrap_or_default();
        let result = std::fs::create_dir_all(data_dir).and_then(|_| std::fs::write(&tmp, json)).and_then(|_| std::fs::rename(&tmp, &file));
        if let Err(e) = result {
            eprintln!("não consegui gravar {}: {e}", file.display());
        }
    }

    pub fn is_pinned(&self, repo: &Path) -> bool {
        self.pinned.iter().any(|p| p == repo)
    }

    pub fn is_hidden(&self, repo: &Path) -> bool {
        self.hidden.iter().any(|p| p == repo)
    }

    /// Fixa ou desafixa. Fixar mostra de novo, se estava oculto.
    pub fn toggle_pin(&mut self, repo: &Path) {
        if self.is_pinned(repo) {
            self.pinned.retain(|p| p != repo);
        } else {
            self.pinned.push(repo.to_path_buf());
            self.hidden.retain(|p| p != repo);
        }
    }

    /// Oculta ou mostra de novo. Ocultar desafixa.
    pub fn toggle_hidden(&mut self, repo: &Path) {
        if self.is_hidden(repo) {
            self.hidden.retain(|p| p != repo);
        } else {
            self.hidden.push(repo.to_path_buf());
            self.pinned.retain(|p| p != repo);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixar_e_ocultar_se_excluem_e_sobrevivem_a_gravacao() {
        let dir = std::env::temp_dir().join(format!("repos-prefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Prefs::load(&dir), Prefs::default(), "sem arquivo");
        let (a, b) = (PathBuf::from("/p/a"), PathBuf::from("/p/b"));
        let mut p = Prefs::default();
        p.toggle_hidden(&a);
        p.toggle_pin(&a);
        assert!(p.is_pinned(&a) && !p.is_hidden(&a));
        p.toggle_hidden(&a);
        assert!(!p.is_pinned(&a) && p.is_hidden(&a));
        p.toggle_pin(&b);
        p.reminded = Some("2026-11-14".into());
        p.save(&dir);
        assert_eq!(Prefs::load(&dir), p);
        std::fs::write(dir.join(FILE), "{ quebrado").unwrap();
        assert_eq!(Prefs::load(&dir), Prefs::default());
        // campo que falta (arquivo de uma versão anterior) tem padrão
        std::fs::write(dir.join(FILE), r#"{"pinned":["/p/a"]}"#).unwrap();
        assert!(Prefs::load(&dir).is_pinned(&a));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
