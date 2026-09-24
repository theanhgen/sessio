//! The dashboard. Port of the picker, preview and event loop (bin/sessio.mjs:360-782).

use std::collections::{HashMap, HashSet};
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

// Everything below this line drives a terminal. The browser build keeps the dashboard and drops
// the driver: crossterm does not compile for wasm at all, and there is nothing there to drive.
#[cfg(not(target_arch = "wasm32"))]
use ratatui::backend::CrosstermBackend;
#[cfg(not(target_arch = "wasm32"))]
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
#[cfg(not(target_arch = "wasm32"))]
use ratatui::crossterm::execute;
#[cfg(not(target_arch = "wasm32"))]
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
#[cfg(not(target_arch = "wasm32"))]
use ratatui::widgets::Paragraph;
#[cfg(not(target_arch = "wasm32"))]
use ratatui::Terminal;
use unicode_width::UnicodeWidthStr;

use crate::discover::Source;
use crate::md::md_lines;
use crate::model::{self, Item, ACTIVE_MS, ALL_TAB, ARCHIVED_TAB, OPEN_TAB, RECENT_MS, WAITING_TAB};
use crate::parse::{Detail, Entry, Who};
use crate::safety::sanitize;
use crate::store::Archive;
use crate::{discover, resume, search};

pub use crate::theme;
use crate::theme::{dim, panel_selected, tab_selected, Tone};

const REFRESH: Duration = Duration::from_secs(2);
/// How long a flash stays on screen. It has to be a duration, not a frame: this loop redraws on
/// every 120ms input poll, and a message cleared after one frame — as the JS reference does, where
/// a frame is a keypress or the 2s tick — was gone before anyone could read it.
const FLASH: Duration = Duration::from_secs(5);
/// The dashboard's fixed regions, top to bottom, in the column right of the project panel: the
/// key bar, the query, the selected project's context line and the session strip. Each is exactly
/// one row, so the preview under them starts on the same row whatever is selected, typed or
/// flashed. The session strip, like a browser's, scrolls rather than wraps: it never takes a
/// second row.
const KEYBAR_ROW: usize = 0;
const QUERY_ROW: usize = 1;
const CONTEXT_ROW: usize = 2;
const STRIP_ROW: usize = 3;
/// Rows above the preview (plus one while the reply composer is open).
const CHROME: usize = STRIP_ROW + 1;
/// The feedback region: the bottom row(s), the whole terminal wide, under the panel and the
/// preview alike. One row, a second only while a message needs it; blank when nothing is said.
const FEEDBACK_MAX: usize = 2;
/// The smallest window the dashboard lays out. Below it the frame says so and keeps only the
/// essentials, rather than squeezing the regions into each other. The width is the panel at its
/// narrowest, its rule, and a body that still holds a readable strip; the height is the chrome,
/// the preview's rule, title and facts, a few lines of content and the feedback row.
const MIN_COLS: usize = 50;
const MIN_ROWS: usize = 12;
/// No one tab may take more than this, however long its title — the focused tab is allowed to be
/// the wide one, but not so wide that nothing is left to steer by.
const TAB_MAX: usize = 44;
/// Columns held back for the `‹N` / `+N›` counts when the strip scrolls.
const MARKERS: usize = 12;
/// How many entries a followed session's tail keeps. More than a tall window shows, so the
/// preview can bottom-anchor on whole entries rather than run out of them.
const FOLLOW_ENTRIES: usize = 40;

enum Msg {
    Items(Vec<Item>, crate::live::LiveMap),
    Detail { key: String, mtime: i64, detail: Box<Detail> },
    Search { gen: u64, query: String, files: Option<HashSet<PathBuf>> },
    /// A headless reply came back (or failed). `key` identifies the session it belongs to.
    Replied { key: String, ok: bool, text: String },
    /// A `^k` finished: what happened to the process, ready to flash.
    Ended(Tone, String),
    /// A fresh read of the followed session's tail.
    Tail { key: String, entries: Vec<Entry> },
}

/// `^t`: the preview pinned to a running session, showing the end of its transcript and kept
/// there on every refresh. Read-only: the transcript is read, never written, and nothing is
/// attached to the `claude` running it.
struct Follow {
    key: String,
    id: String,
    name: String,
    file: PathBuf,
    /// `None` until the first read lands.
    entries: Option<Vec<Entry>>,
    /// The session stopped running while it was followed. The tail stays on screen.
    ended: bool,
}

struct Deep {
    query: String,
    keys: HashSet<String>,
    files: Vec<PathBuf>,
}

struct App {
    items: Vec<Item>,
    archive: Archive,
    tabs: Vec<String>,
    q: String,
    cur: usize,
    p_idx: usize,
    expand: bool,
    help: bool,
    flash: String,
    /// What kind of thing the flash is saying: success, warning, error or neither.
    flash_tone: Tone,
    /// When the current flash stops being shown. `None` means there is nothing to expire.
    flash_until: Option<Instant>,
    deep: Option<Deep>,
    search_gen: u64,
    details: HashMap<String, (i64, Detail)>,
    detail_inflight: HashSet<String>,
    /// Sessions with a `claude` process attached right now, by session id.
    live: crate::live::LiveMap,
    /// Session the user has been warned about and may now resume anyway, and whether the warning
    /// came from `^o` (new window) rather than `↵` — consent is to the key that was warned about.
    confirm: Option<(String, bool)>,
    /// Session (and the pid it was running as) the user was asked about with `^k`. A second `^k`
    /// ends it only while both still match.
    kill_confirm: Option<(String, i32)>,
    /// The reply being typed, and the session it is addressed to. `None` when not replying.
    draft: Option<(String, String)>, // (session id, text)
    /// Sessions with a headless reply in flight, by id.
    sending: HashSet<String>,
    /// Whether the "this spends tokens" warning has been acknowledged this run.
    reply_ok: bool,
    /// The running session `^t` pinned the preview to. Any move unpins it.
    follow: Option<Follow>,
    /// Whether `^g` has swapped the preview for the highlighted folder's GitHub issues.
    issues: bool,
    /// The highlighted row in that list.
    issue_cur: usize,
    tx: Sender<Msg>,
}

impl App {
    /// Say something back about the key just pressed, and keep saying it long enough to be read.
    /// `tone` picks its colour and, for an error, its `✗`: see `theme::Tone`.
    fn say(&mut self, tone: Tone, msg: String) {
        self.flash = msg;
        self.flash_tone = tone;
        // `Instant::now()` is not implemented on wasm32-unknown-unknown and panics outright. The
        // browser has no event loop expiring these anyway — `flash_expired` runs only in the
        // terminal — so there it simply has no deadline.
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.flash_until = Some(Instant::now() + FLASH);
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.flash_until = None;
        }
    }

    fn archived(&self, it: &Item) -> bool {
        self.archive.contains(&it.key, &it.id)
    }

    /// Whether `it` belongs under `tab`, before any query. The one definition the list and the
    /// panel's counts share, so a count can never disagree with what the tab shows.
    fn in_tab(&self, tab: &str, it: &Item) -> bool {
        if tab == ARCHIVED_TAB {
            return self.archived(it);
        }
        let in_tab = match tab {
            ALL_TAB => true,
            OPEN_TAB => it.open,
            WAITING_TAB => self.is_waiting(it),
            p => it.project == p,
        };
        in_tab && !self.archived(it)
    }

    /// Sessions under `tab` touched in the last 24 hours, and all of them.
    fn tab_counts(&self, tab: &str, now: i64) -> (usize, usize) {
        self.items.iter().filter(|it| self.in_tab(tab, it)).fold((0, 0), |(day, all), it| {
            (day + usize::from(now - it.mtime < DAY_MS), all + 1)
        })
    }

    /// Indices into `items`, filtered by tab and query, ranked. Port of `view()` at :476.
    fn view(&self) -> Vec<usize> {
        let tab = self.tabs.get(self.p_idx).map(String::as_str).unwrap_or(ALL_TAB);
        let mut idx: Vec<usize> =
            (0..self.items.len()).filter(|&i| self.in_tab(tab, &self.items[i])).collect();

        if let Some(d) = &self.deep {
            idx.retain(|&i| d.keys.contains(&self.items[i].key));
        } else if !self.q.is_empty() {
            let pairs: Vec<(&str, i64)> =
                idx.iter().map(|&i| (self.items[i].hay.as_str(), self.items[i].mtime)).collect();
            let order = crate::rank::rank(&pairs, &self.q);
            idx = order.into_iter().map(|p| idx[p]).collect();
        }
        idx
    }

    fn selected(&self) -> Option<usize> {
        self.view().get(self.cur).copied()
    }

    /// Lazily full-read the highlighted session for its preview.
    fn ensure_detail(&mut self) {
        let Some(i) = self.selected() else { return };
        let (key, mtime, file, source) = (
            self.items[i].key.clone(),
            self.items[i].mtime,
            self.items[i].file.clone(),
            self.items[i].source,
        );
        if let Some((m, d)) = self.details.get(&key) {
            if *m == mtime {
                let d = d.clone();
                apply_detail(&mut self.items[i], d);
                return;
            }
        }
        if !self.detail_inflight.insert(key.clone()) {
            return;
        }
        // The read happens on a worker so the input path never waits on a multi-megabyte file.
        // The browser has neither threads nor the file, and does not need them: the demo's
        // details are already attached to its items, so a miss here has no answer to wait for.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let d = model::read_detail(source, &file);
                let _ = tx.send(Msg::Detail { key, mtime, detail: Box::new(d) });
            });
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (key, mtime, file, source);
    }

    /// Every edit to the query invalidates the content search and the scroll position.
    fn requery(&mut self) {
        self.deep = None;
        self.search_gen += 1;
        self.reset_position();
        self.ensure_detail();
    }

    fn reset_position(&mut self) {
        self.cur = 0;
        self.follow = None;
    }

    /// The projects are a column, so they move on ↑↓; the sessions move on ←→. Kept here rather
    /// than inline in `handle_key` so the mapping can be tested without standing up a Terminal.
    fn step_project(&mut self, delta: isize) {
        if self.tabs.is_empty() {
            return;
        }
        let n = self.tabs.len() as isize;
        self.p_idx = ((self.p_idx as isize + delta).rem_euclid(n)) as usize;
        self.reset_position();
        self.ensure_detail();
    }

    fn step_session(&mut self, delta: isize) {
        self.follow = None;
        if delta < 0 {
            self.cur = self.cur.saturating_sub(delta.unsigned_abs());
        } else if self.cur + 1 < self.view().len() {
            self.cur += 1;
        }
        self.ensure_detail();
    }

    /// Open the reply composer on the highlighted session, or say why it cannot be opened.
    fn begin_reply(&mut self) {
        let Some(i) = self.selected() else { return };
        let (id, name) = (self.items[i].id.clone(), self.items[i].name.clone());
        if self.items[i].source != Source::Claude {
            // `claude -p --resume` is the only headless turn there is; Copilot has no equivalent
            // sessio drives.
            self.say(Tone::Warning, "reply is Claude-only — ↵ resumes this Copilot session".into());
        } else if let Some(live) = self.live.get(&id) {
            // The guard ↵ already uses: there is no safe way to put text into the stdin of a
            // `claude` someone is sitting in front of.
            let where_ = running_where(live);
            self.say(Tone::Warning, format!("that session is running — {where_} · answer it there"));
        } else if self.sending.contains(&id) {
            self.say(Tone::Warning, "still waiting on the last reply".into());
        } else if !self.reply_ok {
            // Once per run: this spends tokens from a list, with no turn-by-turn to watch.
            self.reply_ok = true;
            self.say(Tone::Warning, format!(
                "^r sends a turn to \"{}\" and spends tokens — ^r again to write it",
                first_words(&name, 4)
            ));
        } else {
            self.draft = Some((id, String::new()));
        }
    }

    /// Whether a `claude` on this session has stopped and wants something from you.
    fn is_waiting(&self, it: &Item) -> bool {
        self.live.get(&it.id).is_some_and(|l| l.needs_you())
    }

    /// The project panel: `model::tabs_for`, plus a waiting tab right below the open one while any
    /// visible session is waiting on you — so they are one ↑↓ away instead of scattered across
    /// projects. It comes and goes with the live refresh.
    fn tabs_now(&self) -> Vec<String> {
        let mut tabs = model::tabs_for(&self.items, &self.archive);
        if self.items.iter().any(|it| self.is_waiting(it) && !self.archived(it)) {
            let at = tabs.iter().position(|t| t == OPEN_TAB).map_or(1, |i| i + 1);
            tabs.insert(at, WAITING_TAB.to_string());
        }
        tabs
    }

    /// `^t`: pin the preview to the highlighted session and follow its tail, or unpin it. Only a
    /// running session has a tail worth following; on anything else it says so and does nothing.
    fn toggle_follow(&mut self) {
        if self.follow.take().is_some() {
            self.say(Tone::Info, "stopped following".into());
            return;
        }
        let Some(i) = self.selected() else { return };
        let it = &self.items[i];
        let short = first_words(&it.name, 4);
        if it.source != Source::Claude {
            // Neither the running check nor the tail reader knows Copilot's events.
            self.say(Tone::Warning, "follow is Claude-only".into());
            return;
        }
        if !self.live.contains_key(&it.id) {
            self.say(Tone::Warning, format!("\"{short}\" is not running — nothing to follow"));
            return;
        }
        self.follow = Some(Follow {
            key: it.key.clone(),
            id: it.id.clone(),
            name: it.name.clone(),
            file: it.file.clone(),
            entries: None,
            ended: false,
        });
        self.issues = false; // both take the preview's place; only one can have it
        self.say(Tone::Info, format!("following \"{short}\" — read-only · any move stops it"));
        self.read_tail();
    }

    /// Read the followed session's tail on a worker, like the detail read. The browser has no
    /// transcript to read.
    fn read_tail(&self) {
        let Some(f) = &self.follow else { return };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tx, key, file) = (self.tx.clone(), f.key.clone(), f.file.clone());
            std::thread::spawn(move || {
                let entries =
                    crate::parse::follow_tail(&file, crate::parse::FOLLOW_WINDOW, FOLLOW_ENTRIES);
                let _ = tx.send(Msg::Tail { key, entries });
            });
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (&f.file, FOLLOW_ENTRIES);
    }

    /// A refresh landed: note whether the followed session is still running.
    fn note_follow_live(&mut self) {
        if let Some(f) = &mut self.follow {
            f.ended = !self.live.contains_key(&f.id);
        }
    }

    /// GitHub issues for the highlighted session's folder, from cache; a miss queues a fetch.
    /// `None` when there is no folder to ask about, and always in the browser, which has no `gh`.
    fn issue_status(&self) -> Option<crate::issues::Status> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let i = self.selected()?;
            self.items[i].cwd.as_deref().map(crate::issues::status)
        }
        #[cfg(target_arch = "wasm32")]
        None
    }

    /// `^g`: show the issues list, or say why there is none to show.
    fn toggle_issues(&mut self) {
        use crate::issues::Status;
        if self.issues {
            self.issues = false;
            return;
        }
        match self.issue_status() {
            Some(Status::Ready { issues, .. }) if !issues.is_empty() => {
                self.issues = true;
                self.issue_cur = 0;
                self.follow = None; // both take the preview's place; only one can have it
            }
            Some(Status::Ready { slug, .. }) => self.say(Tone::Warning, format!("no open issues in {slug}")),
            Some(Status::Loading) => self.say(Tone::Info, "issues still loading…".into()),
            Some(Status::Failed(why)) => self.say(Tone::Error, format!("issues: {why}")),
            Some(Status::NoRemote) | None => self.say(Tone::Warning, "no GitHub remote for this folder".into()),
        }
    }

    /// `^k`: end the highlighted session's `claude` if it has sat idle for more than 48 hours.
    ///
    /// The first press says exactly what would be ended and asks for `^k` again; the second, on
    /// the same session and process, does it. The signal goes out on a worker, which re-checks
    /// the pid against a fresh `ps` first and then waits for the process to go.
    #[cfg(not(target_arch = "wasm32"))]
    fn kill_key(&mut self) {
        let Some(i) = self.selected() else { return };
        let it = &self.items[i];
        if it.source != Source::Claude {
            // The running check and the pid guard only know Claude's registry.
            self.kill_confirm = None;
            self.say(Tone::Warning, "^k is Claude-only".into());
            return;
        }
        let (id, title) = (it.id.clone(), first_words(&sanitize(it.display_name()), 6));
        let live = self.live.get(&id).cloned();
        let v = crate::kill::verdict(live.as_ref(), it.mtime, model::now_ms());
        if let Some(why) = v.refusal() {
            self.kill_confirm = None;
            self.say(Tone::Warning, format!("can't end \"{title}\": {why}"));
            return;
        }
        let (Some(live), crate::kill::Verdict::Stale { idle_ms }) = (live, v) else { return };
        if crate::kill::own_ancestry().contains(&live.pid) {
            self.kill_confirm = None;
            self.say(Tone::Warning, format!("can't end \"{title}\": sessio is running inside it"));
            return;
        }
        if self.kill_confirm.as_ref() != Some(&(id.clone(), live.pid)) {
            self.kill_confirm = Some((id, live.pid));
            let mut at = format!("pid {}", live.pid);
            if !live.tty.is_empty() {
                at.push_str(&format!(" · {}", live.tty));
            }
            let days = crate::kill::idle_days(idle_ms);
            self.say(Tone::Warning, format!("end \"{title}\" ({at}, idle {days}d)? ^k again"));
            return;
        }
        self.kill_confirm = None;
        self.say(Tone::Info, format!("ending \"{title}\" (pid {})…", live.pid));
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            use crate::kill::Outcome;
            let (tone, text) = match crate::kill::end(&id, live.pid) {
                Ok(Outcome::Ended) => {
                    (Tone::Success, format!("✓ ended \"{title}\" (pid {})", live.pid))
                }
                Ok(Outcome::StillRunning) => (
                    Tone::Warning,
                    format!("sent SIGTERM to pid {} — still running after 3s", live.pid),
                ),
                Err(why) => (Tone::Error, format!("didn't end \"{title}\": {why}")),
            };
            let _ = tx.send(Msg::Ended(tone, text));
        });
    }

    fn rebuild_tabs(&mut self) {
        let active = self.tabs.get(self.p_idx).cloned();
        self.tabs = self.tabs_now();
        self.p_idx = active
            .and_then(|name| self.tabs.iter().position(|t| *t == name))
            .unwrap_or(0);
    }
}

/// Detail overrides the head-derived title once it lands, as in `ensureDetail` at :500.
fn apply_detail(it: &mut Item, d: Detail) {
    if d.custom.is_some() || d.ai.is_some() {
        let t = d.custom.clone().or_else(|| d.ai.clone());
        if let Some(t) = t {
            it.title = Some(t.clone());
            it.name = t;
        }
    }
    it.detail = Some(d);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn run() -> io::Result<()> {
    let mut archive = Archive::load();
    let items = model::load(&[]);
    archive.release_reactivated(items.iter().map(|i| (i.key.as_str(), i.id.as_str(), i.mtime)));
    if items.is_empty() {
        println!("No sessions found.");
        return Ok(());
    }
    if !std::io::IsTerminal::is_terminal(&io::stdin()) || !std::io::IsTerminal::is_terminal(&io::stdout()) {
        eprintln!("sessions requires an interactive terminal.");
        std::process::exit(1);
    }

    let (tx, rx) = mpsc::channel();
    let tabs = model::tabs_for(&items, &archive);
    // Launched from inside a project, open on it rather than on everything.
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let p_idx = std::env::current_dir()
        .ok()
        .and_then(|dir| model::tab_for_dir(&items, &tabs, &dir, home.as_deref()))
        .unwrap_or(0);
    let mut app = App {
        items,
        archive,
        tabs,
        q: String::new(),
        cur: 0,
        p_idx,
        expand: false,
        help: false,
        flash: String::new(),
        flash_tone: Tone::Info,
        flash_until: None,
        deep: None,
        search_gen: 0,
        details: HashMap::new(),
        detail_inflight: HashSet::new(),
        live: crate::live::scan(),
        confirm: None,
        kill_confirm: None,
        draft: None,
        sending: HashSet::new(),
        reply_ok: false,
        follow: None,
        issues: false,
        issue_cur: 0,
        tx: tx.clone(),
    };

    let mut term = setup()?;
    let result = event_loop(&mut term, &mut app, &rx, &tx);
    restore(&mut term);
    result
}

#[cfg(not(target_arch = "wasm32"))]
fn setup() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    // Never leave the terminal in raw mode or the cursor hidden, whatever the exit path.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        hook(info);
    }));
    let mut term = Terminal::new(CrosstermBackend::new(out))?;
    term.hide_cursor()?;
    Ok(term)
}

#[cfg(not(target_arch = "wasm32"))]
fn restore(term: &mut Terminal<CrosstermBackend<Stdout>>) {
    let _ = disable_raw_mode();
    let _ = execute!(term.backend_mut(), LeaveAlternateScreen);
    let _ = term.show_cursor();
}

#[cfg(not(target_arch = "wasm32"))]
fn event_loop(
    term: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    rx: &Receiver<Msg>,
    tx: &Sender<Msg>,
) -> io::Result<()> {
    app.ensure_detail();
    let mut last_refresh = Instant::now();
    let mut refreshing = false;

    loop {
        if flash_expired(Instant::now(), app.flash_until) {
            app.flash.clear();
            app.flash_until = None;
            // The warning and the consent it asks for run out together: "↵ again" has to mean
            // again *now*, not an hour later with the message long gone and the session still
            // marked as one the user already agreed to open twice.
            app.confirm = None;
            app.kill_confirm = None;
        }
        term.draw(|f| draw(f, app))?;

        // Live refresh: rescan every 2s on a worker so input never stalls behind the scan.
        if !refreshing && last_refresh.elapsed() >= REFRESH {
            refreshing = true;
            last_refresh = Instant::now();
            let tx2 = tx.clone();
            let extra = app.deep.as_ref().map(|d| d.files.clone()).unwrap_or_default();
            // A session that has stopped writes nothing more, so its tail is not re-read.
            let follow =
                app.follow.as_ref().filter(|f| !f.ended).map(|f| (f.key.clone(), f.file.clone()));
            std::thread::spawn(move || {
                let items = model::load(&extra);
                // Same worker: one `ps` per refresh, off the input path.
                let _ = tx2.send(Msg::Items(items, crate::live::scan()));
                // Read after the scan, so the refresh that finds the session gone also reads the
                // last thing it wrote.
                if let Some((key, file)) = follow {
                    let entries = crate::parse::follow_tail(
                        &file,
                        crate::parse::FOLLOW_WINDOW,
                        FOLLOW_ENTRIES,
                    );
                    let _ = tx2.send(Msg::Tail { key, entries });
                }
            });
        }

        if event::poll(Duration::from_millis(120))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    match handle_key(term, app, k)? {
                        Flow::Quit => return Ok(()),
                        Flow::Continue => {}
                    }
                }
            }
        }

        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Items(new_items, live) => {
                    refreshing = false;
                    announce_waiting(app, &live);
                    app.live = live;
                    app.note_follow_live();
                    absorb_items(app, new_items);
                }
                Msg::Tail { key, entries } => {
                    if let Some(f) = app.follow.as_mut().filter(|f| f.key == key) {
                        f.entries = Some(entries);
                    }
                }
                Msg::Detail { key, mtime, detail } => {
                    app.detail_inflight.remove(&key);
                    app.details.insert(key.clone(), (mtime, (*detail).clone()));
                    if let Some(it) = app.items.iter_mut().find(|i| i.key == key && i.mtime == mtime)
                    {
                        apply_detail(it, *detail);
                    }
                }
                Msg::Search { gen, query, files } => {
                    if gen != app.search_gen {
                        continue; // a newer query superseded this search
                    }
                    match files {
                        None => app.say(Tone::Error, "content search failed".into()),
                        Some(files) => {
                            let keys =
                                files.iter().filter_map(|f| discover::key_for_file(f)).collect();
                            let list: Vec<PathBuf> = files.into_iter().collect();
                            app.items = model::load(&list);
                            app.deep = Some(Deep { query, keys, files: list });
                            app.rebuild_tabs();
                            app.p_idx = 0;
                            app.reset_position();
                            app.ensure_detail();
                        }
                    }
                }
                Msg::Replied { key, ok, text } => {
                    app.sending.remove(&key);
                    // Force the next draw to re-read the transcript: the reply is in the file
                    // now, and the cached detail is one turn out of date.
                    app.details.remove(&key);
                    app.ensure_detail();
                    if ok {
                        app.say(Tone::Success, format!("↩ replied · {}", first_words(&text, 8)));
                    } else {
                        app.say(Tone::Error, format!("reply failed · {}", first_words(&text, 10)));
                    }
                }
                Msg::Ended(tone, text) => app.say(tone, text),
            }
        }
    }
}

/// Tell the user about sessions that have just started waiting on them, comparing the snapshot
/// about to replace `app.live` against it. The first snapshot is taken at startup and only ever
/// plays `prev`, so a session already waiting when sessio opened is not announced.
#[cfg(not(target_arch = "wasm32"))]
fn announce_waiting(app: &mut App, live: &crate::live::LiveMap) {
    use crate::notify;
    let ids = notify::newly_waiting(&app.live, live);
    if ids.is_empty() || !notify::enabled() {
        return;
    }
    let waiting: Vec<(String, String)> = ids
        .iter()
        .map(|id| {
            let title = app
                .items
                .iter()
                .find(|it| &it.id == id)
                .map(|it| sanitize(it.display_name()))
                .unwrap_or_else(|| id.chars().take(8).collect());
            let why = live.get(id).map(|l| sanitize(&l.waiting_for)).unwrap_or_default();
            (title, why)
        })
        .collect();
    if let Some(msg) = notify::message(&waiting) {
        notify::post(&msg);
        // Not over a pending "↵ again" warning: `say` would hide it and restart its clock, leaving
        // the consent armed behind a message that no longer asks for it.
        if app.confirm.is_none() {
            app.say(Tone::Warning, msg);
        }
    }
}

/// Swap in a refreshed list, preserving the highlighted session, active tab and search.
#[cfg(not(target_arch = "wasm32"))]
fn absorb_items(app: &mut App, new_items: Vec<Item>) {
    let sel_key = app.selected().map(|i| app.items[i].key.clone());
    let active_tab = app.tabs.get(app.p_idx).cloned();
    app.items = new_items;
    // Re-attach any details already read, so the preview doesn't blank on every tick.
    for it in &mut app.items {
        if let Some((m, d)) = app.details.get(&it.key) {
            if *m == it.mtime {
                apply_detail(it, d.clone());
            }
        }
    }
    // A session you archived but have since worked in again is not one you are done with.
    let freed = {
        let (archive, items) = (&mut app.archive, &app.items);
        archive.release_reactivated(
            items.iter().map(|i| (i.key.as_str(), i.id.as_str(), i.mtime)),
        )
    };
    if freed > 0 {
        let s = if freed == 1 { "" } else { "s" };
        app.say(Tone::Success, format!("↩ {freed} archived session{s} back — active again"));
    }
    app.tabs = app.tabs_now();
    app.p_idx = active_tab
        .and_then(|name| app.tabs.iter().position(|t| *t == name))
        .unwrap_or(0);
    let v = app.view();
    app.cur = sel_key
        .and_then(|k| v.iter().position(|&i| app.items[i].key == k))
        .unwrap_or_else(|| app.cur.min(v.len().saturating_sub(1)));
    app.ensure_detail();
}

#[cfg(not(target_arch = "wasm32"))]
enum Flow {
    Continue,
    Quit,
}

#[cfg(not(target_arch = "wasm32"))]
fn handle_key(
    term: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    k: KeyEvent,
) -> io::Result<Flow> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);

    // While the composer is open it takes every key, so typing a reply cannot also filter the
    // list, archive a session, or walk the tabs out from under what you are answering.
    if app.draft.is_some() {
        return compose_key(app, k, ctrl);
    }

    // "↵ again to open it twice" means the *very next* key, and the same key. Moving, typing or
    // switching tabs is not consent to start a second process on a live transcript.
    if !is_resume_key(&k) {
        app.confirm = None;
    }
    // The same for `^k`: consent to end a process is the very next key, and only that key.
    if !(ctrl && k.code == KeyCode::Char('k')) {
        app.kill_confirm = None;
    }

    if ctrl && k.code == KeyCode::Char('c') {
        return Ok(Flow::Quit);
    }
    if app.help {
        app.help = false; // any key closes the overlay
        return Ok(Flow::Continue);
    }

    // The issues list borrows ↑↓ and ↵ from the dashboard while it is up; everything else keeps
    // its usual meaning, so ←→ still walks the sessions and the list follows their folder.
    if app.issues {
        match k.code {
            KeyCode::Esc => {
                app.issues = false;
                return Ok(Flow::Continue);
            }
            KeyCode::Char('g') if ctrl => {
                app.issues = false;
                return Ok(Flow::Continue);
            }
            KeyCode::Up => {
                app.issue_cur = app.issue_cur.saturating_sub(1);
                return Ok(Flow::Continue);
            }
            KeyCode::Down => {
                app.issue_cur += 1; // clamped against the list when it is drawn
                return Ok(Flow::Continue);
            }
            KeyCode::Enter => {
                open_issue(app);
                return Ok(Flow::Continue);
            }
            _ => {}
        }
    }

    match k.code {
        KeyCode::Char('g') if ctrl => app.toggle_issues(),
        KeyCode::Char('?') if !ctrl => app.help = true,
        KeyCode::Esc => {
            if app.deep.is_some() {
                app.deep = None;
                app.search_gen += 1;
                app.reset_position();
                app.ensure_detail();
            } else {
                return Ok(Flow::Quit);
            }
        }
        KeyCode::Char('f') if ctrl => {
            if search::rg_path().is_some() && !app.q.is_empty() {
                app.search_gen += 1;
                let gen = app.search_gen;
                let term_q = app.q.clone();
                let tx = app.tx.clone();
                app.say(Tone::Info, "searching…".into());
                std::thread::spawn(move || {
                    let files = search::content_search(&term_q, &search::roots());
                    let _ = tx.send(Msg::Search { gen, query: term_q, files });
                });
            }
        }
        KeyCode::Char('a') if ctrl => {
            if let Some(i) = app.selected() {
                let (key, id) = (app.items[i].key.clone(), app.items[i].id.clone());
                app.follow = None; // the selection is about to move out from under the pin
                app.archive.toggle(&key, &id);
                app.rebuild_tabs();
                let len = app.view().len();
                app.cur = app.cur.min(len.saturating_sub(1));
                app.ensure_detail();
            }
        }
        // A bare `r` cannot open the composer: plain letters filter the list, and "r" is the
        // first character of plenty of things worth searching for.
        KeyCode::Char('r') if ctrl => app.begin_reply(),
        KeyCode::Char('t') if ctrl => app.toggle_follow(),
        KeyCode::Char('k') if ctrl => app.kill_key(),
        KeyCode::Tab => app.expand = !app.expand,
        KeyCode::Char('e') if ctrl => app.expand = !app.expand,
        // Each axis matches the shape of the thing it moves: the projects are a column, so they
        // take ↑↓, and the sessions take ←→. This holds whichever layout is on screen — a key
        // that changed meaning when the window got narrow would be worse than a mismatched arrow.
        KeyCode::Up => app.step_project(-1),
        KeyCode::Down => app.step_project(1),
        KeyCode::Left => app.step_session(-1),
        KeyCode::Right => app.step_session(1),
        // ⌥⌫ (which arrives as alt+backspace) and ^w rub out a word; ⌘⌫ sends ^u in every
        // terminal that binds it, and clears the query outright.
        KeyCode::Backspace if k.modifiers.contains(KeyModifiers::ALT) => {
            let kept = drop_word(&app.q);
            app.q.truncate(kept);
            app.requery();
        }
        KeyCode::Char('w') if ctrl => {
            let kept = drop_word(&app.q);
            app.q.truncate(kept);
            app.requery();
        }
        KeyCode::Char('u') if ctrl => {
            app.q.clear();
            app.requery();
        }
        KeyCode::Backspace => {
            app.q.pop();
            app.requery();
        }
        KeyCode::Enter => return resume_selected(term, app, false),
        KeyCode::Char('o') if ctrl => return resume_selected(term, app, true),
        // A fresh `claude` in the highlighted session's folder: ↵'s launch with nothing to
        // resume. `^n` rather than a bare `n` for the reason `^r` is: plain letters filter.
        KeyCode::Char('n') if ctrl => {
            if let Some(i) = app.selected() {
                let (cwd, project) = (app.items[i].cwd.clone(), app.items[i].project.clone());
                // Without a recorded folder there is nowhere to start it; sessio's own folder
                // would be a guess dressed up as the project.
                let Some(dir) = cwd else {
                    app.say(Tone::Warning, "no folder recorded for this session — nowhere to start one".into());
                    return Ok(Flow::Continue);
                };
                if !resume::in_ghostty() {
                    start_over(term, &dir);
                }
                // Stay put and say why on failure, as ^o does: falling back to this window would
                // replace sessio with something the user did not ask for.
                match resume::ghostty_launch_fresh(std::path::Path::new(&dir)) {
                    Ok(()) => app.say(Tone::Success, format!("↗ new session in {project} in a new window")),
                    Err(why) => app.say(Tone::Error, format!("couldn't open a new window ({why})")),
                }
                return Ok(Flow::Continue);
            }
        }
        KeyCode::Char(c) if !ctrl && !k.modifiers.contains(KeyModifiers::ALT) && c >= ' ' => {
            app.q.push(c);
            app.requery();
        }
        _ => {}
    }
    Ok(Flow::Continue)
}

/// Byte length of `q` with its last word removed: trailing separators first, then the word.
///
/// Separators are whitespace and the punctuation that shows up in project paths and session
/// titles, so one ⌥⌫ over `mybit/tooling` leaves `mybit/`.
/// Keys while the reply composer is open. Esc abandons the draft, ↵ sends it, everything else
/// types. Deliberately small: this is a one-line composer, not an editor.
#[cfg(not(target_arch = "wasm32"))]
fn compose_key(app: &mut App, k: KeyEvent, ctrl: bool) -> io::Result<Flow> {
    let Some((id, text)) = app.draft.clone() else { return Ok(Flow::Continue) };
    match k.code {
        KeyCode::Esc => {
            app.draft = None;
            app.say(Tone::Info, "reply discarded".into());
        }
        KeyCode::Enter => {
            let body = text.trim().to_string();
            app.draft = None;
            if body.is_empty() {
                app.say(Tone::Warning, "nothing to send".into());
            } else {
                send_reply(app, &id, &body);
            }
        }
        KeyCode::Backspace if k.modifiers.contains(KeyModifiers::ALT) => {
            let kept = drop_word(&text);
            app.draft = Some((id, text[..kept].to_string()));
        }
        KeyCode::Char('w') if ctrl => {
            let kept = drop_word(&text);
            app.draft = Some((id, text[..kept].to_string()));
        }
        KeyCode::Char('u') if ctrl => app.draft = Some((id, String::new())),
        KeyCode::Backspace => {
            let mut t = text;
            t.pop();
            app.draft = Some((id, t));
        }
        KeyCode::Char(c) if !ctrl && !k.modifiers.contains(KeyModifiers::ALT) && c >= ' ' => {
            let mut t = text;
            t.push(c);
            app.draft = Some((id, t));
        }
        _ => {}
    }
    Ok(Flow::Continue)
}

/// Send one turn to a session without opening it: `claude -p --resume <id>` appends to the same
/// transcript, so the next refresh shows the answer in the list you are still standing in.
///
/// On a worker, always. It takes as long as Claude takes to think, and the TUI still has to
/// redraw, refresh and respond to keys while it does.
#[cfg(not(target_arch = "wasm32"))]
fn send_reply(app: &mut App, id: &str, body: &str) {
    let Some(i) = app.items.iter().position(|it| it.id == id) else { return };
    let (key, cwd) = (app.items[i].key.clone(), app.items[i].cwd.clone());
    app.sending.insert(id.to_string());
    app.say(Tone::Info, format!("⏳ sending to \"{}\" …", first_words(&app.items[i].name, 4)));

    let (tx, id, body) = (app.tx.clone(), id.to_string(), body.to_string());
    std::thread::spawn(move || {
        let mut cmd = std::process::Command::new("claude");
        cmd.arg("-p").arg("--resume").arg(&id).arg(&body);
        if let Some(dir) = cwd.as_deref() {
            cmd.current_dir(dir);
        }
        let out = cmd.output();
        let msg = match out {
            Ok(o) if o.status.success() => Msg::Replied {
                key,
                ok: true,
                text: crate::safety::sanitize(&String::from_utf8_lossy(&o.stdout)),
            },
            Ok(o) => Msg::Replied {
                key,
                ok: false,
                text: crate::safety::sanitize(&String::from_utf8_lossy(&o.stderr)),
            },
            Err(e) => Msg::Replied { key, ok: false, text: e.to_string() },
        };
        let _ = tx.send(msg);
    });
}

/// ↵ in the issues list: open the highlighted issue in the default browser.
#[cfg(not(target_arch = "wasm32"))]
fn open_issue(app: &mut App) {
    let Some(crate::issues::Status::Ready { issues, .. }) = app.issue_status() else { return };
    let Some(issue) = issues.get(app.issue_cur.min(issues.len().saturating_sub(1))) else { return };
    // The URL came off the network. Only ever hand the opener a GitHub page, never a scheme or a
    // path it would act on some other way.
    if !issue.url.starts_with("https://github.com/") {
        app.say(Tone::Warning, format!("#{} has no GitHub URL to open", issue.number));
        return;
    }
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let spawned = std::process::Command::new(opener)
        .arg(&issue.url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawned {
        Ok(mut child) => {
            // Reaped off the input path, so no zombie is left for the rest of the run.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            app.say(Tone::Success, format!("↗ opened #{} in the browser", issue.number));
        }
        Err(e) => app.say(Tone::Error, format!("couldn't open a browser ({e}) · {}", issue.url)),
    }
}

fn drop_word(q: &str) -> usize {
    let sep = |c: char| c.is_whitespace() || matches!(c, '/' | '-' | '_' | '.' | ':' | ',');
    let trimmed = q.trim_end_matches(sep);
    match trimmed.rfind(sep) {
        Some(i) => i + trimmed[i..].chars().next().map_or(1, char::len_utf8),
        None => 0,
    }
}

/// Whether ↵ may try to raise the running session's window.
///
/// Off by default, because raising cannot be made to land. Measured on Ghostty: `AXRaise` alone
/// puts the target at z-position 2 — never 1, since slot 1 is the key window and under Ghostty
/// that is sessio's own, which stays running as a launcher. Adding `set frontmost to true` makes
/// macOS promote whatever it considers the app's main window instead, so each ↵ shuffles the
/// stack and a different unrelated window surfaces: the "it cycles through all the windows"
/// report. Setting `AXMain` first does not change that.
///
/// The guard below is the part that carries the value and is always right, so that is what runs.
/// `SESSIO_FOCUS=1` opts back in for anyone working on making the raise behave.
fn focus_enabled() -> bool {
    std::env::var_os("SESSIO_FOCUS").is_some_and(|v| v != "0" && !v.is_empty())
}

/// What ↵ should do for the highlighted session.
#[derive(Debug, PartialEq, Eq)]
enum EnterAction {
    /// Nothing is attached: resume as usual.
    Resume,
    /// A `claude` is already on this transcript — go to it rather than starting a second one.
    GoToRunning,
    /// The user pressed ↵ again on the session they were warned about. Their call.
    ResumeAnyway,
}

/// Whether a flash set to run until `until` is done by `now`.
fn flash_expired(now: Instant, until: Option<Instant>) -> bool {
    until.is_some_and(|t| now >= t)
}

fn enter_action(is_live: bool, confirmed: bool) -> EnterAction {
    match (is_live, confirmed) {
        (false, _) => EnterAction::Resume,
        (true, false) => EnterAction::GoToRunning,
        (true, true) => EnterAction::ResumeAnyway,
    }
}

/// Where the running process is, for a user who has to find it themselves.
pub fn running_where(live: &crate::live::Live) -> String {
    let mut s = format!("pid {}", live.pid);
    if !live.tty.is_empty() {
        s.push_str(&format!(" · {}", live.tty));
    }
    // What it is doing, when the registry has said. Tells the user whether the window they are
    // being sent to is mid-turn or sitting there wanting something from them — and `waiting` is
    // the one status worth acting on, so it carries its reason.
    if !live.status.is_empty() {
        s.push_str(&format!(" · {}", live.status));
        if !live.waiting_for.is_empty() {
            s.push_str(&format!(" · {}", live.waiting_for));
        }
    }
    s
}

/// `↵` and `^o`: the two keys that start a `claude` on the highlighted session.
#[cfg(not(target_arch = "wasm32"))]
fn is_resume_key(k: &KeyEvent) -> bool {
    k.code == KeyCode::Enter
        || (k.code == KeyCode::Char('o') && k.modifiers.contains(KeyModifiers::CONTROL))
}

/// `↵` resumes in this window; `^o` opens a new Ghostty window and keeps sessio as a launcher.
///
/// Both go through the already-running guard: a second `claude` on a live transcript is the same
/// mistake whichever window it lands in.
#[cfg(not(target_arch = "wasm32"))]
fn resume_selected(
    term: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    new_window: bool,
) -> io::Result<Flow> {
    let Some(i) = app.selected() else { return Ok(Flow::Continue) };
    let (cwd, id, name, source) = (
        app.items[i].cwd.clone(),
        app.items[i].id.clone(),
        app.items[i].name.clone(),
        app.items[i].source,
    );
    let key = if new_window { "^o" } else { "↵" };

    // Only Ghostty can be asked for a window. Say so rather than quietly doing something else.
    if new_window && !resume::in_ghostty() {
        app.say(Tone::Warning, "^o opens a new window under Ghostty only — ↵ resumes here".into());
        return Ok(Flow::Continue);
    }

    // Already running? Resuming would point a second `claude` at the same transcript and both
    // would append to it. Go to the session instead — and if we can't find its window, say where
    // it is and make the duplicate an explicit second press of the same key.
    let running = app.live.get(&id).cloned();
    let confirmed = app.confirm.as_ref() == Some(&(id.clone(), new_window));
    if let EnterAction::GoToRunning = enter_action(running.is_some(), confirmed) {
        let live = running.expect("GoToRunning implies a live process");
        let short: String = name.chars().take(40).collect();
        // Ghostty can name the exact terminal by tty, tab and split included — so going to the
        // session is the default there, not an opt-in. Title matching stays behind SESSIO_FOCUS.
        if resume::in_ghostty() && resume::focus_tty(&live.tty) {
            app.say(Tone::Success, format!("↗ switched to \"{short}\" — already running ({})", running_where(&live)));
        } else if focus_enabled() && resume::focus_window_titled(&name) {
            app.say(Tone::Success, format!("↗ focused \"{short}\" — already running"));
        } else {
            app.confirm = Some((id, new_window));
            app.say(Tone::Warning, format!("already running ({}) — {key} again to open it twice", running_where(&live)));
        }
        return Ok(Flow::Continue);
    }
    app.confirm = None;

    if !new_window {
        hand_over(term, cwd.as_deref(), source, &id);
    }
    // A new window needs a folder to open in; sessio's own would be a guess.
    let Some(dir) = cwd else {
        app.say(Tone::Warning, "no folder recorded for this session — ↵ resumes it here".into());
        return Ok(Flow::Continue);
    };
    let short: String = name.chars().take(40).collect();
    match resume::ghostty_launch(std::path::Path::new(&dir), &resume::resume_argv(source, &id)) {
        Ok(()) => app.say(Tone::Success, format!("↗ opened \"{short}\" in a new window")),
        // Stay put and say why. Falling back to this window would replace sessio with something
        // the user did not ask for.
        Err(why) => app.say(Tone::Error, format!("couldn't open a new window ({why}) — ↵ resumes here")),
    }
    Ok(Flow::Continue)
}

/// Replace sessio with `claude --resume` (or `copilot --resume=`). The terminal is restored
/// *first* — `exec` never returns, so there is no later opportunity to undo raw mode.
#[cfg(not(target_arch = "wasm32"))]
fn hand_over(
    term: &mut Terminal<CrosstermBackend<Stdout>>,
    cwd: Option<&str>,
    source: Source,
    id: &str,
) -> ! {
    restore(term);
    let path = cwd.map(std::path::Path::new);
    let err = resume::resume_in_place(path, &resume::resume_argv(source, id));
    println!(
        "\nCouldn't launch {} ({err}). Run it yourself:\n  {}\n",
        resume::program(source),
        resume::manual_command(path, source, id)
    );
    std::process::exit(1);
}

/// `hand_over` for `^n`: replace sessio with a fresh `claude` in `cwd`.
#[cfg(not(target_arch = "wasm32"))]
fn start_over(term: &mut Terminal<CrosstermBackend<Stdout>>, cwd: &str) -> ! {
    restore(term);
    let path = std::path::Path::new(cwd);
    let err = resume::start_in_place(path);
    println!(
        "\nCouldn't launch claude ({err}). Run it yourself:\n  {}\n",
        resume::manual_start_command(path)
    );
    std::process::exit(1);
}

// ---------- rendering ----------

#[cfg(not(target_arch = "wasm32"))]
fn draw(f: &mut ratatui::Frame, app: &mut App) {
    let area = f.area();
    let lines = frame_lines(app, area.width as usize, area.height as usize);
    f.render_widget(Paragraph::new(lines), area);
}

/// The whole dashboard as lines, given a terminal size. Split out of `draw` so the frame can be
/// built without a `Frame` — the tests render it directly, and so does the wasm build, which has
/// a browser instead of a terminal but the same dashboard to draw.
fn frame_lines(app: &mut App, cols: usize, rows: usize) -> Vec<Line<'static>> {
    if app.help {
        return help_lines();
    }

    let view = app.view();
    if app.cur >= view.len() {
        app.cur = view.len().saturating_sub(1);
    }
    let sel = view.get(app.cur).copied();

    if cols < MIN_COLS || rows < MIN_ROWS {
        return too_small(app, &view, cols, rows);
    }

    // Feedback is carved off the bottom first, across the whole width, so a message never shares
    // a row with the hints or gets clipped to the column beside the panel.
    let feedback = feedback_lines(app, cols);
    let height = rows - feedback.len();

    // The panel is carved off the left next, so everything below measures itself against the
    // width that is actually left rather than the terminal's.
    let side = sidebar(app, cols);
    let cols = cols.saturating_sub(side + SIDE_GAP);

    // The chrome is a fixed height: key bar, query, project context and the one-row session
    // strip. Nothing here is derived from what the highlighted session contains, so the preview
    // under it never moves as you walk the projects or the tabs.
    let chrome = CHROME + usize::from(app.draft.is_some());
    let preview_box = height.saturating_sub(chrome);
    let prev = if let Some(f) = &app.follow {
        follow_preview(app, f, cols, preview_box)
    } else if app.issues {
        issues_list(app, cols, preview_box)
    } else {
        let base = sel.map(|i| preview(app, &app.items[i], cols, 0).len()).unwrap_or(0);
        let reply_max = preview_box.saturating_sub(base).max(1);
        sel.map(|i| preview(app, &app.items[i], cols, reply_max)).unwrap_or_default()
    };

    let mut lines: Vec<Line> = vec![Line::default(); CHROME];
    lines[KEYBAR_ROW] = header(app, cols);
    lines[QUERY_ROW] = query_line(app, view.len());
    lines[CONTEXT_ROW] = context_line(app, sel, cols);
    lines[STRIP_ROW] = session_tabs(app, &view, cols);
    if let Some((_, text)) = &app.draft {
        lines.push(compose_line(text));
    }
    lines.extend(prev);
    // Every row is cut to its region: a wide title, a CJK path or an emoji-laden branch ends in
    // `…` at the region's edge instead of running on past it.
    lines.truncate(height);
    let lines = lines.into_iter().map(|l| clip(l, cols)).collect();

    let mut out = join_side(side_panel(app, side, height), lines, side);
    out.extend(feedback);
    out
}

/// The frame for a window under `MIN_COLS` x `MIN_ROWS`: say so, then what you need to act on —
/// any feedback, the reply being typed, where you are, the highlighted session and the keys that
/// act on it. Every key still does what it does at any other size; only the drawing gives up.
fn too_small(app: &App, view: &[usize], cols: usize, rows: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        format!("window too small (need {MIN_COLS}x{MIN_ROWS})"),
        theme::attention(),
    ))];
    if !app.flash.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("{}{}", app.flash_tone.mark(), sanitize(&app.flash)),
            app.flash_tone.style(),
        )));
    }
    if let Some((_, text)) = &app.draft {
        lines.push(compose_line(text));
    }
    let tab = app.tabs.get(app.p_idx).map_or(ALL_TAB, String::as_str);
    let pos = if view.is_empty() {
        "no sessions".to_string()
    } else {
        format!("{}/{}", app.cur + 1, view.len())
    };
    lines.push(Line::from(vec![
        Span::styled(sanitize(tab), Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(format!(" · {pos}"), dim()),
    ]));
    if let Some(&i) = view.get(app.cur) {
        let it = &app.items[i];
        let mut spans = tab_marks(app, it, None);
        spans.push(Span::styled(sanitize(it.display_name()), theme::accent()));
        lines.push(Line::from(spans));
    }
    let mut keys = String::new();
    if app.live.values().any(crate::live::Live::needs_you) {
        keys.push_str("◆ waiting on you · ");
    }
    keys.push_str("↑↓ ←→ · ↵ resume · ? help · esc quit");
    lines.push(Line::from(Span::styled(keys, dim())));
    lines.truncate(rows);
    lines.into_iter().map(|l| clip(l, cols)).collect()
}

/// The feedback region: the flash in its tone, wrapped onto a second row only when one row cannot
/// hold it. One blank row when there is nothing to say, so the region is always there.
fn feedback_lines(app: &App, cols: usize) -> Vec<Line<'static>> {
    if app.flash.is_empty() {
        return vec![Line::default()];
    }
    let text = format!("{}{}", app.flash_tone.mark(), sanitize(&app.flash));
    // One column of margin, as the panel's rows have.
    wrap_plain(&text, cols.saturating_sub(1), FEEDBACK_MAX)
        .into_iter()
        .map(|l| Line::from(vec![Span::raw(" "), Span::styled(l, app.flash_tone.style())]))
        .collect()
}

/// A row cut to `w` display columns, ending in `…` when anything was lost. Styles are kept; a
/// wide glyph that would straddle the edge is dropped rather than half-drawn.
fn clip(line: Line<'static>, w: usize) -> Line<'static> {
    if spans_width(&line.spans) <= w {
        return line;
    }
    let room = w.saturating_sub(1);
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for sp in line.spans {
        let sw = UnicodeWidthStr::width(sp.content.as_ref());
        if used + sw <= room {
            used += sw;
            out.push(sp);
            continue;
        }
        let cut = fit_width(&sp.content, room - used).trim_end().to_string();
        out.push(Span::styled(cut, sp.style));
        break;
    }
    if w > 0 {
        out.push(Span::styled("…", dim()));
    }
    Line::from(out)
}

/// One hint in the key bar. `p` is how expendable it is: the bar sheds the highest `p` first.
struct Seg {
    p: u8,
    t: &'static str,
    accent: bool,
}

/// Marks the filter row as a search field rather than a blank line. An emoji, like the `⏸` and
/// `🗄` already in the tab bar, so it reads as one at a glance.
const SEARCH_ICON: &str = "🔍";

const SEP: &str = " · ";
const SEP_W: usize = 3;

/// Widest prefix of the bar that fits `cols`, shedding whole hints rather than clipping one
/// mid-word. Separate from `header` so it can be tested without an `App`.
fn fit_segments(segs: &[Seg], cols: usize) -> Vec<&Seg> {
    let mut cut = 5u8;
    loop {
        let keep: Vec<&Seg> = segs.iter().filter(|s| s.p <= cut).collect();
        let w: usize = keep
            .iter()
            .map(|s| UnicodeWidthStr::width(s.t))
            .sum::<usize>()
            + SEP_W * keep.len().saturating_sub(1);
        if w <= cols || cut == 0 {
            return keep;
        }
        cut -= 1;
    }
}


fn header(app: &App, cols: usize) -> Line<'static> {
    let waiting = app.live.values().filter(|l| l.needs_you()).count();
    // The full bar is ~136 columns under Ghostty. Anything narrower would be clipped mid-word by
    // the paragraph, so drop the least essential hints instead. `? help` is p0 — it reveals
    // everything that was dropped — and the resume key is p1 because it is the whole point.
    //
    // The panel takes columns off this bar, enough at some widths to lose `^f`, `^a` and `live`.
    // So it also takes the `↑↓` hint off it: the panel labels that key itself, right above the
    // column it moves through, which is a better place for it than a bar of ten hints.
    let mut segs: Vec<Seg> =
        vec![Seg { p: 3, t: "←→ session", accent: false }, Seg { p: 4, t: "type", accent: false }];
    if search::rg_path().is_some() {
        segs.push(Seg { p: 5, t: "^f search-in-text", accent: false });
    }
    segs.push(if app.tabs.get(app.p_idx).map(String::as_str) == Some(ARCHIVED_TAB) {
        Seg { p: 5, t: "^a unarchive", accent: false }
    } else {
        Seg { p: 5, t: "^a archive", accent: false }
    });
    segs.push(if app.expand {
        Seg { p: 4, t: "⇥ collapse", accent: true }
    } else {
        Seg { p: 4, t: "⇥ expand-reply", accent: false }
    });
    if resume::in_ghostty() {
        segs.push(Seg { p: 1, t: "↵ resume", accent: false });
        segs.push(Seg { p: 2, t: "^o new-window", accent: false });
    } else {
        segs.push(Seg { p: 1, t: "↵ resume", accent: false });
    }
    segs.push(Seg { p: 2, t: "^r reply", accent: false });
    segs.push(if app.follow.is_some() {
        Seg { p: 4, t: "^t unfollow", accent: true }
    } else {
        Seg { p: 5, t: "^t follow", accent: false }
    });
    segs.push(Seg { p: 4, t: "^g issues", accent: false });
    // Only offered when it would do something: most sessions are not running, and of the ones
    // that are, few have sat for two days.
    let stale = app.selected().is_some_and(|i| {
        let it = &app.items[i];
        crate::kill::verdict(app.live.get(&it.id), it.mtime, model::now_ms()).is_stale()
    });
    if stale {
        segs.push(Seg { p: 3, t: "^k end-stale", accent: false });
    }
    // Never shed: a session waiting on you is the most urgent thing the bar can say, so it
    // outranks every hint including `? help`.
    if waiting > 0 {
        segs.push(Seg {
            p: 0,
            t: if waiting == 1 { "◆ 1 waiting on you" } else { "◆ several waiting on you" },
            accent: true,
        });
    }
    segs.push(Seg { p: 0, t: "? help", accent: false });
    segs.push(Seg { p: 2, t: "esc quit", accent: false });
    segs.push(Seg { p: 5, t: "live", accent: true });
    // The issues list has keys of its own, and the dashboard's would be wrong while it is up.
    if app.issues {
        segs = vec![
            Seg { p: 0, t: "↑↓ issue", accent: false },
            Seg { p: 0, t: "↵ open in browser", accent: false },
            Seg { p: 2, t: "←→ session", accent: false },
            Seg { p: 0, t: "^g esc back", accent: true },
        ];
    }

    // Feedback has its own region at the bottom of the frame, so the bar no longer sheds hints to
    // make room for a message, and a message is never clipped to what the hints left over.
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, seg) in fit_segments(&segs, cols).into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(SEP, dim()));
        }
        let style = if seg.accent { theme::accent() } else { dim() };
        spans.push(Span::styled(seg.t, style));
    }
    Line::from(spans)
}

/// The widest the project panel may get, however long the project names are. Past this it is
/// eating the list for names `fit_width` can abbreviate instead.
const SIDE_MAX: usize = 22;
/// The narrowest the panel gets. A name clipped shorter than this is unreadable.
const SIDE_MIN: usize = 10;
/// The rule between the panel and the dashboard: `" │ "`.
const SIDE_GAP: usize = 3;
/// The dashboard width the panel shrinks to protect. Below it the list rows lose their meta and
/// the preview its measure, so the panel gives up its own columns first.
const BODY_MIN: usize = 80;

/// How wide the project panel is. It is always drawn, at every window size.
///
/// There used to be a fallback: below a certain width the projects moved into a horizontal strip
/// above the list. That strip wrapped onto as many rows as the names needed, and the wrap changed
/// whenever the project set did, dragging the whole frame with it. Never bring it back. On a
/// narrow window the panel narrows instead, down to `SIDE_MIN`, and `fit_width` abbreviates the
/// names.
fn sidebar(app: &App, cols: usize) -> usize {
    let widest = app
        .tabs
        .iter()
        .map(|t| UnicodeWidthStr::width(sanitize(t).as_str()))
        .max()
        .unwrap_or(0);
    // The count columns ride on top of the name budget rather than eating into it.
    let counts = count_cols(app, model::now_ms()).map_or(0, |(a, b)| a + b + 2);
    let natural = (widest + 2 + counts).clamp(SIDE_MIN, SIDE_MAX + counts);
    natural.min(cols.saturating_sub(SIDE_GAP + BODY_MIN)).max(SIDE_MIN)
}

/// The project panel: a label, then one row per tab. When there are more projects than rows the
/// list scrolls to keep the selected one on screen, and the label says where you are in it.
fn side_panel(app: &App, w: usize, rows: usize) -> Vec<Line<'static>> {
    let body = rows.saturating_sub(1);
    let n = app.tabs.len();
    // The panel's rows: every tab, with a rule wherever the kind of tab changes, so the
    // collections (everything, open, waiting), the projects and the archive read as three groups
    // rather than one list in which `⌂ everything` looks like a project.
    let entries = panel_entries(&app.tabs);
    let at = entries.iter().position(|e| *e == Some(app.p_idx)).unwrap_or(0);
    let m = entries.len();
    let off = if m <= body { 0 } else { at.saturating_sub(body / 2).min(m - body) };
    // The key bar drops `↑↓ project` while the panel is up, so the panel has to carry it.
    let lead = if w >= 14 { " ↑↓ projects" } else { " projects" };
    let label =
        if m > body { format!("{lead} {}/{n}", app.p_idx + 1) } else { lead.to_string() };

    // Per tab: sessions touched in the last 24h, then all of them, right-aligned in two columns.
    // Dropped whole when the panel is too narrow to keep a readable name beside them.
    let now = model::now_ms();
    let cols = count_cols(app, now).filter(|(a, b)| w >= a + b + 2 + COUNT_NAME_MIN);
    let counts_w = cols.map_or(0, |(a, b)| a + b + 2);
    let head = match cols {
        Some((a, b)) if w > UnicodeWidthStr::width(label.as_str()) + counts_w => {
            let name = fit_width(&label, w - counts_w);
            format!("{name}{:>a$} {:>b$} ", "24h", "all", a = a.max(3), b = b.max(3))
        }
        _ => fit_width(&label, w),
    };
    let mut lines = vec![Line::from(Span::styled(fit_width(&head, w), dim()))];
    for r in 0..body {
        let t = match entries.get(off + r) {
            Some(Some(t)) => *t,
            Some(None) => {
                lines.push(Line::from(Span::styled(
                    format!(" {}", "╌".repeat(w.saturating_sub(2))),
                    dim(),
                )));
                continue;
            }
            None => {
                lines.push(Line::from("")); // hold the column open to the full height
                continue;
            }
        };
        let name = &app.tabs[t];
        let style = if t == app.p_idx { panel_selected() } else { dim() };
        let text = match cols {
            Some((a, b)) => {
                let (day, all) = app.tab_counts(name, now);
                let day = if day == 0 { "·".to_string() } else { day.to_string() };
                format!(
                    "{}{day:>a$} {all:>b$} ",
                    fit_width(&format!(" {}", sanitize(name)), w - counts_w),
                    a = a.max(3),
                    b = b.max(3),
                )
            }
            None => format!(" {}", sanitize(name)),
        };
        lines.push(Line::from(Span::styled(fit_width(&text, w), style)));
    }
    lines
}

/// Which group a panel tab belongs to: the collections, the projects, or the archive.
fn tab_group(tab: &str) -> u8 {
    match tab {
        ALL_TAB | OPEN_TAB | WAITING_TAB => 0,
        ARCHIVED_TAB => 2,
        _ => 1,
    }
}

/// The panel's rows as tab indices, `None` for the rule between two groups.
fn panel_entries(tabs: &[String]) -> Vec<Option<usize>> {
    let mut out = Vec::with_capacity(tabs.len() + 2);
    for (i, t) in tabs.iter().enumerate() {
        if i > 0 && tab_group(&tabs[i - 1]) != tab_group(t) {
            out.push(None);
        }
        out.push(Some(i));
    }
    out
}

/// The name room the panel keeps before it drops the count columns: names come first.
const COUNT_NAME_MIN: usize = 12;

/// A day, for the panel's "touched in the last 24h" column.
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Widths of the two count columns, sized to the largest number in each (never narrower than
/// their "24h" / "all" headings). `None` when there are no tabs.
fn count_cols(app: &App, now: i64) -> Option<(usize, usize)> {
    let digits = |n: usize| n.to_string().len();
    app.tabs
        .iter()
        .map(|t| app.tab_counts(t, now))
        .map(|(d, a)| (digits(d), digits(a)))
        .reduce(|x, y| (x.0.max(y.0), x.1.max(y.1)))
        .map(|(d, a)| (d.max(3), a.max(3)))
}

/// The panel down the left, the dashboard to its right, one rule between them on every row.
fn join_side(
    side: Vec<Line<'static>>,
    body: Vec<Line<'static>>,
    w: usize,
) -> Vec<Line<'static>> {
    (0..side.len().max(body.len()))
        .map(|i| {
            let mut spans = side.get(i).map(|l| l.spans.clone()).unwrap_or_default();
            let pad = w.saturating_sub(spans_width(&spans));
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(" │ ", dim()));
            if let Some(b) = body.get(i) {
                spans.extend(b.spans.clone());
            }
            Line::from(spans)
        })
        .collect()
}

fn query_line(app: &App, matches: usize) -> Line<'static> {
    // Empty, this row read as a stray blank line. The glyph is what says it is a search field
    // even with nothing typed in it — and which of the two searches is running.
    let mut spans = match &app.deep {
        Some(_) => vec![
            Span::styled(SEARCH_ICON, theme::attention()),
            Span::styled(" content", theme::attention()),
        ],
        None => vec![Span::styled(SEARCH_ICON, theme::accent())],
    };
    spans.push(Span::raw(" "));
    spans.push(Span::raw(sanitize(&app.q)));
    spans.push(Span::styled("▏", dim()));
    if app.deep.is_some() {
        let plural = if matches == 1 { "" } else { "es" };
        spans.push(Span::styled(
            format!("  {matches} match{plural}"),
            theme::attention(),
        ));
    }
    Line::from(spans)
}

/// The selected project's context line, above the session strip: its whole name (the panel may
/// have cut it to ten columns), how many sessions it holds, and where they live — or, for a
/// collection, what it collects. The name wins the width, then the counts, then the place, which
/// is cut from the left so the folder's own name survives.
fn context_line(app: &App, sel: Option<usize>, cols: usize) -> Line<'static> {
    let tab = app.tabs.get(app.p_idx).map_or(ALL_TAB, String::as_str);
    let name = sanitize(tab);
    let name_w = UnicodeWidthStr::width(name.as_str());
    let mut spans = vec![Span::styled(name, Style::default().add_modifier(Modifier::BOLD))];

    let (day, all) = app.tab_counts(tab, model::now_ms());
    let counts = format!(
        " · {all} session{} · {day} in 24h",
        if all == 1 { "" } else { "s" }
    );
    let counts_w = UnicodeWidthStr::width(counts.as_str());
    if name_w + counts_w > cols {
        return Line::from(spans);
    }
    spans.push(Span::styled(counts, dim()));

    let place = match tab {
        ALL_TAB => "every project".to_string(),
        OPEN_TAB => "unfinished, in every project".to_string(),
        WAITING_TAB => "waiting on you, in every project".to_string(),
        ARCHIVED_TAB => "hidden with ^a · ^a brings one back".to_string(),
        project => {
            // The folder of the highlighted session when it is this project's, else the first.
            let cwd = sel
                .map(|i| &app.items[i])
                .filter(|it| it.project == project)
                .or_else(|| app.items.iter().find(|it| app.in_tab(tab, it)))
                .and_then(|it| it.cwd.as_deref())
                .unwrap_or("");
            home_short(&sanitize(cwd))
        }
    };
    let room = cols.saturating_sub(name_w + counts_w + SEP_W);
    // A place cut to a handful of columns says nothing; leave it off instead.
    if !place.is_empty() && room >= 8 {
        spans.push(Span::styled(format!("{SEP}{}", fit_left(&place, room)), dim()));
    }
    Line::from(spans)
}

/// A path with the home folder written `~`.
fn home_short(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && path.starts_with(&h) => format!("~{}", &path[h.len()..]),
        _ => path.to_string(),
    }
}

/// The last `w` display columns of `s`, led by `…` when anything was cut: for paths, whose end
/// is the part that names them.
fn fit_left(s: &str, w: usize) -> String {
    if UnicodeWidthStr::width(s) <= w {
        return s.to_string();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 1; // the ellipsis
    for c in s.chars().rev() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + cw > w {
            break;
        }
        kept.push(c);
        used += cw;
    }
    let tail: String = kept.into_iter().rev().collect();
    format!("…{tail}")
}

/// The reply composer: one line, under the tabs and above the session it answers — so the
/// question you are replying to is still on screen while you write it.
fn compose_line(text: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(" ↳ reply ", tab_selected()),
        Span::raw(" "),
        Span::raw(sanitize(text)),
        Span::styled("▏", dim()),
        Span::styled("   ↵ send · esc cancel", dim()),
    ])
}

/// The recency dot: a filled ring means a `claude` process is attached right now, which is a
/// stronger claim than recency â the transcript was written recently vs. the session is *open*.
fn dot_for(app: &App, it: &Item) -> (&'static str, Style) {
    let age = model::now_ms() - it.mtime;
    // A session waiting on you outranks every other thing a dot can say. It is the only state
    // that is *your* move, and the only one that gets worse the longer it goes unseen.
    if app.live.get(&it.id).is_some_and(crate::live::Live::needs_you) {
        ("◆", theme::attention().add_modifier(Modifier::BOLD))
    } else if app.live.contains_key(&it.id) {
        ("◉", theme::running())
    } else if age < ACTIVE_MS {
        ("●", theme::running())
    } else if age < RECENT_MS {
        // Hollow, not just another colour: "just now" and "today" must read apart in monochrome.
        ("○", theme::recent())
    } else {
        (" ", Style::default())
    }
}

/// The first `n` words of a title, for a tab that is not the one being read.
fn first_words(s: &str, n: usize) -> String {
    s.split_whitespace().take(n).collect::<Vec<_>>().join(" ")
}

/// What a tab says: the whole title when it is the one in focus, two words when it is not. So
/// the tab you are reading is legible and the rest are just enough to steer by.
fn tab_label(it: &Item, active: bool) -> String {
    let name = sanitize(it.display_name());
    if active {
        name
    } else {
        first_words(&name, 2)
    }
}

/// The status glyphs a tab carries in front of its title: the recency dot and the pick-up mark.
/// Only a tab with something to say spends a column saying it — reserving both slots on every tab
/// left a gap in front of every title that had nothing to put in it.
///
/// They keep their own colour on the focused tab and take only its background. Painting them in
/// the selection's foreground lost the very distinction they exist to draw: a running session and
/// a stale one became the same white dot the moment you moved onto it.
fn tab_marks(app: &App, it: &Item, sel: Option<Style>) -> Vec<Span<'static>> {
    let on = |st: Style| match sel {
        Some(s) => theme::on_selection(st, s),
        None => st,
    };
    let mut spans = Vec::new();
    let (dot, dot_style) = dot_for(app, it);
    if dot != " " {
        spans.push(Span::styled(dot.to_string(), on(dot_style)));
    }
    if it.open {
        spans.push(Span::styled("▸", on(theme::attention())));
    }
    if let Some(tag) = source_tag(it) {
        spans.push(Span::styled(format!("{tag} "), on(theme::agent_tag())));
    }
    spans
}

/// The small tag a session from another agent carries; Claude's, the default, carry none.
fn source_tag(it: &Item) -> Option<&'static str> {
    (it.source != Source::Claude).then(|| it.source.as_str())
}

/// The sessions as browser tabs: one row, each tab as wide as what it has to say, the strip
/// scrolling around the focused tab rather than wrapping. The dot survives at any width, because
/// whether a session is running is the one fact worth a column even when the title is cut.
fn session_tabs(app: &App, view: &[usize], cols: usize) -> Line<'static> {
    if view.is_empty() {
        return Line::from(Span::styled("  no sessions here", dim()));
    }
    let n = view.len();
    let cur = app.cur.min(n - 1);

    // Where you are in the strip, first, so a scrolled strip still says how far along it you are.
    let pos = format!("{}/{n} ", cur + 1);
    let room = cols.saturating_sub(UnicodeWidthStr::width(pos.as_str()));

    // A cell is dot + label + the rule that gives the tab an edge. One tab may not take the whole
    // strip, however long its title, or there is nothing left to steer by. `cap` bounds the label.
    let marks_w = |slot: usize| spans_width(&tab_marks(app, &app.items[view[slot]], None));
    let cell_in = |slot: usize, cap: usize| -> (String, usize) {
        let label = tab_label(&app.items[view[slot]], slot == cur);
        let label = if UnicodeWidthStr::width(label.as_str()) > cap {
            format!("{}…", fit_width(&label, cap.saturating_sub(1)).trim_end())
        } else {
            label
        };
        // label + the rule that closes the tab + whichever status marks it actually carries.
        let w = UnicodeWidthStr::width(label.as_str()) + 1 + marks_w(slot);
        (label, w)
    };

    // Grow outwards from the focused tab, alternating sides, while the row still has room.
    let overflows = (0..n).map(|i| cell_in(i, TAB_MAX).1).sum::<usize>() > room;
    let budget = room.saturating_sub(if overflows { MARKERS } else { 0 });
    // The focused tab always fits: on a narrow strip its title is cut, never the tab itself.
    let focus_cap = TAB_MAX.min(budget.saturating_sub(marks_w(cur) + 1));
    let cell = |slot: usize| cell_in(slot, if slot == cur { focus_cap } else { TAB_MAX });
    let (mut lo, mut hi, mut used) = (cur, cur, cell(cur).1);
    loop {
        let grew_right = hi + 1 < n && used + cell(hi + 1).1 <= budget;
        if grew_right {
            hi += 1;
            used += cell(hi).1;
        }
        let grew_left = lo > 0 && used + cell(lo - 1).1 <= budget;
        if grew_left {
            lo -= 1;
            used += cell(lo).1;
        }
        if !grew_right && !grew_left {
            break;
        }
    }

    let mut spans: Vec<Span<'static>> = vec![Span::styled(pos, dim())];
    if lo > 0 {
        spans.push(Span::styled(format!("‹{lo} "), dim()));
    }
    for (slot, &idx) in (lo..=hi).zip(&view[lo..=hi]) {
        let (label, _) = cell(slot);
        let it = &app.items[idx];
        let style = if slot == cur { tab_selected() } else { dim() };
        spans.extend(tab_marks(app, it, (slot == cur).then_some(style)));
        spans.push(Span::styled(label, style));
        spans.push(Span::styled("│", dim()));
    }
    if hi + 1 < n {
        spans.push(Span::styled(format!(" +{}›", n - 1 - hi), dim()));
    }
    Line::from(spans)
}


/// Where a gutter-labelled block's text starts. The label sits right-aligned in front of it, so
/// `recap`, `first` and `reply` all line their content up on one left edge and no row is spent on
/// a label alone.
const GUTTER: usize = 12;
/// Prose stops being readable long before a very wide terminal runs out of room, so the measure
/// is bounded — but it is bounded relative to the window rather than pinned to a constant. A flat
/// 90-column cap left ~70 columns of a 181-column window empty and made the preview look broken;
/// a flat "fill it" wraps a 300-column window at 288 and the eye loses its way back to the next
/// line. This tracks the window and stops growing once the line is genuinely too long to scan.
const MEASURE_MAX: usize = 140;

fn preview(app: &App, it: &Item, width: usize, reply_max: usize) -> Vec<Line<'static>> {
    let w = width.max(1);
    let mut lines: Vec<Line> = vec![Line::from(Span::styled("─".repeat(w), dim()))];

    // The title owns its line, with the one fact that is not about the past — whether it is
    // running right now — pushed to the far edge where it cannot be mistaken for metadata.
    let title = sanitize(it.display_name());
    let mut head = Vec::new();
    let mut tag_w = 0;
    if let Some(tag) = source_tag(it) {
        head.push(Span::styled(format!("{tag} "), theme::agent_tag()));
        tag_w = tag.len() + 1;
    }
    head.push(Span::styled(title.clone(), theme::accent()));
    if let Some(live) = app.live.get(&it.id) {
        // A process nobody has touched in two days says so, quietly: it is the one `^k` can end.
        let stale = match crate::kill::verdict(Some(live), it.mtime, model::now_ms()) {
            crate::kill::Verdict::Stale { idle_ms } => {
                format!(" · stale · idle {}d", crate::kill::idle_days(idle_ms))
            }
            _ => String::new(),
        };
        let rest = format!("{stale} · {}", running_where(live));
        let used = tag_w
            + UnicodeWidthStr::width(title.as_str())
            + UnicodeWidthStr::width("◉ running")
            + UnicodeWidthStr::width(rest.as_str());
        if used + 2 <= w {
            head.push(Span::raw(" ".repeat(w - used)));
            head.push(Span::styled("◉ running", theme::running()));
            head.push(Span::styled(rest, dim()));
        }
    }
    lines.push(Line::from(head));

    // Where it is, in one quiet line: the locators lead, and how the title was come by trails,
    // because it is the least actionable thing here. The age lives on the list row already.
    let kind = if it.custom.is_some() {
        "named"
    } else if it.ai.is_some() {
        "auto-named"
    } else {
        "unnamed"
    };
    let prompts = match it.prompt_count() {
        Some(c) => format!(" · {c} prompt{}", if c == 1 { "" } else { "s" }),
        None => String::new(),
    };
    let facts = format!(
        "{}{}{prompts} · {} · {kind}",
        sanitize(&it.project),
        it.branch.as_deref().map(|b| format!(" · {}", sanitize(b))).unwrap_or_default(),
        size_fmt(it.size),
    );
    lines.push(Line::from(vec![Span::raw("   "), Span::styled(facts, dim())]));
    if let Some(t) = it.detail.as_ref().and_then(|d| d.tokens.as_ref()) {
        let tokens = format!("tokens  {}", tokens_fmt(t));
        lines.push(Line::from(vec![Span::raw("   "), Span::styled(tokens, dim())]));
    }
    if let Some(l) = app.issue_status().as_ref().and_then(issues_line) {
        lines.push(l);
    }

    if it.open {
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled("▸ pick up", theme::attention()),
            Span::styled(format!(" · {}", it.open_why().unwrap_or("unfinished")), dim()),
        ]));
    }
    if app.archived(it) {
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled("🗄 archived · hidden from other tabs · ^a to unarchive", dim()),
        ]));
    }
    if let Some(d) = &app.deep {
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(
                format!("✓ contains \"{}\"", sanitize(&d.query)),
                theme::attention(),
            ),
        ]));
    }
    let detail = it.detail.as_ref();

    // The recap is newer, shorter and says whose move it is — prefer it over the compact
    // summary. The full read supersedes the tail read once it lands.
    let recap = detail.and_then(|d| d.recap.as_deref()).or(it.recap.as_deref());
    let recap_ts = detail.and_then(|d| d.recap_ts.as_deref()).or(it.recap_ts.as_deref());

    // One column. This used to split recap against thread, on the theory that you would read one
    // against the other — but there is not enough here to split: the recap caps at six lines and
    // the reply runs to twenty, so the left column sat empty for most of the preview's height
    // while the right one did all the work.
    let prose = prose_width(w);

    let mut recap_block: Vec<Line> = Vec::new();
    if let Some(recap) = recap {
        let body: Vec<Line> = md_lines(recap, prose).into_iter().take(6).map(italic).collect();
        recap_block = gutter_block(
            &format!("recap {}", recap_ts.map(since).unwrap_or_default()),
            theme::voice().add_modifier(Modifier::BOLD),
            body,
        );
    } else if let Some(summary) = detail.and_then(|d| d.summary.as_ref()) {
        let body: Vec<Line> = md_lines(summary, prose).into_iter().take(6).map(italic).collect();
        let ts = detail.and_then(|d| d.summary_ts.as_deref()).map(since).unwrap_or_default();
        recap_block = gutter_block(&format!("summary {ts}"), dim(), body);
    }

    // Three things, and no more: where it got to, what it was for, and where you left off.
    // A full turn-by-turn tail was tried here and read as a wall — you cannot skim eight
    // alternating speakers, and the whole point of a preview is that it is skimmable.
    let mut thread: Vec<Line> = Vec::new();
    if !app.expand {
        thread.extend(gutter_block(
            &format!("first {}", it.first_ts.as_deref().map(since).unwrap_or_default()),
            dim(),
            wrap_plain(it.first.as_deref().unwrap_or(""), prose, 2)
                .into_iter()
                .map(Line::from)
                .collect(),
        ));
        if let Some(d) = detail.filter(|d| d.count > 1) {
            thread.extend(gutter_block(
                &format!("last {}", d.last_ts.as_deref().map(since).unwrap_or_default()),
                dim(),
                wrap_plain(d.last.as_deref().unwrap_or(""), prose, 2)
                    .into_iter()
                    .map(Line::from)
                    .collect(),
            ));
        }
    }
    if detail.is_none() {
        thread.push(Line::from(Span::styled("…", dim()))); // detail still loading
    }
    if let Some(reply) = detail.and_then(|d| d.reply.as_ref()) {
        if reply_max > 0 {
            let rl = md_lines(reply, prose);
            let total = rl.len();
            // The marker comes out of the reply's own rows, so a cut reply still fits its box
            // rather than pushing the marker off the bottom of the preview.
            let shown = if total > reply_max { reply_max.saturating_sub(1).max(1) } else { total };
            let mut body: Vec<Line> = rl.into_iter().take(shown).collect();
            if total > shown {
                body.push(Line::from(Span::styled("… ⇥ for full", dim())));
            }
            let ts = detail.and_then(|d| d.reply_ts.as_deref()).map(since).unwrap_or_default();
            thread.extend(gutter_block(
                &format!("reply {ts}"),
                theme::voice(),
                body,
            ));
        }
    }

    lines.extend(recap_block);
    lines.extend(thread);
    lines
}

/// The preview while `^t` follows a session: what it is, whether it is still running, and the
/// end of its transcript, bottom-anchored so the newest line is always the last one on screen.
fn follow_preview(app: &App, f: &Follow, width: usize, rows: usize) -> Vec<Line<'static>> {
    let w = width.max(1);
    let mut lines: Vec<Line> = vec![Line::from(Span::styled("─".repeat(w), dim()))];

    let name = app.items.iter().find(|it| it.key == f.key).map_or(f.name.as_str(), Item::display_name);
    let title = sanitize(name);
    let (mark, style, rest) = match app.live.get(&f.id).filter(|_| !f.ended) {
        Some(live) => {
            ("◉ following", theme::running(), format!(" · {}", running_where(live)))
        }
        None => ("◌ ended", theme::attention(), " · no longer running".to_string()),
    };
    let mut head = vec![Span::styled(title.clone(), theme::accent())];
    let used = UnicodeWidthStr::width(title.as_str())
        + UnicodeWidthStr::width(mark)
        + UnicodeWidthStr::width(rest.as_str());
    if used + 2 <= w {
        head.push(Span::raw(" ".repeat(w - used)));
        head.push(Span::styled(mark, style));
        head.push(Span::styled(rest, dim()));
    }
    lines.push(Line::from(head));
    lines.push(Line::from(vec![
        Span::raw("   "),
        Span::styled("read-only tail · refreshes every 2s · ^t or any move stops following", dim()),
    ]));

    let prose = prose_width(w);
    let mut body: Vec<Line> = Vec::new();
    match &f.entries {
        None => body.push(Line::from(Span::styled("…", dim()))),
        Some(e) if e.is_empty() => body.push(Line::from(Span::styled(
            "   nothing said near the end of the transcript yet",
            dim(),
        ))),
        Some(entries) => {
            for e in entries {
                body.extend(match e.who {
                    Who::You => gutter_block(
                        "you",
                        theme::attention(),
                        wrap_plain(&e.text, prose, 4).into_iter().map(Line::from).collect(),
                    ),
                    Who::Claude => gutter_block(
                        "claude",
                        theme::voice(),
                        md_lines(&e.text, prose),
                    ),
                    Who::Tool => gutter_block(
                        "tool",
                        dim(),
                        vec![Line::from(Span::styled(
                            fit_width(&e.text, prose).trim_end().to_string(),
                            dim(),
                        ))],
                    ),
                });
            }
        }
    }
    // Bottom-anchored: when it does not all fit, the oldest lines go.
    let room = rows.saturating_sub(lines.len());
    let skip = body.len().saturating_sub(room);
    lines.extend(body.into_iter().skip(skip));
    lines
}

/// The one-line issues summary under a session's facts. `None` for a folder with no GitHub
/// remote: most folders are not repos, and a line saying so on each of them is noise.
fn issues_line(st: &crate::issues::Status) -> Option<Line<'static>> {
    use crate::issues::Status;
    let text = |t: String| Some(Line::from(vec![Span::raw("   "), Span::styled(t, dim())]));
    match st {
        Status::NoRemote => None,
        Status::Loading => text("⚑ loading issues…".into()),
        Status::Failed(why) => text(format!("⚑ issues: {why}")),
        Status::Ready { slug, issues, .. } if issues.is_empty() => {
            text(format!("⚑ no open issues · {slug}"))
        }
        Status::Ready { slug, issues, .. } => Some(Line::from(vec![
            Span::raw("   "),
            Span::styled(
                format!("⚑ {} open issue{}", issue_count(issues.len()), if issues.len() == 1 { "" } else { "s" }),
                theme::attention(),
            ),
            Span::styled(format!(" · {slug} · ^g show"), dim()),
        ])),
    }
}

/// `100+` once the fetch hit its limit, since there may be more than it asked for.
fn issue_count(n: usize) -> String {
    if n >= crate::issues::LIMIT {
        format!("{n}+")
    } else {
        n.to_string()
    }
}

/// `^g`: the preview's place taken by the highlighted folder's open issues, newest-updated
/// first as `gh` returns them, scrolling to keep the highlighted one on screen.
fn issues_list(app: &mut App, width: usize, height: usize) -> Vec<Line<'static>> {
    use crate::issues::Status;
    let w = width.max(1);
    let mut lines: Vec<Line> = vec![Line::from(Span::styled("─".repeat(w), dim()))];
    let (slug, issues, fetched_ms) = match app.issue_status() {
        Some(Status::Ready { slug, issues, fetched_ms }) => (slug, issues, fetched_ms),
        // The session moved under the list to a folder with nothing to show: say so in place,
        // rather than silently closing a view the user opened.
        other => {
            let why = match other {
                Some(Status::Loading) => "⚑ loading issues…".to_string(),
                Some(Status::Failed(why)) => format!("⚑ issues: {why}"),
                _ => "⚑ no GitHub remote for this folder".to_string(),
            };
            lines.push(Line::from(Span::styled(why, dim())));
            return lines;
        }
    };
    if issues.is_empty() {
        lines.push(Line::from(Span::styled(format!("⚑ no open issues · {slug}"), dim())));
        return lines;
    }
    app.issue_cur = app.issue_cur.min(issues.len() - 1);

    let title = format!("⚑ {slug} · {} open", issue_count(issues.len()));
    let right = format!("fetched {} ago", ago(fetched_ms));
    let gap = w.saturating_sub(UnicodeWidthStr::width(title.as_str()) + UnicodeWidthStr::width(right.as_str()));
    let mut head = vec![Span::styled(title, theme::accent())];
    if gap >= 2 {
        head.push(Span::raw(" ".repeat(gap)));
        head.push(Span::styled(right, dim()));
    }
    lines.push(Line::from(head));

    let rows = height.saturating_sub(lines.len()).max(1);
    let off = app.issue_cur.saturating_sub(rows / 2).min(issues.len().saturating_sub(rows));
    let num_w = issues.iter().map(|i| i.number.to_string().len()).max().unwrap_or(1) + 1;
    for (n, issue) in issues.iter().enumerate().skip(off).take(rows) {
        let sel = n == app.issue_cur;
        let label = issue.labels.first().map(|l| fit_width(l, 12).trim_end().to_string()).unwrap_or_default();
        let age = since(&issue.updated);
        // Right edge: label then age, each in a fixed column so the titles line up.
        let tail = format!("  {label:>12}  {age:>4}");
        let lead = format!("{} {:>num_w$}  ", if sel { "▸" } else { " " }, format!("#{}", issue.number));
        let room = w.saturating_sub(UnicodeWidthStr::width(lead.as_str()) + UnicodeWidthStr::width(tail.as_str()));
        let mut t = issue.title.clone();
        if UnicodeWidthStr::width(t.as_str()) > room {
            t = format!("{}…", fit_width(&t, room.saturating_sub(1)).trim_end());
        }
        let t = fit_width(&t, room);
        let style = if sel { tab_selected() } else { Style::default() };
        lines.push(Line::from(vec![
            Span::styled(lead, if sel { style } else { dim() }),
            Span::styled(t, style),
            Span::styled(tail, if sel { style } else { dim() }),
        ]));
    }
    lines
}

/// How wide the preview sets its prose, given the room it has: all of it, up to a measure that
/// is still scannable. Narrow windows fill completely; wide ones fill to `MEASURE_MAX` and give
/// the remainder back as margin rather than running a line the eye cannot track.
fn prose_width(w: usize) -> usize {
    w.saturating_sub(GUTTER).clamp(1, MEASURE_MAX)
}

/// A labelled block: the label right-aligned in the gutter beside the first row, every row after
/// it starting at the same column.
fn gutter_block(label: &str, style: Style, body: Vec<Line<'static>>) -> Vec<Line<'static>> {
    body.into_iter()
        .enumerate()
        .map(|(i, l)| {
            let mut spans = if i == 0 {
                vec![Span::styled(fit_right(label, GUTTER - 2), style), Span::raw("  ")]
            } else {
                vec![Span::raw(" ".repeat(GUTTER))]
            };
            spans.extend(l.spans);
            Line::from(spans)
        })
        .collect()
}

/// Right-align inside `w`, dropping from the left when it does not fit.
fn fit_right(s: &str, w: usize) -> String {
    let used = UnicodeWidthStr::width(s);
    if used >= w {
        return fit_width(s, w).trim_end().to_string();
    }
    format!("{}{}", " ".repeat(w - used), s)
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| UnicodeWidthStr::width(s.content.as_ref())).sum()
}





fn help_lines() -> Vec<Line<'static>> {
    let key = theme::accent();
    let mut v = vec![
        Line::from(vec![
            Span::styled("sessio", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(" — keys", dim()),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled("↑ ↓", key), Span::raw("    switch project")]),
        Line::from(vec![Span::styled("← →", key), Span::raw("    move session selection (→ reveals more)")]),
        Line::from(vec![Span::styled("type", key), Span::raw("   fuzzy-filter by name / project / first prompt")]),
    ];
    if search::rg_path().is_some() {
        v.push(Line::from(vec![
            Span::styled("^f", key),
            Span::raw("     full-text search across all transcripts on disk"),
        ]));
    }
    v.extend([
        Line::from(vec![Span::styled("^w ⌥⌫", key), Span::raw("  delete the last word of the query")]),
        Line::from(vec![Span::styled("^u ⌘⌫", key), Span::raw("  clear the whole query")]),
        Line::from(vec![Span::styled("^a", key), Span::raw("     archive / unarchive (a session you work in again comes back on its own)")]),
        Line::from(vec![Span::styled("^r", key), Span::raw("     reply to the session without opening it — sends one turn and stays in the list (Claude only)")]),
        Line::from(vec![Span::styled("^t", key), Span::raw("     follow a running (◉) session's tail, read-only — any move stops following")]),
        Line::from(vec![Span::styled("⇥ ^e", key), Span::raw("   expand / collapse the reply preview")]),
        Line::from(vec![Span::styled("^g", key), Span::raw("     open GitHub issues for the session's repo (needs gh; ↵ opens one in the browser)")]),
        Line::from(vec![Span::styled("↵", key), Span::raw("      resume in this window (◉ = already running: ↵ goes to its window under Ghostty, else says where; ↵ again opens it twice)")]),
        Line::from(vec![Span::styled("^o", key), Span::raw("     resume in a new Ghostty window, keeping sessio open (same guard as ↵)")]),
        Line::from(vec![Span::styled("^n", key), Span::raw("     new session in the selected session's folder (new window under Ghostty)")]),
        Line::from(vec![Span::styled("^k", key), Span::raw("     end a running session idle for more than 48h (not busy, not waiting) — ^k again to confirm")]),
        Line::from(vec![Span::styled("?", key), Span::raw("      toggle this help")]),
        Line::from(vec![Span::styled("esc", key), Span::raw("    clear search, then quit")]),
        Line::from(vec![Span::styled("^c", key), Span::raw("     quit")]),
        Line::from(""),
        Line::from(vec![
            Span::styled("◉", theme::running()),
            Span::raw("      a claude process is attached to this session right now"),
        ]),
        Line::from(vec![
            Span::styled("●", theme::running()),
            Span::raw("      written in the last 5 minutes ("),
            Span::styled("○", theme::recent()),
            Span::raw(" in the last 24h)"),
        ]),
        Line::from(""),
        Line::from(Span::styled("press any key to close", dim())),
    ]);
    v
}

// ---------- formatting helpers ----------

pub fn ago(ms: i64) -> String {
    let s = (model::now_ms() - ms) as f64 / 1000.0;
    if s < 3600.0 {
        format!("{}m", ((s / 60.0).round() as i64).max(1))
    } else if s < 86400.0 {
        format!("{}h", (s / 3600.0).round() as i64)
    } else {
        format!("{}d", (s / 86400.0).round() as i64)
    }
}

/// `in 10.8k · out 5.6M · cache w 31.6M · r 727M`: the session's token totals, humanized.
pub fn tokens_fmt(t: &crate::parse::Usage) -> String {
    format!(
        "in {} · out {} · cache w {} · r {}",
        count_fmt(t.input),
        count_fmt(t.output),
        count_fmt(t.cache_write),
        count_fmt(t.cache_read),
    )
}

/// A count in k/M/B: one decimal below 100 of a unit, whole numbers above.
fn count_fmt(n: u64) -> String {
    let (v, unit) = match n {
        0..=999 => return n.to_string(),
        1_000..=999_999 => (n as f64 / 1e3, "k"),
        1_000_000..=999_999_999 => (n as f64 / 1e6, "M"),
        _ => (n as f64 / 1e9, "B"),
    };
    if v < 99.95 {
        format!("{v:.1}{unit}")
    } else {
        format!("{}{unit}", v.round() as u64)
    }
}

fn size_fmt(b: u64) -> String {
    if b < 1024 {
        format!("{b}B")
    } else if b < 1_048_576 {
        format!("{}K", (b as f64 / 1024.0).round() as u64)
    } else {
        format!("{:.1}M", b as f64 / 1_048_576.0)
    }
}


/// How long ago an ISO timestamp was, in the units the list uses: `22m`, `3h`, `2d`.
///
/// One time format for the whole preview. It used to carry three — `1m ago` in the head,
/// `20 Aug 18:07 [22m]` on the recap, `20 Aug 17:02 [1h]` on the prompts — all dim, all competing,
/// all saying the same kind of thing.
fn since(iso: &str) -> String {
    crate::parse::parse_iso_ms(iso).map(ago).unwrap_or_default()
}

/// A recap body line: italic, the way Claude Code prints one. The gutter supplies the indent.
fn italic(l: Line<'static>) -> Line<'static> {
    Line::from(
        l.spans
            .into_iter()
            .map(|sp| Span::styled(sp.content, sp.style.add_modifier(Modifier::ITALIC)))
            .collect::<Vec<_>>(),
    )
}

fn fit_width(s: &str, w: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + cw > w {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push_str(&" ".repeat(w.saturating_sub(used)));
    out
}

/// Plain-text word wrap with an ellipsis when it overflows `max_lines`.
/// Port of `wrap()` at bin/sessio.mjs:197.
fn wrap_plain(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    // A budget of zero still buys one line: the tail below has always guaranteed one, and the
    // call sites render whatever comes back. It also keeps `max_lines - 1` in range.
    let max_lines = max_lines.max(1);
    let clean = sanitize(text);
    let words: Vec<&str> = clean.split_whitespace().collect();
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    // Whether anything was left behind, tracked as it happens. Inferring it afterwards by
    // comparing the input against the joined output cannot be made exact: a hard-split word makes
    // the output longer than the text it came from, in bytes and in display width alike.
    let mut cleared = false;
    let mut consumed = 0;
    for (i, w) in words.iter().enumerate() {
        consumed = i + 1;
        let cand = if cur.is_empty() { w.to_string() } else { format!("{cur} {w}") };
        if UnicodeWidthStr::width(cand.as_str()) > width {
            // Only break a line that has something on it — an empty `cur` here means the word
            // alone overruns the measure, and pushing it would spend a line on a blank.
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            cur = w.to_string();
            // A word with no break in it — a URL, a path, a base64 blob — has nowhere to wrap,
            // so cut it at the measure. Whole, it reaches `Line::from` unclipped and spills
            // across the label gutter.
            while lines.len() < max_lines && UnicodeWidthStr::width(cur.as_str()) > width {
                let head = fit_width(&cur, width).trim_end().to_string();
                if head.is_empty() {
                    cur.clear(); // the measure holds no glyph of it at all
                    cleared = true;
                    break;
                }
                cur = cur[head.len()..].to_string();
                lines.push(head);
            }
        } else {
            cur = cand;
        }
        if lines.len() >= max_lines {
            break;
        }
    }
    if !cur.is_empty() && lines.len() < max_lines {
        lines.push(std::mem::take(&mut cur));
    }
    // `cur` still holding text here means the budget ran out before it could be pushed. The
    // width guard is for the zero-width measure, which has no room for the ellipsis itself.
    let dropped = cleared || !cur.is_empty() || consumed < words.len();
    if width > 0 && lines.len() == max_lines && dropped {
        let last = lines[max_lines - 1].clone();
        lines[max_lines - 1] = format!("{}…", fit_width(&last, width.saturating_sub(1)).trim_end());
    }
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_counts_are_humanized() {
        assert_eq!(count_fmt(0), "0");
        assert_eq!(count_fmt(999), "999");
        assert_eq!(count_fmt(10_800), "10.8k");
        assert_eq!(count_fmt(5_600_000), "5.6M");
        assert_eq!(count_fmt(727_000_000), "727M");
        assert_eq!(count_fmt(1_200_000_000), "1.2B");
        let t = crate::parse::Usage {
            input: 10_800,
            output: 5_600_000,
            cache_write: 31_600_000,
            cache_read: 727_000_000,
        };
        assert_eq!(tokens_fmt(&t), "in 10.8k · out 5.6M · cache w 31.6M · r 727M");
    }

    #[test]
    fn size_formatting_matches_the_js() {
        assert_eq!(size_fmt(512), "512B");
        assert_eq!(size_fmt(2048), "2K");
        assert_eq!(size_fmt(3_145_728), "3.0M");
    }

    #[test]
    fn wrap_plain_respects_width_and_line_cap() {
        let out = wrap_plain("alpha beta gamma delta epsilon zeta", 11, 2);
        assert_eq!(out.len(), 2);
        for l in &out {
            assert!(UnicodeWidthStr::width(l.as_str()) <= 11, "{l:?}");
        }
        assert!(out[1].ends_with('…'), "overflow must be marked: {:?}", out[1]);
    }

    #[test]
    fn wrap_plain_of_empty_text_is_one_blank_line() {
        assert_eq!(wrap_plain("", 20, 2), vec![String::new()]);
    }

    /// #5: a prompt that is one unbroken token — a URL, a path, a base64 blob. It used to come
    /// back as a blank line plus the whole 78-column token, which spills across the gutter.
    #[test]
    fn wrap_plain_hard_splits_a_lone_oversized_token() {
        let url = "https://github.com/theanhgen/sessio/blob/main/src/ui.rs#L1188-verylongfragment";
        let out = wrap_plain(url, 10, 2);
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(!out[0].is_empty(), "no line may be spent on a blank: {out:?}");
        for l in &out {
            assert!(UnicodeWidthStr::width(l.as_str()) <= 10, "{l:?}");
        }
        assert!(out[1].ends_with('…'), "overflow must be marked: {:?}", out[1]);
    }

    /// The shape that hid #5: with a word after it, the ellipsis path already fired.
    #[test]
    fn wrap_plain_still_clips_an_oversized_token_between_words() {
        let out = wrap_plain("short averyveryverylongtokenindeed tail", 10, 2);
        assert_eq!(out, vec!["short", "averyvery…"]);
    }

    /// #6: `lines[max_lines - 1]` underflowed. The tail has always guaranteed a line, so a
    /// budget of zero still yields one.
    #[test]
    fn wrap_plain_with_no_line_budget_yields_one_line() {
        let out = wrap_plain("anything at all", 10, 0);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(UnicodeWidthStr::width(out[0].as_str()) <= 10, "{out:?}");
    }

    #[test]
    fn wrap_plain_splits_wide_glyphs_on_display_width() {
        for text in ["日本語日本語日本語日本語", "🎉🎉🎉🎉🎉🎉🎉🎉"] {
            let out = wrap_plain(text, 7, 2);
            assert!(out.len() <= 2, "{out:?}");
            for l in &out {
                assert!(UnicodeWidthStr::width(l.as_str()) <= 7, "{l:?}");
            }
        }
    }

    /// Hard-splitting a word inserts separators the original never had, so the joined output can
    /// be as wide as the text it came from while still having lost the tail. Comparing the two —
    /// in bytes or in display width — cannot see that; only tracking the loss can.
    #[test]
    fn wrap_plain_marks_a_loss_the_joined_width_cannot_see() {
        // "abc def ghi" is exactly as wide as "abcdefghijk", yet "jk" is gone.
        assert_eq!(wrap_plain("abcdefghijk", 3, 3), vec!["abc", "def", "gh…"]);
    }

    #[test]
    fn wrap_plain_at_zero_width_is_one_blank_line() {
        assert_eq!(wrap_plain("anything at all", 0, 2), vec![String::new()]);
    }

    #[test]
    fn fit_width_pads_and_truncates_by_display_width() {
        assert_eq!(fit_width("ab", 4), "ab  ");
        assert_eq!(UnicodeWidthStr::width(fit_width("日本語", 4).as_str()), 4);
    }

    #[test]
    fn ago_buckets() {
        let now = model::now_ms();
        assert!(ago(now).ends_with('m'));
        assert!(ago(now - 7_200_000).ends_with('h'));
        assert!(ago(now - 3 * 86_400_000).ends_with('d'));
    }

    fn bar() -> Vec<Seg> {
        vec![
            Seg { p: 3, t: "↑↓ project", accent: false },
            Seg { p: 3, t: "←→ session", accent: false },
            Seg { p: 4, t: "type", accent: false },
            Seg { p: 5, t: "^f search-in-text", accent: false },
            Seg { p: 5, t: "^a archive", accent: false },
            Seg { p: 4, t: "⇥ expand-reply", accent: false },
            Seg { p: 1, t: "↵ resume", accent: false },
            Seg { p: 2, t: "^o new-window", accent: false },
            Seg { p: 0, t: "? help", accent: false },
            Seg { p: 2, t: "esc quit", accent: false },
            Seg { p: 5, t: "live", accent: true },
        ]
    }

    fn rendered(cols: usize) -> String {
        fit_segments(&bar(), cols)
            .into_iter()
            .map(|s| s.t)
            .collect::<Vec<_>>()
            .join(SEP)
    }

    /// The bug this replaces: the bar was emitted whole, wrapped on any window narrower than
    /// ~136 columns, and pushed the frame past the terminal height.
    /// A dashboard with `tabs` projects, enough sessions to fill the list, and nothing lazily
    /// loaded â everything the panel tests need and nothing that touches the disk.
    fn fixture(tabs: &[&str]) -> App {
        let item = |n: usize, project: &str| Item {
            id: format!("id{n}"),
            key: format!("key{n}"),
            file: PathBuf::from("/dev/null"),
            mtime: model::now_ms() - (n as i64 * 60_000),
            size: 1024,
            dir: project.into(),
            source: Source::Claude,
            first: Some("first prompt".into()),
            first_ts: None,
            cwd: Some(format!("/Users/x/{project}")),
            branch: Some("main".into()),
            custom: None,
            ai: None,
            open: n == 1,
            open_reason: None,
            recap: Some("Weighing a sidebar for the project tabs.".into()),
            recap_ts: None,
            title: None,
            name: format!("session {n} in {project}"),
            project: project.into(),
            hay: format!("session {n} in {project} {project}"),
            detail: None,
        };
        let projects: Vec<&str> =
            tabs.iter().copied().filter(|t| ![ALL_TAB, OPEN_TAB, WAITING_TAB].contains(t)).collect();
        // Always the same number of sessions, dealt round-robin across whatever projects there
        // are: otherwise a test comparing list height across tab sets compares item counts.
        let items = (0..12)
            .map(|n| item(n + 1, projects[n % projects.len().max(1)]))
            .collect();
        let (tx, _rx) = mpsc::channel();
        App {
            items,
            archive: Archive::default(),
            tabs: tabs.iter().map(|t| t.to_string()).collect(),
            q: String::new(),
            cur: 0,
            p_idx: 0,
            expand: false,
            help: false,
            flash: String::new(),
            flash_tone: Tone::Info,
            flash_until: None,
            deep: None,
            search_gen: 0,
            details: HashMap::new(),
            detail_inflight: HashSet::new(),
            live: crate::live::LiveMap::new(),
            confirm: None,
            kill_confirm: None,
            draft: None,
            sending: HashSet::new(),
            reply_ok: false,
            follow: None,
            issues: false,
            issue_cur: 0,
            tx,
        }
    }

    /// One more session in the same project, for the strip tests.
    fn nth_item(n: usize) -> Item {
        let mut it = fixture(&[ALL_TAB, "sessio"]).items.remove(0);
        it.id = format!("id{n}");
        it.key = format!("key{n}");
        it.name = format!("Session number {n} with a fairly long title");
        it.hay = it.name.clone();
        it.mtime = model::now_ms() - (n as i64 * 60_000);
        it
    }

    fn real_tabs() -> Vec<&'static str> {
        vec![
            ALL_TAB, OPEN_TAB, "lifelab", "sessio", "02-personal", "saint-gobain",
            "marketplaces", "00-agent-plugins", "mybit", "zigani", "apify-lab", "Desktop",
        ]
    }

    /// Render a whole frame and read it back as rows of text.
    fn frame(app: &mut App, cols: u16, rows: u16) -> Vec<String> {
        let mut term =
            Terminal::new(ratatui::backend::TestBackend::new(cols, rows)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..rows)
            .map(|y| {
                (0..cols).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>()
            })
            .collect()
    }

    /// The panel's tab rows of a rendered frame: under its label, above the feedback row, without
    /// the rules between groups or the blank rows that hold the column open.
    fn panel_rows(rows: &[String]) -> Vec<String> {
        rows[1..rows.len() - 1]
            .iter()
            .map(|r| r.split(" │ ").next().unwrap_or("").to_string())
            .filter(|p| !p.trim().is_empty() && !p.contains('╌'))
            .collect()
    }

    #[test]
    fn a_wide_terminal_puts_the_projects_down_the_side() {
        let mut app = fixture(&real_tabs());
        let rows = frame(&mut app, 136, 26);
        assert!(rows[0].starts_with(" ↑↓ projects"), "panel is labelled: {:?}", rows[0]);
        // One project per row, in order, down the left edge, past the rule between the groups.
        let listed = panel_rows(&rows);
        for (i, name) in real_tabs().iter().enumerate() {
            assert!(listed[i].trim_start().starts_with(name), "row {i} should hold {name}: {listed:?}");
        }
        // And the tabs are no longer spent on a row of their own up top.
        assert!(rows[0].contains("? help"), "key bar sits beside the panel: {:?}", rows[0]);
    }

    /// The whole point of the panel: chrome stops growing with the tab set.
    #[test]
    fn the_panel_costs_the_same_chrome_however_many_projects_there_are() {
        // Where the preview's rule lands is exactly header + tabs + query + list + the more-row.
        let listed = |tabs: &[&str]| {
            let mut app = fixture(tabs);
            frame(&mut app, 136, 26)
                .iter()
                .position(|r| r.contains("────"))
                .expect("the preview draws a rule")
        };
        let few = listed(&[ALL_TAB, "sessio", "lifelab"]);
        let many = listed(&real_tabs());
        assert_eq!(few, many, "the session list keeps its rows as projects pile up");
    }

    /// The panel is the layout at every width. A narrow window used to swap it for a strip of
    /// projects above the list that wrapped onto several rows; that must never come back.
    #[test]
    fn a_narrow_terminal_keeps_the_panel() {
        for cols in [MIN_COLS as u16, 60, 80, 100] {
            let mut app = fixture(&real_tabs());
            let rows = frame(&mut app, cols, 26);
            assert!(rows[0].starts_with(" "), "{cols} cols: panel label first: {:?}", rows[0]);
            assert!(rows[0].contains("projects"), "{cols} cols: panel is labelled: {:?}", rows[0]);
            let listed = panel_rows(&rows);
            for (i, name) in real_tabs().iter().enumerate() {
                let head: String = name.chars().take(3).collect();
                assert!(
                    listed[i].trim_start().starts_with(&head),
                    "{cols} cols: row {i} should hold {name}: {listed:?}",
                );
            }
        }
    }

    /// On a narrow window the panel gives up its columns before the dashboard beside it does.
    #[test]
    fn the_panel_shrinks_before_the_dashboard_does() {
        let app = fixture(&real_tabs());
        let natural = sidebar(&app, 300);
        let counts = count_cols(&app, model::now_ms()).map_or(0, |(a, b)| a + b + 2);
        for cols in 0..300 {
            let w = sidebar(&app, cols);
            assert!((SIDE_MIN..=SIDE_MAX + counts).contains(&w), "{cols} cols gave a {w}-wide panel");
            if cols >= natural + SIDE_GAP + BODY_MIN {
                assert_eq!(w, natural, "{cols} cols can afford the full panel");
            } else if cols >= SIDE_MIN + SIDE_GAP + BODY_MIN {
                assert_eq!(cols - w - SIDE_GAP, BODY_MIN, "{cols} cols: the panel shrinks first");
            } else {
                assert_eq!(w, SIDE_MIN, "{cols} cols: as narrow as the panel goes");
            }
        }
    }

    #[test]
    fn the_panel_counts_the_last_day_then_everything_right_aligned() {
        let mut app = fixture(&real_tabs());
        let now = model::now_ms();
        for (n, it) in app.items.iter_mut().enumerate() {
            // Two fresh sessions, the rest a week old.
            it.mtime = if n < 2 { now - 60_000 } else { now - 7 * DAY_MS };
        }
        let all = app.items.len();
        assert_eq!(app.tab_counts(ALL_TAB, now), (2, all));

        let w = sidebar(&app, 300);
        let panel = side_panel(&app, w, 30);
        let text = |l: &Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
        assert!(text(&panel[0]).trim_end().ends_with("24h all"), "{:?}", text(&panel[0]));
        let everything = text(&panel[1]);
        assert!(everything.trim_end().ends_with(&format!("2 {all:>3}")), "{everything:?}");
        // Every count row ends at the same column.
        let ends: Vec<usize> = panel[1..=app.tabs.len()]
            .iter()
            .map(|l| UnicodeWidthStr::width(text(l).trim_end()))
            .collect();
        assert!(ends.windows(2).all(|p| p[0] == p[1]), "misaligned: {ends:?}");

        // Too narrow for names and numbers both: the numbers go, the names stay.
        let narrow = side_panel(&app, SIDE_MIN, 30);
        assert!(!text(&narrow[0]).contains("24h"));
    }

    #[test]
    fn no_row_overflows_the_terminal() {
        for cols in [30u16, 40, 60, 80, 100, 103, 120, 136, 200] {
            let mut app = fixture(&real_tabs());
            for (y, row) in frame(&mut app, cols, 26).iter().enumerate() {
                let w = UnicodeWidthStr::width(row.trim_end());
                assert!(w <= cols as usize, "{cols} cols, row {y} rendered {w}: {row:?}");
            }
        }
    }

    #[test]
    fn the_selected_project_stays_on_screen_when_the_panel_scrolls() {
        let many: Vec<String> = (0..40).map(|i| format!("project-{i:02}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let mut app = fixture(&refs);
        for (p_idx, name) in refs.iter().enumerate() {
            app.p_idx = p_idx;
            let panel = side_panel(&app, 18, 12);
            assert!(
                panel.iter().any(|l| l.spans.iter().any(|s| s.content.contains(name))),
                "project {p_idx} fell off the panel"
            );
        }
    }

    /// The arrows match the shape of what they move, and a narrow window does not change that.
    #[test]
    fn up_down_walks_the_projects_and_left_right_the_sessions() {
        let mut app = fixture(&real_tabs());
        let sessions = app.view().len();
        assert!(sessions > 2 && app.tabs.len() > 2, "fixture needs both to move through");

        app.step_project(1);
        assert_eq!(app.p_idx, 1, "↓ goes to the next project");
        assert_eq!(app.cur, 0, "and starts that project at its first session");
        app.step_project(-1);
        assert_eq!(app.p_idx, 0, "↑ goes back");
        app.step_project(-1);
        assert_eq!(app.p_idx, app.tabs.len() - 1, "and wraps around the end");

        app.p_idx = 0;
        app.reset_position();
        app.step_session(1);
        assert_eq!(app.cur, 1, "→ goes to the next session");
        assert_eq!(app.p_idx, 0, "and leaves the project alone");
        app.step_session(-1);
        assert_eq!(app.cur, 0, "← goes back");
        app.step_session(-1);
        assert_eq!(app.cur, 0, "and stops at the first rather than wrapping");
    }

    /// Browser rule one: the strip is one row, always. It must never wrap, however many tabs
    /// are open — everything below it would move.
    #[test]
    fn the_tab_strip_is_always_exactly_one_row() {
        for n in [1usize, 2, 8, 40, 200] {
            let mut app = fixture(&real_tabs());
            app.items = (0..n).map(nth_item).collect();
            app.tabs = vec![ALL_TAB.to_string()];
            app.p_idx = 0;
            for cols in [60usize, 100, 181] {
                let line = session_tabs(&app, &app.view(), cols);
                let w = spans_width(&line.spans);
                assert!(w <= cols, "{n} tabs at {cols} cols rendered {w}");
            }
        }
    }

    /// Browser rule two: the active tab is always on screen, and the strip says how many are
    /// hidden off each end.
    #[test]
    fn the_active_tab_is_always_visible() {
        let mut app = fixture(&real_tabs());
        app.items = (0..40).map(nth_item).collect();
        app.tabs = vec![ALL_TAB.to_string()];
        app.p_idx = 0;
        let view = app.view();
        for cur in 0..view.len() {
            app.cur = cur;
            let line = session_tabs(&app, &view, 120);
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let name = app.items[view[cur]].display_name();
            let head: String = name.chars().take(8).collect();
            assert!(text.contains(&head), "tab {cur} ({head}) fell off the strip: {text:?}");
        }
    }

    /// The focused tab says everything; the rest say just enough to steer by.
    #[test]
    fn the_focused_tab_is_full_width_and_the_rest_are_two_words() {
        let mut app = fixture(&real_tabs());
        app.items = (0..6).map(nth_item).collect();
        app.tabs = vec![ALL_TAB.to_string()];
        app.p_idx = 0;
        app.cur = 2;
        let view = app.view();
        let full = app.items[view[2]].display_name().to_string();
        let text: String =
            session_tabs(&app, &view, 181).spans.iter().map(|s| s.content.as_ref()).collect();

        assert!(text.contains(&full), "the focused tab carries its whole title: {text:?}");
        // Its neighbour is the same title, cut to two words.
        let neighbour = app.items[view[3]].display_name().to_string();
        let two = first_words(&neighbour, 2);
        assert!(text.contains(&two), "an idle tab keeps two words: {text:?}");
        assert!(!text.contains(&neighbour), "but not the whole title: {text:?}");
    }

    /// Moving focus re-sizes both tabs, and costs nothing below the strip.
    #[test]
    fn moving_focus_moves_which_tab_is_wide() {
        let mut app = fixture(&real_tabs());
        app.items = (0..6).map(nth_item).collect();
        app.tabs = vec![ALL_TAB.to_string()];
        app.p_idx = 0;
        let view = app.view();
        let strip = |app: &App| -> String {
            session_tabs(app, &view, 181).spans.iter().map(|s| s.content.as_ref()).collect()
        };
        app.cur = 0;
        let first = strip(&app);
        app.cur = 1;
        let second = strip(&app);
        assert_ne!(first, second, "the wide tab follows the focus");
        assert!(second.contains(app.items[view[1]].display_name()), "tab 1 opened up");
        assert!(!second.contains(app.items[view[0]].display_name()), "tab 0 closed down");
    }

    /// The row that read as a stray blank line has to announce itself even when empty.
    #[test]
    fn the_search_row_shows_its_icon_when_empty() {
        let mut app = fixture(&real_tabs());
        let idle = frame(&mut app, 136, 26);
        assert!(
            idle.iter().any(|r| r.contains(SEARCH_ICON)),
            "an empty search row still says it is a search row"
        );

        app.q = "sess".into();
        let typed = frame(&mut app, 136, 26);
        let row = typed.iter().find(|r| r.contains(SEARCH_ICON)).expect("the row survives typing");
        assert!(row.contains("sess"), "and carries the query: {row:?}");

        // It costs the same row either way, so nothing below it moves as you type.
        let rule = |rows: &[String]| rows.iter().position(|r| r.contains("────"));
        assert_eq!(rule(&idle), rule(&typed), "typing must not shift the frame");
    }

    /// The dot means something — running, active, stale — and the focused tab must not repaint
    /// that meaning away.
    #[test]
    fn the_focused_tab_keeps_its_status_colours() {
        let mut app = fixture(&real_tabs());
        app.items = (0..3).map(nth_item).collect();
        app.tabs = vec![ALL_TAB.to_string()];
        app.p_idx = 0;
        // An old session and a live one, so there are two different dots to tell apart.
        app.items[1].mtime = model::now_ms() - 40 * 60 * 60 * 1000;
        app.items[2].mtime = model::now_ms() - 60 * 60 * 1000; // orange: recent, not active
        let view = app.view();

        let dot_style = |cur: usize, slot: usize| {
            let mut a = fixture(&real_tabs());
            a.items = app.items.clone();
            a.tabs = app.tabs.clone();
            a.cur = cur;
            tab_marks(&a, &a.items[view[slot]], (cur == slot).then_some(tab_selected()))
                .first()
                .map(|s| s.style)
        };

        let idle = dot_style(0, 2).expect("a recent session has a dot");
        let focused = dot_style(2, 2).expect("and keeps it when focused");
        assert_eq!(focused.fg, idle.fg, "the dot keeps its own colour when focused");
        assert_eq!(focused.bg, Some(theme::TAB_SEL), "it only takes the selection's background");
        assert_ne!(idle.fg, Some(theme::ON_SEL), "and was never the selection's foreground");
    }

    /// Two selections are on screen at once, so they must not look the same — and neither may be
    /// a plain reverse of the terminal's own colours.
    #[test]
    fn the_panel_and_the_tab_strip_highlight_differently() {
        let panel = panel_selected();
        let tab = tab_selected();
        assert_ne!(panel.bg, tab.bg, "the two selections need different backgrounds");
        assert!(panel.bg.is_some() && tab.bg.is_some(), "both are explicit colours");
        assert!(
            !panel.add_modifier.contains(Modifier::REVERSED)
                && !tab.add_modifier.contains(Modifier::REVERSED),
            "neither reverses the terminal's colours any more"
        );
    }

    /// The composer owns the keyboard while it is open, and gives it back on esc.
    #[test]
    fn the_composer_takes_every_key_until_it_is_dismissed() {
        let mut app = fixture(&real_tabs());
        let before = app.q.clone();
        app.draft = Some(("id1".into(), String::new()));

        let press = |app: &mut App, c: char| {
            let _ = compose_key(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE), false);
        };
        for c in "yes go".chars() {
            press(&mut app, c);
        }
        assert_eq!(app.draft.as_ref().unwrap().1, "yes go", "keys land in the draft");
        assert_eq!(app.q, before, "and not in the filter");

        let _ = compose_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), false);
        assert!(app.draft.is_none(), "esc abandons it");
    }

    /// It must not offer to send into a session someone is already typing at.
    #[test]
    fn a_running_session_cannot_be_replied_to() {
        let mut app = fixture(&real_tabs());
        app.reply_ok = true;
        let id = app.items[0].id.clone();
        app.live.insert(
            id.clone(),
            crate::live::Live {
                pid: 4674,
                tty: "ttys000".into(),
                status: "busy".into(),
                waiting_for: String::new(),
            },
        );
        app.cur = app.view().iter().position(|&i| app.items[i].id == id).unwrap();

        app.begin_reply();
        assert!(app.draft.is_none(), "no composer opens for a running session");
        assert!(app.flash.contains("running"), "and it says why: {:?}", app.flash);
    }

    /// The fixture with its first session marked running and selected.
    fn running_fixture() -> App {
        let mut app = fixture(&real_tabs());
        let id = app.items[0].id.clone();
        app.live.insert(
            id.clone(),
            crate::live::Live {
                pid: 4674,
                tty: "ttys000".into(),
                status: "busy".into(),
                waiting_for: String::new(),
            },
        );
        app.cur = app.view().iter().position(|&i| app.items[i].id == id).unwrap();
        app
    }

    #[test]
    fn follow_needs_a_running_session() {
        let mut app = fixture(&real_tabs());
        app.toggle_follow();
        assert!(app.follow.is_none(), "nothing to follow in a session nobody is running");
        assert!(app.flash.contains("not running"), "and it says so: {:?}", app.flash);
    }

    #[test]
    fn ctrl_t_pins_and_ctrl_t_again_unpins() {
        let mut app = running_fixture();
        app.toggle_follow();
        let f = app.follow.as_ref().expect("a running session can be followed");
        assert_eq!(f.id, app.items[0].id);
        assert!(f.entries.is_none() && !f.ended);
        app.toggle_follow();
        assert!(app.follow.is_none());
    }

    #[test]
    fn any_move_unpins() {
        let moves: [fn(&mut App); 5] = [
            |a| a.step_session(1),
            |a| a.step_session(-1),
            |a| a.step_project(1),
            |a| {
                a.q.push('x');
                a.requery();
            },
            |a| {
                a.q.pop();
                a.requery();
            },
        ];
        for (n, m) in moves.iter().enumerate() {
            let mut app = running_fixture();
            app.toggle_follow();
            m(&mut app);
            assert!(app.follow.is_none(), "move {n} left the preview pinned");
        }
    }

    #[test]
    fn a_session_that_stops_keeps_its_tail_and_says_so() {
        let mut app = running_fixture();
        app.toggle_follow();
        app.follow.as_mut().unwrap().entries =
            Some(vec![Entry { who: Who::Claude, text: "shipped it".into() }]);

        app.note_follow_live();
        assert!(!app.follow.as_ref().unwrap().ended, "still running");
        assert!(frame(&mut app, 120, 20).join("\n").contains("◉ following"));

        app.live.clear();
        app.note_follow_live();
        let f = app.follow.as_ref().expect("stopping does not unpin");
        assert!(f.ended);
        let shown = frame(&mut app, 120, 20).join("\n");
        assert!(shown.contains("ended"), "{shown}");
        assert!(shown.contains("shipped it"), "the tail stays: {shown}");
    }

    #[test]
    fn the_followed_tail_is_bottom_anchored() {
        let mut app = running_fixture();
        app.toggle_follow();
        let entries = (0..FOLLOW_ENTRIES)
            .map(|n| Entry { who: if n % 2 == 0 { Who::You } else { Who::Claude }, text: format!("entry {n}") })
            .collect();
        app.follow.as_mut().unwrap().entries = Some(entries);
        let rows = frame(&mut app, 120, 16);
        let last = rows.iter().rposition(|r| r.contains("entry")).expect("entries shown");
        assert!(rows[last].contains(&format!("entry {}", FOLLOW_ENTRIES - 1)), "{rows:#?}");
        assert!(!rows.join("\n").contains("entry 0 "), "the oldest went: {rows:#?}");
        let body = frame_lines(&mut app, 120, 16);
        assert!(body.len() <= 16, "fills the window, never past it: {}", body.len());
    }

    /// `^r` drives `claude -p --resume`; a Copilot session has no such door, and says so.
    #[test]
    fn a_copilot_session_cannot_be_replied_to() {
        let mut app = fixture(&real_tabs());
        app.reply_ok = true;
        let i = app.selected().unwrap();
        app.items[i].source = Source::Copilot;
        app.begin_reply();
        assert!(app.draft.is_none());
        assert!(app.flash.contains("reply is Claude-only"), "{:?}", app.flash);
    }

    /// `^k` only knows how to find and check a running `claude`; a Copilot session says so.
    #[test]
    fn a_copilot_session_cannot_be_ended() {
        let mut app = fixture(&real_tabs());
        let i = app.selected().unwrap();
        app.items[i].source = Source::Copilot;
        app.kill_key();
        assert!(app.kill_confirm.is_none());
        assert!(app.flash.contains("^k is Claude-only"), "{:?}", app.flash);
    }

    /// A Copilot session carries its tag on the tab and in the preview header; Claude's carry none.
    #[test]
    fn copilot_sessions_are_tagged_in_the_strip_and_the_preview() {
        let mut app = fixture(&real_tabs());
        let i = app.selected().unwrap();
        let text = |spans: &[Span]| spans.iter().map(|s| s.content.to_string()).collect::<String>();
        assert!(!text(&tab_marks(&app, &app.items[i], None)).contains("copilot"));

        app.items[i].source = Source::Copilot;
        assert!(text(&tab_marks(&app, &app.items[i], None)).contains("copilot"));
        let head = &preview(&app, &app.items[i], 100, 5)[1];
        assert!(text(&head.spans).starts_with("copilot "), "{head:?}");
    }

    /// An empty draft must not spend a turn.
    #[test]
    fn an_empty_reply_is_not_sent() {
        let mut app = fixture(&real_tabs());
        app.draft = Some((app.items[0].id.clone(), "   ".into()));
        let _ = compose_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), false);
        assert!(app.sending.is_empty(), "whitespace is not a turn");
        assert!(app.draft.is_none(), "but the composer still closes");
    }

    /// A session parked on your answer outranks every other thing a dot can say.
    #[test]
    fn a_waiting_session_outranks_a_running_one() {
        let mut app = fixture(&real_tabs());
        let live = |status: &str| crate::live::Live {
            pid: 4674,
            tty: "ttys000".into(),
            status: status.into(),
            waiting_for: "input needed".into(),
        };
        let id = app.items[0].id.clone();

        app.live.insert(id.clone(), live("busy"));
        assert_eq!(dot_for(&app, &app.items[0]).0, "◉", "busy is just running");

        app.live.insert(id.clone(), live("waiting"));
        assert_eq!(dot_for(&app, &app.items[0]).0, "◆", "waiting gets its own mark");

        // And the bar says so, at a priority nothing can shed it from.
        let bar: String = header(&app, 60).spans.iter().map(|s| s.content.to_string()).collect();
        assert!(bar.contains("waiting on you"), "even a narrow bar says it: {bar:?}");
    }

    /// Every state a session can be in has its own glyph, so a monochrome terminal, a colour-blind
    /// reader or a screenshot in greyscale loses nothing (docs/DESIGN.md, "without colour").
    #[test]
    fn every_status_reads_apart_without_colour() {
        let mut app = fixture(&real_tabs());
        let live = |status: &str| crate::live::Live {
            pid: 4674,
            tty: "ttys000".into(),
            status: status.into(),
            waiting_for: String::new(),
        };
        let now = model::now_ms();
        let mut it = app.items[0].clone();
        let glyph = |app: &App, it: &Item| dot_for(app, it).0;

        app.live.insert(it.id.clone(), live("waiting"));
        let waiting = glyph(&app, &it);
        app.live.insert(it.id.clone(), live("busy"));
        let running = glyph(&app, &it);
        app.live.clear();
        it.mtime = now - 60_000;
        let just_now = glyph(&app, &it);
        it.mtime = now - 3 * 60 * 60 * 1000;
        let today = glyph(&app, &it);
        it.mtime = now - 3 * DAY_MS;
        let older = glyph(&app, &it);

        let marks = [waiting, running, just_now, today, older];
        for (i, a) in marks.iter().enumerate() {
            for b in &marks[i + 1..] {
                assert_ne!(a, b, "two states share a glyph: {marks:?}");
            }
        }

        // The other marks a tab or the panel carries: unfinished, the agent tag, and the tabs that
        // gather sessions by state. None may reuse a dot, or each other (the waiting tab shares ◆
        // with the waiting dot on purpose: they mean the same thing).
        it.open = true;
        it.source = Source::Copilot;
        let tab: String = tab_marks(&app, &it, None).iter().map(|s| s.content.to_string()).collect();
        assert!(tab.contains('▸') && tab.contains("copilot"), "{tab:?}");
        let lead = |t: &str| t.chars().next().unwrap().to_string();
        let others = [
            "▸".to_string(),
            lead(OPEN_TAB),
            lead(ARCHIVED_TAB),
            lead(ALL_TAB),
        ];
        for o in &others {
            assert!(!marks.contains(&o.as_str()), "{o} collides with a dot");
        }
        assert_eq!(lead(WAITING_TAB), waiting, "the waiting tab and dot agree");
        let mut all = others.to_vec();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), others.len(), "{others:?}");
    }

    /// Normal, waiting, error and empty frames share one hierarchy: the key bar on row 0, the
    /// query on row 1, the project context on row 2, the session strip on row 3, the preview under
    /// it and feedback on the last row. A state changes what a row says, never which row says it.
    #[test]
    fn every_state_keeps_the_same_rows() {
        let text = |l: &Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
        let check = |app: &mut App, what: &str| {
            let f = frame_lines(app, 140, 30);
            assert_eq!(f.len(), 30, "{what}: fills the window");
            let row = |r: usize| text(&f[r]);
            assert!(row(KEYBAR_ROW).contains("? help"), "{what}: key bar: {:?}", row(KEYBAR_ROW));
            assert!(row(QUERY_ROW).contains(SEARCH_ICON), "{what}: query row");
            assert!(row(CONTEXT_ROW).contains("everything"), "{what}: context: {:?}", row(CONTEXT_ROW));
            let strip = row(STRIP_ROW);
            assert!(strip.contains('│') || strip.contains("no sessions here"), "{what}: {strip:?}");
        };
        let mut app = fixture(&real_tabs());
        check(&mut app, "normal");

        let id = app.items[0].id.clone();
        app.live.insert(
            id,
            crate::live::Live {
                pid: 1,
                tty: "ttys001".into(),
                status: "waiting".into(),
                waiting_for: "input needed".into(),
            },
        );
        check(&mut app, "waiting");
        app.live.clear();

        app.say(Tone::Error, "reply failed · boom".into());
        check(&mut app, "error");
        let f = frame_lines(&mut app, 140, 30);
        assert!(text(&f[29]).contains("✗ reply failed"), "the error rides in the feedback row");
        assert!(!text(&f[KEYBAR_ROW]).contains("reply failed"), "not in the key bar");

        app.q = "zzqqxxnothingmatchesthis".into();
        app.requery();
        check(&mut app, "empty");
    }

    /// Success and failure differ in more than colour: a failure leads with ✗, and neither is
    /// painted in the other's colour.
    #[test]
    fn a_failed_action_does_not_flash_green() {
        let mut app = fixture(&real_tabs());
        app.say(Tone::Error, "reply failed · boom".into());
        let fb = feedback_lines(&app, 140);
        let flash = fb[0].spans.last().unwrap();
        assert!(flash.content.starts_with("✗ reply failed"), "{:?}", flash.content);
        assert_ne!(flash.style, Tone::Success.style());

        app.say(Tone::Success, "↩ replied · ok".into());
        let flash = feedback_lines(&app, 140)[0].spans.last().unwrap().clone();
        assert!(!flash.content.contains('✗'));
        assert_eq!(flash.style, Tone::Success.style());
    }

    // ---------- #21: fixed regions ----------

    /// The sizes the layout is held to, smallest to largest (docs/DESIGN.md, "Layout").
    const FIXTURE_SIZES: [(u16, u16); 4] = [(60, 18), (80, 24), (104, 26), (160, 40)];

    /// A rendered row read back one grapheme per glyph: a wide glyph's trailing cell is skipped,
    /// so the string's display width is the width the terminal draws.
    fn frame_exact(app: &mut App, cols: u16, rows: u16) -> Vec<String> {
        let mut term = Terminal::new(ratatui::backend::TestBackend::new(cols, rows)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..rows)
            .map(|y| {
                let mut row = String::new();
                let mut x = 0;
                while x < cols {
                    let sym = buf[(x, y)].symbol();
                    row.push_str(sym);
                    x += UnicodeWidthStr::width(sym).max(1) as u16;
                }
                row
            })
            .collect()
    }

    /// The display column the panel's rule sits at on a frame line, if the line has one.
    fn rule_col(l: &Line) -> Option<usize> {
        let at = l.spans.iter().position(|s| s.content == " │ ")?;
        Some(spans_width(&l.spans[..at]))
    }

    /// Every row of a frame is inside the terminal, and every row above the feedback region puts
    /// the panel's rule at the same column: nothing on either side pushed it.
    fn assert_regions_hold(app: &mut App, cols: u16, rows: u16, what: &str) {
        let lines = frame_lines(app, cols as usize, rows as usize);
        assert_eq!(lines.len(), rows as usize, "{what} {cols}x{rows}: fills the window exactly");
        let side = sidebar(app, cols as usize);
        let fb = feedback_lines(app, cols as usize).len();
        for (y, l) in lines.iter().enumerate() {
            let w = spans_width(&l.spans);
            assert!(w <= cols as usize, "{what} {cols}x{rows}: row {y} is {w} wide: {l:?}");
            if y < rows as usize - fb {
                assert_eq!(rule_col(l), Some(side), "{what} {cols}x{rows}: row {y} moved the rule");
            }
        }
        for (y, r) in frame_exact(app, cols, rows).iter().enumerate() {
            let w = UnicodeWidthStr::width(r.as_str());
            assert!(w <= cols as usize, "{what} {cols}x{rows}: drawn row {y} is {w} wide: {r:?}");
        }
    }

    /// Where the preview's rule is drawn, as a row of the frame.
    fn preview_row(app: &mut App, cols: u16, rows: u16) -> usize {
        frame_exact(app, cols, rows)
            .iter()
            .position(|r| r.contains("────"))
            .expect("the preview draws a rule")
    }

    /// The acceptance fixtures: at each supported size every region starts on its own row, and
    /// walking every project and session — the ordinary navigation — moves none of them.
    #[test]
    fn the_regions_hold_their_rows_at_every_supported_size() {
        for (cols, rows) in FIXTURE_SIZES {
            let mut app = fixture(&real_tabs());
            // Detail on some sessions and not others, so preview heights differ as you walk.
            app.items[0].detail = Some(crate::parse::Detail {
                count: 3,
                reply: Some("a reply\n".repeat(60)),
                ..Default::default()
            });
            let start = preview_row(&mut app, cols, rows);
            assert_eq!(start, CHROME, "{cols}x{rows}: the preview starts under the chrome");
            for p in 0..app.tabs.len() {
                app.p_idx = p;
                app.reset_position();
                for s in 0..app.view().len().min(4) {
                    app.cur = s;
                    let what = format!("project {p} session {s}");
                    assert_eq!(preview_row(&mut app, cols, rows), start, "{what} {cols}x{rows}");
                    assert_regions_hold(&mut app, cols, rows, &what);
                    let f = frame_exact(&mut app, cols, rows);
                    let body = |y: usize| f[y].split(" │ ").nth(1).unwrap_or("").to_string();
                    assert!(body(QUERY_ROW).contains(SEARCH_ICON), "{what}: {:?}", body(QUERY_ROW));
                    let pos = format!("{}/{} ", s + 1, app.view().len());
                    assert!(body(STRIP_ROW).starts_with(&pos), "{what}: {:?}", body(STRIP_ROW));
                    assert!(f[rows as usize - 1].trim().is_empty(), "{what}: feedback row idle");
                }
            }
        }
    }

    /// Feedback has a row of its own: saying something leaves the key bar and the preview where
    /// they were, and a long message wraps into the region instead of being clipped away.
    #[test]
    fn feedback_takes_its_own_row_and_leaves_the_key_bar_alone() {
        for (cols, rows) in FIXTURE_SIZES {
            let mut app = fixture(&real_tabs());
            let quiet = frame_exact(&mut app, cols, rows);
            app.say(Tone::Warning, "already running (pid 68227 · ttys013 · waiting · input needed) — ↵ again to open it twice".into());
            let loud = frame_exact(&mut app, cols, rows);
            assert_eq!(loud[KEYBAR_ROW], quiet[KEYBAR_ROW], "{cols}x{rows}: key bar untouched");
            assert_eq!(preview_row(&mut app, cols, rows), CHROME, "{cols}x{rows}: preview stays");
            let tail = loud[rows as usize - FEEDBACK_MAX..].join(" ");
            assert!(tail.contains("↵ again"), "{cols}x{rows}: the consent key survives: {tail:?}");
            assert_regions_hold(&mut app, cols, rows, "long flash");
        }
    }

    /// The project panel and the strip overflow; what is selected in each does not fall off.
    #[test]
    fn the_selected_project_and_session_stay_visible_while_lists_overflow() {
        let many: Vec<String> = [ALL_TAB.to_string(), OPEN_TAB.to_string()]
            .into_iter()
            .chain((0..40).map(|i| format!("project-{i:02}")))
            .collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        for (cols, rows) in FIXTURE_SIZES {
            let mut app = fixture(&refs);
            app.items = (0..60).map(nth_item).collect();
            for it in &mut app.items {
                it.project = "project-07".into();
            }
            for p in [0usize, 1, 2, 9, 20, 41] {
                app.p_idx = p;
                app.reset_position();
                let n = app.view().len();
                for cur in [0, 1, n / 2, n.saturating_sub(1)].into_iter().filter(|&c| c < n.max(1)) {
                    app.cur = cur;
                    let lines = frame_lines(&mut app, cols as usize, rows as usize);
                    let styled = |style: Style| {
                        lines
                            .iter()
                            .flat_map(|l| l.spans.iter())
                            .filter(|s| s.style == style)
                            .map(|s| s.content.to_string())
                            .collect::<String>()
                    };
                    let proj = styled(panel_selected());
                    let head: String = sanitize(&app.tabs[p]).chars().take(4).collect();
                    assert!(proj.contains(&head), "{cols}x{rows} p{p}: project {head} hidden: {proj:?}");
                    let ctx = lines[CONTEXT_ROW].spans.iter().map(|s| s.content.to_string()).collect::<String>();
                    assert!(ctx.contains(&sanitize(&app.tabs[p])), "{cols}x{rows}: context names it: {ctx:?}");
                    if n > 0 {
                        let title = app.items[app.view()[cur]].display_name().to_string();
                        let tab = styled(tab_selected());
                        let head: String = title.chars().take(6).collect();
                        assert!(tab.contains(&head), "{cols}x{rows} s{cur}: focused tab hidden: {tab:?}");
                    }
                    assert_regions_hold(&mut app, cols, rows, "overflow");
                }
            }
        }
    }

    /// The strip says where you are in it, however far it has scrolled.
    #[test]
    fn the_strip_shows_the_session_position() {
        let mut app = fixture(&real_tabs());
        app.items = (0..18).map(nth_item).collect();
        app.tabs = vec![ALL_TAB.to_string()];
        app.p_idx = 0;
        app.cur = 2;
        let view = app.view();
        let strip: String =
            session_tabs(&app, &view, 80).spans.iter().map(|s| s.content.to_string()).collect();
        assert!(strip.starts_with("3/18 "), "{strip:?}");
    }

    /// Below the minimum the frame says what it needs and keeps only what you act on — and the
    /// keys still act, since none of them depends on the window size.
    #[test]
    fn below_the_minimum_the_frame_says_so_and_keeps_the_essentials() {
        let need = format!("window too small (need {MIN_COLS}x{MIN_ROWS})");
        for (cols, rows) in [
            (MIN_COLS - 1, 24),
            (80, MIN_ROWS - 1),
            (40, 10),
            (30, 6),
            (12, 3),
            (1, 1),
            (0, 0),
        ] {
            let mut app = fixture(&real_tabs());
            let lines = frame_lines(&mut app, cols, rows);
            assert!(lines.len() <= rows, "{cols}x{rows}: {} rows", lines.len());
            let text: Vec<String> = lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
                .collect();
            for (y, t) in text.iter().enumerate() {
                let w = UnicodeWidthStr::width(t.as_str());
                assert!(w <= cols, "{cols}x{rows}: row {y} is {w} wide: {t:?}");
            }
            if cols >= need.len() && rows >= 1 {
                assert_eq!(text[0], need, "{cols}x{rows}");
            }
            if cols >= 40 && rows >= 4 {
                let all = text.join("\n");
                assert!(all.contains("⌂ everything · 1/12"), "{cols}x{rows}: {all}");
                assert!(all.contains(app.items[app.view()[0]].display_name()), "{cols}x{rows}: {all}");
                assert!(all.contains("↵ resume"), "{cols}x{rows}: {all}");

                // ←→ and ↑↓ still move, and the frame follows.
                app.step_session(1);
                let moved = frame_lines(&mut app, cols, rows);
                let all: String = moved.iter().flat_map(|l| l.spans.iter()).map(|s| s.content.to_string()).collect();
                assert!(all.contains("2/12"), "{cols}x{rows}: {all}");
                app.step_project(1);
                let moved = frame_lines(&mut app, cols, rows);
                let all: String = moved.iter().flat_map(|l| l.spans.iter()).map(|s| s.content.to_string()).collect();
                assert!(all.contains(&sanitize(&app.tabs[1])), "{cols}x{rows}: {all}");
            }
            if cols > 0 && rows > 0 {
                assert_eq!(frame_exact(&mut app, cols as u16, rows as u16).len(), rows);
            }
        }
        // The boundary itself lays out the dashboard.
        let mut app = fixture(&real_tabs());
        assert_regions_hold(&mut app, MIN_COLS as u16, MIN_ROWS as u16, "minimum");
    }

    /// CJK and emoji titles, a CJK project, an emoji branch and a very long path: each is cut at
    /// its own region's edge, and the panel's rule never moves.
    #[test]
    fn unicode_titles_and_long_paths_stay_in_their_regions() {
        let cjk = "日本語のセッションタイトルがとても長い場合のテスト日本語日本語日本語日本語";
        let emoji = "🎉🚀 ship it 🎉🚀🎉🚀🎉🚀🎉🚀🎉🚀🎉🚀🎉🚀🎉🚀🎉🚀🎉🚀🎉🚀🎉🚀🎉🚀";
        let project = "プロジェクト名がとても長いプロジェクト";
        let deep = format!("/Users/x/{}日本語/🎉", "a-very-deep/folder-name/".repeat(12));
        let mut app = fixture(&[ALL_TAB, project]);
        for (n, it) in app.items.iter_mut().enumerate() {
            it.project = project.into();
            it.name = [cjk, emoji][n % 2].into();
            it.cwd = Some(deep.clone());
            it.branch = Some("feat/🎉-日本語-".repeat(8));
        }
        app.items[0].detail = Some(crate::parse::Detail {
            count: 2,
            first: Some(cjk.repeat(3)),
            last: Some(emoji.into()),
            reply: Some(format!("{cjk}\n{emoji}\n`{deep}`")),
            recap: Some(cjk.repeat(2)),
            ..Default::default()
        });
        for (cols, rows) in FIXTURE_SIZES.into_iter().chain([(MIN_COLS as u16, MIN_ROWS as u16)]) {
            for p in 0..app.tabs.len() {
                app.p_idx = p;
                for cur in 0..2 {
                    app.cur = cur;
                    app.say(Tone::Error, format!("couldn't open {deep}: {cjk}"));
                    assert_regions_hold(&mut app, cols, rows, "unicode");
                    app.flash.clear();
                    assert_regions_hold(&mut app, cols, rows, "unicode");
                }
            }
        }
        // The context line cuts the path from the left, so its end — the folder — survives.
        app.p_idx = 1;
        app.cur = 0;
        let ctx = context_line(&app, app.selected(), 100);
        let t: String = ctx.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(t.contains('…') && t.ends_with("日本語/🎉"), "{t:?}");
        assert!(UnicodeWidthStr::width(t.as_str()) <= 100, "{t:?}");
    }

    #[test]
    fn the_panel_rules_off_collections_projects_and_the_archive() {
        let tabs: Vec<String> =
            [ALL_TAB, OPEN_TAB, WAITING_TAB, "sessio", "mybit", ARCHIVED_TAB].map(String::from).to_vec();
        let e = panel_entries(&tabs);
        assert_eq!(e, vec![Some(0), Some(1), Some(2), None, Some(3), Some(4), None, Some(5)]);
        let only: Vec<String> = [ALL_TAB, "sessio"].map(String::from).to_vec();
        assert_eq!(panel_entries(&only), vec![Some(0), None, Some(1)]);
    }

    #[test]
    fn a_waiting_tab_sits_below_open_and_holds_only_waiting_sessions() {
        let mut app = fixture(&real_tabs());
        let live = |status: &str| crate::live::Live {
            pid: 1,
            tty: "ttys000".into(),
            status: status.into(),
            waiting_for: String::new(),
        };
        let (a, b) = (app.items[0].id.clone(), app.items[3].id.clone());
        app.items[1].open = true; // so there is an open tab to sit below

        app.live.insert(a.clone(), live("busy"));
        assert!(!app.tabs_now().iter().any(|t| t == WAITING_TAB), "busy is not waiting");

        app.live.insert(a.clone(), live("waiting"));
        app.live.insert(b.clone(), live("waiting"));
        app.rebuild_tabs();
        let open = app.tabs.iter().position(|t| t == OPEN_TAB).unwrap();
        assert_eq!(app.tabs[open + 1], WAITING_TAB, "right below open: {:?}", app.tabs);

        app.p_idx = open + 1;
        let ids: Vec<&str> = app.view().iter().map(|&i| app.items[i].id.as_str()).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&a.as_str()) && ids.contains(&b.as_str()), "{ids:?}");

        // Once nothing waits the tab goes, and the panel falls back rather than pointing at air.
        app.live.clear();
        app.rebuild_tabs();
        assert!(!app.tabs.iter().any(|t| t == WAITING_TAB));
        assert!(app.p_idx < app.tabs.len());
    }

    /// Eyeball it: `cargo test dump_the_dashboard -- --nocapture --ignored`.
    #[test]
    #[ignore]
    fn dump_the_dashboard() {
        let mut app = fixture(&real_tabs());
        app.p_idx = 0;
        // A session loaded the way a real one is: a recap, a first and last prompt, and a reply
        // long enough to be the thing that sets the preview's height.
        let reply = (1..=14)
            .map(|i| format!("Line {i} of a reply that runs well past the recap beside it."))
            .collect::<Vec<_>>()
            .join("\n");
        app.items[0].detail = Some(crate::parse::Detail {
            count: 5,
            first: Some("pull down the most updated version please".into()),
            last: Some("update it so i can run with sessio".into()),
            reply: Some(reply),
            recap: Some("Goal: try a sidebar layout for sessio's project tabs.".into()),
            ..Default::default()
        });
        // Sizes worth eyeballing: override with SESSIO_DUMP_COLS=90,181 or =80x24,60x18.
        let sizes: Vec<(u16, u16)> = match std::env::var("SESSIO_DUMP_COLS") {
            Ok(v) => v
                .split(',')
                .filter_map(|c| match c.trim().split_once('x') {
                    Some((w, h)) => Some((w.parse().ok()?, h.parse().ok()?)),
                    None => Some((c.trim().parse().ok()?, 34)),
                })
                .collect(),
            Err(_) => vec![(136, 34)],
        };
        if std::env::var("SESSIO_DUMP_REPLY").is_ok() {
            app.draft = Some((app.items[0].id.clone(), "yes, start with the mock generator".into()));
        }
        for (cols, rows) in sizes {
            println!("\n-- {cols}x{rows} --");
            for (y, r) in frame(&mut app, cols, rows).iter().enumerate() {
                let t = r.trim_end();
                let tag =
                    if t.chars().all(|c| c == ' ' || c == '\u{2502}') { "   <-- BLANK" } else { "" };
                println!("{y:>2}|{t}|{tag}");
            }
        }
    }

    #[test]
    fn the_key_bar_never_exceeds_the_terminal_width() {
        for cols in 8..200 {
            let w = UnicodeWidthStr::width(rendered(cols).as_str());
            assert!(w <= cols, "{cols} cols rendered {w}");
        }
    }

    #[test]
    fn a_recap_body_is_italic_throughout() {
        let l = italic(Line::from(vec![
            Span::raw("plain "),
            Span::styled("code", theme::code()),
        ]));
        for sp in l.spans.iter() {
            assert!(
                sp.style.add_modifier.contains(Modifier::ITALIC),
                "every span stays italic, including pre-styled ones: {sp:?}"
            );
        }
    }

    #[test]
    fn the_measure_tracks_the_window_but_stays_scannable() {
        // Two bugs, one test. Wrapping at the full width made a 400-column window render a
        // 388-character line; pinning a flat 90 left a 181-column window three-quarters empty.
        for w in [40usize, 80, 120, 181, 200, 400, 1000] {
            let prose = prose_width(w);
            assert!(prose <= w, "{w} cols gave a {prose}-wide measure");
            assert!(prose <= MEASURE_MAX, "{w} cols gave a {prose}-wide measure");
        }
        assert!(prose_width(80) > 20, "a narrow window still has to hold words");
        // The width the user actually runs: it must use appreciably more than the old flat cap.
        // 90 was the old flat cap; a wide window must now use appreciably more than that.
        assert!(prose_width(160) > 90, "a wide window fills: {} at 160 cols", prose_width(160));
        assert_eq!(prose_width(1000), MEASURE_MAX, "but not without limit");
    }

    #[test]
    fn a_block_spends_no_row_on_its_label() {
        let body = vec![Line::from(Span::raw("one")), Line::from(Span::raw("two"))];
        let out = gutter_block("recap 22m", dim(), body);

        assert_eq!(out.len(), 2, "the label rides the first row rather than taking its own");
        assert!(out[0].spans[0].content.ends_with("recap 22m"), "label in the gutter");
        assert_eq!(
            spans_width(&out[0].spans[..2]),
            GUTTER,
            "content starts at the gutter column"
        );
        assert_eq!(out[1].spans[0].content, " ".repeat(GUTTER), "and stays there");
    }


    #[test]
    fn one_time_format_for_the_whole_preview() {
        // Three formats used to sit within four lines of each other. `since` is the only one now.
        assert_eq!(since("not a timestamp"), "");
        assert!(!since("2026-08-20T18:07:00.000Z").contains('['), "no bracketed second format");
    }

    #[test]
    fn word_delete_takes_one_word_at_a_time() {
        let q = "mybit tooling";
        assert_eq!(&q[..drop_word(q)], "mybit ");
        assert_eq!(&q[..drop_word("mybit ")], "");
        assert_eq!(&q[..drop_word("single")], "");
        assert_eq!(&q[..drop_word("")], "");
    }

    #[test]
    fn word_delete_treats_path_punctuation_as_a_boundary() {
        let q = "mybit/tooling";
        assert_eq!(&q[..drop_word(q)], "mybit/");
        let q = "a-b-c";
        assert_eq!(&q[..drop_word(q)], "a-b-");
    }

    #[test]
    fn word_delete_never_splits_a_character() {
        // The returned length is a byte index; slicing at it must not panic on multi-byte input.
        for q in ["ěščř žluť", "日本語 テスト", "…  x"] {
            let _ = &q[..drop_word(q)];
        }
    }

    #[test]
    fn enter_resumes_only_when_nothing_is_attached() {
        assert_eq!(enter_action(false, false), EnterAction::Resume);
        // A stale confirm on a session that is no longer running must not change anything.
        assert_eq!(enter_action(false, true), EnterAction::Resume);
    }

    #[test]
    fn enter_on_a_running_session_goes_to_it_rather_than_duplicating() {
        assert_eq!(enter_action(true, false), EnterAction::GoToRunning);
    }

    #[test]
    fn a_second_enter_is_consent_to_open_it_twice() {
        assert_eq!(enter_action(true, true), EnterAction::ResumeAnyway);
    }

    #[test]
    fn a_flash_outlives_the_frame_that_drew_it() {
        // The regression this guards: the loop redraws on every 120ms input poll, so clearing the
        // flash after one draw — what the JS reference does, where a frame is a keypress or the 2s
        // tick — put "↵ again to open it twice" on screen for about a tenth of a second. Long
        // enough to repaint, far too short to read, which is indistinguishable from ↵ doing
        // nothing at all.
        let set_at = Instant::now();
        let until = Some(set_at + FLASH);

        assert!(FLASH >= Duration::from_secs(3), "a message this long needs seconds, not frames");
        assert!(!flash_expired(set_at, until), "gone on the frame it was set");
        assert!(!flash_expired(set_at + Duration::from_millis(120), until), "gone after one poll");
        assert!(!flash_expired(set_at + FLASH - Duration::from_millis(1), until));
        assert!(flash_expired(set_at + FLASH, until));
        // Nothing to show is not something to expire.
        assert!(!flash_expired(set_at, None));
    }

    #[test]
    fn the_warning_names_somewhere_to_look() {
        let live = |pid, tty: &str, status: &str, waiting_for: &str| crate::live::Live {
            pid,
            tty: tty.into(),
            status: status.into(),
            waiting_for: waiting_for.into(),
        };
        assert_eq!(running_where(&live(68227, "ttys013", "", "")), "pid 68227 · ttys013");
        assert_eq!(running_where(&live(7, "", "", "")), "pid 7");
        assert_eq!(running_where(&live(7, "ttys001", "busy", "")), "pid 7 · ttys001 · busy");
        // The one status the user has to do something about says what it needs.
        assert_eq!(
            running_where(&live(7, "ttys001", "waiting", "input needed")),
            "pid 7 · ttys001 · waiting · input needed"
        );
    }

    #[test]
    fn a_wide_terminal_keeps_every_hint() {
        assert_eq!(fit_segments(&bar(), 200).len(), bar().len());
    }

    #[test]
    fn help_survives_the_narrowest_bar() {
        assert!(rendered(6).contains("? help"));
    }

    #[test]
    fn hints_are_shed_from_the_most_expendable_end() {
        let narrow = rendered(80);
        assert!(narrow.contains("↵ resume"), "resume is the point: {narrow}");
        assert!(!narrow.contains("^f search-in-text"), "p5 sheds first: {narrow}");
    }

    /// The first session of `fixture`, running as a made-up pid, last written `age_h` hours ago.
    /// Nothing here reaches `kill::end`: every test stops at the question or the refusal.
    fn running_for(app: &mut App, status: &str, age_h: i64) -> String {
        let id = app.items[0].id.clone();
        app.items[0].mtime = model::now_ms() - age_h * 3_600_000;
        app.live.insert(
            id.clone(),
            crate::live::Live {
                pid: 4242,
                tty: "ttys009".into(),
                status: status.into(),
                waiting_for: String::new(),
            },
        );
        app.cur = app.view().iter().position(|&i| app.items[i].id == id).unwrap();
        id
    }

    #[test]
    fn ctrl_k_refuses_and_says_why() {
        let mut app = fixture(&real_tabs());
        app.cur = 0;
        app.kill_key();
        assert!(app.flash.contains("not running"), "{:?}", app.flash);

        for (status, age, why) in
            [("busy", 100, "busy"), ("waiting", 100, "waiting on you"), ("idle", 5, "active 5h ago")]
        {
            let mut app = fixture(&real_tabs());
            running_for(&mut app, status, age);
            app.kill_key();
            assert!(app.flash.contains(why), "{status}: {:?}", app.flash);
            assert!(app.kill_confirm.is_none(), "{status}: nothing to confirm");
        }
    }

    #[test]
    fn the_first_ctrl_k_only_asks() {
        let mut app = fixture(&real_tabs());
        let id = running_for(&mut app, "idle", 72);
        app.kill_key();
        assert_eq!(app.kill_confirm, Some((id, 4242)));
        assert!(app.flash.contains("pid 4242 · ttys009, idle 3d)? ^k again"), "{:?}", app.flash);
    }

    #[test]
    fn a_stale_session_is_marked_in_the_preview_and_the_bar() {
        let mut app = fixture(&real_tabs());
        running_for(&mut app, "idle", 72);
        let rows = frame(&mut app, 180, 30).join("\n");
        assert!(rows.contains("◉ running · stale · idle 3d · pid 4242"), "{rows}");
        let bar: String = header(&app, 400).spans.iter().map(|s| s.content.to_string()).collect();
        assert!(bar.contains("^k end-stale"), "{bar}");

        let mut app = fixture(&real_tabs());
        running_for(&mut app, "idle", 5);
        let rows = frame(&mut app, 180, 30).join("\n");
        assert!(rows.contains("◉ running · pid 4242") && !rows.contains("stale"), "{rows}");
        let bar: String = header(&app, 400).spans.iter().map(|s| s.content.to_string()).collect();
        assert!(!bar.contains("^k"), "{bar}");
    }
}


// ---------- the browser demo ----------

/// The dashboard, running in a browser instead of a terminal.
///
/// This exists so the website demo cannot lie. The previous site hand-wrote its terminal mock in
/// HTML and it drifted the moment the layout changed — by the time anyone noticed, it documented
/// keys that no longer existed. Here the page calls the same `frame_lines` the terminal does,
/// against fixture sessions, so a layout change shows up on the site the next time it is built or
/// it does not show up at all.
#[cfg(target_arch = "wasm32")]
pub mod demo {
    use super::*;
    use std::cell::RefCell;
    use wasm_bindgen::prelude::*;

    thread_local! {
        static APP: RefCell<App> = RefCell::new(build());
    }

    /// A fixed clock, so the demo's ages read the same on every visit.
    const NOW: i64 = 1_788_000_000_000;

    fn item(n: usize, project: &str, name: &str, ago_min: i64, open: bool) -> Item {
        Item {
            id: format!("demo-{n}"),
            key: format!("demo-{n}"),
            file: std::path::PathBuf::new(),
            mtime: NOW - ago_min * 60_000,
            size: 8_000 + (n as u64) * 5_300,
            dir: project.into(),
            source: Source::Claude,
            first: Some("where did we get to with the limiter?".into()),
            first_ts: None,
            cwd: Some(format!("~/code/{project}")),
            branch: Some(if n % 3 == 0 { "main" } else { "feat/limiter" }.into()),
            custom: None,
            ai: Some(name.into()),
            open,
            open_reason: open.then_some(crate::parse::OpenReason::CallToAction),
            recap: None,
            recap_ts: None,
            title: Some(name.into()),
            name: name.into(),
            project: project.into(),
            hay: format!("{name} {project}").to_lowercase(),
            detail: None,
        }
    }

    fn build() -> App {
        crate::model::DEMO_NOW.store(NOW, std::sync::atomic::Ordering::Relaxed);
        let rows: &[(&str, &str, i64, bool)] = &[
            ("api", "add rate limiting to auth routes", 2, true),
            ("api", "refactor the cache layer", 41, false),
            ("web-app", "fix login redirect loop", 180, true),
            ("web-app", "dark mode tokens", 900, false),
            ("cli-tool", "ship the release workflow", 1500, false),
            ("docs", "write onboarding docs", 2600, false),
        ];
        let mut items: Vec<Item> =
            rows.iter().enumerate().map(|(n, r)| item(n, r.0, r.1, r.2, r.3)).collect();

        items[0].recap = Some(
            "Limiter is in and tested; the Retry-After header still needs a decision on units. \
             Next action is yours."
                .into(),
        );
        items[0].detail = Some(crate::parse::Detail {
            count: 14,
            first: Some("where did we get to with the limiter?".into()),
            last: Some("use seconds, and document it".into()),
            reply: Some(
                "Added a token-bucket limiter on the login and signup routes; over-limit \
                 requests now return **429** with a `Retry-After`.\n\nThe remaining decision is \
                 units — seconds is what every client library already expects, so unless you \
                 want HTTP-date for a specific consumer I'd use that.\n\nWant me to wire the \
                 same limiter into the password-reset route while I'm here?"
                    .into(),
            ),
            recap: items[0].recap.clone(),
            ..Default::default()
        });

        let archive = Archive::default();
        let tabs = crate::model::tabs_for(&items, &archive);
        let (tx, _rx) = mpsc::channel();
        // The receiver is dropped on purpose: nothing in the browser sends on this channel.
        App {
            items,
            archive,
            tabs,
            q: String::new(),
            cur: 0,
            p_idx: 0,
            expand: false,
            help: false,
            flash: String::new(),
            flash_tone: Tone::Info,
            flash_until: None,
            deep: None,
            search_gen: 0,
            details: HashMap::new(),
            detail_inflight: HashSet::new(),
            live: crate::live::LiveMap::new(),
            confirm: None,
            kill_confirm: None,
            draft: None,
            sending: HashSet::new(),
            reply_ok: false,
            follow: None,
            issues: false,
            issue_cur: 0,
            tx,
        }
    }

    /// A role colour as the page draws it: the xterm-256 value `theme::rgb` gives, as hex.
    fn css(c: ratatui::style::Color) -> Option<String> {
        theme::rgb(c).map(|(r, g, b)| format!("#{r:02x}{g:02x}{b:02x}"))
    }

    fn escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }

    /// One frame of the real dashboard, as HTML.
    #[wasm_bindgen]
    pub fn render(cols: usize, rows: usize) -> String {
        APP.with(|a| {
            let mut app = a.borrow_mut();
            let mut out = String::with_capacity(cols * rows * 12);
            for line in frame_lines(&mut app, cols, rows) {
                for sp in line.spans {
                    let mut style = String::new();
                    if let Some(fg) = sp.style.fg.and_then(css) {
                        style.push_str(&format!("color:{fg};"));
                    }
                    if let Some(bg) = sp.style.bg.and_then(css) {
                        style.push_str(&format!("background:{bg};"));
                    }
                    if sp.style.add_modifier.contains(Modifier::BOLD) {
                        style.push_str("font-weight:700;");
                    }
                    if sp.style.add_modifier.contains(Modifier::ITALIC) {
                        style.push_str("font-style:italic;");
                    }
                    let text = escape(sp.content.as_ref());
                    if style.is_empty() {
                        out.push_str(&text);
                    } else {
                        out.push_str(&format!("<span style=\"{style}\">{text}</span>"));
                    }
                }
                out.push('\n');
            }
            out
        })
    }

    /// A keypress from the page. Names match the terminal's keys; unknown ones are ignored, so
    /// the page can forward anything without having to know what the dashboard supports.
    #[wasm_bindgen]
    pub fn key(name: &str) {
        APP.with(|a| {
            let mut app = a.borrow_mut();
            match name {
                "ArrowUp" => app.step_project(-1),
                "ArrowDown" => app.step_project(1),
                "ArrowLeft" => app.step_session(-1),
                "ArrowRight" => app.step_session(1),
                "Tab" => app.expand = !app.expand,
                "?" => app.help = !app.help,
                "Escape" => {
                    if app.draft.is_some() {
                        app.draft = None;
                    } else if !app.q.is_empty() {
                        app.q.clear();
                        app.requery();
                    } else {
                        app.help = false;
                    }
                }
                "Backspace" => {
                    if let Some((id, mut t)) = app.draft.take() {
                        t.pop();
                        app.draft = Some((id, t));
                    } else {
                        app.q.pop();
                        app.requery();
                    }
                }
                "^r" => app.begin_reply(),
                "^t" => app.toggle_follow(),
                _ => {
                    let mut ch = name.chars();
                    if let (Some(c), None) = (ch.next(), ch.next()) {
                        if c >= ' ' {
                            if let Some((id, mut t)) = app.draft.take() {
                                t.push(c);
                                app.draft = Some((id, t));
                            } else {
                                app.q.push(c);
                                app.requery();
                            }
                        }
                    }
                }
            }
        })
    }
}
