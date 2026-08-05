//! sessio-local state under `<claude-home>/.sessio`. Port of the archive + cache persistence
//! helpers at bin/sessio.mjs:18-43.

use std::collections::HashSet;
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

/// The archive is a sessio-local hide list only: transcript files are never touched, so
/// `claude --resume` still works and Claude's own cleanup still applies.
#[derive(Debug, Default)]
pub struct Archive {
    keys: HashSet<String>,
}

impl Archive {
    pub fn load() -> Self {
        let keys = fs::read_to_string(archive_file())
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
            .map(|v| v.into_iter().collect())
            .unwrap_or_default();
        Self { keys }
    }

    /// Pre-0.3.1 archives stored the bare session id; accept either spelling.
    pub fn contains(&self, key: &str, id: &str) -> bool {
        self.keys.contains(key) || self.keys.contains(id)
    }

    pub fn toggle(&mut self, key: &str, id: &str) {
        if self.contains(key, id) {
            self.keys.remove(key);
            self.keys.remove(id);
        } else {
            self.keys.insert(key.to_string());
        }
        self.save();
    }

    fn save(&self) {
        let mut list: Vec<&String> = self.keys.iter().collect();
        list.sort(); // stable on disk; the JS writes insertion order
        save_private_json(&archive_file(), &list);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_accepts_legacy_id_only_entries() {
        let mut a = Archive::default();
        a.keys.insert("abc-123".into()); // pre-0.3.1 shape
        assert!(a.contains("dir/abc-123.jsonl", "abc-123"));
        assert!(!a.contains("dir/other.jsonl", "other"));
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
