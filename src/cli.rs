//! The non-interactive half of sessio: the sessions the dashboard shows, as commands a script or
//! an agent can run and read. A bare `sessions` still opens the dashboard.
//!
//! Every command is one load, one answer, one exit, so nothing here may lean on the dashboard's
//! next tick to fill something in. That is why it loads through `model::load_settled`.

use std::collections::HashSet;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sessio::live::LiveMap;
use sessio::model::{self, Item};
use sessio::safety::sanitize;
use sessio::store::Archive;
use sessio::discover::Source;
use sessio::{discover, parse, rank, resume, search, ui};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const COMMANDS: &[&str] = &["ls", "find", "show", "resume", "reply", "archive", "unarchive", "kill", "help"];

/// What `ls` and `find` print when not told otherwise. `--limit 0` lifts it.
const DEFAULT_LIMIT: usize = 20;
/// The project column never takes more than this; longer names are clipped.
const PROJECT_MAX: usize = 20;

pub fn usage() -> String {
    format!(
        "sessio {} — find and resume past Claude Code and Copilot CLI sessions.
GitHub Copilot CLI sessions (~/.copilot) are listed too, tagged `copilot`.

USAGE:
  sessions                          browse and resume (interactive)
  sessions ls [filters]             list sessions, newest first
  sessions find <text> [filters]    rank by title, project and first prompt
  sessions find --text <term>       search inside every transcript (needs rg)
  sessions show <id> [--json]       recap, first and last prompt, and the last reply
  sessions resume <id>              resume it in its own folder, in this terminal
           [--print]                  print the command instead of running it
           [--force]                  resume even though it is already running
  sessions reply <id> <message>     send one turn without opening it (spends tokens;
                                    `-` reads the message from stdin; Claude only)
  sessions archive <id>...          hide from the dashboard and from ls
  sessions unarchive <id>...
  sessions kill <id> [--json]       end a running session idle for more than 48h
                                    (SIGTERM; refuses busy, waiting or recent ones)
  sessions --update                 update instructions for this install
  sessions --dump-json              print the computed session list (oracle harness)
  sessions --version

FILTERS (ls, find):
  -p, --project <name>   one project       --here       the project of this folder
  --open                 unfinished work   --running    a claude is attached now
  --waiting              waiting on you    --archived   archived sessions only
  -n, --limit <N>        default {DEFAULT_LIMIT}, 0 for all
  --json                 one JSON array, for scripts and agents

An <id> is any prefix only one session has; ls prints eight characters.
Marks: ◆ waiting on you · ◉ running · ▶ unfinished

KEYS (the dashboard; ? there explains each):
  ↑/↓ project · ←/→ session · type to filter · ^w/⌥⌫ word · ^u/⌘⌫ clear
  ^f search-in-text · ^a archive · ⇥/^e expand-reply · PgUp/PgDn scroll-reply
  ^r reply · ^t follow · ^g issues · ^k end-stale
  ↵ resume · ^o new-window · ^n new-session · ? help · esc quit · ^c quit
",
        env!("CARGO_PKG_VERSION")
    )
}

/// A command that did not succeed, and which kind of failure it was.
#[derive(Debug)]
enum Fail {
    /// The command line itself was wrong. Exit 2.
    Usage(String),
    /// The command was understood and could not be done. Exit 1.
    Error(String),
}

fn usage_err(m: impl Into<String>) -> Fail {
    Fail::Usage(m.into())
}

fn err(m: impl Into<String>) -> Fail {
    Fail::Error(m.into())
}

/// Run one command; returns the exit status.
pub fn run(cmd: &str, args: &[String]) -> i32 {
    let result = if !COMMANDS.contains(&cmd) {
        Err(usage_err(format!("unknown command '{cmd}'")))
    } else {
        match parse(cmd, args) {
            Ok(o) if o.help || cmd == "help" => {
                out(&usage());
                Ok(())
            }
            Ok(o) => dispatch(cmd, &o),
            Err(e) => Err(e),
        }
    };
    match result {
        Ok(()) => 0,
        Err(Fail::Usage(m)) => {
            eprintln!("sessions: {m}\nRun `sessions --help` for usage.");
            2
        }
        Err(Fail::Error(m)) => {
            eprintln!("sessions: {m}");
            1
        }
    }
}

fn dispatch(cmd: &str, o: &Opts) -> Result<(), Fail> {
    match cmd {
        "ls" => {
            if !o.args.is_empty() {
                return Err(usage_err("ls takes no text; to filter by text use `sessions find`"));
            }
            list(o, None)
        }
        "find" => {
            let q = o.args.join(" ");
            if q.trim().is_empty() {
                return Err(usage_err("find needs something to look for"));
            }
            list(o, Some(&q))
        }
        "show" => show(o),
        "resume" => resume_cmd(o),
        "reply" => reply(o),
        "archive" => archive(o, true),
        "unarchive" => archive(o, false),
        "kill" => kill_cmd(o),
        _ => unreachable!("run() only dispatches known commands"),
    }
}

// ---------- arguments ----------

#[derive(Debug, Default)]
struct Opts {
    args: Vec<String>,
    project: Option<String>,
    limit: Option<usize>,
    here: bool,
    open: bool,
    running: bool,
    waiting: bool,
    archived: bool,
    json: bool,
    text: bool,
    force: bool,
    print: bool,
    help: bool,
}

const LIST_FLAGS: &[&str] =
    &["--project", "--here", "--open", "--running", "--waiting", "--archived", "--limit", "--json"];

/// The flags each command accepts. Anything else is refused rather than ignored: an agent that
/// misspells `--open` should hear about it, not get every session back.
fn flags_for(cmd: &str) -> Vec<&'static str> {
    match cmd {
        "ls" => LIST_FLAGS.to_vec(),
        "find" => [LIST_FLAGS, &["--text"]].concat(),
        "show" => vec!["--json"],
        "kill" => vec!["--json"],
        "resume" => vec!["--force", "--print"],
        _ => vec![],
    }
}

fn parse(cmd: &str, raw: &[String]) -> Result<Opts, Fail> {
    let allowed = flags_for(cmd);
    let mut o = Opts::default();
    let mut it = raw.iter();
    while let Some(a) = it.next() {
        if a == "--" {
            o.args.extend(it.by_ref().cloned());
            break;
        }
        if !a.starts_with('-') || a == "-" {
            o.args.push(a.clone());
            continue;
        }
        if a == "-h" || a == "--help" {
            o.help = true;
            continue;
        }
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (a.as_str(), None),
        };
        let flag = match name {
            "-p" => "--project",
            "-n" => "--limit",
            n => n,
        };
        if !allowed.contains(&flag) {
            let hint = if cmd == "reply" { " (a message starting with '-' goes after `--`)" } else { "" };
            return Err(usage_err(format!("{cmd}: unknown option '{name}'{hint}")));
        }
        let takes_value = matches!(flag, "--project" | "--limit");
        if inline.is_some() && !takes_value {
            return Err(usage_err(format!("{flag} takes no value")));
        }
        let value = if takes_value {
            match inline.or_else(|| it.next().cloned()) {
                Some(v) => v,
                None => return Err(usage_err(format!("{flag} needs a value"))),
            }
        } else {
            String::new()
        };
        match flag {
            "--project" => o.project = Some(value),
            "--limit" => {
                o.limit = Some(value.parse().map_err(|_| usage_err("--limit takes a number"))?)
            }
            "--here" => o.here = true,
            "--open" => o.open = true,
            "--running" => o.running = true,
            "--waiting" => o.waiting = true,
            "--archived" => o.archived = true,
            "--json" => o.json = true,
            "--text" => o.text = true,
            "--force" => o.force = true,
            "--print" => o.print = true,
            _ => unreachable!("every allowed flag is handled"),
        }
    }
    Ok(o)
}

// ---------- loading ----------

struct World {
    items: Vec<Item>,
    archive: Archive,
    live: LiveMap,
}

impl World {
    fn load(extra: &[PathBuf]) -> Self {
        let items = model::load_settled(extra);
        let mut archive = Archive::load();
        // What every dashboard refresh does: a session worked in since it was archived is back.
        archive.release_reactivated(items.iter().map(|i| (i.key.as_str(), i.id.as_str(), i.mtime)));
        World { items, archive, live: sessio::live::scan() }
    }

    fn archived(&self, it: &Item) -> bool {
        self.archive.contains(&it.key, &it.id)
    }
}

/// A session by its id, or by a prefix only it has: `ls` prints eight characters, and those have
/// to be enough to act on. Looks past the 300-session cap, since an id can come from anywhere.
fn session(arg: &str) -> Result<(World, usize), Fail> {
    let rows = discover::scan_all();
    let row = match rows.iter().find(|r| r.id == arg) {
        Some(r) => r,
        None => {
            let hits: Vec<&discover::Row> =
                rows.iter().filter(|r| !arg.is_empty() && r.id.starts_with(arg)).collect();
            // One transcript can sit in two project folders under the same id; that is still one
            // session, and rows are newest first, so the first is the one to take.
            let ids: HashSet<&str> = hits.iter().map(|r| r.id.as_str()).collect();
            match ids.len() {
                0 => return Err(err(format!("no session '{arg}'"))),
                1 => hits[0],
                n => {
                    let mut some: Vec<&str> = ids.into_iter().collect();
                    some.sort_unstable();
                    some.truncate(4);
                    return Err(err(format!(
                        "'{arg}' matches {n} sessions ({}{}) — use more of the id",
                        some.join(", "),
                        if n > 4 { ", …" } else { "" }
                    )));
                }
            }
        }
    };
    let file = row.file.clone();
    let w = World::load(std::slice::from_ref(&file));
    let i = w
        .items
        .iter()
        .position(|it| it.file == file)
        .ok_or_else(|| err(format!("session {} has no prompts in it", short(&row.id))))?;
    Ok((w, i))
}

fn one_id<'a>(o: &'a Opts, cmd: &str) -> Result<&'a str, Fail> {
    match o.args.as_slice() {
        [id] => Ok(id),
        [] => Err(usage_err(format!("{cmd} needs a session id"))),
        _ => Err(usage_err(format!("{cmd} takes one session id"))),
    }
}

// ---------- ls / find ----------

fn list(o: &Opts, query: Option<&str>) -> Result<(), Fail> {
    if o.text && query.is_none() {
        return Err(usage_err("--text needs a term"));
    }
    // --text greps every transcript on disk, and its matches are loaded even past the 300 cap.
    let files: Option<HashSet<PathBuf>> = match query.filter(|_| o.text) {
        Some(q) => {
            if search::rg_path().is_none() {
                return Err(err(format!(
                    "find --text needs ripgrep (rg) on PATH · {}",
                    search::INSTALL_HINT
                )));
            }
            Some(
                search::content_search(q, &search::roots())
                    .map_err(|e| err(format!("full-text search failed: {e}")))?,
            )
        }
        None => None,
    };
    let extra: Vec<PathBuf> = files.iter().flatten().cloned().collect();
    let w = World::load(&extra);
    let hits: Option<HashSet<String>> =
        files.map(|fs| fs.iter().filter_map(|f| discover::key_for_file(f)).collect());
    let project = project_filter(o, &w)?;

    let mut idx: Vec<usize> = (0..w.items.len())
        .filter(|&i| {
            let it = &w.items[i];
            let live = w.live.get(&it.id);
            w.archived(it) == o.archived
                && project.as_ref().is_none_or(|p| it.project == *p)
                && (!o.open || it.open)
                && (!o.running || live.is_some())
                && (!o.waiting || live.is_some_and(|l| l.needs_you()))
                && hits.as_ref().is_none_or(|h| h.contains(&it.key))
        })
        .collect();
    if let (Some(q), None) = (query, &hits) {
        let pairs: Vec<(&str, i64)> =
            idx.iter().map(|&i| (w.items[i].hay.as_str(), w.items[i].mtime)).collect();
        idx = rank::rank(&pairs, q).into_iter().map(|p| idx[p]).collect();
    }
    let limit = o.limit.unwrap_or(DEFAULT_LIMIT);
    if limit > 0 {
        idx.truncate(limit);
    }

    if o.json {
        let rows: Vec<SessionJson> =
            idx.iter().map(|&i| session_json(&w, &w.items[i], &w.items[i].name)).collect();
        out(&format!("{}\n", serde_json::to_string(&rows).expect("plain data")));
    } else if idx.is_empty() {
        eprintln!("no matching sessions");
    } else {
        out(&rows_text(&w, &idx, term_width()));
    }
    Ok(())
}

fn project_filter(o: &Opts, w: &World) -> Result<Option<String>, Fail> {
    if o.here && o.project.is_some() {
        return Err(usage_err("--here and --project both pick a project; use one"));
    }
    if let Some(p) = &o.project {
        if !w.items.iter().any(|it| it.project == *p) {
            return Err(err(format!("no project named '{p}' — `sessions ls --json` shows them")));
        }
        return Ok(Some(p.clone()));
    }
    if !o.here {
        return Ok(None);
    }
    let dir = std::env::current_dir().map_err(|e| err(format!("can't read this folder: {e}")))?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    // Every project is a candidate, archived ones included, so `--archived --here` still works.
    let tabs = model::tabs_for(&w.items, &Archive::default());
    model::tab_for_dir(&w.items, &tabs, &dir, home.as_deref())
        .map(|t| Some(tabs[t].clone()))
        .ok_or_else(|| err(format!("no sessions were started in {} or above it", dir.display())))
}

/// One row per session: mark, short id, age, project, title. Clipped to `width` on a terminal;
/// a pipe gets whole lines, so `grep` and `fzf` see every word.
fn rows_text(w: &World, idx: &[usize], width: usize) -> String {
    let pw = idx
        .iter()
        .map(|&i| w.items[i].project.width())
        .max()
        .unwrap_or(0)
        .min(PROJECT_MAX);
    let mut s = String::new();
    for &i in idx {
        let it = &w.items[i];
        let line = format!(
            "{} {}  {:>3}  {}  {}",
            mark(w, it),
            short(&it.id),
            ui::ago(it.mtime),
            pad(&it.project, pw),
            one_line(&it.name)
        );
        s.push_str(&clip(&line, width));
        s.push('\n');
    }
    s
}

/// The one state worth a glyph, in the dashboard's order: waiting on you, running, unfinished.
fn mark(w: &World, it: &Item) -> char {
    match w.live.get(&it.id) {
        Some(l) if l.needs_you() => '◆',
        Some(_) => '◉',
        None if it.open => '▶',
        None => ' ',
    }
}

// ---------- show ----------

fn show(o: &Opts) -> Result<(), Fail> {
    let (w, i) = session(one_id(o, "show")?)?;
    let it = &w.items[i];
    let d = model::read_detail(it.source, &it.file);
    // The detail read has the latest title; the head read may only have the first.
    let title = d.custom.as_deref().or(d.ai.as_deref()).unwrap_or(&it.name);

    if o.json {
        let full = ShowJson {
            session: session_json(&w, it, title),
            prompts: d.count,
            last_prompt: d.last.as_deref(),
            recap: d.recap.as_deref(),
            summary: d.summary.as_deref(),
            last_reply: d.reply.as_deref(),
            tokens: d.tokens,
        };
        out(&format!("{}\n", serde_json::to_string(&full).expect("plain data")));
        return Ok(());
    }

    let mut s = format!("{}\n", one_line(title));
    let mut field = |k: &str, v: &str| s.push_str(&format!("  {k:<9} {v}\n"));
    field("id", &it.id);
    if it.source != Source::Claude {
        field("source", it.source.as_str());
    }
    match it.cwd.as_deref() {
        Some(cwd) => field("project", &format!("{} · {cwd}", it.project)),
        None => field("project", &it.project),
    }
    if let Some(b) = d.branch.as_deref().or(it.branch.as_deref()) {
        field("branch", b);
    }
    let n = d.count;
    field("updated", &format!("{} ago · {n} prompt{}", ui::ago(it.mtime), if n == 1 { "" } else { "s" }));
    if let Some(l) = w.live.get(&it.id) {
        let what = if l.needs_you() { "◆ waiting on you" } else { "◉ running" };
        field("status", &format!("{what} — {}", ui::running_where(l)));
    }
    if let Some(why) = it.open_why() {
        field("open", &format!("▶ {why}"));
    }
    if w.archived(it) {
        field("archived", "yes");
    }
    if let Some(t) = &d.tokens {
        field("tokens", &ui::tokens_fmt(t));
    }

    let mut section = |label: &str, body: Option<&str>| {
        if let Some(b) = body.filter(|b| !b.trim().is_empty()) {
            s.push_str(&format!("\n{label}\n"));
            for line in b.lines() {
                if !line.is_empty() {
                    s.push_str("  ");
                }
                s.push_str(line);
                s.push('\n');
            }
        }
    };
    match d.recap.as_deref() {
        Some(r) => section("recap", Some(r)),
        None => section("summary", d.summary.as_deref()),
    }
    section("first prompt", d.first.as_deref());
    if d.count > 1 {
        section("last prompt", d.last.as_deref());
    }
    section("last reply", d.reply.as_deref());
    out(&s);
    Ok(())
}

// ---------- resume / reply / archive ----------

fn resume_cmd(o: &Opts) -> Result<(), Fail> {
    let (w, i) = session(one_id(o, "resume")?)?;
    let it = &w.items[i];
    let cwd = it.cwd.as_deref().map(Path::new);
    let manual = resume::manual_command(cwd, it.source, &it.id);
    let running = w.live.get(&it.id);

    if o.print {
        if let Some(l) = running {
            eprintln!("note: already running ({}) — this would open it twice", ui::running_where(l));
        }
        out(&format!("{manual}\n"));
        return Ok(());
    }
    // The dashboard's guard: a second `claude` on the same transcript and both append to it.
    if let (Some(l), false) = (running, o.force) {
        return Err(err(format!(
            "already running ({}) — go to that window, or pass --force to open it twice",
            ui::running_where(l)
        )));
    }
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        return Err(err(format!("resume needs an interactive terminal. Run it yourself:\n  {manual}")));
    }
    let e = resume::resume_in_place(cwd, &resume::resume_argv(it.source, &it.id)); // returns only on failure
    Err(err(format!(
        "couldn't launch {} ({e}). Run it yourself:\n  {manual}",
        resume::program(it.source)
    )))
}

/// One turn through `claude -p --resume`, as `^r` sends it, with the answer on stdout.
fn reply(o: &Opts) -> Result<(), Fail> {
    let Some((id, words)) = o.args.split_first() else {
        return Err(usage_err("reply needs a session id and a message"));
    };
    let msg = if words == ["-"] {
        let mut m = String::new();
        std::io::stdin()
            .read_to_string(&mut m)
            .map_err(|e| err(format!("couldn't read the message from stdin: {e}")))?;
        m
    } else {
        words.join(" ")
    };
    if msg.trim().is_empty() {
        return Err(usage_err("reply needs a message"));
    }
    let (w, i) = session(id)?;
    let it = &w.items[i];
    if it.source != Source::Claude {
        return Err(err(format!(
            "reply is Claude-only — resume this {} session instead: sessions resume {}",
            it.source.as_str(),
            short(&it.id)
        )));
    }
    // There is no safe way to put text into the stdin of a `claude` someone is sitting in front of.
    if let Some(l) = w.live.get(&it.id) {
        return Err(err(format!("that session is running ({}) — answer it there", ui::running_where(l))));
    }
    if std::io::stderr().is_terminal() {
        eprintln!("⏳ sending to \"{}\" …", clip(&one_line(&it.name), 60));
    }

    let mut cmd = Command::new("claude");
    cmd.arg("-p").arg("--resume").arg(&it.id).arg(&msg).stdin(Stdio::null());
    if let Some(dir) = it.cwd.as_deref() {
        cmd.current_dir(dir);
    }
    let res = cmd.output().map_err(|e| err(format!("couldn't run claude: {e}")))?;
    if !res.status.success() {
        let why = sanitize(&String::from_utf8_lossy(&res.stderr));
        let why = why.trim();
        return Err(err(if why.is_empty() {
            format!("claude failed ({})", res.status)
        } else {
            format!("claude failed ({}): {why}", res.status)
        }));
    }
    // Claude's answer is transcript text like any other: no escape sequences reach the terminal.
    let mut text = sanitize(&String::from_utf8_lossy(&res.stdout));
    if !text.ends_with('\n') {
        text.push('\n');
    }
    out(&text);
    Ok(())
}

fn archive(o: &Opts, archived: bool) -> Result<(), Fail> {
    if o.args.is_empty() {
        let cmd = if archived { "archive" } else { "unarchive" };
        return Err(usage_err(format!("{cmd} needs at least one session id")));
    }
    let mut s = String::new();
    for arg in &o.args {
        let (mut w, i) = session(arg)?;
        let (key, id, title) = (w.items[i].key.clone(), w.items[i].id.clone(), one_line(&w.items[i].name));
        let verb = match (archived, w.archive.set(&key, &id, archived)) {
            (true, true) => "archived",
            (true, false) => "already archived",
            (false, true) => "unarchived",
            (false, false) => "not archived",
        };
        s.push_str(&format!("{verb}  {}  {title}\n", short(&id)));
    }
    out(&s);
    Ok(())
}

/// `^k` as a command: SIGTERM to a running session idle for more than 48 hours, under the same
/// rules. There is no second press to ask for here — running the command is the consent — so the
/// rules themselves are the whole guard, and `kill::end` re-checks the pid before signalling.
fn kill_cmd(o: &Opts) -> Result<(), Fail> {
    let (w, i) = session(one_id(o, "kill")?)?;
    let it = &w.items[i];
    if it.source != Source::Claude {
        return Err(err(format!("won't end {}: kill is Claude-only", short(&it.id))));
    }
    let live = w.live.get(&it.id);
    let v = sessio::kill::verdict(live, it.mtime, model::now_ms());
    if let Some(why) = v.refusal() {
        return Err(err(format!("won't end {}: {why}", short(&it.id))));
    }
    let (Some(l), sessio::kill::Verdict::Stale { idle_ms }) = (live, v) else {
        unreachable!("no refusal means stale, and stale means running");
    };
    let outcome = sessio::kill::end(&it.id, l.pid)
        .map_err(|why| err(format!("won't end {}: {why}", short(&it.id))))?;
    let ended = outcome == sessio::kill::Outcome::Ended;
    let days = sessio::kill::idle_days(idle_ms);
    let title = one_line(&it.name);
    if o.json {
        let j = KillJson {
            id: &it.id,
            title: &title,
            pid: l.pid,
            tty: (!l.tty.is_empty()).then_some(l.tty.as_str()),
            idle_days: days,
            ended,
        };
        out(&format!("{}\n", serde_json::to_string(&j).expect("plain data")));
    } else if ended {
        out(&format!("ended  {}  pid {} · idle {days}d  {title}\n", short(&it.id), l.pid));
    }
    if ended {
        Ok(())
    } else {
        Err(err(format!("sent SIGTERM to pid {} — still running after 3s", l.pid)))
    }
}

// ---------- JSON ----------

#[derive(serde::Serialize)]
struct KillJson<'a> {
    id: &'a str,
    title: &'a str,
    pid: i32,
    tty: Option<&'a str>,
    idle_days: i64,
    /// `false` when the process was signalled and was still there when the wait ran out.
    ended: bool,
}

#[derive(serde::Serialize)]
struct RunningJson<'a> {
    pid: i32,
    tty: Option<&'a str>,
    /// `idle`, `busy`, `shell` or `waiting`, as Claude Code's registry reports it.
    status: Option<&'a str>,
    waiting_for: Option<&'a str>,
}

#[derive(serde::Serialize)]
struct SessionJson<'a> {
    id: &'a str,
    /// `claude` or `copilot`: which agent wrote the session, and so which one resumes it.
    source: &'static str,
    title: &'a str,
    project: &'a str,
    cwd: Option<&'a str>,
    branch: Option<&'a str>,
    modified: String,
    open: bool,
    open_reason: Option<&'static str>,
    running: Option<RunningJson<'a>>,
    archived: bool,
    first_prompt: Option<&'a str>,
    transcript: String,
}

#[derive(serde::Serialize)]
struct ShowJson<'a> {
    #[serde(flatten)]
    session: SessionJson<'a>,
    prompts: usize,
    last_prompt: Option<&'a str>,
    recap: Option<&'a str>,
    summary: Option<&'a str>,
    last_reply: Option<&'a str>,
    /// Summed once per API response; `null` when the transcript carries no usage data.
    tokens: Option<parse::Usage>,
}

fn session_json<'a>(w: &'a World, it: &'a Item, title: &'a str) -> SessionJson<'a> {
    let some = |s: &'a str| (!s.is_empty()).then_some(s);
    SessionJson {
        id: &it.id,
        source: it.source.as_str(),
        title,
        project: &it.project,
        cwd: it.cwd.as_deref(),
        branch: it.branch.as_deref(),
        modified: chrono::DateTime::from_timestamp_millis(it.mtime)
            .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
            .unwrap_or_default(),
        open: it.open,
        open_reason: it.open_why(),
        running: w.live.get(&it.id).map(|l| RunningJson {
            pid: l.pid,
            tty: some(&l.tty),
            status: some(&l.status),
            waiting_for: some(&l.waiting_for),
        }),
        archived: w.archived(it),
        first_prompt: it.first.as_deref(),
        transcript: it.file.display().to_string(),
    }
}

// ---------- text helpers ----------

/// Print to stdout, and stop quietly if the reader has gone: `sessions ls | head -1` closes the
/// pipe early, and `println!` panics on that.
fn out(s: &str) {
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(s.as_bytes()).and_then(|_| o.flush());
}

fn term_width() -> usize {
    if !std::io::stdout().is_terminal() {
        return usize::MAX;
    }
    ratatui::crossterm::terminal::size().map_or(usize::MAX, |(c, _)| c as usize)
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// A title as one line: a first prompt can run to paragraphs.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// At most `max` columns wide, by display width, with `…` marking a cut.
fn clip(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let mut w = 0;
    let mut o = String::new();
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        w += cw;
        o.push(c);
    }
    o.push('…');
    o
}

/// Exactly `w` columns: clipped if longer, space-padded if shorter.
fn pad(s: &str, w: usize) -> String {
    let c = clip(s, w);
    let fill = w.saturating_sub(c.width());
    format!("{c}{}", " ".repeat(fill))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn flags_and_values_parse_in_either_spelling() {
        let o = parse("ls", &args(&["-p", "sessio", "--open", "-n", "5", "--json"])).unwrap();
        assert_eq!(o.project.as_deref(), Some("sessio"));
        assert_eq!(o.limit, Some(5));
        assert!(o.open && o.json && !o.running);

        let o = parse("ls", &args(&["--project=my app", "--limit=0"])).unwrap();
        assert_eq!(o.project.as_deref(), Some("my app"));
        assert_eq!(o.limit, Some(0));
    }

    #[test]
    fn a_flag_another_command_owns_is_refused() {
        // Ignoring it would hand an agent that meant `--open` every session instead.
        assert!(matches!(parse("show", &args(&["abc", "--open"])), Err(Fail::Usage(_))));
        assert!(matches!(parse("ls", &args(&["--text"])), Err(Fail::Usage(_))));
        assert!(matches!(parse("ls", &args(&["--opne"])), Err(Fail::Usage(_))));
        assert!(matches!(parse("ls", &args(&["--open=yes"])), Err(Fail::Usage(_))));
        assert!(matches!(parse("ls", &args(&["-n", "many"])), Err(Fail::Usage(_))));
        assert!(matches!(parse("ls", &args(&["--project"])), Err(Fail::Usage(_))));
    }

    #[test]
    fn everything_after_a_double_dash_is_the_message() {
        let o = parse("reply", &args(&["abc", "--", "--help", "-n", "is", "broken"])).unwrap();
        assert!(!o.help);
        assert_eq!(o.args, args(&["abc", "--help", "-n", "is", "broken"]));
        // A lone dash is stdin, not an option.
        assert_eq!(parse("reply", &args(&["abc", "-"])).unwrap().args, args(&["abc", "-"]));
    }

    #[test]
    fn help_is_accepted_by_every_command() {
        for cmd in COMMANDS {
            assert!(parse(cmd, &args(&["--help"])).unwrap().help, "{cmd}");
        }
    }

    #[test]
    fn titles_print_on_one_line() {
        assert_eq!(one_line("fix the\n\n  thing\tnow "), "fix the thing now");
    }

    #[test]
    fn clipping_counts_columns_not_bytes() {
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("abcdefghij", 5), "abcd…");
        assert_eq!(clip("日本語のテキスト", 7), "日本語…");
        assert!(clip("日本語のテキスト", 7).width() <= 7);
        assert_eq!(pad("ab", 4), "ab  ");
        assert_eq!(pad("abcdef", 4), "abc…");
    }

    #[test]
    fn a_short_id_is_eight_characters_or_the_whole_id() {
        assert_eq!(short("3f2a91c0-1111-2222-3333-444455556666"), "3f2a91c0");
        assert_eq!(short("abc"), "abc");
    }
}
