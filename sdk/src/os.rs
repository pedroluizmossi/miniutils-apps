//! Miudezas por sistema que um app externo costuma precisar: rodar um programa sem janela
//! de console, abrir pasta ou arquivo no programa padrão e abrir um terminal numa pasta.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// `CREATE_NO_WINDOW`: programa de console sem janela (o app roda sem console, e cada
/// filho abriria uma janela piscando).
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// `CREATE_NEW_CONSOLE`: o terminal que o usuário pediu, com janela própria.
#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

/// `Command` para um programa de segundo plano (no Windows, sem janela de console).
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Caminho de um programa no PATH (no Windows, com as extensões do `PATHEXT`).
pub fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names: Vec<String> = if cfg!(windows) {
        let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT".into());
        std::iter::once(program.to_string()).chain(exts.split(';').map(|e| format!("{program}{}", e.to_lowercase()))).collect()
    } else {
        vec![program.to_string()]
    };
    std::env::split_paths(&path).find_map(|d| names.iter().map(|n| d.join(n)).find(|p| p.is_file()))
}

/// Lança e esquece: uma thread recolhe o processo quando ele sair (sem zumbi).
pub fn spawn_detached(cmd: &mut Command) -> std::io::Result<()> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Abre uma pasta ou arquivo no programa padrão do sistema.
pub fn open_path(path: &Path) -> std::io::Result<()> {
    if cfg!(windows) {
        // o explorer abre pasta e, para arquivo, o programa associado
        spawn_detached(command("explorer.exe").arg(path))
    } else {
        spawn_detached(command("xdg-open").arg(path))
    }
}

/// Terminais conhecidos do Linux, na ordem de preferência (depois do `$TERMINAL`).
#[cfg(not(windows))]
const TERMINALS: &[&str] = &[
    "x-terminal-emulator",
    "konsole",
    "gnome-terminal",
    "kgx",
    "xfce4-terminal",
    "kitty",
    "alacritty",
    "wezterm",
    "foot",
    "tilix",
    "terminator",
    "xterm",
];

/// Abre um terminal começando em `dir`.
#[cfg(not(windows))]
pub fn open_terminal(dir: &Path) -> std::io::Result<()> {
    let from_env = std::env::var("TERMINAL").ok().filter(|t| !t.is_empty());
    let term = from_env
        .into_iter()
        .chain(TERMINALS.iter().map(|t| t.to_string()))
        .find(|t| which(t).is_some())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "nenhum terminal encontrado (defina $TERMINAL)"))?;
    let mut cmd = command(&term);
    // a maioria começa na pasta do processo; estes pedem a pasta por argumento
    match term.as_str() {
        "gnome-terminal" | "xfce4-terminal" | "tilix" | "terminator" => {
            cmd.arg(format!("--working-directory={}", dir.display()));
        }
        "konsole" => {
            cmd.arg("--workdir").arg(dir);
        }
        "wezterm" => {
            cmd.args(["start", "--cwd"]).arg(dir);
        }
        _ => {}
    }
    spawn_detached(cmd.current_dir(dir))
}

/// Abre um terminal começando em `dir`: o Windows Terminal, se houver; senão o `cmd`.
#[cfg(windows)]
pub fn open_terminal(dir: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    if which("wt").is_some() {
        return spawn_detached(command("wt.exe").arg("-d").arg(dir));
    }
    let mut cmd = Command::new("cmd.exe");
    cmd.creation_flags(CREATE_NEW_CONSOLE).current_dir(dir);
    spawn_detached(&mut cmd)
}
