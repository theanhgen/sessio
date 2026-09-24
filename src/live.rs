//! "Is this session already running somewhere?"
//!
//! Resuming a live session points a second `claude` at the same transcript, which both processes
//! then append to. sessio cannot stop Claude Code from allowing that, but it can refuse to be the
//! thing that starts it by accident.
//!
//! Two sources, merged, because neither is complete on its own:
//!
//! * `~/.claude/sessions/<pid>.json` — a registry Claude Code keeps for every running session,
//!   holding its `sessionId`, `procStart` and current `status`. This is what `claude agents`
//!   reads, and reading it directly is the difference between tens of milliseconds and the 0.4-14s
//!   that shelling out to `claude agents --json` costs — that command opens every session's socket
//!   in turn, so it slows down as sessions pile up and swings wildly with how busy they are.
//!   Nowhere near a 2s refresh. The registry also covers sessions started as a bare `claude`,
//!   which argv cannot.
//! * `ps` — proves the pid is alive, supplies the tty, and still maps `--resume <id>` from argv
//!   for a Claude Code old enough to keep no registry.
//!
//! The registry is not cleaned up when a session dies abnormally, and a pid can be recycled, so a
//! row is only believed when `ps` agrees. What it must NOT do is check that the process is named
//! `claude`: a native install execs its versioned binary directly, so argv[0] is the version
//! (`2.1.259`) and that test silently drops every native session — a false negative in exactly the
//! direction this guard exists to prevent. `procStart` is the check that works regardless of how
//! the binary was launched.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Live {
    pub pid: i32,
    /// The controlling terminal, e.g. `ttys013`, or empty when the process has none.
    pub tty: String,
    /// What the session is doing, straight from the registry: `idle`, `busy`, `shell` and
    /// `waiting` have all been observed, so this stays a string rather than an enum that a new
    /// Claude Code could invalidate. Empty when only argv proved the session is running, and
    /// empty for a moment after a session starts — the row is written before the status is.
    pub status: String,
    /// What a `waiting` session is waiting for, e.g. `input needed`. Empty on every other status.
    pub waiting_for: String,
}

impl Live {
    /// Whether this session has stopped and is waiting on a person.
    ///
    /// The one status worth interrupting someone for. `busy` needs nothing, `idle` needs nothing
    /// yet, but `waiting` means a session is parked until you go and look at it — and a session
    /// parked in a window you have forgotten is the failure this exists to catch.
    pub fn needs_you(&self) -> bool {
        self.status == "waiting"
    }
}

/// Session id -> the process running it.
pub type LiveMap = HashMap<String, Live>;

/// One live process, as `ps` sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    pub tty: String,
    /// `ps lstart`, e.g. `Wed Sep  2 21:04:10 2026`, in local time.
    pub lstart: String,
}

/// What one `ps` sweep tells us.
pub struct Ps {
    /// Sessions identified from `--resume <id>` in argv.
    pub by_id: LiveMap,
    /// Every live process, by pid. Not just `claude` ones: the registry names sessions whose
    /// argv[0] is a version string, and they have to be found here.
    pub procs: HashMap<i32, Proc>,
}

/// One `~/.claude/sessions/<pid>.json`, reduced to what the guard needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub pid: i32,
    pub status: String,
    pub waiting_for: String,
    /// Process start as the registry recorded it, e.g. `Wed Sep  2 19:04:10 2026`. Compared
    /// against `ps lstart` to catch a recycled pid.
    pub proc_start: String,
}

/// `~/.claude/sessions`, honouring `CLAUDE_CONFIG_DIR` the same way the transcript scan does.
pub fn registry_root() -> PathBuf {
    let base = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".claude")))
        .unwrap_or_else(|| PathBuf::from(".claude"));
    base.join("sessions")
}

/// Parse `ps -Ao pid=,tty=,lstart=,args=`. `lstart` is always five tokens
/// (`Wed Sep  2 21:04:10 2026`), so argv starts at the eighth.
pub fn parse_ps(out: &str) -> Ps {
    let mut by_id = LiveMap::new();
    let mut procs = HashMap::new();
    for line in out.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 8 {
            continue;
        }
        let Ok(pid) = f[0].parse::<i32>() else { continue };
        let tty = if f[1] == "??" { String::new() } else { f[1].to_string() };
        let lstart = f[2..7].join(" ");
        let args = &f[7..];
        procs.insert(pid, Proc { tty: tty.clone(), lstart });

        // The argv path is only safe while the command really is claude — otherwise an editor
        // holding a session id in a filename registers as that session.
        if args[0].rsplit('/').next() != Some("claude") {
            continue;
        }
        if let Some(i) = args.iter().position(|a| *a == "--resume") {
            if let Some(id) = args.get(i + 1).filter(|id| is_session_id(id)) {
                // First wins: if the same session somehow has two processes, the guard only needs
                // to name one of them.
                by_id.entry((*id).to_string()).or_insert(Live {
                    pid,
                    tty,
                    status: String::new(),
                    waiting_for: String::new(),
                });
            }
        }
    }
    Ps { by_id, procs }
}

/// The registry row shape varies by session kind and Claude Code version, so serde takes the five
/// fields the guard needs and ignores the rest. `status` is optional because a session gets a row
/// slightly before it gets a status.
#[derive(serde::Deserialize)]
struct Entry {
    pid: i32,
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "waitingFor")]
    waiting_for: Option<String>,
    #[serde(default, rename = "procStart")]
    proc_start: Option<String>,
}

/// One registry file, or `None` if it is not a session row.
pub fn parse_registry(json: &str) -> Option<Row> {
    let e: Entry = serde_json::from_str(json).ok()?;
    if !is_session_id(&e.session_id) {
        return None;
    }
    Some(Row {
        id: e.session_id,
        pid: e.pid,
        status: e.status.unwrap_or_default(),
        waiting_for: e.waiting_for.unwrap_or_default(),
        proc_start: e.proc_start.unwrap_or_default(),
    })
}

/// `Wed Sep  2 21:04:10 2026` -> a naive timestamp.
///
/// The weekday is dropped rather than parsed. chrono validates `%a` against the date and refuses
/// a mismatch, which this module would read as "cannot tell" and wave through — so a wrong
/// weekday disables the check instead of failing. The date already determines the weekday, so
/// there is nothing to read it for. Note that Python's `strptime` accepts a mismatched weekday
/// silently, which makes a fixture verified in a probe script no evidence at all about chrono.
///
/// Splitting on whitespace also absorbs the padding in front of a single-digit day.
fn parse_ctime(s: &str) -> Option<chrono::NaiveDateTime> {
    let norm = s.split_whitespace().skip(1).collect::<Vec<_>>().join(" ");
    chrono::NaiveDateTime::parse_from_str(&norm, "%b %d %H:%M:%S %Y").ok()
}

/// Whether a registry row and a `ps` row describe the *same* process start.
///
/// They are not directly comparable: `ps lstart` is local time, and every row on this machine sits
/// exactly 7200s from its `procStart`. That is consistent with the registry writing UTC, but it is
/// not evidence of it — one host in one timezone cannot tell "written in UTC" apart from "written
/// under some other rule that happens to differ from local by two hours here". So this does not
/// convert between them at all. It leans on the one thing every timezone offset has in common: it
/// is a whole number of quarter-hours. A recycled pid started at an unrelated moment lands on that
/// grid about 0.4% of the time; a genuine session always does. The blind spot is exact multiples
/// of the grid: a day is 96 quarter-hours, so a process that started the same second on an earlier
/// day is indistinguishable here. Nothing cheap distinguishes it either, and the cost is one
/// spurious "already running" warning, so it stays.
///
/// Not converting also sidesteps the fall-back hour, when a local wall-clock time maps to two
/// instants and any local->instant conversion has to guess which. An hour a year of correctness,
/// for free. (The DST *transition* itself is not the problem a fixed offset makes it look like:
/// `lstart` renders under the rule in force at each process's own start, which is exactly what a
/// proper tz-aware conversion inverts. It is the ambiguity that has no right answer.)
///
/// The tolerance is two-sided — `>= 898` as well as `<= 2` — because a delta may miss the grid
/// from either direction, and `abs()` comes before the modulo because Rust's `%` keeps the sign of
/// the dividend, so `-7199 % 900` is `-899`, which would slip past a `<= 2` test as a false accept.
///
/// Either timestamp being unreadable means "cannot tell", and cannot-tell must not reject: a
/// guard that stops firing is worse than one that occasionally fires early.
pub fn starts_agree(proc_start: &str, lstart: &str) -> bool {
    let (Some(a), Some(b)) = (parse_ctime(proc_start), parse_ctime(lstart)) else {
        return true;
    };
    let off = (b - a).num_seconds().abs() % 900;
    off <= 2 || off >= 898
}

/// Combine the two sources. Registry rows win over argv rows — they carry a status, and they see
/// sessions argv cannot — but only where `ps` still has that pid running the same process.
pub fn merge(ps: Ps, rows: Vec<Row>) -> LiveMap {
    let Ps { mut by_id, procs } = ps;
    for r in rows {
        // A row outliving its process, or a pid since handed to something else, is not a running
        // session.
        let Some(p) = procs.get(&r.pid) else { continue };
        if !starts_agree(&r.proc_start, &p.lstart) {
            continue;
        }
        by_id.insert(
            r.id,
            Live {
                pid: r.pid,
                tty: p.tty.clone(),
                status: r.status,
                waiting_for: r.waiting_for,
            },
        );
    }
    by_id
}

/// Transcript ids are UUIDs. Anything else in that argv slot is not a session.
fn is_session_id(s: &str) -> bool {
    s.len() == 36
        && s.as_bytes()
            .iter()
            .enumerate()
            .all(|(i, b)| match i {
                8 | 13 | 18 | 23 => *b == b'-',
                _ => b.is_ascii_hexdigit(),
            })
}

/// Read every registry row. A missing directory (Claude Code too old, or a config dir elsewhere)
/// yields nothing and leaves the argv guard to carry the load.
fn read_registry(dir: &Path) -> Vec<Row> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        // `<pid>.json` only — the directory also holds `<pid>.<hash>.key` files.
        if !name.ends_with(".json") || name.matches('.').count() != 1 {
            continue;
        }
        if let Ok(body) = std::fs::read_to_string(e.path()) {
            if let Some(row) = parse_registry(&body) {
                out.push(row);
            }
        }
    }
    out
}

/// Snapshot the live sessions. Returns an empty map if `ps` is unavailable or misbehaves —
/// the guard is an improvement when it fires, never a prerequisite.
pub fn scan() -> LiveMap {
    let Ok(out) = Command::new("ps")
        .args(["-Ao", "pid=,tty=,lstart=,args="])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return LiveMap::new();
    };
    let ps = parse_ps(&String::from_utf8_lossy(&out.stdout));
    merge(ps, read_registry(&registry_root()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "Wed Sep  2 21:04:10 2026";

    const PS: &str = "\
 3415 ttys003  Wed Sep  2 21:04:10 2026 claude --resume 832b0138-795b-496d-a496-f09899048be6
68227 ttys013  Wed Sep  2 21:04:10 2026 claude --resume 6efcc19e-55b3-4f53-9f5a-8758fc9c58b5
73921 ttys018  Wed Sep  2 21:04:10 2026 claude --dangerously-skip-permissions
55501 ttys021  Wed Sep  2 21:04:10 2026 /Users/x/.local/share/claude/versions/2.1.259 --session-id 57f54301-f795-4b11-bbee-3809357a5ec7
80338 ??       Wed Sep  2 21:04:10 2026 /Users/x/.local/share/claude/versions/2.1.235 --chrome-native-host
99999 ttys001  Wed Sep  2 21:04:10 2026 vim notes-about-claude---resume-40bb4a73-3331-4207-9335-6451fb96bd8f.md
12345 ttys002  Wed Sep  2 21:04:10 2026 /opt/homebrew/bin/claude --resume 40bb4a73-3331-4207-9335-6451fb96bd8f";

    fn row(id: &str, pid: i32, status: &str) -> Row {
        Row {
            id: id.into(),
            pid,
            status: status.into(),
            waiting_for: String::new(),
            // 21:04:10 local is 19:04:10 UTC — the +2h this machine actually reports.
            proc_start: "Wed Sep  2 19:04:10 2026".into(),
        }
    }

    #[test]
    fn maps_resumed_sessions_to_their_process() {
        let m = parse_ps(PS).by_id;
        assert_eq!(
            m.get("6efcc19e-55b3-4f53-9f5a-8758fc9c58b5"),
            Some(&Live {
                pid: 68227,
                tty: "ttys013".into(),
                status: String::new(),
                waiting_for: String::new()
            })
        );
        // An absolute path to claude still counts.
        assert_eq!(m.get("40bb4a73-3331-4207-9335-6451fb96bd8f").map(|l| l.pid), Some(12345));
    }

    #[test]
    fn ignores_processes_that_merely_mention_a_session() {
        let m = parse_ps(PS).by_id;
        // vim editing a file whose *name* contains an id must not register as that session.
        assert!(m.values().all(|l| l.pid != 99999));
        // A bare `claude` has no id to map, and the native host is not a session at all.
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn rejects_argv_that_is_not_a_uuid() {
        let m = parse_ps(&format!(
            "1 ttys000 {T} claude --resume ../../etc/passwd\n2 ttys000 {T} claude --resume x"
        ));
        assert!(m.by_id.is_empty());
    }

    #[test]
    fn a_process_with_no_terminal_has_no_tty() {
        let out = format!("7 ?? {T} claude --resume 832b0138-795b-496d-a496-f09899048be6");
        assert_eq!(parse_ps(&out).by_id["832b0138-795b-496d-a496-f09899048be6"].tty, "");
    }

    #[test]
    fn every_process_is_available_to_the_registry_not_just_the_ones_named_claude() {
        // 55501 is a native install: it execs the versioned binary, so argv[0] is `2.1.259`.
        // Requiring the name `claude` here dropped every native session from the guard.
        let procs = parse_ps(PS).procs;
        assert_eq!(procs[&55501].tty, "ttys021");
        assert_eq!(procs[&73921].tty, "ttys018", "a bare claude, named by the registry");
        assert!(procs.contains_key(&99999), "vim is a process; only argv matching excludes it");
    }

    const REG: &str = r#"{"pid":13461,"sessionId":"5db200c5-5765-4b51-9092-caf22dfdc21a",
        "cwd":"/w/kindle","kind":"interactive","name":"kindle-59","status":"shell",
        "procStart":"Wed Sep  2 19:07:23 2026","version":"2.1.258","startedAt":1788376044336}"#;

    #[test]
    fn a_registry_row_yields_what_the_guard_needs() {
        let r = parse_registry(REG).unwrap();
        assert_eq!(r.id, "5db200c5-5765-4b51-9092-caf22dfdc21a");
        assert_eq!((r.pid, r.status.as_str()), (13461, "shell"));
        assert_eq!(r.proc_start, "Wed Sep  2 19:07:23 2026");
    }

    #[test]
    fn a_waiting_row_says_what_it_is_waiting_for() {
        let j = r#"{"pid":9,"sessionId":"832b0138-795b-496d-a496-f09899048be6",
            "status":"waiting","waitingFor":"input needed"}"#;
        let r = parse_registry(j).unwrap();
        assert_eq!((r.status.as_str(), r.waiting_for.as_str()), ("waiting", "input needed"));
    }

    #[test]
    fn a_row_written_before_its_status_still_counts() {
        // Observed on the first poll after `claude --bg`: the row exists, `status` does not yet.
        // Refusing to deserialize here would blind the guard exactly when a session is new.
        let fresh = r#"{"pid":42,"sessionId":"832b0138-795b-496d-a496-f09899048be6"}"#;
        let r = parse_registry(fresh).unwrap();
        assert_eq!((r.pid, r.status.as_str()), (42, ""));
    }

    #[test]
    fn junk_in_the_registry_is_not_a_session() {
        assert_eq!(parse_registry("not json"), None);
        assert_eq!(parse_registry(r#"{"pid":1,"sessionId":"../../etc/passwd"}"#), None);
        assert_eq!(parse_registry(r#"{"cwd":"/w"}"#), None);
    }

    #[test]
    fn the_registry_sees_sessions_argv_cannot() {
        // 73921 is a bare `claude` and 55501 is a native install using --session-id. Neither is
        // reachable from argv; both are ordinary registry rows.
        let bare = "abcdef01-2345-4678-9abc-def012345678";
        let native = "57f54301-f795-4b11-bbee-3809357a5ec7";
        let m = merge(parse_ps(PS), vec![row(bare, 73921, "busy"), row(native, 55501, "idle")]);
        assert_eq!(m[bare].tty, "ttys018");
        assert_eq!(
            m[native],
            Live {
                pid: 55501,
                tty: "ttys021".into(),
                status: "idle".into(),
                waiting_for: String::new()
            }
        );
    }

    #[test]
    fn a_registry_row_whose_process_is_gone_is_not_running() {
        let id = "abcdef01-2345-4678-9abc-def012345678";
        let m = merge(parse_ps(PS), vec![row(id, 55555, "idle")]);
        assert!(!m.contains_key(id), "no such live pid");
    }

    #[test]
    fn a_recycled_pid_does_not_inherit_the_session() {
        // Same pid, but the process running now started at an unrelated moment.
        let id = "abcdef01-2345-4678-9abc-def012345678";
        let mut stale = row(id, 68227, "idle");
        stale.proc_start = "Wed Aug 12 04:31:57 2026".into();
        assert!(!merge(parse_ps(PS), vec![stale]).contains_key(id));
    }

    #[test]
    fn the_registry_wins_over_argv_because_it_knows_the_status() {
        let id = "6efcc19e-55b3-4f53-9f5a-8758fc9c58b5";
        let m = merge(parse_ps(PS), vec![row(id, 68227, "busy")]);
        assert_eq!(m[id].status, "busy");
        assert_eq!(m[id].tty, "ttys013", "still the tty ps reported");
    }

    #[test]
    fn any_whole_quarter_hour_offset_is_the_same_process() {
        let utc = "Wed Sep  2 19:04:10 2026";
        // Offsets in real use, in both directions, including the half- and quarter-hour ones.
        for local in [
            "Wed Sep  2 19:04:10 2026", // UTC itself
            "Wed Sep  2 21:04:10 2026", // +2  (measured on this machine)
            "Wed Sep  2 12:04:10 2026", // -7
            "Thu Sep  3 00:34:10 2026", // +5:30, over midnight
            "Thu Sep  3 04:49:10 2026", // +9:45
        ] {
            assert!(starts_agree(utc, local), "{local} is the same start as {utc}");
        }
    }

    #[test]
    fn a_start_off_the_quarter_hour_grid_is_a_different_process() {
        let utc = "Wed Sep  2 19:04:10 2026";
        assert!(!starts_agree(utc, "Wed Sep  2 21:07:41 2026"));
        assert!(!starts_agree(utc, "Wed Aug 12 04:31:57 2026"));
    }

    #[test]
    fn the_tolerance_is_two_sided() {
        // A delta can miss the grid from either direction. 7199s and 7201s are both one second
        // off a +2h offset, and only one of them is caught by a one-sided `% 900 <= 2`.
        let utc = "Wed Sep  2 19:04:10 2026";
        assert!(starts_agree(utc, "Wed Sep  2 21:04:09 2026"), "one second short");
        assert!(starts_agree(utc, "Wed Sep  2 21:04:11 2026"), "one second over");
        // Three seconds out on either side is past the tolerance and must not be waved through.
        assert!(!starts_agree(utc, "Wed Sep  2 21:04:07 2026"));
        assert!(!starts_agree(utc, "Wed Sep  2 21:04:13 2026"));
    }

    #[test]
    fn a_wrong_weekday_does_not_disable_the_check() {
        // Aug 12 2026 is a Wednesday. Parsing `%a` makes chrono reject this outright, which
        // `starts_agree` reads as "cannot tell" and accepts — silently turning the pid-reuse
        // guard off. Dropping the weekday means the timestamps are still compared, so this must
        // REJECT. Restore `%a` and this test fails loudly.
        //
        // The span has to be off the grid to prove anything, and a whole number of DAYS is not:
        // 86400 % 900 == 0, so "three weeks earlier, same clock time" would be accepted and this
        // test would pass for no reason. These differ by 21 days + 14:32:13, i.e. 1866733s,
        // 133 past a grid line.
        assert!(!starts_agree("Wed Sep  2 19:04:10 2026", "Tue Aug 12 04:31:57 2026"));
        // Same instant with the weekday corrected — the token is ignored either way.
        assert!(!starts_agree("Wed Sep  2 19:04:10 2026", "Wed Aug 12 04:31:57 2026"));
        // 450 past a grid line: the furthest a delta can sit from both tolerance edges, so
        // widening them cannot quietly turn this green.
        assert!(!starts_agree("Wed Sep  2 19:04:10 2026", "Wed Aug 12 04:41:40 2026"));
    }

    #[test]
    fn a_whole_number_of_days_is_the_known_blind_spot() {
        // 86400 % 900 == 0, so a process that started at the same clock time on an earlier day is
        // accepted as the same start. This is a real hole in the check, not an oversight — it is
        // documented on `starts_agree` — and it is here so that a fixture built from a round
        // number of days is never mistaken for a working negative test.
        assert!(starts_agree("Wed Sep  2 19:04:10 2026", "Wed Aug 12 19:04:10 2026"));
    }

    #[test]
    fn an_unreadable_timestamp_never_rejects() {
        // Cannot-tell must not become a "no": a guard that stops firing is the failure this whole
        // module exists to prevent. An old Claude Code writes no procStart at all.
        assert!(starts_agree("", "Wed Sep  2 21:04:10 2026"));
        assert!(starts_agree("Wed Sep  2 19:04:10 2026", "not a date"));
    }
}
