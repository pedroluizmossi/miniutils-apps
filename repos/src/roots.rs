//! As pastas vigiadas: um arquivo de texto na pasta de dados do app (`pastas.txt`), uma
//! pasta por linha. Na primeira vez, o app o cria com as pastas de projetos que achar.

use std::path::{Path, PathBuf};

pub const FILE: &str = "pastas.txt";

/// Pastas de projetos comuns, relativas à pasta pessoal.
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

/// Lê as pastas de `text`: `#` comenta, `~` é a pasta pessoal, caminho relativo é ignorado.
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

/// O texto inicial do arquivo, com as pastas de projetos que existem em `home`.
pub fn default_text(home: Option<&Path>) -> String {
    let mut text = String::from(
        "# Pastas onde o app procura repositórios Git, uma por linha.\n\
         # ~ é a sua pasta pessoal. Salve e clique em atualizar no app.\n",
    );
    // num sistema que não diferencia maiúsculas, "Projetos" e "projetos" são a mesma pasta:
    // o caminho canônico tira a repetição
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

/// O arquivo de pastas em `data_dir` (criado com o texto inicial se não existir).
pub fn ensure_file(data_dir: &Path) -> PathBuf {
    let file = data_dir.join(FILE);
    if !file.exists() {
        let _ = std::fs::create_dir_all(data_dir);
        if let Err(e) = std::fs::write(&file, default_text(home().as_deref())) {
            eprintln!("não consegui criar {}: {e}", file.display());
        }
    }
    file
}

/// As pastas vigiadas que existem.
pub fn load(data_dir: &Path) -> Vec<PathBuf> {
    let file = ensure_file(data_dir);
    let text = std::fs::read_to_string(&file).unwrap_or_else(|_| default_text(home().as_deref()));
    let mut roots = parse(&text, home().as_deref());
    roots.retain(|r| r.is_dir());
    roots
}

/// A linha de `pasta` no arquivo: com `~` quando ela fica dentro da pasta pessoal.
pub fn line_for(pasta: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| pasta.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display().to_string().replace('\\', "/")),
        None => pasta.display().to_string(),
    }
}

/// Acrescenta `pasta` ao arquivo (a pasta escolhida no seletor). `false` se já estava.
pub fn append(data_dir: &Path, pasta: &Path) -> std::io::Result<bool> {
    let file = ensure_file(data_dir);
    let mut text = std::fs::read_to_string(&file).unwrap_or_default();
    if parse(&text, home().as_deref()).iter().any(|r| r == pasta) {
        return Ok(false);
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&line_for(pasta, home().as_deref()));
    text.push('\n');
    std::fs::write(&file, text)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_as_pastas() {
        let home = std::env::temp_dir().join("casa");
        let abs = std::env::temp_dir().join("abs").display().to_string();
        let text = format!("# comentário\n\n~/Projetos\n~\n{abs}\nrelativa/x\n");
        let got = parse(&text, Some(&home));
        assert_eq!(got, [home.join("Projetos"), home.clone(), PathBuf::from(&abs)]);
        assert!(parse("~/x", None).is_empty());
    }

    #[test]
    fn pasta_escolhida_vira_linha_com_til() {
        let home = std::env::temp_dir().join("casa");
        assert_eq!(line_for(&home.join("Projetos").join("x"), Some(&home)), "~/Projetos/x");
        assert_eq!(line_for(&home, Some(&home)), "~");
        let fora = std::env::temp_dir().join("fora");
        assert_eq!(line_for(&fora, Some(&home)), fora.display().to_string());
        // e a linha lida de volta dá a mesma pasta
        assert_eq!(parse(&line_for(&home.join("a"), Some(&home)), Some(&home)), [home.join("a")]);
    }

    #[test]
    fn texto_inicial_traz_as_pastas_que_existem() {
        let home = std::env::temp_dir().join(format!("repos-casa-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("Projetos")).unwrap();
        let text = default_text(Some(&home));
        // em sistema que não diferencia maiúsculas, "projetos" também existe
        assert!(text.contains("~/Projetos\n"));
        assert!(!text.contains("~/code"));
        assert!(default_text(None).ends_with("~\n"));
        let _ = std::fs::remove_dir_all(&home);
    }
}
