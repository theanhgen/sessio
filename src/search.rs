//! Full-text search across every transcript body, via ripgrep. Port of `contentSearch`
//! (bin/sessio.mjs:517) and the `RG` probe at :49.
//!
//! Without `rg` on the system everything else still works; only content search is disabled.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// Locate ripgrep once. `None` disables `^f` gracefully.
pub fn rg_path() -> Option<&'static str> {
    static RG: OnceLock<Option<String>> = OnceLock::new();
    RG.get_or_init(|| {
        for p in ["rg", "/opt/homebrew/bin/rg", "/usr/local/bin/rg", "/usr/bin/rg"] {
            let ok = Command::new(p)
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                return Some(p.to_string());
            }
        }
        None
    })
    .as_deref()
}

/// Every folder `^f` and `find --text` search: Claude Code's projects and Copilot CLI's
/// session-state.
pub fn roots() -> Vec<PathBuf> {
    vec![crate::discover::projects_root(), crate::copilot::root()]
}

/// What to install when `rg` is missing. The one place the hint is spelled, so the dashboard,
/// its help and `find --text` cannot disagree about it.
pub const INSTALL_HINT: &str = "brew install ripgrep";

/// Grep every transcript body for `term`, returning the matching files, or why it could not.
///
/// `-F` keeps the query literal, so a prompt containing regex metacharacters searches for
/// itself rather than exploding. Runs to completion on a worker thread; the caller drops the
/// result if a newer search has superseded it.
pub fn content_search(term: &str, roots: &[PathBuf]) -> Result<HashSet<PathBuf>, String> {
    content_search_with(rg_path(), term, roots)
}

/// `content_search` with the ripgrep binary passed in, so a missing one can be tested.
fn content_search_with(
    rg: Option<&str>,
    term: &str,
    roots: &[PathBuf],
) -> Result<HashSet<PathBuf>, String> {
    if term.is_empty() {
        return Err("nothing to search for".into());
    }
    let rg = rg.ok_or_else(|| format!("ripgrep (rg) is not installed · {INSTALL_HINT}"))?;
    // A root that does not exist (no Copilot installed) would make rg exit 2 — a failure — so it
    // is left out rather than passed along.
    let present: Vec<&Path> = roots.iter().map(PathBuf::as_path).filter(|r| r.is_dir()).collect();
    if present.is_empty() {
        return Ok(HashSet::new());
    }
    let out = Command::new(rg)
        .args(["-l", "-i", "-F", "--glob", "*.jsonl", "--", term])
        .args(&present)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run rg: {e}"))?;

    // rg exits 1 with no matches — that is an empty result set, not a failure.
    if !out.status.success() && out.status.code() != Some(1) {
        return Err(failure_reason(out.status.code(), &out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// One short line saying why rg failed: its exit code and the first thing it said.
fn failure_reason(code: Option<i32>, stderr: &[u8]) -> String {
    let said = String::from_utf8_lossy(stderr);
    let first = said.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let first: String = first.chars().take(120).collect();
    let code = code.map_or_else(|| "was killed".to_string(), |c| format!("exited {c}"));
    if first.is_empty() {
        format!("rg {code}")
    } else {
        format!("rg {code}: {first}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_term_searches_nothing() {
        assert!(content_search("", &[PathBuf::from("/tmp")]).is_err());
    }

    #[test]
    fn a_missing_ripgrep_says_how_to_install_it() {
        let e = content_search_with(None, "x", &[PathBuf::from("/tmp")]).unwrap_err();
        assert!(e.contains(INSTALL_HINT), "{e}");
    }

    #[test]
    fn a_failure_says_why() {
        assert_eq!(failure_reason(Some(2), b"\nrg: bad glob\nmore\n"), "rg exited 2: rg: bad glob");
        assert_eq!(failure_reason(Some(2), b""), "rg exited 2");
        assert_eq!(failure_reason(None, b""), "rg was killed");
    }

    #[test]
    fn searches_every_root_and_skips_missing_ones() {
        let Some(_) = rg_path() else {
            return;
        };
        let base = std::env::temp_dir().join(format!("sessio-search-roots-{}", std::process::id()));
        let (claude, copilot) = (base.join("projects"), base.join("session-state"));
        std::fs::create_dir_all(claude.join("p")).unwrap();
        std::fs::create_dir_all(copilot.join("sid")).unwrap();
        let a = claude.join("p").join("a.jsonl");
        let b = copilot.join("sid").join("events.jsonl");
        std::fs::write(&a, "{\"x\":\"shared-needle\"}\n").unwrap();
        std::fs::write(&b, "{\"x\":\"shared-needle\"}\n").unwrap();

        let hits = content_search("shared-needle", &[claude.clone(), copilot.clone()]).unwrap();
        assert!(hits.contains(&a) && hits.contains(&b), "{hits:?}");
        // No ~/.copilot: still a search, not a failure.
        let hits = content_search("shared-needle", &[claude.clone(), base.join("absent")]).unwrap();
        assert_eq!(hits.len(), 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn finds_a_literal_match() {
        let Some(_) = rg_path() else {
            return; // ripgrep is optional; skip rather than fail the suite
        };
        let dir = std::env::temp_dir().join(format!("sessio-search-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.jsonl");
        std::fs::write(&f, "{\"x\":\"needle(1)\"}\n").unwrap();

        // A regex metacharacter must be matched literally thanks to -F.
        let roots = [dir.clone()];
        let hits = content_search("needle(1)", &roots).unwrap();
        assert!(hits.contains(&f));
        assert!(content_search("absent", &roots).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
