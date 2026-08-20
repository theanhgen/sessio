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

/// Raise the terminal window already showing this session, so ↵ moves you to the running
/// session instead of starting a second `claude` on the same transcript.
///
/// Ghostty exposes no way to target a window — `+new-window` is the only IPC action and it is not
/// even supported on macOS — but its windows are ordinary accessibility objects, and Claude Code
/// sets the terminal title to the session's name. So: match the title and `AXRaise`.
///
/// A window shows the title of whichever *split* has focus and says nothing about the others, so
/// a session sharing a window with another is invisible from outside until it is the focused one.
/// Walking the splits with `goto_split:next` fixes that, because the title after each step says
/// where the walk landed — that feedback is what makes this a search rather than a guess. A window
/// with one split reports the same title back and is dropped after a single keystroke, and the
/// walk wraps, so a window that does not hold the session ends on the split it started on.
///
/// Still cannot reach a session in a background *tab*: those are not accessibility objects at all,
/// so there is nothing to enumerate and no title to read. `false` means "couldn't find it", never
/// "not running".
#[cfg(target_os = "macos")]
pub fn focus_window_titled(name: &str) -> bool {
    // A short name would match half the desktop. Claude's titles are sentences; anything this
    // short is a first-prompt fallback that was never a window title anyway.
    if name.chars().count() < 8 {
        return false;
    }
    const SCRIPT: &str = r#"on run argv
  set target to item 1 of argv
  set mayWalk to (item 2 of argv is "walk")
  set maxSteps to 6
  tell application "System Events"
    if not (exists process "Ghostty") then return "noproc"
    tell process "Ghostty"
      repeat with w in windows
        if (name of w as text) contains target then
          perform action "AXRaise" of w
          set frontmost to true
          return "ok"
        end if
      end repeat
      if not mayWalk then return "nomatch"
      set wasMain to missing value
      try
        set wasMain to first window whose value of attribute "AXMain" is true
      end try
      repeat with w in windows
        set home to name of w as text
        perform action "AXRaise" of w
        set frontmost to true
        delay 0.15
        repeat with i from 1 to maxSteps
          keystroke "]" using command down
          delay 0.18
          set here to name of w as text
          if here contains target then return "ok"
          if here is home then exit repeat
        end repeat
      end repeat
      if wasMain is not missing value then
        perform action "AXRaise" of wasMain
        set frontmost to true
      end if
    end tell
  end tell
  return "nomatch"
end run"#;

    // `name` is passed as argv, never interpolated into the script, so a session title cannot
    // become AppleScript.
    let walk = if split_walk_is_safe() { "walk" } else { "raise-only" };
    Command::new("osascript")
        .args(["-e", SCRIPT, name, walk])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| o.stdout.starts_with(b"ok"))
        .unwrap_or(false)
}

/// Whether ⌘] still belongs to Ghostty. Asked once, from Ghostty itself (~25ms).
///
/// This is not politeness about a rebound key: a keystroke Ghostty does not claim is delivered to
/// whatever is running in that split, so walking with an unbound ⌘] would type brackets into the
/// user's session instead of moving between panes.
#[cfg(target_os = "macos")]
fn split_walk_is_safe() -> bool {
    use std::sync::OnceLock;
    static SAFE: OnceLock<bool> = OnceLock::new();
    *SAFE.get_or_init(|| {
        Command::new("ghostty")
            .arg("+list-keybinds")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map(|o| walk_key_is_bound(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or(false)
    })
}

/// True when `super+]` is bound to `goto_split:next` and nothing else.
pub fn walk_key_is_bound(keybinds: &str) -> bool {
    keybinds.lines().any(|l| {
        let l = l.trim().strip_prefix("keybind = ").unwrap_or(l.trim());
        l.strip_prefix("super+]=").is_some_and(|action| action.trim() == "goto_split:next")
    })
}

/// No accessibility API to lean on outside macOS; the caller falls back to the guard.
#[cfg(not(target_os = "macos"))]
pub fn focus_window_titled(_name: &str) -> bool {
    false
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
    fn the_split_walk_only_runs_when_ghostty_owns_the_key() {
        // ⌘] is only ours to send while Ghostty claims it. Rebound or removed, the same keystroke
        // is delivered to whatever runs in that split — so this decides between walking the panes
        // and typing brackets into someone's session.
        let stock = "keybind = super+d=new_split:right\nkeybind = super+]=goto_split:next\n";
        assert!(walk_key_is_bound(stock));
        // Ghostty prints them bare from some code paths.
        assert!(walk_key_is_bound("super+]=goto_split:next"));

        assert!(!walk_key_is_bound("keybind = super+]=goto_split:previous"), "wrong direction");
        assert!(!walk_key_is_bound("keybind = super+]=text:hello"), "rebound to something else");
        assert!(!walk_key_is_bound("keybind = super+[=goto_split:next"), "different key");
        assert!(!walk_key_is_bound("keybind = super+shift+]=goto_split:next"), "needs modifiers");
        assert!(!walk_key_is_bound(""), "no keybinds at all");
    }

    #[test]
    fn manual_command_is_copy_pasteable() {
        let cmd = manual_command(Some(Path::new("/tmp/a b")), "abc-123");
        assert_eq!(cmd, "cd -- '/tmp/a b' && claude --resume 'abc-123'");
    }
}
