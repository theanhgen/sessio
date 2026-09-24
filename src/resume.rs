//! Handing a session over to `claude --resume`. Port of bin/sessio.mjs:652-720.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn in_ghostty() -> bool {
    std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
        || std::env::var("TERM_PROGRAM").map(|v| v == "ghostty").unwrap_or(false)
}

/// What a new window runs: a *login* shell that `cd`s into the session's folder and execs
/// `claude --resume` — or, with no `id`, a fresh `claude` there (`^n`).
///
/// The login shell is on purpose: a GUI-launched Ghostty can have a minimal PATH, and running
/// `claude` directly would fail to find claude/node. Homebrew et al. append to the login profile,
/// which `-l` sources. Do not "simplify" this to a direct exec.
///
/// The script `cd`s itself rather than trusting the terminal's working-directory setting, which
/// has not always been honoured, and `claude --resume` in the wrong folder cannot find the
/// session. The folder and id travel as positional arguments, never spliced into the script.
fn window_command(shell: &str, cwd: &Path, id: Option<&str>) -> Vec<String> {
    let script = match id {
        Some(_) => "cd -- \"$1\" && exec claude --resume \"$2\"",
        None => "cd -- \"$1\" && exec claude",
    };
    let mut argv = vec![
        shell.to_string(),
        "-l".into(),
        "-c".into(),
        script.into(),
        "sessio".into(),                    // $0 for the -c script
        cwd.to_string_lossy().into_owned(), // $1
    ];
    argv.extend(id.map(str::to_string)); // $2
    argv
}

/// Resume session `id` in a new Ghostty window.
pub fn ghostty_launch(cwd: &Path, id: &str) -> Result<(), String> {
    ghostty_window(cwd, Some(id))
}

/// A fresh `claude` in `cwd`, in a new Ghostty window — `ghostty_launch` with nothing to resume.
pub fn ghostty_launch_fresh(cwd: &Path) -> Result<(), String> {
    ghostty_window(cwd, None)
}

fn login_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
}

/// macOS: ask the Ghostty that is already running for a new window, through its AppleScript
/// dictionary (Ghostty 1.3+: `new window with configuration`).
///
/// This replaced `open -na Ghostty.app --args …`, which must never come back. `-n` starts a
/// *second Ghostty process*, and a fresh process restores the saved window state — so every ↵
/// reopened every window the user had, about twenty at a time, on top of the one asked for.
/// (`ghostty +new-window` is no alternative: on macOS it answers "not supported on this platform".)
///
/// Ghostty runs `command` through a shell, so the argv is quoted into one string here. The folder
/// and id reach osascript as argv items and are never interpolated into the AppleScript.
#[cfg(target_os = "macos")]
fn ghostty_window(cwd: &Path, id: Option<&str>) -> Result<(), String> {
    const SCRIPT: &str = r#"on run argv
  tell application "Ghostty"
    set cfg to new surface configuration
    set initial working directory of cfg to item 1 of argv
    set command of cfg to item 2 of argv
    new window with configuration cfg
  end tell
  return "ok"
end run"#;
    let command = shell_join(&window_command(&login_shell(), cwd, id));
    let mut child = Command::new("osascript")
        .args(["-e", SCRIPT])
        .arg(cwd)
        .arg(&command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("osascript: {e}"))?;
    // Bound any hang so the TUI can't freeze behind a wedged launch. Generous, because the first
    // run can sit behind macOS's "allow control of Ghostty" prompt.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Ghostty did not answer".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() && out.stdout.starts_with(b"ok") {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    // osascript prefixes "NN:MM: execution error: "; the rest is the part worth showing.
    let why = err.rsplit("error: ").next().unwrap_or("").trim();
    Err(if why.is_empty() { "Ghostty refused".into() } else { why.chars().take(80).collect() })
}

/// Elsewhere: Ghostty's CLI can open a new window in the running instance (GTK builds).
#[cfg(not(target_os = "macos"))]
fn ghostty_window(cwd: &Path, id: Option<&str>) -> Result<(), String> {
    let mut child = Command::new("ghostty")
        .arg("+new-window")
        .arg(format!("--working-directory={}", cwd.display()))
        .arg("-e")
        .args(window_command(&login_shell(), cwd, id))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("ghostty: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("ghostty +new-window: {status}")),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("ghostty did not answer".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Bring forward the Ghostty terminal on `tty` — the one a running session lives in — through
/// Ghostty's AppleScript dictionary (1.3+), where every terminal reports its tty and `focus`
/// selects its tab and split and raises its window. Unlike matching window titles, this reaches
/// background tabs and unfocused splits, and cannot pick the wrong window.
///
/// `false` means Ghostty had no terminal on that tty (the session runs in another terminal app,
/// or the process is gone) or refused; the caller falls back to saying where it is.
#[cfg(target_os = "macos")]
pub fn focus_tty(tty: &str) -> bool {
    const SCRIPT: &str = r#"on run argv
  tell application "Ghostty"
    set ts to (every terminal whose tty is (item 1 of argv))
    if (count of ts) is 0 then return "nomatch"
    focus (item 1 of ts)
    activate
  end tell
  return "ok"
end run"#;
    if tty.is_empty() {
        return false;
    }
    let dev = if tty.starts_with("/dev/") { tty.to_string() } else { format!("/dev/{tty}") };
    Command::new("osascript")
        .args(["-e", SCRIPT, &dev])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| o.stdout.starts_with(b"ok"))
        .unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
pub fn focus_tty(_tty: &str) -> bool {
    false
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
/// Replacing this process with `claude` is a unix idea; wasm has no process to replace.
#[cfg(target_arch = "wasm32")]
pub fn resume_in_place(_cwd: Option<&Path>, _id: &str) -> std::io::Error {
    std::io::Error::other("resume is not available in the browser")
}

#[cfg(not(target_arch = "wasm32"))]
pub fn resume_in_place(cwd: Option<&Path>, id: &str) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new("claude");
    cmd.arg("--resume").arg(id);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.exec() // only returns on failure
}

/// `resume_in_place` with nothing to resume: hand THIS terminal to a fresh `claude` in `cwd`.
/// The same contract — restore the terminal first, and it returns only on failure.
#[cfg(target_arch = "wasm32")]
pub fn start_in_place(_cwd: &Path) -> std::io::Error {
    std::io::Error::other("starting claude is not available in the browser")
}

#[cfg(not(target_arch = "wasm32"))]
pub fn start_in_place(cwd: &Path) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    Command::new("claude").current_dir(cwd).exec() // only returns on failure
}

/// The command to print when we cannot launch claude ourselves.
pub fn manual_command(cwd: Option<&Path>, id: &str) -> String {
    let dir = cwd.map(|c| c.display().to_string()).unwrap_or_else(|| ".".into());
    format!("cd -- {} && claude --resume {}", shell_quote(&dir), shell_quote(id))
}

/// The command to print when we cannot start a fresh claude ourselves.
pub fn manual_start_command(cwd: &Path) -> String {
    format!("cd -- {} && claude", shell_quote(&cwd.display().to_string()))
}

/// One shell word per argument, for the places that take a command line rather than an argv.
#[cfg(any(target_os = "macos", test))]
fn shell_join(args: &[String]) -> String {
    args.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ")
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

    #[cfg(unix)]
    #[test]
    fn the_new_window_script_resumes_in_the_sessions_folder() {
        // Run the exact -c script Ghostty is handed, from the wrong folder, with a stand-in
        // `claude` that reports where it started and what it was asked to resume.
        use std::os::unix::fs::PermissionsExt;
        let tmp = std::env::temp_dir().join(format!("sessio-launch-{}", std::process::id()));
        let bin = tmp.join("bin");
        let cwd = tmp.join("a b");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let fake = bin.join("claude");
        std::fs::write(&fake, "#!/bin/sh\npwd -P\necho \"$@\"\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let args = window_command("/bin/sh", &cwd, Some("abc-123"));
        let c = args.iter().position(|a| a == "-c").unwrap();
        let out = Command::new("/bin/sh")
            .args(&args[c..])
            .current_dir("/")
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output()
            .unwrap();
        let want = cwd.canonicalize().unwrap();
        let _ = std::fs::remove_dir_all(&tmp);

        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some(want.to_string_lossy().as_ref()));
        assert_eq!(lines.next(), Some("--resume abc-123"));
    }

    #[cfg(unix)]
    #[test]
    fn the_window_command_line_splits_back_into_the_same_argv() {
        // Ghostty takes the new window's command as one string and hands it to a shell. Whatever
        // the folder is called, that shell must see exactly the argv we built — checked by having
        // a shell split it and print each word, so nothing is ever launched.
        let cwd = Path::new("/tmp/it's a \"dir\" $HOME `x`");
        for id in [Some("abc-123"), None] {
            let argv = window_command("/bin/zsh", cwd, id);
            let out = Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("printf '%s\\n' {}", shell_join(&argv)))
                .output()
                .unwrap();
            let words: Vec<String> =
                String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect();
            assert_eq!(words, argv, "id = {id:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_fresh_window_script_starts_a_new_claude_in_the_folder() {
        // ^n: the same launch as ^o, with nothing to resume. It must land in the folder and hand
        // claude no arguments at all — a stray `--resume` would reopen some other session.
        use std::os::unix::fs::PermissionsExt;
        let tmp = std::env::temp_dir().join(format!("sessio-fresh-{}", std::process::id()));
        let bin = tmp.join("bin");
        let cwd = tmp.join("a b");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let fake = bin.join("claude");
        std::fs::write(&fake, "#!/bin/sh\npwd -P\necho \"args:$#\"\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let args = window_command("/bin/sh", &cwd, None);
        let c = args.iter().position(|a| a == "-c").unwrap();
        let out = Command::new("/bin/sh")
            .args(&args[c..])
            .current_dir("/")
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output()
            .unwrap();
        let want = cwd.canonicalize().unwrap();
        let _ = std::fs::remove_dir_all(&tmp);

        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some(want.to_string_lossy().as_ref()));
        assert_eq!(lines.next(), Some("args:0"));
        assert_eq!(
            manual_start_command(Path::new("/tmp/it's")),
            r"cd -- '/tmp/it'\''s' && claude"
        );
    }

    #[test]
    fn manual_command_is_copy_pasteable() {
        let cmd = manual_command(Some(Path::new("/tmp/a b")), "abc-123");
        assert_eq!(cmd, "cd -- '/tmp/a b' && claude --resume 'abc-123'");
    }
}
