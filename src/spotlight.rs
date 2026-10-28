//! Spotlight launchers for open sessions (macOS + Ghostty): `sessions spotlight install|sync|
//! uninstall|status`.
//!
//! `~/Sessions` holds one small app per running Claude Code session, named
//! `<project> — <title>.app`, so Spotlight finds a session by its project or title words. Each
//! app's executable is a copy of `sessio-focus`, which hands the session id to the helper app
//! (`~/Applications/Sessio Focus.app`, kept running by launchd as `com.sessio.focus`); the helper
//! focuses that session's Ghostty pane. How the two halves talk is in `src/bin/sessio-focus.rs`.
//!
//! Only open sessions get a launcher, and `sync` removes the ones whose session has closed, so a
//! search that finds nothing means nothing by that name is running. A hook keeps it current:
//! `sessions spotlight sync` next to `sessions layout save --auto`.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const LABEL: &str = "com.sessio.focus";
const APP_NAME: &str = "Sessio Focus.app";
const EXE: &str = "sessio-focus";
const GHOSTTY_ICON: &str = "/Applications/Ghostty.app/Contents/Resources/Ghostty.icns";

/// Where everything lives. Tests point it at a temporary folder.
#[derive(Debug, Clone)]
pub struct Paths {
    /// The launchers Spotlight indexes.
    pub launchers: PathBuf,
    /// The helper app.
    pub app: PathBuf,
    /// The LaunchAgent that keeps the helper running.
    pub agent: PathBuf,
}

impl Paths {
    pub fn for_home(home: &Path) -> Paths {
        Paths {
            launchers: home.join("Sessions"),
            app: home.join("Applications").join(APP_NAME),
            agent: home.join("Library/LaunchAgents").join(format!("{LABEL}.plist")),
        }
    }

    pub fn user() -> Paths {
        Paths::for_home(&PathBuf::from(std::env::var_os("HOME").unwrap_or_default()))
    }

    fn helper_exe(&self) -> PathBuf {
        self.app.join("Contents/MacOS").join(EXE)
    }

    pub fn installed(&self) -> bool {
        self.helper_exe().is_file()
    }
}

/// A running session that should have a launcher.
#[derive(Debug, Clone)]
pub struct Open {
    pub id: String,
    pub project: String,
    pub title: String,
}

/// A title as a file name: no `/` or `:` (Finder shows `:` as `/`), no line breaks, at most 80
/// characters.
pub fn clean(s: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in s.chars() {
        if matches!(c, '/' | ':' | '\n' | '\r' | '\t') || c.is_whitespace() {
            gap = !out.is_empty();
        } else {
            if gap {
                out.push(' ');
                gap = false;
            }
            out.push(c);
        }
    }
    out.chars().take(80).collect::<String>().trim_end().to_string()
}

/// `(session id, bundle file name)` for each session. Sorted by id first, so when two sessions
/// share a name the same one keeps the plain name on every sync instead of the two swapping.
pub fn launcher_names(open: &[Open]) -> Vec<(String, String)> {
    let mut sorted: Vec<&Open> = open.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    let mut taken = std::collections::HashSet::new();
    let mut out = Vec::new();
    for s in sorted {
        let project = match clean(&s.project) {
            p if p.is_empty() => "session".to_string(),
            p => p,
        };
        let title = match clean(&s.title) {
            t if t.is_empty() => s.id.chars().take(8).collect(),
            t => t,
        };
        let mut name = format!("{project} — {title}");
        if !taken.insert(name.clone()) {
            name = format!("{name} ({})", s.id.chars().take(8).collect::<String>());
            taken.insert(name.clone());
        }
        out.push((s.id.clone(), format!("{name}.app")));
    }
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

enum Val<'a> {
    Str(&'a str),
    Bool(bool),
}

fn plist(entries: &[(&str, Val)]) -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n",
    );
    for (k, v) in entries {
        s.push_str(&format!("\t<key>{}</key>\n", xml_escape(k)));
        match v {
            Val::Str(v) => s.push_str(&format!("\t<string>{}</string>\n", xml_escape(v))),
            Val::Bool(true) => s.push_str("\t<true/>\n"),
            Val::Bool(false) => s.push_str("\t<false/>\n"),
        }
    }
    s.push_str("</dict>\n</plist>\n");
    s
}

/// The string after `<key>{key}</key>` in an XML plist this module wrote.
pub fn plist_string(xml: &str, key: &str) -> Option<String> {
    let rest = &xml[xml.find(&format!("<key>{key}</key>"))?..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")?;
    Some(xml_unescape(&rest[start..start + end]))
}

/// Copy `src` to `dst` through a temporary file, so a running copy is never rewritten in place.
fn place_exe(src: &Path, dst: &Path) -> Result<(), String> {
    let tmp = dst.with_extension("new");
    fs::copy(src, &tmp).map_err(|e| format!("can't copy {} to {}: {e}", src.display(), tmp.display()))?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    fs::rename(&tmp, dst).map_err(|e| format!("can't replace {}: {e}", dst.display()))
}

/// A copy of `src` that is its own file (one link) with the same bytes.
fn same_copy(src: &Path, dst: &Path) -> bool {
    let Ok(meta) = fs::metadata(dst) else { return false };
    meta.nlink() == 1 && fs::read(src).ok().is_some_and(|a| fs::read(dst).ok().is_some_and(|b| a == b))
}

/// A lock that a killed sync can't leave behind for good: it expires after two minutes.
struct Lock(PathBuf);

impl Lock {
    fn take(dir: &Path) -> Option<Lock> {
        let path = dir.join(".sync.lock");
        if let Ok(meta) = fs::metadata(&path) {
            if meta.modified().ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age.as_secs() > 120) {
                let _ = fs::remove_dir(&path);
            }
        }
        fs::create_dir(&path).ok().map(|_| Lock(path))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

/// The launchers this module made: bundle file name → session id.
fn existing(dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            if !name.ends_with(".app") {
                return None;
            }
            let xml = fs::read_to_string(e.path().join("Contents/Info.plist")).ok()?;
            Some((name, plist_string(&xml, "SessionID")?))
        })
        .collect()
}

/// Make `~/Sessions` hold exactly one launcher per open session. Only bundles carrying a
/// `SessionID` are touched, so anything else the user keeps there is left alone. A bundle that
/// stays keeps its Spotlight entry; only an executable from an older build is replaced.
pub fn sync(p: &Paths, open: &[Open]) -> Result<String, String> {
    if !p.installed() {
        return Err("spotlight launchers are not installed — run `sessions spotlight install`".into());
    }
    fs::create_dir_all(&p.launchers).map_err(|e| format!("can't create {}: {e}", p.launchers.display()))?;
    let Some(_lock) = Lock::take(&p.launchers) else {
        return Ok("another sync is running".into());
    };
    let exe = p.helper_exe();
    let want = launcher_names(open);
    let (mut added, mut removed, mut refreshed) = (0, 0, 0);

    let have = existing(&p.launchers);
    for (name, sid) in &have {
        if !want.iter().any(|(id, n)| id == sid && n == name) {
            fs::remove_dir_all(p.launchers.join(name)).map_err(|e| format!("can't remove {name}: {e}"))?;
            removed += 1;
        }
    }
    for (id, name) in &want {
        let contents = p.launchers.join(name).join("Contents");
        let target = contents.join("MacOS").join(EXE);
        if have.iter().any(|(n, sid)| n == name && sid == id) {
            if !same_copy(&exe, &target) {
                place_exe(&exe, &target)?;
                refreshed += 1;
            }
            continue;
        }
        fs::create_dir_all(contents.join("MacOS")).map_err(|e| e.to_string())?;
        fs::create_dir_all(contents.join("Resources")).map_err(|e| e.to_string())?;
        if Path::new(GHOSTTY_ICON).is_file() {
            let _ = fs::copy(GHOSTTY_ICON, contents.join("Resources/app.icns"));
        }
        // Each launcher gets its own copy: LaunchServices treats bundles sharing one executable
        // file (hard links) as the same app, and opening a second one fails with -600.
        place_exe(&exe, &target)?;
        let short: String = id.chars().take(8).collect();
        let info = plist(&[
            ("CFBundleExecutable", Val::Str(EXE)),
            ("CFBundleIdentifier", Val::Str(&format!("com.sessio.launcher.{short}"))),
            ("CFBundleName", Val::Str(name.trim_end_matches(".app"))),
            ("CFBundlePackageType", Val::Str("APPL")),
            ("CFBundleIconFile", Val::Str("app.icns")),
            ("LSUIElement", Val::Bool(true)),
            ("SessionID", Val::Str(id)),
        ]);
        fs::write(contents.join("Info.plist"), info).map_err(|e| e.to_string())?;
        added += 1;
    }
    Ok(format!(
        "{} launchers in {} (added {added}, removed {removed}, refreshed {refreshed})",
        want.len(),
        p.launchers.display()
    ))
}

fn launchctl(args: &[&str]) -> Result<(), String> {
    let o = Command::new("/bin/launchctl").args(args).output().map_err(|e| format!("launchctl: {e}"))?;
    if o.status.success() {
        Ok(())
    } else {
        Err(format!("launchctl {}: {}", args.join(" "), String::from_utf8_lossy(&o.stderr).trim()))
    }
}

fn gui_domain() -> String {
    let uid = Command::new("/usr/bin/id").arg("-u").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    format!("gui/{}", uid.unwrap_or_default())
}

/// `sessio-focus` next to the running `sessions` binary (npm, Homebrew and cargo all install the
/// two side by side).
pub fn bundled_focus_binary() -> Option<PathBuf> {
    let exe = fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    let p = exe.parent()?.join(EXE);
    p.is_file().then_some(p)
}

/// Install (or update) the helper app and its LaunchAgent, then sync the launchers.
pub fn install(p: &Paths, focus: &Path, open: &[Open]) -> Result<String, String> {
    let contents = p.app.join("Contents");
    fs::create_dir_all(contents.join("MacOS")).map_err(|e| format!("can't create {}: {e}", p.app.display()))?;
    fs::create_dir_all(contents.join("Resources")).map_err(|e| e.to_string())?;
    // The binary carries its own ad-hoc signature; a bundle seal left from an older build would no
    // longer match and stop the app from launching.
    let _ = fs::remove_dir_all(contents.join("_CodeSignature"));
    place_exe(focus, &p.helper_exe())?;
    if Path::new(GHOSTTY_ICON).is_file() {
        let _ = fs::copy(GHOSTTY_ICON, contents.join("Resources/app.icns"));
    }
    let info = plist(&[
        ("CFBundleExecutable", Val::Str(EXE)),
        ("CFBundleIdentifier", Val::Str(LABEL)),
        ("CFBundleName", Val::Str("Sessio Focus")),
        ("CFBundlePackageType", Val::Str("APPL")),
        ("CFBundleIconFile", Val::Str("app.icns")),
        ("LSUIElement", Val::Bool(true)),
        ("LSBackgroundOnly", Val::Bool(true)),
        (
            "NSAppleEventsUsageDescription",
            Val::Str("Sessio Focus brings the Ghostty pane of the Claude Code session you picked to the front."),
        ),
    ]);
    fs::write(contents.join("Info.plist"), info).map_err(|e| e.to_string())?;

    if let Some(dir) = p.agent.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let helper = p.helper_exe();
    let agent = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\
         \t<key>Label</key>\n\t<string>{LABEL}</string>\n\
         \t<key>Program</key>\n\t<string>{}</string>\n\
         \t<key>RunAtLoad</key>\n\t<true/>\n\t<key>KeepAlive</key>\n\t<true/>\n\
         \t<key>ProcessType</key>\n\t<string>Interactive</string>\n\
         \t<key>ThrottleInterval</key>\n\t<integer>1</integer>\n</dict>\n</plist>\n",
        xml_escape(&helper.display().to_string())
    );
    fs::write(&p.agent, agent).map_err(|e| format!("can't write {}: {e}", p.agent.display()))?;
    let domain = gui_domain();
    let _ = launchctl(&["bootout", &format!("{domain}/{LABEL}")]);
    launchctl(&["bootstrap", &domain, &p.agent.display().to_string()])?;
    let synced = sync(p, open)?;
    Ok(format!(
        "installed {} and its LaunchAgent {LABEL}\n{synced}\n\
         The first jump asks to let Sessio Focus control Ghostty: allow it. Keep the launchers \
         current from a SessionStart/Stop hook: `sessions spotlight sync`.",
        p.app.display()
    ))
}

/// Stop the helper and remove it, its LaunchAgent and every launcher this module made.
pub fn uninstall(p: &Paths) -> Result<String, String> {
    let _ = launchctl(&["bootout", &format!("{}/{LABEL}", gui_domain())]);
    let _ = fs::remove_file(&p.agent);
    let mut removed = 0;
    for (name, _) in existing(&p.launchers) {
        if fs::remove_dir_all(p.launchers.join(&name)).is_ok() {
            removed += 1;
        }
    }
    let _ = fs::remove_dir(&p.launchers); // only if nothing else is in it
    let _ = fs::remove_dir_all(&p.app);
    Ok(format!("removed {}, its LaunchAgent and {removed} launchers", p.app.display()))
}

/// What `status` prints.
pub fn status(p: &Paths) -> String {
    if !p.installed() {
        return "not installed — `sessions spotlight install` sets it up".into();
    }
    let launchers = existing(&p.launchers).len();
    let running = Command::new("/bin/launchctl")
        .args(["print", &format!("{}/{LABEL}", gui_domain())])
        .output()
        .ok()
        .is_some_and(|o| String::from_utf8_lossy(&o.stdout).contains("state = running"));
    format!(
        "installed: {}\nhelper: {}\nlaunchers: {launchers} in {}",
        p.app.display(),
        if running { "running" } else { "not running" },
        p.launchers.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(id: &str, project: &str, title: &str) -> Open {
        Open { id: id.into(), project: project.into(), title: title.into() }
    }

    #[test]
    fn cleans_titles_into_file_names() {
        assert_eq!(clean("a/b: c\n\td"), "a b c d");
        assert_eq!(clean("  spaced  "), "spaced");
        assert_eq!(clean(&"x".repeat(100)).len(), 80);
    }

    #[test]
    fn names_are_stable_for_duplicate_titles() {
        let a = open("bbbb2222-x", "p", "Same");
        let b = open("aaaa1111-x", "p", "Same");
        let n1 = launcher_names(&[a.clone(), b.clone()]);
        let n2 = launcher_names(&[b, a]);
        assert_eq!(n1, n2);
        assert_eq!(n1[0], ("aaaa1111-x".into(), "p — Same.app".into()));
        assert_eq!(n1[1], ("bbbb2222-x".into(), "p — Same (bbbb2222).app".into()));
        assert_eq!(launcher_names(&[open("cccc3333-y", "", "")])[0].1, "session — cccc3333.app");
    }

    #[test]
    fn plist_round_trips_escaped_strings() {
        let xml = plist(&[("SessionID", Val::Str("a<b>&c")), ("LSUIElement", Val::Bool(true))]);
        assert_eq!(plist_string(&xml, "SessionID").as_deref(), Some("a<b>&c"));
        assert_eq!(plist_string(&xml, "Missing"), None);
    }

    fn fake_install(root: &Path) -> Paths {
        let p = Paths::for_home(root);
        fs::create_dir_all(p.helper_exe().parent().unwrap()).unwrap();
        fs::write(p.helper_exe(), b"build-1").unwrap();
        p
    }

    #[test]
    fn sync_adds_removes_and_refreshes_only_its_own_launchers() {
        let root = std::env::temp_dir().join(format!("sessio-spotlight-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let p = fake_install(&root);
        assert!(sync(&Paths::for_home(&root.join("nope")), &[]).is_err(), "refuses when not installed");

        let s1 = open("11111111-a", "mybit", "Maika messages");
        let s2 = open("22222222-b", "citymaps", "Arena: research");
        sync(&p, &[s1.clone(), s2.clone()]).unwrap();
        let mut names: Vec<String> = existing(&p.launchers).into_iter().map(|(n, _)| n).collect();
        names.sort();
        assert_eq!(names, ["citymaps — Arena research.app", "mybit — Maika messages.app"]);
        let exe = p.launchers.join("mybit — Maika messages.app/Contents/MacOS").join(EXE);
        assert_eq!(fs::read(&exe).unwrap(), b"build-1");
        assert_eq!(fs::metadata(&exe).unwrap().nlink(), 1, "a copy, not a hard link");

        // Something the user put there is not ours and stays.
        fs::create_dir_all(p.launchers.join("Mine.app/Contents")).unwrap();
        // s2 closes, the helper is updated: s2's launcher goes, s1's executable is refreshed.
        fs::write(p.helper_exe(), b"build-2").unwrap();
        let msg = sync(&p, &[s1]).unwrap();
        assert!(msg.contains("removed 1") && msg.contains("refreshed 1"), "{msg}");
        assert_eq!(fs::read(&exe).unwrap(), b"build-2");
        assert!(!p.launchers.join("citymaps — Arena research.app").exists());
        assert!(p.launchers.join("Mine.app").exists());

        // A lock left by a killed run blocks only until it expires.
        fs::create_dir(p.launchers.join(".sync.lock")).unwrap();
        assert_eq!(sync(&p, &[]).unwrap(), "another sync is running");
        let _ = fs::remove_dir_all(&root);
    }
}
