//! "Is this session already running somewhere?"
//!
//! Resuming a live session points a second `claude` at the same transcript, which both processes
//! then append to. sessio cannot stop Claude Code from allowing that, but it can refuse to be the
//! thing that starts it by accident.
//!
//! The mapping is argv-based: a session that was resumed carries `--resume <id>`, so the id is
//! right there in the process. A session started as a bare `claude` has its id nowhere the outside
//! world can see — not in argv, not in the environment, and the transcript file is not held open,
//! so `lsof` on it finds nothing either. Those stay invisible here. Guarding the cases we can
//! prove beats guessing at the rest.

use std::collections::HashMap;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Live {
    pub pid: i32,
    /// The controlling terminal, e.g. `ttys013`, or empty when the process has none.
    pub tty: String,
}

/// Session id -> the process running it.
pub type LiveMap = HashMap<String, Live>;

/// Parse `ps -Ao pid=,tty=,args=` output into a session-id map.
pub fn parse_ps(out: &str) -> LiveMap {
    let mut map = LiveMap::new();
    for line in out.lines() {
        let mut parts = line.split_whitespace();
        let (Some(pid), Some(tty)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Ok(pid) = pid.parse::<i32>() else { continue };
        let args: Vec<&str> = parts.collect();
        // The command must actually be claude — not a grep for it, not an editor holding the
        // string in its title.
        let Some(cmd) = args.first() else { continue };
        if cmd.rsplit('/').next() != Some("claude") {
            continue;
        }
        if let Some(i) = args.iter().position(|a| *a == "--resume") {
            if let Some(id) = args.get(i + 1).filter(|id| is_session_id(id)) {
                // First wins: if the same session somehow has two processes, the guard only needs
                // to name one of them.
                map.entry((*id).to_string()).or_insert(Live {
                    pid,
                    tty: if tty == "??" { String::new() } else { tty.to_string() },
                });
            }
        }
    }
    map
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

/// Snapshot the live sessions. Returns an empty map if `ps` is unavailable or misbehaves —
/// the guard is an improvement when it fires, never a prerequisite.
pub fn scan() -> LiveMap {
    let Ok(out) = Command::new("ps")
        .args(["-Ao", "pid=,tty=,args="])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return LiveMap::new();
    };
    parse_ps(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = "\
 3415 ttys003  claude --resume 832b0138-795b-496d-a496-f09899048be6
68227 ttys013  claude --resume 6efcc19e-55b3-4f53-9f5a-8758fc9c58b5
73921 ttys018  claude --dangerously-skip-permissions
80338 ??       /Users/x/.local/share/claude/versions/2.1.235 --chrome-native-host
99999 ttys001  vim notes-about-claude---resume-40bb4a73-3331-4207-9335-6451fb96bd8f.md
12345 ttys002  /opt/homebrew/bin/claude --resume 40bb4a73-3331-4207-9335-6451fb96bd8f";

    #[test]
    fn maps_resumed_sessions_to_their_process() {
        let m = parse_ps(PS);
        assert_eq!(
            m.get("6efcc19e-55b3-4f53-9f5a-8758fc9c58b5"),
            Some(&Live { pid: 68227, tty: "ttys013".into() })
        );
        // An absolute path to claude still counts.
        assert_eq!(m.get("40bb4a73-3331-4207-9335-6451fb96bd8f").map(|l| l.pid), Some(12345));
    }

    #[test]
    fn ignores_processes_that_merely_mention_a_session() {
        let m = parse_ps(PS);
        // vim editing a file whose *name* contains an id must not register as that session.
        assert!(m.values().all(|l| l.pid != 99999));
        // A bare `claude` has no id to map, and the native host is not a session at all.
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn rejects_argv_that_is_not_a_uuid() {
        let m = parse_ps("1 ttys000  claude --resume ../../etc/passwd\n2 ttys000  claude --resume");
        assert!(m.is_empty());
    }

    #[test]
    fn a_process_with_no_terminal_has_no_tty() {
        let m = parse_ps("7 ??  claude --resume 832b0138-795b-496d-a496-f09899048be6");
        assert_eq!(m["832b0138-795b-496d-a496-f09899048be6"].tty, "");
    }
}
