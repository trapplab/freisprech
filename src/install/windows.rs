//! Windows: a Start menu shortcut plus an entry under "Installed apps", and autostart via
//! the `Run` key. Uses the system's `reg` and `powershell` instead of the Win32 API.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use super::ignore_missing;

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const UNINSTALL_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Freisprech";
const VALUE_NAME: &str = "Freisprech";
/// Keeps helper programs from flashing a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn install_path() -> Result<PathBuf> {
    Ok(dirs::data_local_dir()
        .context("No local app data folder")?
        .join("Programs")
        .join("Freisprech")
        .join("freisprech.exe"))
}

/// Start menu shortcut, and an entry under "Installed apps" whose uninstall runs
/// `freisprech.exe --uninstall`.
pub fn add_menu_entry(exe: &Path) -> Result<()> {
    let script = format!(
        "$s = (New-Object -ComObject WScript.Shell).CreateShortcut({}); \
         $s.TargetPath = {}; $s.Arguments = '--settings'; $s.Save()",
        ps_quote(&shortcut_path()?),
        ps_quote(exe),
    );
    run(Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]))
        .context("Failed to create the Start menu shortcut")?;

    let quoted = format!("\"{}\"", exe.display());
    let path = exe.display().to_string();
    let folder = exe.parent().context("Install path has no folder")?.display().to_string();
    for (name, value) in [
        ("DisplayName", "Freisprech"),
        ("DisplayVersion", env!("CARGO_PKG_VERSION")),
        ("DisplayIcon", &path),
        ("InstallLocation", &folder),
        ("UninstallString", &format!("{quoted} --uninstall")),
    ] {
        run(&mut reg(&["add", UNINSTALL_KEY, "/v", name, "/t", "REG_SZ", "/d", value, "/f"]))?;
    }
    for name in ["NoModify", "NoRepair"] {
        run(&mut reg(&["add", UNINSTALL_KEY, "/v", name, "/t", "REG_DWORD", "/d", "1", "/f"]))?;
    }
    Ok(())
}

pub fn remove_menu_entry() -> Result<()> {
    ignore_missing(std::fs::remove_file(shortcut_path()?))?;
    if succeeds(&mut reg(&["query", UNINSTALL_KEY]))? {
        run(&mut reg(&["delete", UNINSTALL_KEY, "/f"]))?;
    }
    Ok(())
}

pub fn autostart_enabled() -> bool {
    succeeds(&mut reg(&["query", RUN_KEY, "/v", VALUE_NAME])).unwrap_or(false)
}

pub fn add_autostart(exe: &Path) -> Result<()> {
    let quoted = format!("\"{}\"", exe.display());
    run(&mut reg(&["add", RUN_KEY, "/v", VALUE_NAME, "/t", "REG_SZ", "/d", &quoted, "/f"]))
}

pub fn remove_autostart() -> Result<()> {
    if autostart_enabled() {
        run(&mut reg(&["delete", RUN_KEY, "/v", VALUE_NAME, "/f"]))?;
    }
    Ok(())
}

/// A running executable and loaded DLLs cannot be deleted, so leave that to a command
/// that waits until the app has exited.
pub fn remove_files(exe: &Path, native: &Path) -> Result<()> {
    let folder = exe.parent().context("Install path has no folder")?;
    super::run_after_exit(powershell_after_exit(&format!(
        "Remove-Item -Recurse -Force -LiteralPath {}, {}",
        ps_quote(folder),
        ps_quote(native),
    )));
    Ok(())
}

/// Waits until this process is gone and has released the shortcut.
pub fn delayed_start(exe: &Path) -> Command {
    powershell_after_exit(&format!(
        "Start-Process -FilePath {} -ArgumentList '--settings'",
        ps_quote(exe)
    ))
}

/// Ends other running copies of the installed app, so their files can be removed.
pub fn stop_other_instances() {
    let pid = format!("PID ne {}", std::process::id());
    let _ = succeeds(Command::new("taskkill").args(["/f", "/im", "freisprech.exe", "/fi", &pid]));
}

fn shortcut_path() -> Result<PathBuf> {
    Ok(dirs::data_dir()
        .context("No app data folder")?
        .join(r"Microsoft\Windows\Start Menu\Programs\Freisprech.lnk"))
}

/// PowerShell running `commands` once this process has exited (at most 30 s later):
/// shutting down can take a while.
fn powershell_after_exit(commands: &str) -> Command {
    let script = format!(
        "$ErrorActionPreference = 'SilentlyContinue'; \
         Wait-Process -Id {} -Timeout 30; {commands}",
        std::process::id()
    );
    let mut command = Command::new("powershell");
    command
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW);
    command
}

fn reg(args: &[&str]) -> Command {
    let mut command = Command::new("reg");
    command.args(args);
    command
}

/// A PowerShell string literal; single-quoted strings expand nothing.
fn ps_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "''"))
}

fn succeeds(command: &mut Command) -> Result<bool> {
    let status = command
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("Failed to run {}", command.get_program().to_string_lossy()))?;
    Ok(status.success())
}

fn run(command: &mut Command) -> Result<()> {
    anyhow::ensure!(
        succeeds(command)?,
        "{} failed",
        command.get_program().to_string_lossy()
    );
    Ok(())
}
