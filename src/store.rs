//! sessio-local state under `<claude-home>/.sessio`. Port of the archive + cache persistence
//! helpers at bin/sessio.mjs:18-43.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// `<claude-home>/.sessio`, honouring `CLAUDE_CONFIG_DIR` the same way `projects_root` does.
pub fn state_dir() -> PathBuf {
    let base = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".claude")))
        .unwrap_or_else(|| PathBuf::from(".claude"));
    base.join(".sessio")
}

pub fn archive_file() -> PathBuf {
    state_dir().join("archived.json")
}

/// Write owner-only JSON atomically: temp file in the same directory, then rename.
/// A partial write can never replace a good file, and the contents are never world-readable.
pub fn save_private_json<T: serde::Serialize>(file: &Path, value: &T) -> bool {
    let Some(dir) = file.parent() else {
        return false;
    };
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    set_dir_private(dir);

    let tmp = dir.join(format!(
        "{}.{}.tmp",
        file.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let write = (|| -> std::io::Result<()> {
        let mut f = create_private(&tmp)?;
        f.write_all(serde_json::to_string(value)?.as_bytes())?;
        f.sync_all()
    })();
    if write.is_err() {
        let _ = fs::remove_file(&tmp);
        return false;
    }
    if fs::rename(&tmp, file).is_err() {
        let _ = fs::remove_file(&tmp);
        return false;
    }
    set_file_private(file);
    true
}

#[cfg(unix)]
fn create_private(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private(path: &Path) -> std::io::Result<fs::File> {
    fs::File::create(path)
}

#[cfg(unix)]
fn set_file_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_file_private(_path: &Path) {}

#[cfg(unix)]
fn set_dir_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn set_dir_private(_path: &Path) {}

/// One archived session and when it was hidden.
#[derive(serde::Serialize)]
struct Entry<'a> {
    k: &'a str,
    at: i64,
}

/// What the file may hold: the stamped shape sessio writes now, or a bare id from before
/// entries carried a timestamp.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum RawEntry {
    Stamped { k: String, at: i64 },
    Legacy(String),
}

/// The archive is a sessio-local hide list only: transcript files are never touched, so
/// `claude --resume` still works and Claude's own cleanup still applies.
#[derive(Debug, Default)]
pub struct Archive {
    /// key (or a legacy bare id) -> when it was archived, epoch ms.
    entries: HashMap<String, i64>,
    /// Where this archive persists. Held rather than recomputed so a test can point an Archive
    /// at a temp file — `save()` is called from inside `toggle` and `release_reactivated`, and a
    /// test that reached the real path would silently rewrite the user's archive list.
    file: PathBuf,
}

impl Archive {
    pub fn load() -> Self {
        let raw: Vec<RawEntry> = fs::read_to_string(archive_file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        // An entry with no stamp is dated to *now*, not to the epoch: it was archived at some
        // unknown past moment, and dating it to 0 would make the next write look like fresh
        // activity and unarchive the lot on the first launch after upgrading.
        let now = crate::model::now_ms();
        let mut migrated = false;
        let entries = raw
            .into_iter()
            .map(|e| match e {
                RawEntry::Stamped { k, at } => (k, at),
                RawEntry::Legacy(k) => {
                    migrated = true;
                    (k, now)
                }
            })
            .collect();
        let me = Self { entries, file: archive_file() };
        if migrated {
            me.save(); // persist the stamps, so the migration happens exactly once
        }
        me
    }

    /// Pre-0.3.1 archives stored the bare session id; accept either spelling.
    pub fn contains(&self, key: &str, id: &str) -> bool {
        self.entries.contains_key(key) || self.entries.contains_key(id)
    }

    fn archived_at(&self, key: &str, id: &str) -> Option<i64> {
        self.entries.get(key).or_else(|| self.entries.get(id)).copied()
    }

    pub fn toggle(&mut self, key: &str, id: &str) {
        if self.contains(key, id) {
            self.forget(key, id);
        } else {
            self.entries.insert(key.to_string(), crate::model::now_ms());
        }
        self.save();
    }

    fn forget(&mut self, key: &str, id: &str) {
        self.entries.remove(key);
        self.entries.remove(id);
    }

    /// Archiving hides a session you are done with. Working in it again says you are not, so any
    /// session written to *after* it was archived comes back out on its own — otherwise a session
    /// you archived months ago stays invisible however much you use it now.
    ///
    /// Returns how many were released.
    pub fn release_reactivated<'a>(
        &mut self,
        rows: impl Iterator<Item = (&'a str, &'a str, i64)>,
    ) -> usize {
        let stale: Vec<(String, String)> = rows
            .filter(|(key, id, mtime)| self.archived_at(key, id).is_some_and(|at| *mtime > at))
            .map(|(key, id, _)| (key.to_string(), id.to_string()))
            .collect();
        for (key, id) in &stale {
            self.forget(key, id);
        }
        if !stale.is_empty() {
            self.save();
        }
        stale.len()
    }

    fn save(&self) {
        let mut list: Vec<Entry> = self
            .entries
            .iter()
            .map(|(k, at)| Entry { k, at: *at })
            .collect();
        list.sort_by(|a, b| a.k.cmp(b.k)); // stable on disk
        if self.file.as_os_str().is_empty() {
            return; // a default-constructed Archive is not backed by a file
        }
        save_private_json(&self.file, &list);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_accepts_legacy_id_only_entries() {
        let mut a = Archive::default();
        a.entries.insert("abc-123".into(), 0); // pre-0.3.1 shape
        assert!(a.contains("dir/abc-123.jsonl", "abc-123"));
        assert!(!a.contains("dir/other.jsonl", "other"));
    }

    /// Backed by a throwaway path: `release_reactivated` and `toggle` both persist, and the real
    /// archive_file() is the user's own list.
    fn scratch(name: &str) -> Archive {
        let dir = std::env::temp_dir().join(format!("sessio-arch-{}-{name}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        Archive { entries: HashMap::new(), file: dir.join("archived.json") }
    }

    #[test]
    fn a_session_written_after_it_was_archived_comes_back() {
        let mut a = scratch("reactivated");
        a.entries.insert("d/live.jsonl".into(), 1_000);
        a.entries.insert("d/done.jsonl".into(), 1_000);
        let rows = [
            ("d/live.jsonl", "live", 2_000), // written since — reactivated
            ("d/done.jsonl", "done", 1_000), // untouched since (equal is not newer)
        ];
        assert_eq!(a.release_reactivated(rows.into_iter()), 1);
        assert!(!a.contains("d/live.jsonl", "live"));
        assert!(a.contains("d/done.jsonl", "done"));
    }

    #[test]
    fn a_legacy_entry_matched_by_bare_id_is_released_too() {
        let mut a = scratch("legacy");
        a.entries.insert("abc-123".into(), 1_000);
        assert_eq!(a.release_reactivated([("d/abc-123.jsonl", "abc-123", 2_000)].into_iter()), 1);
        assert!(!a.contains("d/abc-123.jsonl", "abc-123"));
    }

    #[test]
    fn both_entry_shapes_load() {
        let raw = r#"["bare-id",{"k":"d/x.jsonl","at":42}]"#;
        let parsed: Vec<RawEntry> = serde_json::from_str(raw).expect("either shape");
        assert!(matches!(parsed[0], RawEntry::Legacy(_)));
        assert!(matches!(parsed[1], RawEntry::Stamped { at: 42, .. }));
    }

    #[test]
    fn atomic_write_round_trips() {
        let dir = std::env::temp_dir().join(format!("sessio-store-{}", std::process::id()));
        let file = dir.join("t.json");
        assert!(save_private_json(&file, &vec!["a", "b"]));
        let back: Vec<String> =
            serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(back, vec!["a", "b"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn written_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("sessio-perm-{}", std::process::id()));
        let file = dir.join("t.json");
        assert!(save_private_json(&file, &vec!["x"]));
        let mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "state may contain session ids; keep it private");
        let _ = fs::remove_dir_all(&dir);
    }
}
