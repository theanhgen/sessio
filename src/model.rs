//! The session list model. Port of `load()` at bin/sessio.mjs:235-294.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::discover::{self, Row, CAP};
use crate::parse::{self, Detail, OpenReason};
use crate::safety::sanitize;
use crate::store::Archive;

pub const OPEN_TAB: &str = "⏸ open";
pub const ARCHIVED_TAB: &str = "🗄 archived";
pub const ALL_TAB: &str = "All";

/// "Claude asked / proposed next" is a weak signal — most replies offer a next step — so it
/// only keeps a session open while it is recent. Unanswered prompts and git WIP count at any age.
pub const CTA_MAX_AGE_MS: i64 = 3 * 86_400_000;

/// Green dot: active (written in the last 5 minutes).
pub const ACTIVE_MS: i64 = 5 * 60 * 1000;
/// Orange dot: recent (last 24h, but not active).
pub const RECENT_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone)]
pub struct Item {
    pub id: String,
    pub key: String,
    pub file: PathBuf,
    pub mtime: i64,
    pub size: u64,
    pub dir: String,

    pub first: Option<String>,
    pub first_ts: Option<String>,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub custom: Option<String>,
    pub ai: Option<String>,

    pub open: bool,
    pub open_reason: Option<OpenReason>,

    /// Claude's away-recap, from the cheap tail read, so the preview has it before the lazy
    /// full read lands. `detail` supersedes it once loaded.
    pub recap: Option<String>,
    pub recap_ts: Option<String>,

    pub title: Option<String>,
    pub name: String,
    pub project: String,
    pub hay: String,

    /// Filled lazily for the highlighted row only.
    pub detail: Option<Detail>,
}

impl Item {
    /// The user-visible reason shown beside the "pick up" marker.
    pub fn open_why(&self) -> Option<&'static str> {
        self.open_reason.map(OpenReason::label)
    }
    pub fn prompt_count(&self) -> Option<usize> {
        self.detail.as_ref().map(|d| d.count)
    }
    /// Detail overrides the head-derived title once it has been read.
    pub fn display_name(&self) -> &str {
        &self.name
    }
}

pub fn load(extra: &[PathBuf]) -> Vec<Item> {
    let root = discover::projects_root();
    let rows = discover::scan(&root);
    let selected = discover::select(&rows, CAP, extra);
    let parsed = parse_all(&selected);

    let mut items: Vec<Item> = selected
        .into_iter()
        .zip(parsed)
        .filter(|(_, (h, _))| h.first.is_some()) // skip empty (0-prompt) sessions
        .map(|(row, (head, tail))| {
            let title = head.custom.clone().or_else(|| head.ai.clone());
            let first = head.first.clone().unwrap_or_default();
            let name = title.clone().unwrap_or(first);
            let open = tail.open;
            let reason = tail.reason;
            let recap = tail.recap;
            let recap_ts = tail.recap_ts;

            let open = decay(open, reason, row.mtime, now_ms());
            let open_reason = if open { reason } else { None };

            Item {
                id: row.id,
                key: row.key,
                file: row.file,
                mtime: row.mtime,
                size: row.size,
                dir: row.dir,
                first: head.first,
                first_ts: head.first_ts,
                cwd: head.cwd,
                branch: head.branch,
                custom: head.custom,
                ai: head.ai,
                open,
                open_reason,
                recap,
                recap_ts,
                title,
                name,
                project: String::new(), // assigned below, once dir labels are known
                hay: String::new(),
                detail: None,
            }
        })
        .collect();

    // git WIP: flag the most-recent session in each project whose folder has uncommitted
    // changes. `dirty()` never blocks — unknown reads as clean until a background check lands.
    let mut seen: HashSet<&str> = HashSet::new();
    let flags: Vec<bool> = items
        .iter()
        .map(|it| match &it.cwd {
            Some(cwd) if seen.insert(cwd.as_str()) => crate::git::dirty(cwd),
            _ => false,
        })
        .collect();
    for (it, is_dirty) in items.iter_mut().zip(flags) {
        if is_dirty {
            it.open = true;
            if it.open_reason.is_none() {
                it.open_reason = Some(OpenReason::GitWip);
            }
        }
    }

    // One canonical label per project dir: prefer a sibling's real cwd basename (hyphens
    // intact); fall back to the dash-decoded dir name only if no session in the dir has a cwd.
    let mut dir_label: HashMap<String, String> = HashMap::new();
    for it in &items {
        if let Some(cwd) = &it.cwd {
            dir_label.entry(it.dir.clone()).or_insert_with(|| {
                Path::new(cwd)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| cwd.clone())
            });
        }
    }
    for it in &mut items {
        it.project = sanitize(
            dir_label
                .get(&it.dir)
                .cloned()
                .unwrap_or_else(|| discover::decode_dir_label(&it.dir))
                .as_str(),
        );
        it.hay = format!(
            "{} {} {}",
            it.project,
            it.name,
            it.first.as_deref().unwrap_or("")
        )
        .to_lowercase();
    }
    items
}

/// Whether a session still counts as open, given its age.
///
/// Split out so it is testable without a filesystem: this is the rule the JS had but never
/// applied, because its decay check compared against a different string literal than the one
/// it wrote. See `OpenReason` for why that can no longer happen.
pub fn decay(open: bool, reason: Option<OpenReason>, mtime: i64, now: i64) -> bool {
    if open && reason == Some(OpenReason::CallToAction) && now - mtime > CTA_MAX_AGE_MS {
        return false;
    }
    open
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400_000;
    const NOW: i64 = 1_700_000_000_000;

    #[test]
    fn stale_call_to_action_stops_counting_as_open() {
        assert!(!decay(true, Some(OpenReason::CallToAction), NOW - 4 * DAY, NOW));
    }

    #[test]
    fn recent_call_to_action_stays_open() {
        assert!(decay(true, Some(OpenReason::CallToAction), NOW - 2 * DAY, NOW));
        // Exactly at the boundary is still open — the JS used a strict `>`.
        assert!(decay(true, Some(OpenReason::CallToAction), NOW - 3 * DAY, NOW));
    }

    #[test]
    fn unanswered_prompts_and_git_wip_never_decay() {
        for reason in [OpenReason::Unanswered, OpenReason::GitWip] {
            assert!(
                decay(true, Some(reason), NOW - 400 * DAY, NOW),
                "{reason:?} must count at any age"
            );
        }
    }

    #[test]
    fn a_closed_session_stays_closed() {
        assert!(!decay(false, Some(OpenReason::CallToAction), NOW, NOW));
        assert!(!decay(false, None, NOW, NOW));
    }

    #[test]
    fn labels_are_stable_user_visible_strings() {
        // The preview renders these; the oracle compares them against the JS constants.
        assert_eq!(OpenReason::Unanswered.label(), "your prompt got no reply");
        assert_eq!(OpenReason::CallToAction.label(), "Claude asked / proposed next");
        assert_eq!(OpenReason::GitWip.label(), "uncommitted changes");
    }
}

/// Tabs are built from live (non-archived) sessions; the archived tab appears only while
/// something is archived, and always sits last. Port of `tabsFor` at bin/sessio.mjs:451.
pub fn tabs_for(items: &[Item], archive: &Archive) -> Vec<String> {
    let live: Vec<&Item> = items
        .iter()
        .filter(|i| !archive.contains(&i.key, &i.id))
        .collect();
    let mut tabs = vec![ALL_TAB.to_string()];
    if live.iter().any(|i| i.open) {
        tabs.push(OPEN_TAB.to_string());
    }
    let mut seen = HashSet::new();
    for i in &live {
        if seen.insert(i.project.clone()) {
            tabs.push(i.project.clone());
        }
    }
    if items.iter().any(|i| archive.contains(&i.key, &i.id)) {
        tabs.push(ARCHIVED_TAB.to_string());
    }
    tabs
}

/// Read head+tail for every selected row, bounded by core count.
fn parse_all(rows: &[Row]) -> Vec<(parse::Head, parse::Tail)> {
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(rows.len().max(1));
    let next = AtomicUsize::new(0);
    let results = std::sync::Mutex::new(Vec::with_capacity(rows.len()));

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                // Accumulate locally and merge once, so the lock isn't taken per file.
                let mut local = Vec::new();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= rows.len() {
                        break;
                    }
                    local.push((i, parse::head(&rows[i].file), parse::tail(&rows[i].file)));
                }
                results.lock().expect("no worker panics").extend(local);
            });
        }
    });

    // Restore input order — completion order is arbitrary.
    let mut collected = results.into_inner().expect("workers joined");
    collected.sort_by_key(|(i, _, _)| *i);
    collected.into_iter().map(|(_, h, t)| (h, t)).collect()
}

pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
