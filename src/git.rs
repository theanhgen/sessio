//! Background git-WIP detection. Port of `gitRefresh` / `gitDirty` at bin/sessio.mjs:176-195.
//!
//! The rule that matters: `dirty()` never blocks. It answers from cache immediately and kicks
//! off a refresh in the background when the entry is stale or missing, so a slow repo can't
//! stall a redraw. Callers pick up the fresh value on a later tick.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// WIP doesn't change fast; re-check at most this often.
const TTL: Duration = Duration::from_secs(30);
/// Bound on a single `git status`, so a huge or networked repo can't wedge a worker.
const TIMEOUT: Duration = Duration::from_secs(2);
/// Checks run on a fixed pool rather than a thread per directory. A browse across 300 sessions
/// can touch a hundred distinct repos; one OS thread and one `git` process each would spike CPU
/// and file descriptors for work whose results are only ever read a tick later.
const WORKERS: usize = 4;

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

/// The work queue feeding the fixed worker pool, started on first use.
fn queue() -> &'static Sender<String> {
    static Q: OnceLock<Sender<String>> = OnceLock::new();
    Q.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<String>();
        let rx = std::sync::Arc::new(Mutex::new(rx));
        for _ in 0..WORKERS {
            let rx = std::sync::Arc::clone(&rx);
            std::thread::spawn(move || loop {
                // Hold the receiver lock only long enough to take one item.
                let next = { rx.lock().expect("queue is never poisoned").recv() };
                let Ok(cwd) = next else { return }; // sender dropped: process is exiting
                let result = run_status(&cwd);
                let mut s = state().lock().expect("git cache is never poisoned");
                s.inflight.remove(&cwd);
                s.cache.insert(cwd, (result, Instant::now()));
            });
        }
        tx
    })
}

fn spawn_refresh(s: &mut State, cwd: &str) {
    if s.inflight.contains(cwd) {
        return;
    }
    if !Path::new(cwd).exists() {
        // Directory gone: cache clean and don't queue anything.
        s.cache.insert(cwd.to_string(), (false, Instant::now()));
        return;
    }
    s.inflight.insert(cwd.to_string());
    if queue().send(cwd.to_string()).is_err() {
        s.inflight.remove(cwd); // pool is gone; let a later call retry
    }
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

    // Drain the pipe while waiting, not after: a working tree dirty enough to fill the OS pipe
    // buffer blocks `git` on write, so a child polled with nobody reading never exits and every
    // such repo would time out and read as clean.
    let mut stdout = child.stdout.take();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut out = String::new();
        let read = stdout.as_mut().and_then(|s| s.read_to_string(&mut out).ok());
        let _ = tx.send(read.map(|_| out));
    });

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
                // Not joined: killing the child closes the pipe and the reader ends on its own,
                // and TIMEOUT must bound this call whatever the reader is doing.
                return false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => return false,
        }
    }

    // Collected on the same deadline as the wait, never joined. The child exiting does not by
    // itself close the write end — a grandchild that inherited the descriptor holds it open, and
    // read_to_string waits for EOF, not for the child. `git status` is not known to leave one
    // behind, but TIMEOUT is the promise this function makes and nothing here may outlast it.
    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(Some(out)) => !out.trim().is_empty(),
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
    fn many_directories_stay_non_blocking_on_a_bounded_pool() {
        // Regression: a thread (and a `git` process) per directory used to be spawned here.
        // Queueing must stay cheap no matter how many repos a browse touches.
        let start = Instant::now();
        for i in 0..200 {
            let _ = dirty(&format!("/nonexistent/sessio/bulk/{i}"));
        }
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    /// A fresh scratch directory, named per-test so parallel tests don't collide.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sessio-git-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir is writable");
        dir
    }

    #[test]
    fn output_past_the_pipe_buffer_is_still_dirty_and_never_hits_the_deadline() {
        // Regression: stdout was only read after the child exited, so a repo whose porcelain
        // output overflowed the pipe (16-64 KB) deadlocked, timed out, and reported clean.
        let dir = scratch("dirty");
        let path = dir.to_str().expect("temp path is utf-8");
        let init = Command::new("git")
            .args(["-C", path, "init", "-q"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        assert!(matches!(init, Ok(s) if s.success()), "git init failed");
        // Long names rather than many files: ~250 KB of porcelain from only 1200 stats.
        let pad = "n".repeat(200);
        for i in 0..1200 {
            std::fs::write(dir.join(format!("{i:04}{pad}")), b"x").expect("write scratch file");
        }

        let start = Instant::now();
        let result = run_status(path);
        let elapsed = start.elapsed();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(result, "a repo full of untracked files must read as dirty");
        assert!(
            elapsed < TIMEOUT / 2,
            "took {elapsed:?}: still on the deadline path, not a real read"
        );
    }

    #[test]
    fn a_directory_outside_any_repo_is_clean() {
        // git exits non-zero here; that must stay "clean", not an error the caller sees.
        let dir = scratch("norepo");
        let result = run_status(dir.to_str().expect("temp path is utf-8"));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!result);
    }

    #[test]
    fn first_call_is_non_blocking_and_reports_unknown_as_clean() {
        // Whatever the repo state, the first call must return immediately.
        let start = Instant::now();
        let _ = dirty(env!("CARGO_MANIFEST_DIR"));
        assert!(start.elapsed() < Duration::from_millis(100));
    }
}
