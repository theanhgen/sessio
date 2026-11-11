//! `sessions pull <host>`: copy another machine's Claude Code transcripts here, over ssh, so
//! `--host <host>` can read what happened there.
//!
//! The copy is read-only from sessio's side and only ever grows: a transcript the other machine
//! deletes stays here, the same way sessio treats history everywhere else. Nothing is written on
//! the other machine; the only remote command is a `find` that lists what to copy.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::discover::{host_home, local_claude_home, valid_host};

/// What a pull copied and where it put it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Report {
    pub host: String,
    /// Transcripts the other machine listed in the window.
    pub files: usize,
    /// Their total size, as copied.
    pub bytes: u64,
    /// The `~/.claude`-shaped folder `--host` reads.
    pub into: PathBuf,
    /// The days the pull asked for; `0` is everything.
    pub days: u32,
    /// The oldest change among the copied transcripts, in ms: how far back the copy reaches.
    #[serde(serialize_with = "iso_opt")]
    pub oldest: Option<i64>,
}

/// A host pulled before, and when.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Pulled {
    pub host: String,
    /// When the last pull finished; `None` if none did.
    #[serde(serialize_with = "iso_opt")]
    pub at: Option<i64>,
    /// The days that pull asked for; `0` is everything.
    pub days: Option<u32>,
}

/// What a finished pull leaves behind, so `--host` can say how old and how deep the copy is.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Stamp {
    pub at: i64,
    pub days: u32,
}

/// Times go out as ISO 8601, like every other time sessio prints in JSON.
fn iso_opt<S: serde::Serializer>(ms: &Option<i64>, s: S) -> Result<S::Ok, S::Error> {
    match ms.and_then(chrono::DateTime::from_timestamp_millis) {
        Some(t) => s.serialize_str(&t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        None => s.serialize_none(),
    }
}

/// `<claude-home>/.sessio/hosts`.
pub fn hosts_root() -> PathBuf {
    local_claude_home().join(".sessio").join("hosts")
}

/// The stamp a finished pull leaves.
fn marker(home: &Path) -> PathBuf {
    home.join(".sessio").join("pulled.json")
}

/// When `host` was last pulled and how many days it asked for, if it ever finished.
pub fn stamp(host: &str) -> Option<Stamp> {
    serde_json::from_slice(&fs::read(marker(&host_home(host))).ok()?).ok()
}

/// Every host pulled before, by name.
pub fn pulled() -> Vec<Pulled> {
    let Ok(entries) = fs::read_dir(hosts_root()) else { return Vec::new() };
    let mut out: Vec<Pulled> = entries
        .flatten()
        .filter(|e| e.path().join("projects").is_dir())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|h| valid_host(h))
        .map(|host| {
            let s = stamp(&host);
            Pulled { at: s.map(|s| s.at), days: s.map(|s| s.days), host }
        })
        .collect();
    out.sort_by(|a, b| a.host.cmp(&b.host));
    out
}

/// The remote side of a pull: list the transcripts sessio reads (one folder deep, as
/// `discover::scan` walks them), changed in the last `days` days, or all of them for `0`.
/// `days` is a number, and nothing else from outside reaches this string.
pub fn remote_find(days: u32) -> String {
    let age = if days == 0 { String::new() } else { format!(" -mtime -{days}") };
    format!(
        "cd .claude/projects 2>/dev/null || exit 3; \
         find . -mindepth 2 -maxdepth 2 -type f -name '*.jsonl'{age}"
    )
}

/// Keep only the paths `remote_find` can produce: `./<folder>/<file>.jsonl`, nothing that climbs
/// out or hides another level. The other machine is trusted, but its answer still decides what
/// gets written here. A leading `-` is fine: the list goes to rsync in a file, never as arguments.
pub fn safe_paths(listing: &str) -> Vec<String> {
    listing
        .lines()
        .filter_map(|l| l.strip_prefix("./"))
        .filter(|p| {
            let parts: Vec<&str> = p.split('/').collect();
            parts.len() == 2
                && parts.iter().all(|s| !s.is_empty() && *s != "." && *s != "..")
                && p.ends_with(".jsonl")
        })
        .map(str::to_string)
        .collect()
}

/// The ssh options every call uses: never prompt (a pull runs from scripts and agents too), and
/// give up quickly on a machine that is asleep.
const SSH_OPTS: &[&str] = &["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"];

/// Copy `host`'s transcripts from the last `days` days (all for `0`) into `host_home(host)`.
pub fn pull(host: &str, days: u32) -> Result<Report, String> {
    if !valid_host(host) {
        return Err(format!("'{host}' is not a host name (letters, digits, '.', '_', '-')"));
    }
    let home = host_home(host);
    let projects = home.join("projects");
    make_private_dir(&projects).map_err(|e| format!("can't create {}: {e}", projects.display()))?;

    let listed = Command::new("ssh")
        .args(SSH_OPTS)
        .arg(host)
        .arg(remote_find(days))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("couldn't run ssh: {e}"))?;
    match listed.status.code() {
        Some(0) => {}
        Some(3) => return Err(format!("{host} has no ~/.claude/projects")),
        _ => return Err(format!("ssh {host} failed: {}", first_line(&listed.stderr, &listed.status))),
    }
    let files = safe_paths(&String::from_utf8_lossy(&listed.stdout));

    if !files.is_empty() {
        let list = home.join(".sessio").join("pull-list");
        make_private_dir(list.parent().expect("has a parent"))
            .map_err(|e| format!("can't write the file list: {e}"))?;
        fs::write(&list, files.join("\n") + "\n").map_err(|e| format!("can't write the file list: {e}"))?;
        // `-a` keeps the other machine's mtimes, which is what every age sessio shows is read from.
        let synced = Command::new("rsync")
            .arg("-a")
            .arg("-e")
            .arg(format!("ssh {}", SSH_OPTS.join(" ")))
            .arg(format!("--files-from={}", list.display()))
            .arg(format!("{host}:.claude/projects/"))
            .arg(format!("{}/", projects.display()))
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("couldn't run rsync: {e}"))?;
        let _ = fs::remove_file(&list);
        if !synced.status.success() {
            return Err(format!("rsync from {host} failed: {}", first_line(&synced.stderr, &synced.status)));
        }
    }

    let metas: Vec<fs::Metadata> = files.iter().filter_map(|f| fs::metadata(projects.join(f)).ok()).collect();
    let bytes = metas.iter().map(|m| m.len()).sum();
    let oldest = metas.iter().map(crate::discover::mtime_ms).min();
    let s = Stamp { at: crate::model::now_ms(), days };
    crate::store::save_private_json(&marker(&home), &s);
    Ok(Report { host: host.to_string(), files: files.len(), bytes, into: home, days, oldest })
}

/// Transcripts are private: the copy is owner-only, like `~/.claude` itself.
fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // From the hosts folder down, so no level above the copies is left open.
        let root = hosts_root();
        let mut d = Some(dir);
        while let Some(p) = d {
            if !p.starts_with(&root) {
                break;
            }
            fs::set_permissions(p, fs::Permissions::from_mode(0o700))?;
            d = p.parent();
        }
    }
    Ok(())
}

fn first_line(stderr: &[u8], status: &std::process::ExitStatus) -> String {
    let said = crate::safety::sanitize(&String::from_utf8_lossy(stderr));
    match said.lines().map(str::trim).find(|l| !l.is_empty()) {
        Some(l) => l.chars().take(160).collect(),
        None => format!("{status}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_remote_command_takes_only_a_number() {
        assert!(remote_find(14).ends_with("-name '*.jsonl' -mtime -14"));
        assert!(!remote_find(0).contains("mtime"));
    }

    #[test]
    fn only_transcripts_one_folder_deep_are_copied() {
        let got = safe_paths(
            "./-Users-me-app/a.jsonl\n\
             ./-Users-me-app/sub/b.jsonl\n\
             ./../etc/c.jsonl\n\
             ./-Users-me-app/../d.jsonl\n\
             ./x/notes.txt\n\
             /abs/e.jsonl\n\
             ./-Users-me-other/f.jsonl\n",
        );
        assert_eq!(got, ["-Users-me-app/a.jsonl", "-Users-me-other/f.jsonl"]);
    }
}
