//! Open GitHub issues for the repo a session's folder belongs to, fetched through `gh`.
//!
//! Same rule as `git.rs`: `status()` never blocks. It answers from cache and queues a fetch in
//! the background when the entry is stale or missing. A `gh issue list` takes about half a second
//! per repo, so anything that waited on it would stall every redraw that crossed a new project.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::safety::sanitize;

/// Issues change on a human timescale; re-fetch a repo at most this often.
const TTL: Duration = Duration::from_secs(300);
/// A failed fetch (offline, not logged in) is retried sooner, so fixing it shows up without a
/// restart.
const RETRY: Duration = Duration::from_secs(30);
/// Bound on one `gh` call, so a hung network can't pin a worker.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Past this many open issues the list says `100+` rather than paging through the rest.
pub const LIMIT: usize = 100;
/// Two workers: walking the project panel quickly queues a few repos, and each is network-bound.
const WORKERS: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub labels: Vec<String>,
    /// ISO timestamp, as `gh` returns it.
    pub updated: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// First fetch for this folder hasn't landed yet.
    Loading,
    /// Not a git repo, or its `origin` isn't on GitHub. Nothing to show.
    NoRemote,
    /// The fetch failed, with a message short enough for one line.
    Failed(String),
    Ready {
        /// `owner/repo`.
        slug: String,
        issues: Vec<Issue>,
        /// Wall-clock ms of the fetch these issues came from. A later failed refresh keeps the
        /// old list, and this is what tells the user how old it is.
        fetched_ms: i64,
    },
}

struct State {
    cache: HashMap<String, (Status, Instant)>,
    inflight: HashSet<String>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State { cache: HashMap::new(), inflight: HashSet::new() }))
}

/// Best-known issues for `cwd`, immediately.
pub fn status(cwd: &str) -> Status {
    let mut s = state().lock().expect("issues cache is never poisoned");
    if !s.cache.contains_key(cwd) && !Path::new(cwd).exists() {
        // Folder gone: nothing to ask git about, and nothing to queue.
        s.cache.insert(cwd.to_string(), (Status::NoRemote, Instant::now()));
    }
    match s.cache.get(cwd) {
        Some((st, at)) => {
            let ttl = if matches!(st, Status::Failed(_)) { RETRY } else { TTL };
            let st = st.clone();
            if at.elapsed() >= ttl {
                queue_fetch(&mut s, cwd);
            }
            st
        }
        None => {
            queue_fetch(&mut s, cwd);
            Status::Loading
        }
    }
}

fn queue() -> &'static Sender<String> {
    static Q: OnceLock<Sender<String>> = OnceLock::new();
    Q.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<String>();
        let rx = std::sync::Arc::new(Mutex::new(rx));
        for _ in 0..WORKERS {
            let rx = std::sync::Arc::clone(&rx);
            std::thread::spawn(move || loop {
                let next = { rx.lock().expect("queue is never poisoned").recv() };
                let Ok(cwd) = next else { return };
                let fresh = fetch(&cwd);
                let mut s = state().lock().expect("issues cache is never poisoned");
                s.inflight.remove(&cwd);
                // A failed refresh does not throw away a list that was good: stale issues with
                // their age on them beat an error where the issues used to be.
                let keep_old = matches!(fresh, Status::Failed(_))
                    && matches!(s.cache.get(&cwd), Some((Status::Ready { .. }, _)));
                if keep_old {
                    if let Some(entry) = s.cache.get_mut(&cwd) {
                        entry.1 = Instant::now();
                    }
                } else {
                    s.cache.insert(cwd, (fresh, Instant::now()));
                }
            });
        }
        tx
    })
}

fn queue_fetch(s: &mut State, cwd: &str) {
    if !s.inflight.insert(cwd.to_string()) {
        return;
    }
    if queue().send(cwd.to_string()).is_err() {
        s.inflight.remove(cwd);
    }
}

fn fetch(cwd: &str) -> Status {
    let Ok(remote) = run("git", &["-C", cwd, "remote", "get-url", "origin"]) else {
        return Status::NoRemote;
    };
    let Some(slug) = github_slug(remote.trim()) else { return Status::NoRemote };
    let limit = LIMIT.to_string();
    let args = [
        "issue", "list", "-R", &slug, "--state", "open", "--limit", &limit, "--json",
        "number,title,url,labels,updatedAt",
    ];
    match run("gh", &args) {
        Ok(out) => match parse(&out) {
            Some(issues) => Status::Ready { slug, issues, fetched_ms: crate::model::now_ms() },
            None => Status::Failed("couldn't read gh's output".into()),
        },
        Err(e) => Status::Failed(e),
    }
}

#[derive(Deserialize)]
struct RawIssue {
    number: u64,
    title: String,
    url: String,
    #[serde(default)]
    labels: Vec<RawLabel>,
    #[serde(rename = "updatedAt", default)]
    updated_at: String,
}

#[derive(Deserialize)]
struct RawLabel {
    name: String,
}

/// `gh --json` output → issues. Every string is from the network, so each is sanitized here,
/// once, before anything can reach the terminal.
fn parse(json: &str) -> Option<Vec<Issue>> {
    let raw: Vec<RawIssue> = serde_json::from_str(json).ok()?;
    Some(
        raw.into_iter()
            .map(|r| Issue {
                number: r.number,
                title: sanitize(&r.title).replace('\n', " "),
                url: sanitize(&r.url),
                labels: r.labels.into_iter().map(|l| sanitize(&l.name)).collect(),
                updated: sanitize(&r.updated_at),
            })
            .collect(),
    )
}

/// `owner/repo` for a GitHub remote in any of the forms git accepts: https (with or without
/// credentials in it), `git@github.com:owner/repo`, `ssh://git@github.com/owner/repo`. `None` for
/// anything that isn't github.com.
pub fn github_slug(url: &str) -> Option<String> {
    let at = url.find("github.com")?;
    // Must be the host, not a path that happens to contain it.
    let before = &url[..at];
    if !(before.is_empty() || before.ends_with("//") || before.ends_with('@')) {
        return None;
    }
    let rest = url[at + "github.com".len()..].strip_prefix([':', '/'])?;
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let mut parts = rest.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    let ok = |s: &str| {
        !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (parts.next().is_none() && ok(owner) && ok(repo)).then(|| format!("{owner}/{repo}"))
}

/// Run a command to completion within `TIMEOUT`. `Err` carries a one-line reason for the UI.
fn run(bin: &str, args: &[&str]) -> Result<String, String> {
    let mut child = Command::new(bin)
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("{bin} not installed")
            } else {
                e.to_string()
            }
        })?;

    // Drain both pipes while waiting, as `git.rs` does: a full pipe blocks the child forever.
    let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut out = String::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_string(&mut out);
            }
            let _ = tx.send(out);
        });
        rx
    };
    let out_rx = drain(child.stdout.take().map(|p| Box::new(p) as _));
    let err_rx = drain(child.stderr.take().map(|p| Box::new(p) as _));

    let deadline = Instant::now() + TIMEOUT;
    let ok = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{bin} timed out"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let left = deadline.saturating_duration_since(Instant::now());
    let out = out_rx.recv_timeout(left).unwrap_or_default();
    if ok {
        return Ok(out);
    }
    let err = err_rx.recv_timeout(Duration::from_millis(200)).unwrap_or_default();
    Err(explain(&err))
}

/// `gh`'s stderr, cut down to what the user should do about it.
fn explain(stderr: &str) -> String {
    let e = stderr.to_lowercase();
    if e.contains("auth login") || e.contains("not logged") {
        "gh not logged in · run `gh auth login`".into()
    } else if e.contains("could not resolve") || e.contains("error connecting") || e.contains("dial tcp") {
        "offline".into()
    } else {
        let first = stderr.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("failed");
        let first = sanitize(first);
        first.chars().take(80).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_from_every_remote_form() {
        let want = Some("theanhgen/sessio".to_string());
        assert_eq!(github_slug("https://github.com/theanhgen/sessio.git"), want);
        assert_eq!(github_slug("https://github.com/theanhgen/sessio"), want);
        assert_eq!(github_slug("https://github.com/theanhgen/sessio/"), want);
        assert_eq!(github_slug("git@github.com:theanhgen/sessio.git"), want);
        assert_eq!(github_slug("ssh://git@github.com/theanhgen/sessio.git"), want);
        assert_eq!(github_slug("https://x-access-token:abc@github.com/theanhgen/sessio.git"), want);
    }

    #[test]
    fn non_github_remotes_have_no_slug() {
        assert_eq!(github_slug("https://gitlab.com/a/b.git"), None);
        assert_eq!(github_slug("git@bitbucket.org:a/b.git"), None);
        assert_eq!(github_slug("https://example.com/github.com/a/b"), None);
        assert_eq!(github_slug("https://github.com/only-owner"), None);
        assert_eq!(github_slug("https://github.com/a/b/tree/main"), None);
        assert_eq!(github_slug("/local/path/repo"), None);
    }

    #[test]
    fn issue_text_from_the_network_is_sanitized() {
        let json = r#"[{"number":7,"title":"evil\u001b]52;c;aGk=\u0007 title\nsecond","url":"https://github.com/a/b/issues/7","labels":[{"name":"bug"}],"updatedAt":"2026-09-24T08:02:25Z"}]"#;
        let got = parse(json).expect("parses");
        assert_eq!(got.len(), 1);
        assert!(!got[0].title.contains('\u{1b}') && !got[0].title.contains('\u{7}'));
        assert!(!got[0].title.contains('\n'), "a title is one line");
        assert_eq!(got[0].labels, vec!["bug".to_string()]);
    }

    #[test]
    fn missing_folder_is_no_remote_and_spawns_nothing() {
        let missing = "/nonexistent/sessio/issues/path";
        assert_eq!(status(missing), Status::NoRemote);
        let s = state().lock().unwrap();
        assert!(!s.inflight.contains(missing));
    }

    #[test]
    fn auth_errors_say_what_to_run() {
        assert!(explain("To get started with GitHub CLI, please run:  gh auth login").contains("gh auth login"));
        assert_eq!(explain("error connecting to api.github.com"), "offline");
    }
}
