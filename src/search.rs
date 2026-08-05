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

/// Grep every transcript body for `term`, returning the matching files.
///
/// `-F` keeps the query literal, so a prompt containing regex metacharacters searches for
/// itself rather than exploding. Runs to completion on a worker thread; the caller drops the
/// result if a newer search has superseded it.
pub fn content_search(term: &str, root: &Path) -> Option<HashSet<PathBuf>> {
    if term.is_empty() {
        return None;
    }
    let rg = rg_path()?;
    let out = Command::new(rg)
        .args(["-l", "-i", "-F", "--glob", "*.jsonl", "--", term])
        .arg(root)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    // rg exits 1 with no matches — that is an empty result set, not a failure.
    if !out.status.success() && out.status.code() != Some(1) {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_term_searches_nothing() {
        assert!(content_search("", Path::new("/tmp")).is_none());
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
        let hits = content_search("needle(1)", &dir).unwrap();
        assert!(hits.contains(&f));
        assert!(content_search("absent", &dir).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
