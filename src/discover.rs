//! Transcript discovery. Port of the `load()` scan in bin/sessio.mjs:235-253.

use std::fs;
use std::path::{Path, PathBuf};

/// Scan the 300 most-recent sessions; plenty for getting back into recent work.
pub const CAP: usize = 300;

/// Which agent wrote a session. Claude Code is the original and the default; GitHub Copilot CLI
/// sessions come from `~/.copilot/session-state` (see `copilot.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    #[default]
    Claude,
    Copilot,
}

impl Source {
    /// The stable name used in `--json` output and in the UI tag.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Claude => "claude",
            Source::Copilot => "copilot",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Row {
    /// Transcript uuid — the argument to `claude --resume`.
    pub id: String,
    /// Cache/archive key: path relative to the projects root (`<dir>/<id>.jsonl`).
    pub key: String,
    pub file: PathBuf,
    /// Whole milliseconds. The JS carries `mtimeMs` as a float; the dump rounds it, so
    /// stat precision and float formatting can't produce spurious oracle diffs.
    pub mtime: i64,
    pub size: u64,
    /// The project directory's basename, e.g. `-Users-me-work-thing`. A Copilot session has no
    /// such folder; the model derives one from its cwd.
    pub dir: String,
    pub source: Source,
}

/// `~/.claude/projects`, honouring `CLAUDE_CONFIG_DIR`.
///
/// The JS hardcodes `~/.claude` (bin/sessio.mjs:15), so an instance that relocates the agent's
/// config dir sees an empty list. Fixed here; the oracle is run with the variable unset, where
/// both implementations agree.
pub fn projects_root() -> PathBuf {
    let base = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".claude")))
        .unwrap_or_else(|| PathBuf::from(".claude"));
    base.join("projects")
}

/// One level of directories, then `*.jsonl` inside each — the JS walks exactly this shape,
/// so nested transcripts are invisible to both implementations.
pub fn scan(root: &Path) -> Vec<Row> {
    let mut rows = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return rows; // ENOENT is the empty state, not an error
    };
    for entry in entries.flatten() {
        let dir_path = entry.path();
        match fs::metadata(&dir_path) {
            Ok(meta) if meta.is_dir() => {}
            _ => continue,
        }
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        let Ok(files) = fs::read_dir(&dir_path) else {
            continue;
        };
        for file in files.flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".jsonl") {
                continue;
            }
            let path = file.path();
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let id = name[..name.len() - 6].to_string();
            rows.push(Row {
                key: format!("{dir_name}/{name}"),
                id,
                mtime: mtime_ms(&meta),
                size: meta.len(),
                file: path,
                dir: dir_name.clone(),
                source: Source::Claude,
            });
        }
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.mtime));
    rows
}

/// Every session from every source, newest first: Claude Code's transcripts and, when
/// `~/.copilot` exists, Copilot CLI's. One list, so the cap is shared by recency rather than
/// split per source.
pub fn scan_all() -> Vec<Row> {
    let mut rows = scan(&projects_root());
    rows.extend(crate::copilot::scan(&crate::copilot::root()));
    rows.sort_by_key(|r| std::cmp::Reverse(r.mtime));
    rows
}

/// The archive/search key of a transcript file, whichever source it belongs to.
pub fn key_for_file(file: &Path) -> Option<String> {
    key_for_file_in(file, &projects_root(), &crate::copilot::root())
}

pub fn key_for_file_in(file: &Path, claude: &Path, copilot: &Path) -> Option<String> {
    if let Ok(rel) = file.strip_prefix(claude) {
        return Some(rel.to_string_lossy().into_owned());
    }
    crate::copilot::key_for(file.strip_prefix(copilot).ok()?)
}

pub(crate) fn mtime_ms(meta: &fs::Metadata) -> i64 {
    use std::time::UNIX_EPOCH;
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| {
            // Round to whole ms the same way `Math.round(mtimeMs)` does in the dump.
            let ms = d.as_secs() as i128 * 1000 + d.subsec_nanos() as i128 / 1_000_000;
            let rem = d.subsec_nanos() % 1_000_000;
            (ms + if rem >= 500_000 { 1 } else { 0 }) as i64
        })
        .unwrap_or(0)
}

/// Port of `selectTranscriptRows` (lib/session-store.mjs:7): the newest `cap` rows, plus any
/// content-search matches from beyond the cap, re-sorted newest-first.
pub fn select(rows: &[Row], cap: usize, extra: &[PathBuf]) -> Vec<Row> {
    let mut selected: Vec<Row> = rows.iter().take(cap).cloned().collect();
    if !extra.is_empty() {
        let have: std::collections::HashSet<&Path> =
            selected.iter().map(|r| r.file.as_path()).collect();
        let add: Vec<Row> = rows
            .iter()
            .filter(|r| !have.contains(r.file.as_path()) && extra.iter().any(|e| e == &r.file))
            .cloned()
            .collect();
        selected.extend(add);
    }
    selected.sort_by_key(|r| std::cmp::Reverse(r.mtime));
    selected
}

/// Fallback project label when no session in the directory recorded a cwd.
/// Port of `proj()` at bin/sessio.mjs:56.
pub fn decode_dir_label(dir: &str) -> String {
    let stripped = strip_user_prefix(dir);
    let parts: Vec<&str> = stripped.split('-').collect();
    let tail = if parts.len() > 2 {
        &parts[parts.len() - 2..]
    } else {
        &parts[..]
    };
    tail.join("/")
}

/// `d.replace(/^-Users-[^-]+-/, '')`
fn strip_user_prefix(dir: &str) -> &str {
    let Some(rest) = dir.strip_prefix("-Users-") else {
        return dir;
    };
    match rest.find('-') {
        Some(idx) if idx > 0 => &rest[idx + 1..],
        _ => dir,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_dir_labels_like_the_js() {
        assert_eq!(decode_dir_label("-Users-me-Desktop-bitbybit-02-personal-sessio"), "personal/sessio");
        assert_eq!(decode_dir_label("-Users-me-work"), "work");
        assert_eq!(decode_dir_label("-opt-src-thing"), "src/thing");
    }

    #[test]
    fn select_keeps_extras_beyond_the_cap() {
        let rows: Vec<Row> = (0..301)
            .map(|i| Row {
                id: i.to_string(),
                key: format!("d/{i}.jsonl"),
                file: PathBuf::from(format!("/t/{i}.jsonl")),
                mtime: 301 - i,
                size: 0,
                dir: "d".into(),
                source: Source::Claude,
            })
            .collect();
        let extra = vec![PathBuf::from("/t/300.jsonl")];
        let selected = select(&rows, CAP, &extra);
        assert_eq!(selected.len(), 301);
        assert!(selected.iter().any(|r| r.file == extra[0]));
    }
}
