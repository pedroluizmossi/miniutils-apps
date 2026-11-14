//! Miudezas por sistema que um app externo costuma precisar: rodar um programa sem janela
//! de console, abrir pasta ou arquivo no programa padrão, abrir um terminal numa pasta e
//! pedir uma pasta ao usuário.

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

/// Abre o seletor de pasta do sistema e espera a escolha: `Ok(None)` se a pessoa cancelou.
/// Bloqueia até fechar, então chame numa thread. No Linux usa o `zenity`, o `kdialog` ou o
/// `yad` (o primeiro que houver); no Windows, o diálogo do .NET pelo PowerShell.
pub fn pick_folder(title: &str) -> std::io::Result<Option<PathBuf>> {
    let mut cmd = folder_dialog(title)?;
    let out = cmd.stdin(Stdio::null()).stderr(Stdio::null()).output()?;
    // fechar ou cancelar sai com código 1 nos três do Linux; o do Windows não imprime nada
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((out.status.success() && !path.is_empty()).then(|| PathBuf::from(path)))
}

#[cfg(not(windows))]
fn folder_dialog(title: &str) -> std::io::Result<Command> {
    let start = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    if which("zenity").is_some() {
        let mut c = command("zenity");
        c.args(["--file-selection", "--directory"]).arg(format!("--title={title}"));
        return Ok(c);
    }
    if which("kdialog").is_some() {
        let mut c = command("kdialog");
        c.arg("--getexistingdirectory").arg(&start).arg("--title").arg(title);
        return Ok(c);
    }
    if which("yad").is_some() {
        let mut c = command("yad");
        c.args(["--file", "--directory"]).arg(format!("--title={title}"));
        return Ok(c);
    }
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "nenhum seletor de pasta (instale o zenity ou o kdialog)"))
}

#[cfg(windows)]
fn folder_dialog(title: &str) -> std::io::Result<Command> {
    // o título vai por variável de ambiente: nada de montar o script com texto de fora
    let script = "Add-Type -AssemblyName System.Windows.Forms; \
        $owner = New-Object System.Windows.Forms.Form -Property @{ TopMost = $true }; \
        $d = New-Object System.Windows.Forms.FolderBrowserDialog; \
        $d.Description = $env:MINIUTILS_PICK_TITLE; $d.ShowNewFolderButton = $false; \
        if ($d.ShowDialog($owner) -eq 'OK') { \
            [Console]::OutputEncoding = [Text.Encoding]::UTF8; [Console]::Out.Write($d.SelectedPath) }";
    let mut c = command("powershell.exe");
    c.args(["-NoProfile", "-NonInteractive", "-STA", "-Command", script]).env("MINIUTILS_PICK_TITLE", title);
    Ok(c)
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
