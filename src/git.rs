//! Background git-WIP detection. Port of `gitRefresh` / `gitDirty` at bin/sessio.mjs:176-195.
//!
//! The rule that matters: `dirty()` never blocks. It answers from cache immediately and kicks
//! off a refresh in the background when the entry is stale or missing, so a slow repo can't
//! stall a redraw. Callers pick up the fresh value on a later tick.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// WIP doesn't change fast; re-check at most this often.
const TTL: Duration = Duration::from_secs(30);
/// Bound on a single `git status`, so a huge or networked repo can't wedge a worker.
const TIMEOUT: Duration = Duration::from_secs(2);

struct State {
    cache: HashMap<String, (bool, Instant)>,
    inflight: HashSet<String>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(State {
            cache: HashMap::new(),
            inflight: HashSet::new(),
        })
    })
}

/// Best-known dirtiness, immediately. Unknown reads as `false` until the first check lands.
pub fn dirty(cwd: &str) -> bool {
    let mut s = state().lock().expect("git cache is never poisoned");
    match s.cache.get(cwd) {
        Some((d, at)) if at.elapsed() < TTL => *d,
        Some((d, _)) => {
            let stale = *d;
            spawn_refresh(&mut s, cwd);
            stale
        }
        None => {
            spawn_refresh(&mut s, cwd);
            false
        }
    }
}

fn spawn_refresh(s: &mut State, cwd: &str) {
    if s.inflight.contains(cwd) {
        return;
    }
    if !Path::new(cwd).exists() {
        // Directory gone: cache clean and don't spawn anything.
        s.cache.insert(cwd.to_string(), (false, Instant::now()));
        return;
    }
    s.inflight.insert(cwd.to_string());
    let owned = cwd.to_string();
    std::thread::spawn(move || {
        let result = run_status(&owned);
        let mut s = state().lock().expect("git cache is never poisoned");
        s.inflight.remove(&owned);
        s.cache.insert(owned, (result, Instant::now()));
    });
}

/// `git status --porcelain` — not a bare `.git` check, so sessions started in a repo subdir are
/// still detected. Any failure counts as clean, matching the JS `!err && stdout.trim().length`.
fn run_status(cwd: &str) -> bool {
    let Ok(mut child) = Command::new("git")
        .args(["-C", cwd, "status", "--porcelain"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
    else {
        return false;
    };

    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return false;
                }
                break;
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => return false,
        }
    }

    use std::io::Read;
    let mut out = String::new();
    match child.stdout.as_mut().map(|s| s.read_to_string(&mut out)) {
        Some(Ok(_)) => !out.trim().is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_directory_is_clean_and_cached() {
        let missing = "/nonexistent/sessio/test/path";
        assert!(!dirty(missing));
        let s = state().lock().unwrap();
        assert_eq!(s.cache.get(missing).map(|(d, _)| *d), Some(false));
        // Scoped to this path: the cache is process-global and other tests share it.
        assert!(!s.inflight.contains(missing), "a missing dir must not spawn git");
    }

    #[test]
    fn first_call_is_non_blocking_and_reports_unknown_as_clean() {
        // Whatever the repo state, the first call must return immediately.
        let start = Instant::now();
        let _ = dirty(env!("CARGO_MANIFEST_DIR"));
        assert!(start.elapsed() < Duration::from_millis(100));
    }
}
