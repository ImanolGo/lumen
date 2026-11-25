//! Another window: another process of this program, which shares nothing with this one.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

/// This program, for starting it again. On Linux as it runs, even if an upgrade has replaced the file.
fn program() -> Result<std::path::PathBuf> {
    if cfg!(target_os = "linux") { Ok("/proc/self/exe".into()) } else { Ok(std::env::current_exe()?) }
}

fn command(path: &Path) -> Result<Command> {
    let mut command = Command::new(program()?);
    command
        .arg(std::path::absolute(path)?)
        // Whoever drives this window does not drive that one.
        .env_remove("LUMEN_CONTROL")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(command)
}

/// Opens `path` in a window of its own.
pub fn open(path: &Path) -> Result<()> {
    let mut child = command(path)?.spawn().context("could not start another Lumen")?;
    // Waited for, so that what is left of it goes when it ends.
    std::thread::Builder::new().name("lumen-window".into()).spawn(move || child.wait())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_is_given_the_whole_path_and_not_the_control() {
        let c = command(Path::new("-v.png")).unwrap();
        let arg = Path::new(c.get_args().next().unwrap());
        assert!(arg.is_absolute() && arg.ends_with("-v.png"), "never taken for an option");
        assert!(c.get_envs().any(|(k, v)| k == "LUMEN_CONTROL" && v.is_none()));
    }
}
