//! Handing a session over to `claude --resume`. Port of bin/sessio.mjs:652-720.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn in_ghostty() -> bool {
    std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
        || std::env::var("TERM_PROGRAM").map(|v| v == "ghostty").unwrap_or(false)
}

/// Ghostty has no way to target a sibling pane, but its CLI can open a NEW window running a
/// command in the running instance. Returns true if the launch was accepted.
///
/// The command runs through a *login* shell on purpose: a GUI-launched Ghostty can have a
/// minimal PATH, and `-e claude` would exec directly and fail to find claude/node. Homebrew et
/// al. append to the login profile, which `-l` sources. Do not "simplify" this to a direct exec.
pub fn ghostty_launch(cwd: &Path, id: &str) -> bool {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let args: Vec<String> = vec![
        "+new-window".into(),
        format!("--working-directory={}", cwd.display()),
        "-e".into(),
        shell,
        "-l".into(),
        "-c".into(),
        "exec claude --resume \"$1\"".into(),
        "sessio".into(), // $0 for the -c script
        id.to_string(),  // $1
    ];

    for bin in [
        "ghostty",
        "/Applications/Ghostty.app/Contents/MacOS/ghostty",
    ] {
        let Ok(mut child) = Command::new(bin)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        // Bound any hang so the TUI can't freeze behind a wedged launcher.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.success(),
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(_) => break,
            }
        }
    }
    false // ghostty CLI not found / timed out / failed → caller resumes in place
}

/// Hand THIS terminal over to `claude --resume`, replacing sessio.
///
/// Uses `exec` rather than spawn-and-wait: the process image is replaced, so there is no idle
/// parent holding the terminal and signals reach claude directly. It never returns on success.
///
/// The caller MUST have restored the terminal (left the alternate screen, shown the cursor,
/// disabled raw mode) before calling — after `exec` there is no code left to do it.
pub fn resume_in_place(cwd: Option<&Path>, id: &str) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new("claude");
    cmd.arg("--resume").arg(id);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.exec() // only returns on failure
}

/// The command to print when we cannot launch claude ourselves.
pub fn manual_command(cwd: Option<&Path>, id: &str) -> String {
    let dir = cwd.map(|c| c.display().to_string()).unwrap_or_else(|| ".".into());
    format!("cd -- {} && claude --resume {}", shell_quote(&dir), shell_quote(id))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quoting_survives_apostrophes() {
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn manual_command_is_copy_pasteable() {
        let cmd = manual_command(Some(Path::new("/tmp/a b")), "abc-123");
        assert_eq!(cmd, "cd -- '/tmp/a b' && claude --resume 'abc-123'");
    }
}
