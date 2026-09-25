//! Telling you when a running session starts waiting on you.
//!
//! The dashboard already marks a waiting session with `◆`, but only for as long as you are looking
//! at the dashboard. A session parked on a question in a window you have forgotten is exactly the
//! one you are not looking at, so the flip to `waiting` is posted as a macOS notification too.
//!
//! The transition logic is pure and lives here so it can be tested without posting anything; only
//! [`post`] touches the system, and only the terminal build has it.

use crate::live::LiveMap;

/// Longest session title a notification carries, in characters.
pub const TITLE_MAX: usize = 60;

/// Sessions waiting on you in `now` that were not waiting in `prev`: either not running at all
/// then, or running with some other status. Sorted, so a summary names them in a stable order.
///
/// A session still waiting from the last snapshot is not in here, so one wait is one notification
/// however many refreshes it lasts. One that stops waiting and later waits again is in here again.
pub fn newly_waiting(prev: &LiveMap, now: &LiveMap) -> Vec<String> {
    let mut ids: Vec<String> = now
        .iter()
        .filter(|(id, l)| l.needs_you() && !prev.get(*id).is_some_and(crate::live::Live::needs_you))
        .map(|(id, _)| id.clone())
        .collect();
    ids.sort();
    ids
}

/// Whether notifications are on. They are unless `SESSIO_NOTIFY=0`.
pub fn enabled() -> bool {
    std::env::var_os("SESSIO_NOTIFY").is_none_or(|v| v != "0")
}

/// The notification text for a tick in which `waiting` flipped to waiting, as `(title, reason)`
/// pairs. One session is named; several in the same tick are one notification that counts them,
/// rather than a stack of banners arriving together. `None` when nothing flipped.
pub fn message(waiting: &[(String, String)]) -> Option<String> {
    match waiting {
        [] => None,
        [(title, why)] => {
            let mut m = format!("◆ {} is waiting on you", clip(title, TITLE_MAX));
            let why = why.trim();
            if !why.is_empty() {
                m.push_str(" · ");
                m.push_str(why);
            }
            Some(m)
        }
        many => Some(format!("◆ {} sessions are waiting on you", many.len())),
    }
}

/// `s` cut to at most `max` characters, with `…` marking the cut.
fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The shell command a click on a notification runs: bring forward the Ghostty terminal on `tty`,
/// the same AppleScript `↵` uses on a running session. `None` unless `tty` is a plain terminal
/// device, since it is spliced into a shell command line.
pub fn focus_command(tty: &str) -> Option<String> {
    let dev = if tty.starts_with("/dev/") { tty.to_string() } else { format!("/dev/{tty}") };
    let name = dev.strip_prefix("/dev/")?;
    let safe = !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'/');
    if !safe {
        return None;
    }
    Some(format!(
        "osascript -e 'tell application \"Ghostty\" to focus (first terminal whose tty is \"{dev}\")' \
         -e 'tell application \"Ghostty\" to activate'"
    ))
}

/// Post `message` as a notification titled `sessio`, without waiting for it. `tty` is the terminal
/// of the one session it names, if any: clicking the notification then brings that window forward.
///
/// On macOS through [terminal-notifier](https://github.com/julienXX/terminal-notifier) when it is
/// installed, the only route whose click can run something. Without it, through `osascript`, whose
/// click only opens Script Editor; both strings are passed as arguments to the script rather than
/// spliced into it, so a session title cannot close a string literal and run AppleScript of its own.
/// The child is reaped on a thread of its own so the dashboard never stalls behind it. Anywhere else
/// it rings the terminal bell.
#[cfg(not(target_arch = "wasm32"))]
pub fn post(message: &str, tty: Option<&str>) {
    #[cfg(target_os = "macos")]
    {
        use std::process::{Command, Stdio};
        const SCRIPT: &str = r#"on run argv
  display notification (item 2 of argv) with title (item 1 of argv)
end run"#;
        let mut cmd = match terminal_notifier() {
            Some(bin) => {
                let mut c = Command::new(bin);
                c.args(["-title", "sessio", "-message", message, "-group", "sessio"]);
                if let Some(icon) = icon_file() {
                    let url = format!("file://{}", icon.display());
                    c.args(["-appIcon", &url, "-contentImage", &url]);
                }
                match tty.and_then(focus_command) {
                    Some(focus) => c.args(["-execute", &focus]),
                    None => c.args(["-activate", "com.mitchellh.ghostty"]),
                };
                c
            }
            None => {
                let mut c = Command::new("osascript");
                c.args(["-e", SCRIPT, "sessio", message]);
                c
            }
        };
        let child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        if let Ok(mut child) = child {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        use std::io::Write;
        let _ = (message, tty);
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x07");
        let _ = out.flush();
    }
}

/// sessio's icon as a file terminal-notifier can show, written to the cache folder the first time
/// (and again if it goes missing). It ships inside the binary, so npm installs need nothing extra.
/// `None` if it cannot be written; the notification then goes out without it.
#[cfg(target_os = "macos")]
fn icon_file() -> Option<std::path::PathBuf> {
    const ICON: &[u8] = include_bytes!("../docs/brand/icon-256.png");
    let dir = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Library/Caches/sessio");
    let path = dir.join(format!("icon-{}.png", env!("CARGO_PKG_VERSION")));
    if !path.is_file() {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&path, ICON).ok()?;
    }
    Some(path)
}

/// `terminal-notifier` on `PATH`, or where Homebrew puts it: a dashboard started from a launcher
/// may not have Homebrew on its `PATH`.
#[cfg(target_os = "macos")]
fn terminal_notifier() -> Option<std::path::PathBuf> {
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join("terminal-notifier")).collect::<Vec<_>>())
        .unwrap_or_default();
    let brew = ["/opt/homebrew/bin/terminal-notifier", "/usr/local/bin/terminal-notifier"].map(Into::into);
    on_path.into_iter().chain(brew).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::Live;

    fn live(status: &str) -> Live {
        Live { pid: 1, tty: String::new(), status: status.into(), waiting_for: String::new() }
    }

    fn map(rows: &[(&str, &str)]) -> LiveMap {
        rows.iter().map(|(id, s)| (id.to_string(), live(s))).collect()
    }

    #[test]
    fn the_first_snapshot_seeds_silently() {
        // The dashboard's first map is taken at startup and becomes `prev` for the first refresh,
        // so a session already waiting then is the same wait on both sides: nothing to say.
        let start = map(&[("a", "waiting")]);
        assert!(newly_waiting(&start, &start.clone()).is_empty());
    }

    #[test]
    fn a_session_that_starts_waiting_is_reported_once() {
        let busy = map(&[("a", "busy")]);
        let waiting = map(&[("a", "waiting")]);
        assert_eq!(newly_waiting(&busy, &waiting), vec!["a"]);
        assert!(newly_waiting(&waiting, &waiting.clone()).is_empty(), "still waiting: no repeat");
    }

    #[test]
    fn a_new_process_already_waiting_counts() {
        assert_eq!(newly_waiting(&LiveMap::new(), &map(&[("a", "waiting")])), vec!["a"]);
    }

    #[test]
    fn leaving_and_coming_back_is_a_second_wait() {
        let waiting = map(&[("a", "waiting")]);
        let busy = map(&[("a", "busy")]);
        assert!(newly_waiting(&waiting, &busy).is_empty());
        assert_eq!(newly_waiting(&busy, &waiting), vec!["a"]);
        // Gone entirely (the process exited) and back is the same.
        assert_eq!(newly_waiting(&LiveMap::new(), &waiting), vec!["a"]);
    }

    #[test]
    fn only_waiting_is_worth_a_notification() {
        let prev = map(&[("a", "busy")]);
        let now = map(&[("a", "idle"), ("b", "shell"), ("c", "")]);
        assert!(newly_waiting(&prev, &now).is_empty());
    }

    #[test]
    fn several_in_one_tick_are_sorted() {
        let now = map(&[("c", "waiting"), ("a", "waiting"), ("b", "waiting")]);
        assert_eq!(newly_waiting(&LiveMap::new(), &now), vec!["a", "b", "c"]);
    }

    #[test]
    fn one_session_is_named_and_says_why() {
        let one = |t: &str, w: &str| message(&[(t.into(), w.into())]).unwrap();
        assert_eq!(one("fix the login bug", ""), "◆ fix the login bug is waiting on you");
        assert_eq!(
            one("fix the login bug", "input needed"),
            "◆ fix the login bug is waiting on you · input needed"
        );
        let long = "x".repeat(100);
        let m = one(&long, "");
        assert!(m.contains(&format!("{}…", "x".repeat(TITLE_MAX - 1))), "{m}");
        assert!(!m.contains(&"x".repeat(TITLE_MAX)), "clipped to {TITLE_MAX}: {m}");
    }

    #[test]
    fn a_click_focuses_only_a_plain_tty() {
        let c = focus_command("ttys019").unwrap();
        assert!(c.contains(r#"whose tty is "/dev/ttys019""#), "{c}");
        assert_eq!(focus_command("/dev/ttys019"), Some(c));
        assert!(focus_command("/dev/pts/3").is_some());
        for bad in ["", "/dev/", "ttys0'; rm -rf ~", "ttys0\" & x", "ttys 1", "../x"] {
            assert_eq!(focus_command(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn several_are_one_summary() {
        let two = [("a".into(), String::new()), ("b".into(), "input needed".into())];
        assert_eq!(message(&two).unwrap(), "◆ 2 sessions are waiting on you");
        assert_eq!(message(&[]), None);
    }
}
