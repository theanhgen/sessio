//! `^k` and `sessions kill`: end a running session nobody has touched in two days.
//!
//! A `claude` left open in a forgotten window keeps its process, its MCP servers and its socket
//! alive indefinitely, and marks the session `◉` so every resume has to be forced. Ending one is
//! destructive in exactly one way — a turn in progress is lost — so the rules are narrow: the
//! session must be running, its transcript untouched for more than 48 hours, and it must be
//! neither mid-turn (`busy`) nor parked on a question (`waiting`). Everything else is refused, and
//! says why.
//!
//! The rule is a pure function so it can be tested at its edges without a process in sight. The
//! part that sends a signal re-checks everything against a fresh `ps` immediately before it does,
//! because the dashboard's picture of the world can be two seconds old and a pid can be recycled.

use crate::live::Live;

/// How long a running session's transcript must have been untouched before it may be ended.
pub const STALE_MS: i64 = 48 * 3600 * 1000;

const DAY_MS: i64 = 86_400_000;

/// Whether a session may be ended, and if not, why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Running, idle, and untouched for longer than [`STALE_MS`].
    Stale { idle_ms: i64 },
    /// No `claude` is attached: there is nothing to end.
    NotRunning,
    /// Running, but written to within the last 48 hours.
    Recent { idle_ms: i64 },
    /// Mid-turn. Ending it would throw the turn away.
    Busy,
    /// Parked on a question for the user. That is a session to answer, not to end.
    Waiting,
}

/// The rule. `mtime` is when the transcript was last written, `now` the current time, both in ms.
///
/// The status checks come before the age check on purpose: a `waiting` session untouched for a
/// week is still one the user has been asked something in, and "waiting on you" is the more
/// useful thing to hear than any age.
pub fn verdict(live: Option<&Live>, mtime: i64, now: i64) -> Verdict {
    let Some(l) = live else { return Verdict::NotRunning };
    if l.status == "busy" {
        return Verdict::Busy;
    }
    if l.needs_you() {
        return Verdict::Waiting;
    }
    let idle_ms = now - mtime;
    if idle_ms > STALE_MS {
        Verdict::Stale { idle_ms }
    } else {
        Verdict::Recent { idle_ms }
    }
}

impl Verdict {
    pub fn is_stale(&self) -> bool {
        matches!(self, Verdict::Stale { .. })
    }

    /// Why this session may not be ended, or `None` if it may.
    pub fn refusal(&self) -> Option<String> {
        match self {
            Verdict::Stale { .. } => None,
            Verdict::NotRunning => Some("not running — there is nothing to end".into()),
            Verdict::Recent { idle_ms } => Some(format!(
                "active {} ago — only a session idle for more than 48h can be ended",
                span(*idle_ms)
            )),
            Verdict::Busy => Some("busy — it is in the middle of a turn".into()),
            Verdict::Waiting => Some("waiting on you — answer it rather than end it".into()),
        }
    }
}

/// Whole days, rounded down: "idle 2d" must never describe a session idle for 47 hours.
pub fn idle_days(idle_ms: i64) -> i64 {
    idle_ms.max(0) / DAY_MS
}

/// A duration in minutes, hours, then days, rounded down. Hours run to 48 rather than 24 so a
/// refusal near the line reads "active 47h ago", not a "1d" that hides how close it is.
fn span(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    if s < 3600 {
        format!("{}m", (s / 60).max(1))
    } else if s < 2 * 86_400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    }
}

/// Whether `ps` describes a Claude Code process. `comm` is the executable, `args` the whole argv.
///
/// A bare `claude` or an absolute path to one counts, and so does a native install, which execs
/// its versioned binary directly (`…/claude/versions/2.1.259`) so neither name is `claude`.
pub fn is_claude(comm: &str, args: &str) -> bool {
    let argv0 = args.split_whitespace().next().unwrap_or("");
    let named = |s: &str| s.trim().rsplit('/').next() == Some("claude");
    named(comm) || named(argv0) || argv0.contains("/claude/versions/") || comm.contains("/claude/versions/")
}

/// Every process from `start` up to the root, following `parent`. Bounded, so a cycle in a
/// misread process table cannot hang the caller.
pub fn ancestry(start: i32, parent: impl Fn(i32) -> Option<i32>) -> Vec<i32> {
    let mut chain = vec![start];
    let mut cur = start;
    for _ in 0..128 {
        match parent(cur) {
            Some(p) if p > 0 && !chain.contains(&p) => {
                chain.push(p);
                cur = p;
            }
            _ => break,
        }
    }
    chain
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::*;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    /// How long to wait for a signalled process to go before reporting it as still running.
    pub const GRACE: Duration = Duration::from_secs(3);

    /// One `ps` field for one pid, or `None` if the pid is gone.
    fn ps_field(pid: i32, field: &str) -> Option<String> {
        let out = Command::new("ps")
            .args(["-o", &format!("{field}="), "-p", &pid.to_string()])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    /// Whether `pid` is still a running process. A zombie has already exited — it is only
    /// waiting for its parent to collect it — so it counts as gone.
    fn alive(pid: i32) -> bool {
        ps_field(pid, "stat").is_some_and(|s| !s.starts_with('Z'))
    }

    /// This process and every process above it. sessio is started from a shell, and that shell
    /// may belong to a `claude` — the Bash tool of an agent running `sessions kill`, say. Ending
    /// that one would end the thing that asked.
    pub fn own_ancestry() -> Vec<i32> {
        super::ancestry(std::process::id() as i32, |p| {
            ps_field(p, "ppid").and_then(|s| s.parse().ok())
        })
    }

    /// Why `pid` must not be signalled as session `id`, or `Ok` if it may. Everything here is
    /// read fresh: the registry and `ps` again, the process's own command, and our ancestry.
    pub fn verify(id: &str, pid: i32) -> Result<(), String> {
        if pid <= 1 {
            return Err(format!("refusing to signal pid {pid}"));
        }
        match crate::live::scan().get(id) {
            Some(l) if l.pid == pid => {}
            Some(l) => return Err(format!("the session moved to pid {} — look again", l.pid)),
            None => return Err("it is no longer running".into()),
        }
        let comm = ps_field(pid, "comm").unwrap_or_default();
        let args = ps_field(pid, "args").unwrap_or_default();
        if !super::is_claude(&comm, &args) {
            return Err(format!("pid {pid} is not a claude process any more"));
        }
        if own_ancestry().contains(&pid) {
            return Err("sessio is running inside that session — end it from its own window".into());
        }
        Ok(())
    }

    /// What happened to a process that was sent SIGTERM.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Outcome {
        Ended,
        StillRunning,
    }

    /// SIGTERM, then wait up to [`GRACE`] for it to go. No SIGKILL: a `claude` that ignores a
    /// polite request is one a person should look at.
    pub fn terminate(pid: i32) -> Result<Outcome, String> {
        let st = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("couldn't run kill: {e}"))?;
        if !st.status.success() {
            let why = String::from_utf8_lossy(&st.stderr).trim().to_string();
            return Err(if why.is_empty() { format!("kill failed ({})", st.status) } else { why });
        }
        let until = Instant::now() + GRACE;
        while Instant::now() < until {
            if !alive(pid) {
                return Ok(Outcome::Ended);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(if alive(pid) { Outcome::StillRunning } else { Outcome::Ended })
    }

    /// Verify, then terminate. The one entry point both the dashboard and the CLI use.
    pub fn end(id: &str, pid: i32) -> Result<Outcome, String> {
        verify(id, pid)?;
        terminate(pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;
    const H: i64 = 3600 * 1000;

    fn live(status: &str) -> Live {
        Live { pid: 4242, tty: "ttys009".into(), status: status.into(), waiting_for: String::new() }
    }

    #[test]
    fn the_boundary_is_strictly_more_than_48_hours() {
        let l = live("idle");
        assert_eq!(verdict(Some(&l), NOW - 48 * H, NOW), Verdict::Recent { idle_ms: 48 * H });
        assert_eq!(verdict(Some(&l), NOW - 48 * H - 1, NOW), Verdict::Stale { idle_ms: 48 * H + 1 });
        assert!(verdict(Some(&l), NOW - 47 * H, NOW).refusal().unwrap().contains("47h ago"));
    }

    #[test]
    fn a_session_that_is_not_running_is_refused() {
        let v = verdict(None, NOW - 30 * 24 * H, NOW);
        assert_eq!(v, Verdict::NotRunning);
        assert!(v.refusal().unwrap().contains("not running"));
    }

    #[test]
    fn busy_and_waiting_are_refused_however_old() {
        let old = NOW - 10 * 24 * H;
        assert_eq!(verdict(Some(&live("busy")), old, NOW), Verdict::Busy);
        assert_eq!(verdict(Some(&live("waiting")), old, NOW), Verdict::Waiting);
        assert!(Verdict::Waiting.refusal().unwrap().contains("waiting on you"));
        assert!(Verdict::Busy.refusal().unwrap().contains("busy"));
    }

    #[test]
    fn idle_shell_and_unknown_statuses_are_eligible_once_stale() {
        for s in ["idle", "shell", ""] {
            assert!(verdict(Some(&live(s)), NOW - 3 * 24 * H, NOW).is_stale(), "{s:?}");
        }
    }

    #[test]
    fn a_transcript_from_the_future_is_recent() {
        // Clock skew must never make a session look abandoned.
        assert!(!verdict(Some(&live("idle")), NOW + H, NOW).is_stale());
    }

    #[test]
    fn idle_days_round_down() {
        assert_eq!(idle_days(48 * H + 1), 2);
        assert_eq!(idle_days(71 * H), 2);
        assert_eq!(idle_days(72 * H), 3);
    }

    #[test]
    fn claude_is_recognised_however_it_was_launched() {
        assert!(is_claude("claude", "claude --resume x"));
        assert!(is_claude("/opt/homebrew/bin/claude", "/opt/homebrew/bin/claude"));
        assert!(is_claude(
            "/Users/x/.local/share/claude/versions/2.1.259",
            "/Users/x/.local/share/claude/versions/2.1.259 --session-id y"
        ));
        assert!(is_claude("2.1.259", "/Users/x/.local/share/claude/versions/2.1.259"));
        assert!(!is_claude("vim", "vim claude"));
        assert!(!is_claude("/bin/sleep", "sleep 60"));
        assert!(!is_claude("", ""));
    }

    #[test]
    fn ancestry_walks_to_the_root_and_survives_a_cycle() {
        let parents = |p: i32| match p {
            50 => Some(40),
            40 => Some(1),
            1 => Some(0),
            _ => None,
        };
        assert_eq!(ancestry(50, parents), vec![50, 40, 1]);
        let cyc = |p: i32| Some(if p == 7 { 8 } else { 7 });
        assert_eq!(ancestry(7, cyc), vec![7, 8]);
    }
}
