//! Per-OS odds and ends an external app often needs: run a program without a console
//! window, open a folder or file in the default program, open a terminal in a folder and
//! ask the user for a folder.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// `CREATE_NO_WINDOW`: console program without a window (the app runs without a console,
/// and each child would open a flashing window).
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// `CREATE_NEW_CONSOLE`: the terminal the user asked for, with its own window.
#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

/// `Command` for a background program (on Windows, without a console window).
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

/// Path of a program on the PATH (on Windows, with the `PATHEXT` extensions).
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

/// Fire and forget: a thread reaps the process when it exits (no zombie).
pub fn spawn_detached(cmd: &mut Command) -> std::io::Result<()> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Opens a folder or file in the system's default program.
pub fn open_path(path: &Path) -> std::io::Result<()> {
    if cfg!(windows) {
        // explorer opens folders and, for files, the associated program
        spawn_detached(command("explorer.exe").arg(path))
    } else {
        spawn_detached(command("xdg-open").arg(path))
    }
}

/// Opens the system folder picker and waits for the choice: `Ok(None)` if the user
/// cancelled. Blocks until it closes, so call it on a thread. On Linux it uses `zenity`,
/// `kdialog` or `yad` (the first one found); on Windows, the .NET dialog through PowerShell.
/// Without any picker the error is [`std::io::ErrorKind::NotFound`].
pub fn pick_folder(title: &str) -> std::io::Result<Option<PathBuf>> {
    let mut cmd = folder_dialog(title)?;
    let out = cmd.stdin(Stdio::null()).stderr(Stdio::null()).output()?;
    // closing or cancelling exits with code 1 in the three Linux ones; the Windows one prints nothing
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
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no folder picker found (install zenity or kdialog)"))
}

#[cfg(windows)]
fn folder_dialog(title: &str) -> std::io::Result<Command> {
    // the title goes through an environment variable: never build the script from outside text
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

/// Known Linux terminals, in order of preference (after `$TERMINAL`).
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

/// Opens a terminal starting in `dir`. Without any terminal the error is
/// [`std::io::ErrorKind::NotFound`].
#[cfg(not(windows))]
pub fn open_terminal(dir: &Path) -> std::io::Result<()> {
    let from_env = std::env::var("TERMINAL").ok().filter(|t| !t.is_empty());
    let term = from_env
        .into_iter()
        .chain(TERMINALS.iter().map(|t| t.to_string()))
        .find(|t| which(t).is_some())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no terminal found (set $TERMINAL)"))?;
    let mut cmd = command(&term);
    // most start in the process's folder; these take the folder as an argument
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

/// Opens a terminal starting in `dir`: Windows Terminal, if installed; otherwise `cmd`.
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
