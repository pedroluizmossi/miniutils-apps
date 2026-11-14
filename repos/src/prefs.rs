//! User choices that are not manifest settings: repositories pinned to the top, hidden
//! repositories and the day of the last reminder. Kept in `prefs.json`, in the app's data
//! folder.
//!
//! The file only adds things: without it (earlier install), nothing is pinned or hidden.
//! That is why the data format version does not change.
//!
//! Versions up to 0.2.0 named the file `preferencias.json`; [`migrate`] renames it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE: &str = "prefs.json";
/// The file's name up to version 0.2.0.
const OLD_FILE: &str = "preferencias.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prefs {
    #[serde(default)]
    pub pinned: Vec<PathBuf>,
    #[serde(default)]
    pub hidden: Vec<PathBuf>,
    /// Day ("2026-11-14") on which the end-of-day reminder already went out.
    #[serde(default)]
    pub reminded: Option<String>,
}

/// Renames the file of older versions (`preferencias.json`) to `prefs.json`, unless the new
/// one already exists.
pub fn migrate(data_dir: &Path) {
    let (old, new) = (data_dir.join(OLD_FILE), data_dir.join(FILE));
    if old.is_file()
        && !new.exists()
        && let Err(e) = std::fs::rename(&old, &new)
    {
        eprintln!("could not rename {} to {FILE}: {e}", old.display());
    }
}

impl Prefs {
    /// Reads the file; missing or unreadable becomes the default (and unreadable goes to the log).
    pub fn load(data_dir: &Path) -> Prefs {
        let file = data_dir.join(FILE);
        match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("{} unreadable ({e}): starting from scratch", file.display());
                Prefs::default()
            }),
            Err(_) => Prefs::default(),
        }
    }

    /// Writes through a temporary file next to it: a crash halfway does not spoil the old one.
    pub fn save(&self, data_dir: &Path) {
        let file = data_dir.join(FILE);
        let tmp = data_dir.join(format!("{FILE}.tmp"));
        let json = serde_json::to_vec_pretty(self).unwrap_or_default();
        let result = std::fs::create_dir_all(data_dir).and_then(|_| std::fs::write(&tmp, json)).and_then(|_| std::fs::rename(&tmp, &file));
        if let Err(e) = result {
            eprintln!("could not write {}: {e}", file.display());
        }
    }

    pub fn is_pinned(&self, repo: &Path) -> bool {
        self.pinned.iter().any(|p| p == repo)
    }

    pub fn is_hidden(&self, repo: &Path) -> bool {
        self.hidden.iter().any(|p| p == repo)
    }

    /// Pins or unpins. Pinning shows it again, if it was hidden.
    pub fn toggle_pin(&mut self, repo: &Path) {
        if self.is_pinned(repo) {
            self.pinned.retain(|p| p != repo);
        } else {
            self.pinned.push(repo.to_path_buf());
            self.hidden.retain(|p| p != repo);
        }
    }

    /// Hides or shows again. Hiding unpins.
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
    fn pin_and_hide_exclude_each_other_and_survive_saving() {
        let dir = std::env::temp_dir().join(format!("repos-prefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Prefs::load(&dir), Prefs::default(), "no file");
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
        std::fs::write(dir.join(FILE), "{ broken").unwrap();
        assert_eq!(Prefs::load(&dir), Prefs::default());
        // a missing field (file from an earlier version) has a default
        std::fs::write(dir.join(FILE), r#"{"pinned":["/p/a"]}"#).unwrap();
        assert!(Prefs::load(&dir).is_pinned(&a));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrates_the_old_file_name() {
        let dir = std::env::temp_dir().join(format!("repos-prefs-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // nothing to migrate: nothing happens
        migrate(&dir);
        assert!(!dir.join(FILE).exists());
        std::fs::write(dir.join(OLD_FILE), r#"{"pinned":["/p/a"],"reminded":"2026-11-13"}"#).unwrap();
        migrate(&dir);
        assert!(!dir.join(OLD_FILE).exists());
        let p = Prefs::load(&dir);
        assert!(p.is_pinned(Path::new("/p/a")));
        assert_eq!(p.reminded.as_deref(), Some("2026-11-13"));
        // the new file wins over a leftover old one
        std::fs::write(dir.join(OLD_FILE), r#"{"hidden":["/p/x"]}"#).unwrap();
        migrate(&dir);
        assert!(Prefs::load(&dir).is_pinned(Path::new("/p/a")));
        assert!(dir.join(OLD_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
