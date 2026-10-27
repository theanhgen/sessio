//! Save the Ghostty layout — windows, tabs, splits and the Claude session in each pane — and
//! rebuild it later, resuming every session in its place.
//!
//! Two sources, because neither has the whole picture:
//!
//! * Ghostty's AppleScript dictionary (1.3+) lists windows, tabs and each tab's terminals with
//!   their tty and folder, and can make windows, tabs and splits. It says nothing about how the
//!   panes of a tab are arranged.
//! * The macOS accessibility tree has the arrangement: Ghostty draws each split as a group with a
//!   `Left pane`/`Right pane` or `Top pane`/`Bottom pane` pair, with real frames, so the direction
//!   and ratio of every split can be read back. Its leaves come in the same order as the
//!   AppleScript terminals, which is what ties a pane to its tty and so to its session.
//!
//! The ratio is restored through Ghostty's `resize_split` action, which only moves the nearest
//! divider of matching orientation above the pane it is sent to. Once both sides of a split are
//! themselves split the same way, its divider can no longer be reached — so restore sizes each
//! split the moment it makes it, while both sides are still single panes.
//!
//! Without accessibility permission a save still records every pane and its session, just not the
//! arrangement; restore then lays a tab's panes out as an even grid.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ---------- the saved file ----------

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    /// Side by side: `a` on the left.
    Right,
    /// Stacked: `a` on top.
    Down,
}

impl Dir {
    pub fn word(self) -> &'static str {
        match self {
            Dir::Right => "right",
            Dir::Down => "down",
        }
    }
}

/// A tab's split tree. Leaves index into the tab's `panes`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Node {
    Leaf(usize),
    Split { dir: Dir, ratio: f64, a: Box<Node>, b: Box<Node> },
}

impl Node {
    pub fn leaves(&self) -> Vec<usize> {
        match self {
            Node::Leaf(i) => vec![*i],
            Node::Split { a, b, .. } => [a.leaves(), b.leaves()].concat(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Pane {
    pub cwd: String,
    /// The Claude Code session running in this pane when it was saved; `None` for a plain shell.
    #[serde(default)]
    pub session: Option<String>,
    /// The flags that `claude` was started with, minus any resume flag, so a session started with
    /// `--dangerously-skip-permissions` comes back with it.
    #[serde(default)]
    pub flags: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Tab {
    pub panes: Vec<Pane>,
    /// `None` when the arrangement could not be read; restore falls back to a grid.
    #[serde(default)]
    pub tree: Option<Node>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Window {
    /// x, y, width, height in screen points, when accessibility could read it.
    #[serde(default)]
    pub frame: Option<[f64; 4]>,
    pub selected_tab: usize,
    pub tabs: Vec<Tab>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Layout {
    pub version: u32,
    pub saved_at: String,
    pub windows: Vec<Window>,
}

pub fn default_file() -> PathBuf {
    crate::store::state_dir().join("layout.json")
}

/// Where a save moves the layout it replaces: `layout.json` → `layout.prev.json`.
pub fn prev_file(file: &Path) -> PathBuf {
    file.with_extension("prev.json")
}

pub fn session_count(l: &Layout) -> usize {
    l.windows.iter().flat_map(|w| &w.tabs).flat_map(|t| &t.panes).filter(|p| p.session.is_some()).count()
}

/// The split tree an earlier save read for a tab with exactly these panes. `save --auto` keeps a
/// background tab's arrangement this way instead of switching to it on the user's screen.
/// Only an unambiguous match counts: two tabs of plain shells in the same folders can be split
/// differently, and picking the first would give one the other's arrangement.
pub fn known_tree(prev: Option<&Layout>, panes: &[Pane]) -> Option<Node> {
    let mut hits = prev?.windows.iter().flat_map(|w| &w.tabs).filter(|t| t.panes == panes);
    match (hits.next(), hits.next()) {
        (Some(t), None) => t.tree.clone(),
        _ => None,
    }
}

/// Whether a save should copy the layout it replaces (`current`) to `layout.prev.json`.
/// A manual save always does. An unattended one never lets a smaller layout push a bigger one out
/// of the backup, so while sessions close one by one (Ghostty quitting, windows being closed)
/// the backup keeps the biggest layout of the run.
pub fn rotate_prev(auto: bool, current: &Layout, backup: Option<&Layout>) -> bool {
    !auto || backup.is_none_or(|b| session_count(current) >= session_count(b))
}

/// How long after Ghostty starts an unattended save still treats a smaller layout as "not yet
/// restored" and keeps the saved one. Past it the user has clearly moved on, and saving resumes
/// (the bigger layout stays in the backup).
pub const REBOOT_GRACE_SECS: u64 = 30 * 60;

/// Whether an unattended save keeps the saved layout instead of writing `new_count` sessions: the
/// file predates this Ghostty, Ghostty started within the grace period, and the new one is smaller.
pub fn keep_pre_reboot(file_predates_ghostty: bool, ghostty_uptime_secs: u64, saved_count: usize, new_count: usize) -> bool {
    file_predates_ghostty && ghostty_uptime_secs < REBOOT_GRACE_SECS && new_count < saved_count
}

/// Elapsed time as `ps -o etime=` prints it, `[[dd-]hh:]mm:ss`, in seconds.
pub fn parse_etime(s: &str) -> Option<u64> {
    let s = s.trim();
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, s),
    };
    let mut secs = 0;
    for part in rest.split(':') {
        secs = secs * 60 + part.parse::<u64>().ok()?;
    }
    Some(days * 86_400 + secs)
}

pub fn load(file: &Path) -> Result<Layout, String> {
    let body = std::fs::read_to_string(file).map_err(|e| format!("can't read {}: {e}", file.display()))?;
    let l: Layout =
        serde_json::from_str(&body).map_err(|e| format!("{} is not a saved layout: {e}", file.display()))?;
    if l.version != 1 {
        return Err(format!("{} is layout version {}; this sessio reads version 1", file.display(), l.version));
    }
    for (wi, w) in l.windows.iter().enumerate() {
        for (ti, t) in w.tabs.iter().enumerate() {
            if t.panes.is_empty() {
                return Err(format!("window {wi} tab {ti} has no panes"));
            }
            if let Some(tree) = &t.tree {
                let mut seen = tree.leaves();
                seen.sort_unstable();
                if seen != (0..t.panes.len()).collect::<Vec<_>>() {
                    return Err(format!("window {wi} tab {ti}: the split tree does not match its panes"));
                }
            }
        }
    }
    Ok(l)
}

/// The tree a tab is rebuilt with: the saved one, or an even grid of roughly √n columns.
pub fn tree_for(tab: &Tab) -> Node {
    tab.tree.clone().unwrap_or_else(|| grid(tab.panes.len()))
}

/// `n` panes as columns of stacked panes, filled column by column, every column the same width
/// and every pane in a column the same height.
pub fn grid(n: usize) -> Node {
    let n = n.max(1);
    let cols = (n as f64).sqrt().ceil() as usize;
    let rows = n.div_ceil(cols);
    let columns: Vec<Vec<usize>> =
        (0..n).collect::<Vec<_>>().chunks(rows).map(|c| c.to_vec()).collect();
    let stacks: Vec<Node> =
        columns.iter().map(|c| even(c.iter().map(|&i| Node::Leaf(i)).collect(), Dir::Down)).collect();
    even(stacks, Dir::Right)
}

/// Equal shares along one direction, as a right-leaning chain: the first part takes 1/k.
fn even(mut parts: Vec<Node>, dir: Dir) -> Node {
    if parts.len() == 1 {
        return parts.pop().expect("one part");
    }
    let k = parts.len();
    let first = parts.remove(0);
    Node::Split { dir, ratio: 1.0 / k as f64, a: Box::new(first), b: Box::new(even(parts, dir)) }
}

/// The tree as indented text, one line per split and per pane.
pub fn describe(node: &Node, panes: &[Pane], live: &dyn Fn(&str) -> bool, indent: &str, out: &mut String) {
    match node {
        Node::Leaf(i) => {
            let p = &panes[*i];
            let folder = Path::new(&p.cwd).file_name().map(|f| f.to_string_lossy()).unwrap_or_default();
            let what = match &p.session {
                Some(id) if live(id) => format!("{} (running now; opens as a shell)", &id[..8.min(id.len())]),
                Some(id) => format!("claude {}", &id[..8.min(id.len())]),
                None => "shell".to_string(),
            };
            out.push_str(&format!("{indent}{folder}  {what}\n"));
        }
        Node::Split { dir, ratio, a, b } => {
            let how = match dir {
                Dir::Right => "side by side",
                Dir::Down => "stacked",
            };
            out.push_str(&format!("{indent}{how} {}%\n", (ratio * 100.0).round()));
            let deeper = format!("{indent}  ");
            describe(a, panes, live, &deeper, out);
            describe(b, panes, live, &deeper, out);
        }
    }
}

// ---------- reading what Ghostty reports ----------

/// One window as Ghostty's AppleScript lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptWindow {
    pub id: String,
    pub name: String,
    pub selected: usize,
    /// Each tab's terminals as (tty, folder), in Ghostty's order.
    pub tabs: Vec<Vec<(String, String)>>,
}

/// Fields are split by ASCII 31 and records by ASCII 30: window titles are whatever the program in
/// them sets — Claude Code puts its session title there — so no printable separator is safe.
pub const DUMP_SCRIPT: &str = r#"if application "Ghostty" is not running then return ""
set us to (ASCII character 31)
set rs to (ASCII character 30)
tell application "Ghostty"
  set out to ""
  repeat with w in windows
    set out to out & "W" & us & (id of w) & us & (name of w) & rs
    repeat with t in tabs of w
      set out to out & "T" & us & (selected of t) & rs
      repeat with s in terminals of t
        set out to out & "P" & us & (tty of s) & us & (working directory of s) & rs
      end repeat
    end repeat
  end repeat
  return out
end tell"#;

pub fn parse_dump(out: &str) -> Vec<ScriptWindow> {
    let mut wins: Vec<ScriptWindow> = Vec::new();
    for rec in out.split('\u{1e}') {
        let f: Vec<&str> = rec.trim_matches(['\n', '\r']).split('\u{1f}').collect();
        match (f[0], wins.last_mut()) {
            ("W", _) if f.len() >= 2 => wins.push(ScriptWindow {
                id: f[1].to_string(),
                name: f.get(2).unwrap_or(&"").to_string(),
                selected: 0,
                tabs: Vec::new(),
            }),
            ("T", Some(w)) => {
                w.tabs.push(Vec::new());
                if f.get(1) == Some(&"true") {
                    w.selected = w.tabs.len() - 1;
                }
            }
            ("P", Some(w)) if f.len() >= 3 => {
                if let Some(t) = w.tabs.last_mut() {
                    t.push((f[1].to_string(), f[2].to_string()));
                }
            }
            _ => {}
        }
    }
    wins.retain(|w| !w.tabs.is_empty());
    for w in &mut wins {
        w.tabs.retain(|t| !t.is_empty());
        w.selected = w.selected.min(w.tabs.len().saturating_sub(1));
    }
    wins.retain(|w| !w.tabs.is_empty());
    wins
}

/// `claude`'s argv minus its program name and any flag that picks a session, so the rest can be
/// passed again with `--resume <id>`.
pub fn keep_flags(argv: &[String]) -> Vec<String> {
    let mut keep = Vec::new();
    let mut it = argv.iter().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--resume" | "-r" | "--session-id" => {
                it.next();
            }
            "--continue" | "-c" => {}
            s if s.starts_with("--resume=") || s.starts_with("--session-id=") => {}
            _ => keep.push(a.clone()),
        }
    }
    keep
}

/// A title without Claude Code's leading status glyph (✳ idle, ◐◓◑◒ busy), which can change
/// between one read and the next.
pub fn bare_title(t: &str) -> &str {
    match t.split_once(' ') {
        Some((head, rest)) if head.chars().count() == 1 && !head.chars().any(char::is_alphanumeric) => rest,
        _ => t,
    }
}

// ---------- the accessibility tree, as plain data ----------

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// One accessibility element, read out of the live tree so that parsing it needs no macOS.
#[derive(Debug, Clone, Default)]
pub struct El {
    pub role: String,
    pub desc: String,
    pub rect: Rect,
    pub kids: Vec<El>,
}

/// The split tree Ghostty draws, with each node's frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Drawn {
    Pane(Rect),
    Split(Dir, Rect, Box<Drawn>, Box<Drawn>),
}

impl Drawn {
    pub fn rect(&self) -> Rect {
        match self {
            Drawn::Pane(r) | Drawn::Split(_, r, _, _) => *r,
        }
    }

    pub fn panes(&self) -> usize {
        match self {
            Drawn::Pane(_) => 1,
            Drawn::Split(_, _, a, b) => a.panes() + b.panes(),
        }
    }

    /// The split at `path` (false = side a), if the tree has one there.
    pub fn at(&self, path: &[bool]) -> Option<&Drawn> {
        match (path.split_first(), self) {
            (None, d) => Some(d),
            (Some((&right, rest)), Drawn::Split(_, _, a, b)) => if right { b } else { a }.at(rest),
            _ => None,
        }
    }

    pub fn to_node(&self) -> Node {
        let mut next = 0;
        self.number(&mut next)
    }

    fn number(&self, next: &mut usize) -> Node {
        match self {
            Drawn::Pane(_) => {
                *next += 1;
                Node::Leaf(*next - 1)
            }
            Drawn::Split(dir, _, a, b) => {
                let (ra, rb) = (a.rect(), b.rect());
                let ratio = match dir {
                    Dir::Right => ra.w / (ra.w + rb.w),
                    Dir::Down => ra.h / (ra.h + rb.h),
                };
                let ratio = if ratio.is_finite() { (ratio * 1000.0).round() / 1000.0 } else { 0.5 };
                let a = a.number(next);
                let b = b.number(next);
                Node::Split { dir: *dir, ratio, a: Box::new(a), b: Box::new(b) }
            }
        }
    }
}

const PANE_NAMES: [&str; 4] = ["Left pane", "Right pane", "Top pane", "Bottom pane"];

/// The split tree inside one element: a group holding a scroll area is a pane, a group holding a
/// named pair of pane groups is a split, anything else is looked through.
pub fn drawn(e: &El) -> Option<Drawn> {
    if e.kids.iter().any(|k| k.role == "AXScrollArea") {
        return Some(Drawn::Pane(e.rect));
    }
    let pair: Vec<&El> =
        e.kids.iter().filter(|k| k.role == "AXGroup" && PANE_NAMES.contains(&k.desc.as_str())).collect();
    if let [a, b] = pair[..] {
        let dir = if a.desc.starts_with("Left") || a.desc.starts_with("Right") { Dir::Right } else { Dir::Down };
        return Some(Drawn::Split(dir, e.rect, Box::new(drawn(a)?), Box::new(drawn(b)?)));
    }
    e.kids.iter().filter(|k| k.role == "AXGroup").find_map(drawn)
}

/// The tree of a window's visible tab. The tab bar is skipped: it is not a group.
pub fn window_tree(win: &El) -> Option<Drawn> {
    win.kids.iter().filter(|k| k.role == "AXGroup").find_map(drawn)
}

/// Tabs in a window, from its tab bar; a window without one has one tab.
pub fn tab_count(win: &El) -> usize {
    win.kids
        .iter()
        .find(|k| k.role == "AXTabGroup")
        .map(|bar| bar.kids.iter().filter(|k| k.role == "AXRadioButton").count())
        .unwrap_or(1)
}

/// What one accessibility window looks like, for pairing it with an AppleScript window.
#[derive(Debug, Clone)]
pub struct Seen {
    pub title: String,
    pub tabs: usize,
    pub panes: usize,
}

/// Pair each AppleScript window with the accessibility window that shows it. Neither side has an
/// id the other knows, so: the title (without its status glyph) and the visible tab's pane count
/// first; then, for what is left, the tab and pane counts when only one pair has them — titles are
/// free text the two APIs can render differently.
pub fn pair(script: &[ScriptWindow], seen: &[Seen]) -> Vec<Option<usize>> {
    let mut taken = vec![false; seen.len()];
    let mut out: Vec<Option<usize>> = script
        .iter()
        .map(|w| {
            let panes = w.tabs[w.selected].len();
            let hit = seen
                .iter()
                .enumerate()
                .position(|(i, s)| !taken[i] && bare_title(&s.title) == bare_title(&w.name) && s.panes == panes)?;
            taken[hit] = true;
            Some(hit)
        })
        .collect();
    for i in 0..script.len() {
        if out[i].is_some() {
            continue;
        }
        let shape = |w: &ScriptWindow| (w.tabs.len(), w.tabs[w.selected].len());
        let want = shape(&script[i]);
        let free: Vec<usize> =
            (0..seen.len()).filter(|&j| !taken[j] && (seen[j].tabs, seen[j].panes) == want).collect();
        let rivals = (0..script.len()).filter(|&k| out[k].is_none() && shape(&script[k]) == want).count();
        if let (&[j], 1) = (free.as_slice(), rivals) {
            taken[j] = true;
            out[i] = Some(j);
        }
    }
    out
}

// ---------- what a pane runs ----------

/// The command a restored pane runs, through the same login-shell launcher `^o` uses. A session
/// that is already running somewhere gets a shell in its folder instead: a second `claude` on the
/// same transcript would have both appending to it.
pub fn pane_command(p: &Pane, running: bool) -> String {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let argv: Vec<String> = match (&p.session, running) {
        (Some(id), false) => {
            let mut v = vec!["claude".to_string()];
            v.extend(p.flags.iter().cloned());
            v.extend(["--resume".to_string(), id.clone()]);
            v
        }
        _ => vec![shell.clone(), "-l".to_string()],
    };
    crate::resume::shell_join(&crate::resume::window_command(&shell, Path::new(&p.cwd), &argv))
}

/// The tty → (session, flags) map for the running Claude sessions.
pub fn sessions_by_tty(live: &crate::live::LiveMap, argv_of: &dyn Fn(i32) -> Option<Vec<String>>) -> HashMap<String, (String, Vec<String>)> {
    let mut out = HashMap::new();
    for (id, l) in live {
        if l.tty.is_empty() {
            continue;
        }
        let flags = argv_of(l.pid).map(|a| keep_flags(&a)).unwrap_or_default();
        out.insert(format!("/dev/{}", l.tty.trim_start_matches("/dev/")), (id.clone(), flags));
    }
    out
}

/// One save at a time: several sessions reach `Stop` together, and each hook starts a save.
/// The lock is the layout file's name plus `.lock` (`layout.json.lock`), created exclusively, so it
/// can never be the file being saved. A lock older than two minutes is a crashed save: it is
/// removed and taken, even by a save that does not wait. Each lock holds a token of its own, and
/// a save only removes the lock if it still holds its token, so a save that outlived the two
/// minutes cannot delete the lock of the save that took over.
pub struct Lock {
    path: PathBuf,
    token: String,
}

/// Why a lock could not be taken.
#[derive(Debug, PartialEq)]
pub enum LockError {
    /// Another save holds it.
    Busy,
    /// The lock file could not be created at all (a read-only or unwritable folder).
    Io(String),
}

pub fn lock_file(file: &Path) -> PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    file.with_file_name(name)
}

impl Lock {
    pub fn take(file: &Path, wait: bool) -> Result<Lock, LockError> {
        use std::io::{ErrorKind, Write};
        use std::time::{Duration, SystemTime, UNIX_EPOCH};
        let path = lock_file(file);
        let io = |e: std::io::Error| LockError::Io(format!("{}: {e}", path.display()));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let token = format!("{} {nanos}", std::process::id());
        let mut tries = if wait { 100 } else { 1 };
        while tries > 0 {
            tries -= 1;
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    f.write_all(token.as_bytes()).map_err(io)?;
                    return Ok(Lock { path, token });
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .is_ok_and(|t| t.elapsed().is_ok_and(|e| e > Duration::from_secs(120)));
                    if stale && std::fs::remove_file(&path).is_ok() {
                        tries = tries.max(1);
                        continue;
                    }
                    if wait {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                Err(e) => return Err(io(e)),
            }
        }
        Err(LockError::Busy)
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        if std::fs::read_to_string(&self.path).is_ok_and(|t| t == self.token) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

// ---------- Ghostty ----------

#[cfg(target_os = "macos")]
pub use mac::{restore, save};

#[cfg(not(target_os = "macos"))]
pub fn save(_file: &Path, _auto: bool) -> Result<String, String> {
    Err("saving a layout needs Ghostty on macOS".into())
}

#[cfg(not(target_os = "macos"))]
pub fn restore(_file: &Path) -> Result<String, String> {
    Err("restoring a layout needs Ghostty on macOS".into())
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use std::process::{Command, Stdio};
    use std::thread::sleep;
    use std::time::Duration;

    use ax::Ax;

    fn osa(script: &str, argv: &[&str]) -> Result<String, String> {
        let out = Command::new("osascript")
            .args(["-e", script])
            .args(argv)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("osascript: {e}"))?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string());
        }
        let err = String::from_utf8_lossy(&out.stderr);
        let why = err.rsplit("error: ").next().unwrap_or("").trim();
        Err(format!("Ghostty: {}", if why.is_empty() { "no answer" } else { why }))
    }

    fn ghostty_pid() -> Option<i32> {
        let out = Command::new("ps").args(["-Ao", "pid=,comm="]).stdin(Stdio::null()).output().ok()?;
        String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
            let (pid, comm) = l.trim().split_once(' ')?;
            comm.trim().ends_with("/Ghostty.app/Contents/MacOS/ghostty").then(|| pid.parse().ok())?
        })
    }

    fn argv_of(pid: i32) -> Option<Vec<String>> {
        let out = Command::new("ps").args(["-o", "args=", "-p", &pid.to_string()]).output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        let v: Vec<String> = s.split_whitespace().map(str::to_string).collect();
        (!v.is_empty()).then_some(v)
    }

    /// The folder of the first process on `tty` that is not `login`: the shell, or the `claude` the
    /// launcher exec'd into. Processes it starts later (MCP servers) come after it by pid.
    fn cwd_of_tty(tty: &str) -> Option<String> {
        let tty = tty.trim_start_matches("/dev/");
        let out = Command::new("ps").args(["-o", "pid=,comm=", "-t", tty]).stdin(Stdio::null()).output().ok()?;
        let list = String::from_utf8_lossy(&out.stdout).into_owned();
        list.lines().filter(|l| !l.trim_end().ends_with("login")).find_map(|l| {
            let pid = l.split_whitespace().next()?;
            let out = Command::new("lsof").args(["-a", "-p", pid, "-d", "cwd", "-Fn"]).stdin(Stdio::null()).output().ok()?;
            String::from_utf8_lossy(&out.stdout).lines().find_map(|l| l.strip_prefix('n').map(str::to_string))
        })
    }

    fn select_tab(win: &str, index: usize) -> Result<(), String> {
        osa(
            r#"on run argv
  tell application "Ghostty" to select tab (tab ((item 2 of argv) as integer) of (first window whose id is (item 1 of argv)))
end run"#,
            &[win, &(index + 1).to_string()],
        )
        .map(|_| ())
    }

    fn window_name(win: &str) -> Result<String, String> {
        osa(
            r#"on run argv
  tell application "Ghostty" to return name of (first window whose id is (item 1 of argv))
end run"#,
            &[win],
        )
    }

    /// The accessibility window showing `win`'s selected tab now. Native macOS tabs are separate
    /// windows to accessibility, so this changes whenever another tab is selected.
    fn shown(ax: &Ax, win: &str, near: Option<Rect>) -> Option<ax::Element> {
        let name = window_name(win).ok()?;
        let hits: Vec<ax::Element> =
            ax.windows().into_iter().filter(|w| bare_title(&w.title()) == bare_title(&name)).collect();
        if hits.len() > 1 {
            if let Some(r) = near {
                if let Some(i) = hits.iter().position(|w| w.read().rect == r) {
                    return hits.into_iter().nth(i);
                }
            }
        }
        hits.into_iter().next()
    }

    /// How long Ghostty's process has been running, from `ps`.
    fn uptime(pid: i32) -> Option<u64> {
        let out = Command::new("ps").args(["-o", "etime=", "-p", &pid.to_string()]).stdin(Stdio::null()).output().ok()?;
        parse_etime(&String::from_utf8_lossy(&out.stdout))
    }

    pub fn save(file: &Path, auto: bool) -> Result<String, String> {
        let _lock = match Lock::take(file, !auto) {
            Ok(lock) => lock,
            Err(LockError::Busy) if auto => return Ok("another save is running; nothing saved".into()),
            Err(LockError::Busy) => return Err("another save is running".into()),
            Err(LockError::Io(why)) => return Err(format!("couldn't take the save lock {why}")),
        };
        // A save that queued behind another one has nothing to add.
        let just_saved = std::fs::metadata(file)
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().is_ok_and(|e| e < Duration::from_secs(30)));
        if auto && just_saved {
            return Ok("saved moments ago; nothing to add".into());
        }
        capture(file, auto)
    }

    /// `auto` is the unattended save a hook runs: it never starts Ghostty, never switches tabs on
    /// the user's screen, gives up rather than write a layout it could not read whole (Ghostty
    /// quitting under it, no Accessibility), and won't let a fresh Ghostty — just after a reboot,
    /// before `restore` — replace the saved layout with fewer sessions.
    fn capture(file: &Path, auto: bool) -> Result<String, String> {
        let pid = ghostty_pid();
        if auto && pid.is_none() {
            return Ok("Ghostty is not running; nothing saved".into());
        }
        let prev = load(file).ok();
        let script = parse_dump(&osa(DUMP_SCRIPT, &[])?);
        if script.is_empty() {
            return if auto { Ok("Ghostty has no windows open; nothing saved".into()) } else { Err("Ghostty has no windows open".into()) };
        }
        let by_tty = sessions_by_tty(&crate::live::scan(), &argv_of);
        let ax = pid.and_then(Ax::new);
        if auto && ax.is_none() && prev.is_some() {
            // Without Accessibility every split and window frame would be lost; keep the last
            // layout that has them.
            return Ok("no Accessibility permission; kept the saved layout".into());
        }
        let ax_windows: Vec<ax::Element> = ax.as_ref().map(|a| a.windows()).unwrap_or_default();
        let snaps: Vec<El> = ax_windows.iter().map(|w| w.read()).collect();
        let seen: Vec<Seen> = snaps
            .iter()
            .zip(&ax_windows)
            .map(|(s, w)| Seen {
                title: w.title(),
                tabs: tab_count(s),
                panes: window_tree(s).map(|d| d.panes()).unwrap_or(1),
            })
            .collect();
        let pairs = pair(&script, &seen);

        let mut windows = Vec::new();
        let (mut unread, mut split_tabs) = (0, 0);
        for (sw, hit) in script.iter().zip(&pairs) {
            let el = hit.map(|j| &ax_windows[j]);
            let frame = hit.map(|j| snaps[j].rect);
            let mut tabs = Vec::new();
            for (ti, terms) in sw.tabs.iter().enumerate() {
                let panes: Vec<Pane> = terms
                    .iter()
                    .map(|(tty, cwd)| {
                        let s = by_tty.get(tty);
                        // Ghostty learns a folder from shell integration, so a pane that went
                        // straight into `claude` (as every restored one does) reports none.
                        let cwd = if cwd.is_empty() { cwd_of_tty(tty).unwrap_or_default() } else { cwd.clone() };
                        Pane { cwd, session: s.map(|s| s.0.clone()), flags: s.map(|s| s.1.clone()).unwrap_or_default() }
                    })
                    .collect();
                let mut tree = None;
                if terms.len() > 1 {
                    split_tabs += 1;
                    if auto && ti != sw.selected {
                        tree = known_tree(prev.as_ref(), &panes);
                    } else if let (Some(el), Some(ax)) = (el, &ax) {
                        // Only the selected tab is drawn: show the others for a moment.
                        let other = ti != sw.selected;
                        let view = if other {
                            select_tab(&sw.id, ti)?;
                            sleep(Duration::from_millis(250));
                            shown(ax, &sw.id, frame)
                        } else {
                            Some(el.clone())
                        };
                        tree = view
                            .and_then(|v| window_tree(&v.read()))
                            .filter(|d| d.panes() == terms.len())
                            .map(|d| d.to_node());
                        if other {
                            select_tab(&sw.id, sw.selected)?;
                        }
                    }
                    if tree.is_none() {
                        unread += 1;
                    }
                }
                tabs.push(Tab { panes, tree });
            }
            windows.push(Window {
                frame: frame.map(|r| [r.x, r.y, r.w, r.h]),
                selected_tab: sw.selected,
                tabs,
            });
        }
        let layout = Layout {
            version: 1,
            saved_at: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
            windows,
        };
        if auto {
            // Ghostty must be the same process from start to finish: one that quit (or restarted)
            // while this ran leaves a dump whose sessions have gone, which would save as a
            // shrunken layout.
            let (Some(pid), Some(up)) = (pid, pid.and_then(uptime)) else {
                return Ok("Ghostty went away during the save; nothing saved".into());
            };
            if ghostty_pid() != Some(pid) {
                return Ok("Ghostty went away during the save; nothing saved".into());
            }
            if let Some(prev) = &prev {
                let predates = std::fs::metadata(file)
                    .and_then(|m| m.modified())
                    .is_ok_and(|t| t.elapsed().is_ok_and(|e| e.as_secs() > up));
                if keep_pre_reboot(predates, up, session_count(prev), session_count(&layout)) {
                    return Ok(format!(
                        "kept the layout saved before Ghostty started ({} sessions, {} open now); \
                         `sessions layout restore` brings it back, `sessions layout save` replaces it",
                        session_count(prev),
                        session_count(&layout)
                    ));
                }
            }
        }
        if let Some(current) = &prev {
            let backup = load(&prev_file(file)).ok();
            if rotate_prev(auto, current, backup.as_ref()) && !crate::store::save_private_json(&prev_file(file), current) {
                return Err(format!("couldn't write {}", prev_file(file).display()));
            }
        }
        if !crate::store::save_private_json(file, &layout) {
            return Err(format!("couldn't write {}", file.display()));
        }
        let panes: Vec<&Pane> = layout.windows.iter().flat_map(|w| w.tabs.iter().flat_map(|t| &t.panes)).collect();
        let mut msg = format!(
            "saved {} windows, {} panes, {} Claude sessions → {}",
            layout.windows.len(),
            panes.len(),
            panes.iter().filter(|p| p.session.is_some()).count(),
            file.display()
        );
        if ax.is_none() && !auto {
            msg.push_str(
                "\nno Accessibility permission for this terminal, so splits will come back as an even grid \
                 and windows at Ghostty's default size. Allow it in System Settings › Privacy & Security › \
                 Accessibility, then save again.",
            );
        } else if unread > 0 {
            msg.push_str(&format!("\n{unread} of {split_tabs} split tabs could not be read; they will come back as an even grid"));
        }
        Ok(msg)
    }

    const CFG: &str = r#"on cfg(d, c)
  tell application "Ghostty"
    set k to new surface configuration
    set initial working directory of k to d
    set command of k to c
    return k
  end tell
end cfg
"#;

    fn new_window(p: &Pane, running: bool) -> Result<(String, String), String> {
        let out = osa(
            &format!(
                "{CFG}on run argv
  tell application \"Ghostty\"
    set k to my cfg(item 1 of argv, item 2 of argv)
    set w to new window with configuration k
    return (id of w) & linefeed & (id of focused terminal of selected tab of w)
  end tell
end run"
            ),
            &[&p.cwd, &pane_command(p, running)],
        )?;
        let (w, t) = out.split_once('\n').ok_or("Ghostty: no window id")?;
        Ok((w.to_string(), t.to_string()))
    }

    fn new_tab(win: &str, p: &Pane, running: bool) -> Result<String, String> {
        osa(
            &format!(
                "{CFG}on run argv
  tell application \"Ghostty\"
    set k to my cfg(item 1 of argv, item 2 of argv)
    set t to new tab in (first window whose id is (item 3 of argv)) with configuration k
    return id of focused terminal of t
  end tell
end run"
            ),
            &[&p.cwd, &pane_command(p, running), win],
        )
    }

    fn split(term: &str, dir: Dir, p: &Pane, running: bool) -> Result<String, String> {
        osa(
            &format!(
                "{CFG}on run argv
  tell application \"Ghostty\"
    set s to (first terminal whose id is (item 3 of argv))
    set k to my cfg(item 1 of argv, item 2 of argv)
    if (item 4 of argv) is \"right\" then
      set t to split s direction right with configuration k
    else
      set t to split s direction down with configuration k
    end if
    return id of t
  end tell
end run"
            ),
            &[&p.cwd, &pane_command(p, running), term, dir.word()],
        )
    }

    fn resize(term: &str, way: &str, points: f64) -> Result<(), String> {
        osa(
            r#"on run argv
  tell application "Ghostty" to perform action (item 2 of argv) on (first terminal whose id is (item 1 of argv))
end run"#,
            &[term, &format!("resize_split:{way},{}", points.round() as i64)],
        )
        .map(|_| ())
    }

    struct Build<'a> {
        panes: &'a [Pane],
        running: &'a dyn Fn(&Pane) -> bool,
        view: Option<ax::Element>,
        misses: Vec<String>,
    }

    /// Build `node` in the region `term` fills. The outer split is made first, so an inner split
    /// only divides its own side, and each split is sized as soon as it exists (see the module
    /// docs for why it cannot wait).
    fn build(node: &Node, term: &str, path: &mut Vec<bool>, b: &mut Build) -> Result<(), String> {
        let Node::Split { dir, ratio, a, b: second } = node else { return Ok(()) };
        let p = &b.panes[second.leaves()[0]];
        let other = split(term, *dir, p, (b.running)(p))?;
        if let Some(view) = b.view.clone() {
            size(&view, path, *dir, *ratio, term, &mut b.misses)?;
        }
        path.push(false);
        build(a, term, path, b)?;
        path.pop();
        path.push(true);
        build(second, &other, path, b)?;
        path.pop();
        Ok(())
    }

    /// Move the divider of the split at `path` until side a holds `ratio`, measuring after each
    /// step. `term` is side a's only pane.
    fn size(view: &ax::Element, path: &[bool], dir: Dir, ratio: f64, term: &str, misses: &mut Vec<String>) -> Result<(), String> {
        let mut got = 0.5;
        for attempt in 0..5 {
            sleep(Duration::from_millis(if attempt == 0 { 150 } else { 100 }));
            let Some(Drawn::Split(_, r, a, _)) = window_tree(&view.read()).and_then(|d| d.at(path).cloned()) else {
                misses.push(format!("a {} split did not show up", dir.word()));
                return Ok(());
            };
            let (total, have) = match dir {
                Dir::Right => (r.w, a.rect().w),
                Dir::Down => (r.h, a.rect().h),
            };
            got = have / total;
            let delta = ratio * total - have;
            if delta.abs() < 4.0 {
                return Ok(());
            }
            let way = match (dir, delta > 0.0) {
                (Dir::Right, true) => "right",
                (Dir::Right, false) => "left",
                (Dir::Down, true) => "down",
                (Dir::Down, false) => "up",
            };
            resize(term, way, delta.abs())?;
        }
        if (got - ratio).abs() > 0.02 {
            misses.push(format!("a split meant to be {:.0}% came out {:.0}%", ratio * 100.0, got * 100.0));
        }
        Ok(())
    }

    pub fn restore(file: &Path) -> Result<String, String> {
        let layout = load(file)?;
        let live = crate::live::scan();
        let running = |p: &Pane| p.session.as_ref().is_some_and(|id| live.contains_key(id));
        let ax = ghostty_pid().and_then(Ax::new);
        let mut report = Vec::new();
        let (mut resumed, mut skipped) = (0, 0);

        for (wi, w) in layout.windows.iter().enumerate() {
            let before = ax.as_ref().map(|a| a.windows()).unwrap_or_default();
            let mut win = String::new();
            let mut view: Option<ax::Element> = None;
            let mut misses = Vec::new();
            for (ti, t) in w.tabs.iter().enumerate() {
                let tree = tree_for(t);
                let first = &t.panes[tree.leaves()[0]];
                let term = if ti == 0 {
                    let (id, term) = new_window(first, running(first))?;
                    win = id;
                    sleep(Duration::from_millis(300));
                    if let Some(a) = &ax {
                        view = a.windows().into_iter().find(|n| !before.iter().any(|b| b == n));
                        if let (Some(v), Some([x, y, wd, h])) = (&view, w.frame) {
                            v.set_frame(Rect { x, y, w: wd, h });
                            sleep(Duration::from_millis(200));
                        }
                    }
                    term
                } else {
                    new_tab(&win, first, running(first))?
                };
                // A tab made by script is not always the one on screen, and only that one is drawn.
                select_tab(&win, ti)?;
                sleep(Duration::from_millis(150));
                let shown_now = match (&ax, ti) {
                    (Some(_), 0) => view.clone(),
                    (Some(a), _) => shown(a, &win, view.as_ref().map(|v| v.read().rect)),
                    _ => None,
                };
                let mut b = Build { panes: &t.panes, running: &running, view: shown_now, misses: Vec::new() };
                build(&tree, &term, &mut Vec::new(), &mut b)?;
                misses.extend(b.misses.into_iter().map(|m| format!("tab {}: {m}", ti + 1)));
                for p in &t.panes {
                    match (&p.session, running(p)) {
                        (Some(id), true) => {
                            skipped += 1;
                            misses.push(format!("{} is already running; opened a shell in its place", &id[..8.min(id.len())]));
                        }
                        (Some(_), false) => resumed += 1,
                        _ => {}
                    }
                }
            }
            select_tab(&win, w.selected_tab.min(w.tabs.len() - 1))?;
            let panes: usize = w.tabs.iter().map(|t| t.panes.len()).sum();
            report.push(format!("window {}: {} tabs, {panes} panes", wi + 1, w.tabs.len()));
            report.extend(misses.into_iter().map(|m| format!("  {m}")));
        }
        report.push(format!(
            "resumed {resumed} sessions{}",
            if skipped > 0 { format!(", {skipped} already running") } else { String::new() }
        ));
        if ax.is_none() {
            report.push("no Accessibility permission for this terminal: window frames and split sizes were left at Ghostty's defaults".into());
        }
        Ok(report.join("\n"))
    }

    /// Just enough of the macOS accessibility API to read Ghostty's windows and move them.
    mod ax {
        use super::{El, Rect};
        use std::ffi::c_void;

        type CFTypeRef = *const c_void;

        #[repr(C)]
        #[derive(Clone, Copy)]
        struct CFRange {
            location: isize,
            length: isize,
        }

        #[link(name = "ApplicationServices", kind = "framework")]
        extern "C" {
            fn AXIsProcessTrusted() -> u8;
            fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
            fn AXUIElementCopyAttributeValue(el: CFTypeRef, attr: CFTypeRef, value: *mut CFTypeRef) -> i32;
            fn AXUIElementSetAttributeValue(el: CFTypeRef, attr: CFTypeRef, value: CFTypeRef) -> i32;
            fn AXValueCreate(kind: u32, value: *const c_void) -> CFTypeRef;
            fn AXValueGetValue(v: CFTypeRef, kind: u32, out: *mut c_void) -> u8;
            fn AXValueGetTypeID() -> usize;
        }

        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" {
            fn CFRelease(cf: CFTypeRef);
            fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
            fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> u8;
            fn CFGetTypeID(cf: CFTypeRef) -> usize;
            fn CFStringGetTypeID() -> usize;
            fn CFArrayGetTypeID() -> usize;
            fn CFStringCreateWithBytes(alloc: CFTypeRef, bytes: *const u8, len: isize, enc: u32, ext: u8) -> CFTypeRef;
            fn CFStringGetLength(s: CFTypeRef) -> isize;
            fn CFStringGetBytes(
                s: CFTypeRef,
                range: CFRange,
                enc: u32,
                loss: u8,
                ext: u8,
                buf: *mut u8,
                max: isize,
                used: *mut isize,
            ) -> isize;
            fn CFArrayGetCount(a: CFTypeRef) -> isize;
            fn CFArrayGetValueAtIndex(a: CFTypeRef, i: isize) -> CFTypeRef;
        }

        const UTF8: u32 = 0x0800_0100;
        const POINT: u32 = 1;
        const SIZE: u32 = 2;

        /// An owned CoreFoundation reference.
        struct Cf(CFTypeRef);

        impl Drop for Cf {
            fn drop(&mut self) {
                // SAFETY: every Cf holds a reference it owns: created or copied by a Create/Copy
                // call, or retained in `Element::clone` / array reads.
                unsafe { CFRelease(self.0) }
            }
        }

        fn cfstr(s: &str) -> Cf {
            // SAFETY: the bytes are valid UTF-8 for the given length; the result is owned.
            Cf(unsafe { CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, UTF8, 0) })
        }

        fn string(cf: CFTypeRef) -> Option<String> {
            // SAFETY: cf is a live CF object; it is only read as a string after its type is checked.
            unsafe {
                if CFGetTypeID(cf) != CFStringGetTypeID() {
                    return None;
                }
                let range = CFRange { location: 0, length: CFStringGetLength(cf) };
                let mut need = 0isize;
                CFStringGetBytes(cf, range, UTF8, b'?', 0, std::ptr::null_mut(), 0, &mut need);
                let mut buf = vec![0u8; need as usize];
                CFStringGetBytes(cf, range, UTF8, b'?', 0, buf.as_mut_ptr(), need, &mut need);
                String::from_utf8(buf).ok()
            }
        }

        pub struct Element(Cf);

        impl Clone for Element {
            fn clone(&self) -> Self {
                // SAFETY: retaining a live reference gives the clone its own.
                Element(Cf(unsafe { CFRetain(self.0 .0) }))
            }
        }

        impl PartialEq for Element {
            fn eq(&self, other: &Self) -> bool {
                // SAFETY: both are live references.
                unsafe { CFEqual(self.0 .0, other.0 .0) != 0 }
            }
        }

        impl Element {
            fn attr(&self, name: &str) -> Option<Cf> {
                let key = cfstr(name);
                let mut v: CFTypeRef = std::ptr::null();
                // SAFETY: the element and key are live; a successful copy hands us an owned value.
                let ok = unsafe { AXUIElementCopyAttributeValue(self.0 .0, key.0, &mut v) } == 0;
                (ok && !v.is_null()).then(|| Cf(v))
            }

            fn text(&self, name: &str) -> String {
                self.attr(name).and_then(|v| string(v.0)).unwrap_or_default()
            }

            pub fn title(&self) -> String {
                self.text("AXTitle")
            }

            fn value<T: Default>(&self, name: &str, kind: u32) -> T {
                let mut out = T::default();
                if let Some(v) = self.attr(name) {
                    // SAFETY: the value's type is checked before it is read into a T of that kind.
                    unsafe {
                        if CFGetTypeID(v.0) == AXValueGetTypeID() {
                            AXValueGetValue(v.0, kind, &mut out as *mut T as *mut c_void);
                        }
                    }
                }
                out
            }

            fn kids(&self) -> Vec<Element> {
                let Some(arr) = self.attr("AXChildren") else { return Vec::new() };
                // SAFETY: the array is type-checked; each item is retained before it is wrapped.
                unsafe {
                    if CFGetTypeID(arr.0) != CFArrayGetTypeID() {
                        return Vec::new();
                    }
                    (0..CFArrayGetCount(arr.0))
                        .map(|i| Element(Cf(CFRetain(CFArrayGetValueAtIndex(arr.0, i)))))
                        .collect()
                }
            }

            /// This element and everything under it, down to the panes: a scroll area's text is
            /// never needed, and reading a long scrollback's attributes is slow.
            pub fn read(&self) -> El {
                let role = self.text("AXRole");
                let (x, y): (f64, f64) = self.value("AXPosition", POINT);
                let (w, h): (f64, f64) = self.value("AXSize", SIZE);
                let kids = if role == "AXScrollArea" { Vec::new() } else { self.kids().iter().map(Element::read).collect() };
                El { desc: self.text("AXDescription"), role, rect: Rect { x, y, w, h }, kids }
            }

            pub fn set_frame(&self, r: Rect) {
                let (p, s) = ((r.x, r.y), (r.w, r.h));
                for (name, kind, ptr) in
                    [("AXPosition", POINT, &p as *const _ as *const c_void), ("AXSize", SIZE, &s as *const _ as *const c_void)]
                {
                    // SAFETY: ptr points at a (f64, f64), the layout of CGPoint and CGSize.
                    unsafe {
                        let v = Cf(AXValueCreate(kind, ptr));
                        AXUIElementSetAttributeValue(self.0 .0, cfstr(name).0, v.0);
                    }
                }
            }
        }

        pub struct Ax(Element);

        impl Ax {
            /// `None` without accessibility permission for this process.
            pub fn new(pid: i32) -> Option<Ax> {
                // SAFETY: plain calls; the application element is owned.
                unsafe { (AXIsProcessTrusted() != 0).then(|| Ax(Element(Cf(AXUIElementCreateApplication(pid))))) }
            }

            pub fn windows(&self) -> Vec<Element> {
                let Some(arr) = self.0.attr("AXWindows") else { return Vec::new() };
                // SAFETY: as in `kids`.
                unsafe {
                    if CFGetTypeID(arr.0) != CFArrayGetTypeID() {
                        return Vec::new();
                    }
                    (0..CFArrayGetCount(arr.0))
                        .map(|i| Element(Cf(CFRetain(CFArrayGetValueAtIndex(arr.0, i)))))
                        .filter(|w| w.text("AXRole") == "AXWindow")
                        .collect()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }

    fn el(role: &str, desc: &str, rect: Rect, kids: Vec<El>) -> El {
        El { role: role.into(), desc: desc.into(), rect, kids }
    }

    #[test]
    fn the_lock_is_never_the_file_it_guards() {
        assert_eq!(lock_file(Path::new("/s/layout.json")), PathBuf::from("/s/layout.json.lock"));
        assert_eq!(lock_file(Path::new("/s/snapshot.lock")), PathBuf::from("/s/snapshot.lock.lock"));
    }

    #[test]
    fn a_save_that_does_not_wait_takes_over_a_stale_lock() {
        let dir = std::env::temp_dir().join(format!("sessio-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("layout.json");
        let held = Lock::take(&file, false).expect("free lock");
        assert_eq!(Lock::take(&file, false).err(), Some(LockError::Busy), "a fresh lock is respected");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(600);
        std::fs::File::options().write(true).open(lock_file(&file)).unwrap().set_modified(old).unwrap();
        let next = Lock::take(&file, false).expect("a stale lock is taken over");
        drop(held);
        assert!(lock_file(&file).exists(), "the save that outlived its lock leaves its successor's alone");
        drop(next);
        assert!(!lock_file(&file).exists(), "dropping the lock removes it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_lock_that_cannot_be_created_is_an_error_not_a_busy_save() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("sessio-lock-ro-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let started = std::time::Instant::now();
        let got = Lock::take(&dir.join("layout.json"), true);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(got, Err(LockError::Io(_))), "{:?}", got.err());
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "no 10s of retries");
    }

    #[test]
    fn ps_elapsed_time_reads_as_seconds() {
        assert_eq!(parse_etime("  05:07\n"), Some(307));
        assert_eq!(parse_etime("02:05:07"), Some(7507));
        assert_eq!(parse_etime("3-02:05:07"), Some(3 * 86_400 + 7507));
        assert_eq!(parse_etime(""), None);
    }

    #[test]
    fn an_unattended_save_keeps_the_tree_of_a_tab_with_the_same_panes() {
        let p = |s: &str| Pane { cwd: format!("/x/{s}"), session: Some(s.into()), flags: vec![] };
        let tree = Node::Split { dir: Dir::Down, ratio: 0.3, a: Box::new(Node::Leaf(0)), b: Box::new(Node::Leaf(1)) };
        let prev = Layout {
            version: 1,
            saved_at: String::new(),
            windows: vec![Window {
                frame: None,
                selected_tab: 0,
                tabs: vec![Tab { panes: vec![p("a")], tree: None }, Tab { panes: vec![p("b"), p("c")], tree: Some(tree.clone()) }],
            }],
        };
        assert_eq!(known_tree(Some(&prev), &[p("b"), p("c")]), Some(tree));
        assert_eq!(known_tree(Some(&prev), &[p("c"), p("b")]), None);
        assert_eq!(known_tree(None, &[p("b"), p("c")]), None);
        assert_eq!(session_count(&prev), 3);
        assert_eq!(prev_file(Path::new("/s/layout.json")), PathBuf::from("/s/layout.prev.json"));
    }

    #[test]
    fn a_tree_is_reused_only_when_one_tab_has_those_panes() {
        // Two split tabs of plain shells in the same folders: which tree is which can't be told.
        let shell = |d: &str| Pane { cwd: format!("/x/{d}"), session: None, flags: vec![] };
        let two = vec![shell("a"), shell("a")];
        let down = Node::Split { dir: Dir::Down, ratio: 0.5, a: Box::new(Node::Leaf(0)), b: Box::new(Node::Leaf(1)) };
        let right = Node::Split { dir: Dir::Right, ratio: 0.5, a: Box::new(Node::Leaf(0)), b: Box::new(Node::Leaf(1)) };
        let l = |tabs: Vec<Tab>| Layout { version: 1, saved_at: String::new(), windows: vec![Window { frame: None, selected_tab: 0, tabs }] };
        let both = l(vec![Tab { panes: two.clone(), tree: Some(down.clone()) }, Tab { panes: two.clone(), tree: Some(right) }]);
        assert_eq!(known_tree(Some(&both), &two), None);
        let one = l(vec![Tab { panes: two.clone(), tree: Some(down.clone()) }]);
        assert_eq!(known_tree(Some(&one), &two), Some(down));
    }

    #[test]
    fn an_unattended_save_never_lets_a_smaller_layout_push_a_bigger_one_out_of_the_backup() {
        let with = |n: usize| Layout {
            version: 1,
            saved_at: String::new(),
            windows: vec![Window {
                frame: None,
                selected_tab: 0,
                tabs: (0..n).map(|i| Tab { panes: vec![Pane { cwd: "/x".into(), session: Some(format!("s{i}")), flags: vec![] }], tree: None }).collect(),
            }],
        };
        // Ghostty quitting: 9 → 6 → 2 sessions. The backup keeps 9 through both shrinking saves.
        assert!(rotate_prev(true, &with(9), Some(&with(7))), "a bigger layout replaces the backup");
        assert!(!rotate_prev(true, &with(6), Some(&with(9))), "a smaller one does not");
        assert!(rotate_prev(true, &with(6), None), "with no backup yet, anything is one");
        assert!(rotate_prev(false, &with(1), Some(&with(9))), "a save you run yourself always rotates");
    }

    #[test]
    fn after_a_reboot_the_saved_layout_is_kept_only_while_it_could_still_be_restored() {
        assert!(keep_pre_reboot(true, 60, 12, 1), "minutes after Ghostty started, before restore");
        assert!(!keep_pre_reboot(true, REBOOT_GRACE_SECS + 1, 12, 1), "half an hour on, the user moved on");
        assert!(!keep_pre_reboot(false, 60, 12, 1), "saved by this Ghostty: an ordinary save");
        assert!(!keep_pre_reboot(true, 60, 12, 12), "not smaller: nothing to protect");
    }

    /// A pane as Ghostty draws it: a group around a scroll area.
    fn pane(desc: &str, rect: Rect) -> El {
        el("AXGroup", desc, rect, vec![el("AXScrollArea", "", rect, vec![])])
    }

    #[test]
    fn the_drawn_tree_reads_back_as_splits_with_ratios() {
        // What Ghostty drew for a 2x2 window with a 30% left column, captured from the real
        // accessibility tree and trimmed: the window, its content group, then the split groups.
        let left = el(
            "AXGroup",
            "Left pane",
            r(0.0, 0.0, 300.0, 800.0),
            vec![pane("Top pane", r(0.0, 0.0, 300.0, 200.0)), pane("Bottom pane", r(0.0, 200.0, 300.0, 600.0))],
        );
        let right = pane("Right pane", r(300.0, 0.0, 700.0, 800.0));
        let win = el(
            "AXWindow",
            "standard window",
            r(0.0, 0.0, 1000.0, 832.0),
            vec![
                el("AXTabGroup", "tab bar", r(0.0, 0.0, 1000.0, 28.0), vec![el("AXRadioButton", "", Rect::default(), vec![]); 2]),
                el(
                    "AXGroup",
                    "",
                    r(0.0, 0.0, 1000.0, 800.0),
                    vec![el("AXGroup", "Horizontal split view", r(0.0, 0.0, 1000.0, 800.0), vec![left, right])],
                ),
            ],
        );
        let d = window_tree(&win).unwrap();
        assert_eq!(d.panes(), 3);
        assert_eq!(tab_count(&win), 2);
        let want = Node::Split {
            dir: Dir::Right,
            ratio: 0.3,
            a: Box::new(Node::Split { dir: Dir::Down, ratio: 0.25, a: Box::new(Node::Leaf(0)), b: Box::new(Node::Leaf(1)) }),
            b: Box::new(Node::Leaf(2)),
        };
        assert_eq!(d.to_node(), want);
        assert!(matches!(d.at(&[false]), Some(Drawn::Split(Dir::Down, ..))));
        assert!(d.at(&[true, false]).is_none(), "a pane has nothing under it");
    }

    #[test]
    fn a_single_pane_window_has_one_pane_and_one_tab() {
        let win = el("AXWindow", "", r(0.0, 0.0, 10.0, 10.0), vec![el("AXGroup", "", r(0.0, 0.0, 10.0, 10.0), vec![pane("Terminal pane", r(0.0, 0.0, 10.0, 10.0))])]);
        assert_eq!(window_tree(&win).map(|d| d.panes()), Some(1));
        assert_eq!(tab_count(&win), 1);
    }

    #[test]
    fn the_dump_survives_titles_with_newlines_and_tabs() {
        // Claude Code puts its session title in the window title; a line-based dump broke on one.
        let dump = "W\u{1f}w1\u{1f}✳ a title\nwith a newline\tand a tab\u{1e}\
                    T\u{1f}false\u{1e}P\u{1f}/dev/ttys001\u{1f}/a\u{1e}\
                    T\u{1f}true\u{1e}P\u{1f}/dev/ttys002\u{1f}/b c\u{1e}P\u{1f}/dev/ttys003\u{1f}\u{1e}\
                    W\u{1f}w2\u{1f}\u{1e}T\u{1f}true\u{1e}P\u{1f}/dev/ttys004\u{1f}/d\u{1e}\n";
        let w = parse_dump(dump);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].name, "✳ a title\nwith a newline\tand a tab");
        assert_eq!(w[0].selected, 1);
        assert_eq!(w[0].tabs[1], vec![("/dev/ttys002".into(), "/b c".into()), ("/dev/ttys003".into(), String::new())]);
        assert_eq!(w[1].tabs, vec![vec![("/dev/ttys004".to_string(), "/d".to_string())]]);
    }

    #[test]
    fn only_the_session_picking_flags_are_dropped() {
        let argv = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        assert_eq!(keep_flags(&argv("claude --dangerously-skip-permissions")), ["--dangerously-skip-permissions"]);
        assert_eq!(
            keep_flags(&argv("/opt/homebrew/bin/claude --model opus --resume abc -c --session-id x --resume=y")),
            ["--model", "opus"]
        );
        assert!(keep_flags(&argv("2.1.310")).is_empty(), "a native install's argv[0] is its version");
    }

    #[test]
    fn titles_compare_without_the_status_glyph() {
        assert_eq!(bare_title("✳ Fix the parser"), "Fix the parser");
        assert_eq!(bare_title("◐ Fix the parser"), "Fix the parser");
        assert_eq!(bare_title("a b"), "a b", "a one-letter word is a word");
        assert_eq!(bare_title("…/bitbybit/citymaps"), "…/bitbybit/citymaps");
    }

    #[test]
    fn windows_pair_by_title_then_by_shape() {
        let sw = |id: &str, name: &str, tabs: &[usize]| ScriptWindow {
            id: id.into(),
            name: name.into(),
            selected: 0,
            tabs: tabs.iter().map(|&n| vec![(String::new(), String::new()); n]).collect(),
        };
        let seen = |title: &str, tabs, panes| Seen { title: title.into(), tabs, panes };
        let script = [sw("1", "◐ busy", &[4]), sw("2", "/Users/x/citymaps", &[1, 1, 1]), sw("3", "same", &[1]), sw("4", "same", &[2])];
        let ax = [
            seen("…/x/citymaps", 3, 1), // a path title, rendered differently
            seen("same", 1, 2),
            seen("✳ busy", 1, 4), // the spinner moved on between the two reads
            seen("same", 1, 1),
        ];
        assert_eq!(pair(&script, &ax), [Some(2), Some(0), Some(3), Some(1)]);

        // Two leftovers of the same shape cannot be told apart, so neither is guessed.
        let script = [sw("1", "a", &[1]), sw("2", "b", &[1])];
        assert_eq!(pair(&script, &[seen("x", 1, 1), seen("y", 1, 1)]), [None, None]);
    }

    #[test]
    fn the_grid_fallback_fills_even_columns() {
        assert_eq!(grid(1), Node::Leaf(0));
        // Four panes: two columns of two, each half.
        let four = Node::Split {
            dir: Dir::Right,
            ratio: 0.5,
            a: Box::new(Node::Split { dir: Dir::Down, ratio: 0.5, a: Box::new(Node::Leaf(0)), b: Box::new(Node::Leaf(1)) }),
            b: Box::new(Node::Split { dir: Dir::Down, ratio: 0.5, a: Box::new(Node::Leaf(2)), b: Box::new(Node::Leaf(3)) }),
        };
        assert_eq!(grid(4), four);
        // Three: two columns, the first holding two stacked panes.
        let three = Node::Split {
            dir: Dir::Right,
            ratio: 0.5,
            a: Box::new(Node::Split { dir: Dir::Down, ratio: 0.5, a: Box::new(Node::Leaf(0)), b: Box::new(Node::Leaf(1)) }),
            b: Box::new(Node::Leaf(2)),
        };
        assert_eq!(grid(3), three);
        // Five: three columns of two, two, one — the first a third of the width.
        let Node::Split { ratio, .. } = grid(5) else { panic!() };
        assert!((ratio - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(grid(7).leaves(), (0..7).collect::<Vec<_>>());
    }

    #[test]
    fn a_running_session_comes_back_as_a_shell_and_the_rest_resume_with_their_flags() {
        let p = Pane { cwd: "/tmp/it's".into(), session: Some("abc".into()), flags: vec!["--dangerously-skip-permissions".into()] };
        let cmd = pane_command(&p, false);
        assert!(cmd.ends_with("'claude' '--dangerously-skip-permissions' '--resume' 'abc'"), "{cmd}");
        assert!(cmd.contains(r"'/tmp/it'\''s'"), "the folder travels quoted: {cmd}");
        let cmd = pane_command(&p, true);
        assert!(!cmd.contains("claude") && cmd.ends_with("'-l'"), "{cmd}");
    }

    #[test]
    fn a_saved_layout_round_trips_and_a_broken_one_is_refused() {
        let tab = Tab {
            panes: vec![Pane { cwd: "/a".into(), session: None, flags: vec![] }; 2],
            tree: Some(Node::Split { dir: Dir::Down, ratio: 0.6, a: Box::new(Node::Leaf(1)), b: Box::new(Node::Leaf(0)) }),
        };
        let l = Layout { version: 1, saved_at: "2026-10-26 16:00".into(), windows: vec![Window { frame: Some([1.0, 2.0, 3.0, 4.0]), selected_tab: 0, tabs: vec![tab] }] };
        let dir = std::env::temp_dir().join(format!("sessio-layout-{}", std::process::id()));
        let file = dir.join("layout.json");
        assert!(crate::store::save_private_json(&file, &l));
        assert_eq!(load(&file).unwrap(), l);

        let mut bad = l.clone();
        bad.windows[0].tabs[0].tree = Some(Node::Leaf(0));
        assert!(crate::store::save_private_json(&file, &bad));
        assert!(load(&file).unwrap_err().contains("does not match"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
