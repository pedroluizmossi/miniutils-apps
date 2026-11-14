//! The watched folders: a text file in the app's data folder (`roots.txt`), one folder per
//! line. The first time, the app creates it with the project folders it finds.
//!
//! Versions up to 0.2.0 named the file `pastas.txt`; [`migrate`] renames it.

use std::path::{Path, PathBuf};

use crate::i18n::tr;

pub const FILE: &str = "roots.txt";
/// The file's name up to version 0.2.0.
const OLD_FILE: &str = "pastas.txt";

/// Common project folders, relative to the home folder. Folder names on disk, so the
/// Portuguese ones ("Projetos") are here on purpose.
const CANDIDATES: &[&str] = &[
    "Projetos",
    "projetos",
    "Projects",
    "projects",
    "src",
    "code",
    "dev",
    "repos",
    "git",
    "workspace",
    "source/repos",
];

pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

/// Renames the file of older versions (`pastas.txt`) to `roots.txt`, unless the new one
/// already exists.
pub fn migrate(data_dir: &Path) {
    let (old, new) = (data_dir.join(OLD_FILE), data_dir.join(FILE));
    if old.is_file()
        && !new.exists()
        && let Err(e) = std::fs::rename(&old, &new)
    {
        eprintln!("could not rename {} to {FILE}: {e}", old.display());
    }
}

/// Reads the folders in `text`: `#` comments, `~` is the home folder, relative paths are
/// ignored.
pub fn parse(text: &str, home: Option<&Path>) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let p = match (l.strip_prefix('~'), home) {
                (Some(rest), Some(h)) => h.join(rest.trim_start_matches(['/', '\\'])),
                (Some(_), None) => return None,
                (None, _) => PathBuf::from(l),
            };
            p.is_absolute().then_some(p)
        })
        .collect()
}

/// The file's initial text, with the project folders that exist in `home`.
pub fn default_text(home: Option<&Path>) -> String {
    let mut text = String::new();
    for line in [tr("Folders where the app looks for Git repositories, one per line."), tr("~ is your home folder. Save and click Refresh in the app.")] {
        text.push_str(&format!("# {line}\n"));
    }
    // on a case-insensitive system, "Projetos" and "projetos" are the same folder: the
    // canonical path removes the repetition
    let mut seen: Vec<PathBuf> = Vec::new();
    let found: Vec<&str> = home
        .map(|h| {
            CANDIDATES
                .iter()
                .copied()
                .filter(|c| {
                    let Ok(real) = std::fs::canonicalize(h.join(c)) else { return false };
                    let new = !seen.contains(&real);
                    seen.push(real);
                    new
                })
                .collect()
        })
        .unwrap_or_default();
    if found.is_empty() {
        text.push_str("~\n");
    }
    for c in found {
        text.push_str(&format!("~/{c}\n"));
    }
    text
}

/// The folders file in `data_dir` (created with the initial text if missing).
pub fn ensure_file(data_dir: &Path) -> PathBuf {
    let file = data_dir.join(FILE);
    if !file.exists() {
        let _ = std::fs::create_dir_all(data_dir);
        if let Err(e) = std::fs::write(&file, default_text(home().as_deref())) {
            eprintln!("could not create {}: {e}", file.display());
        }
    }
    file
}

/// The watched folders that exist.
pub fn load(data_dir: &Path) -> Vec<PathBuf> {
    let file = ensure_file(data_dir);
    let text = std::fs::read_to_string(&file).unwrap_or_else(|_| default_text(home().as_deref()));
    let mut roots = parse(&text, home().as_deref());
    roots.retain(|r| r.is_dir());
    roots
}

/// The file line for `folder`: with `~` when it is inside the home folder.
pub fn line_for(folder: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| folder.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display().to_string().replace('\\', "/")),
        None => folder.display().to_string(),
    }
}

/// Appends `folder` to the file (the folder chosen in the picker). `false` if it was there.
pub fn append(data_dir: &Path, folder: &Path) -> std::io::Result<bool> {
    let file = ensure_file(data_dir);
    let mut text = std::fs::read_to_string(&file).unwrap_or_default();
    if parse(&text, home().as_deref()).iter().any(|r| r == folder) {
        return Ok(false);
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&line_for(folder, home().as_deref()));
    text.push('\n');
    std::fs::write(&file, text)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_folders() {
        let home = std::env::temp_dir().join("home");
        let abs = std::env::temp_dir().join("abs").display().to_string();
        let text = format!("# comment\n\n~/Projects\n~\n{abs}\nrelative/x\n");
        let got = parse(&text, Some(&home));
        assert_eq!(got, [home.join("Projects"), home.clone(), PathBuf::from(&abs)]);
        assert!(parse("~/x", None).is_empty());
    }

    #[test]
    fn picked_folder_becomes_a_line_with_tilde() {
        let home = std::env::temp_dir().join("home");
        assert_eq!(line_for(&home.join("Projects").join("x"), Some(&home)), "~/Projects/x");
        assert_eq!(line_for(&home, Some(&home)), "~");
        let outside = std::env::temp_dir().join("outside");
        assert_eq!(line_for(&outside, Some(&home)), outside.display().to_string());
        // and the line read back gives the same folder
        assert_eq!(parse(&line_for(&home.join("a"), Some(&home)), Some(&home)), [home.join("a")]);
    }

    #[test]
    fn initial_text_lists_the_folders_that_exist() {
        let home = std::env::temp_dir().join(format!("repos-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("Projetos")).unwrap();
        let text = default_text(Some(&home));
        // on a case-insensitive system, "projetos" exists too
        assert!(text.contains("~/Projetos\n"));
        assert!(!text.contains("~/code"));
        assert!(text.starts_with("# Folders where"));
        assert!(default_text(None).ends_with("~\n"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn migrates_the_old_file_name() {
        let dir = std::env::temp_dir().join(format!("repos-roots-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let abs = std::env::temp_dir().display().to_string();
        std::fs::write(dir.join(OLD_FILE), format!("# mine\n{abs}\n")).unwrap();
        migrate(&dir);
        assert!(!dir.join(OLD_FILE).exists());
        // the user's file is kept as it was, not replaced by the initial text
        assert_eq!(std::fs::read_to_string(ensure_file(&dir)).unwrap(), format!("# mine\n{abs}\n"));
        assert_eq!(load(&dir), [PathBuf::from(&abs)]);
        // the new file wins over a leftover old one
        std::fs::write(dir.join(OLD_FILE), "~\n").unwrap();
        migrate(&dir);
        assert!(dir.join(OLD_FILE).exists());
        assert_eq!(std::fs::read_to_string(dir.join(FILE)).unwrap(), format!("# mine\n{abs}\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
