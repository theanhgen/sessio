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

use crate::md::md_lines;
use crate::model::{self, Item, ACTIVE_MS, ALL_TAB, ARCHIVED_TAB, OPEN_TAB, RECENT_MS};
use crate::parse::Detail;
use crate::safety::sanitize;
use crate::store::Archive;
use crate::{discover, resume, search};

pub mod theme {
    use ratatui::style::Color;
    pub const DIM: Color = Color::DarkGray;
    pub const CODE: Color = Color::Cyan;
    pub const ACCENT: Color = Color::Cyan;
    pub const NAMED: Color = Color::Yellow;
    pub const ACTIVE: Color = Color::Green;
    pub const RECENT: Color = Color::Indexed(208);
    pub const REPLY: Color = Color::Indexed(141);

    /// Selection backgrounds. Reversing the terminal's own colours made every selection the same
    /// slab of black, so the panel and the tab strip could not be told apart at a glance — and on
    /// a light theme it was the heaviest thing on screen.
    ///
    /// One hue, and value does the hierarchy: the session you are reading is plum, the project
    /// that contains it is neutral. Two competing colours read as two things of equal weight,
    /// which is not what they are — the panel is context, the tab is focus.
    pub const PANEL_SEL: Color = Color::Indexed(238);
    pub const TAB_SEL: Color = Color::Indexed(54);
    /// Text on either of them, bright enough to read on both.
    pub const ON_SEL: Color = Color::Indexed(255);
}

/// The highlight for the project the panel is sitting on.
fn panel_selected() -> Style {
    Style::default().bg(theme::PANEL_SEL).fg(theme::ON_SEL)
}

/// The highlight for the session tab in focus. A different hue from the panel's on purpose: two
/// selections are on screen at once and they answer different questions.
fn tab_selected() -> Style {
    Style::default().bg(theme::TAB_SEL).fg(theme::ON_SEL).add_modifier(Modifier::BOLD)
}

const REFRESH: Duration = Duration::from_secs(2);
/// How long a flash stays on screen. It has to be a duration, not a frame: this loop redraws on
/// every 120ms input poll, and a message cleared after one frame — as the JS reference does, where
/// a frame is a keypress or the 2s tick — was gone before anyone could read it.
const FLASH: Duration = Duration::from_secs(5);
/// The session tab strip is always exactly one row. Like a browser, tabs shrink as more open and
/// then the strip scrolls — it never wraps onto a second row, so nothing below it ever moves.
const TAB_ROW: usize = 1;
/// No one tab may take more than this, however long its title — the focused tab is allowed to be
/// the wide one, but not so wide that nothing is left to steer by.
const TAB_MAX: usize = 44;
/// Columns held back for the `‹N` / `+N›` counts when the strip scrolls.
const MARKERS: usize = 12;

fn dim() -> Style {
    Style::default().fg(theme::DIM)
}

enum Msg {
    Items(Vec<Item>, crate::live::LiveMap),
    Detail { key: String, mtime: i64, detail: Box<Detail> },
    Search { gen: u64, query: String, files: Option<HashSet<PathBuf>> },
    /// A headless reply came back (or failed). `key` identifies the session it belongs to.
    Replied { key: String, ok: bool, text: String },
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
    /// When the current flash stops being shown. `None` means there is nothing to expire.
    flash_until: Option<Instant>,
    deep: Option<Deep>,
    search_gen: u64,
    details: HashMap<String, (i64, Detail)>,
    detail_inflight: HashSet<String>,
    /// Sessions with a `claude` process attached right now, by session id.
    live: crate::live::LiveMap,
    /// Session the user has been warned about and may now resume anyway.
    confirm: Option<String>,
    /// The reply being typed, and the session it is addressed to. `None` when not replying.
    draft: Option<(String, String)>, // (session id, text)
    /// Sessions with a headless reply in flight, by id.
    sending: HashSet<String>,
    /// Whether the "this spends tokens" warning has been acknowledged this run.
    reply_ok: bool,
    tx: Sender<Msg>,
}

impl App {
    /// Say something back about the key just pressed, and keep saying it long enough to be read.
    fn say(&mut self, msg: String) {
        self.flash = msg;
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

    /// Indices into `items`, filtered by tab and query, ranked. Port of `view()` at :476.
    fn view(&self) -> Vec<usize> {
        let tab = self.tabs.get(self.p_idx).map(String::as_str).unwrap_or(ALL_TAB);
        let mut idx: Vec<usize> = if tab == ARCHIVED_TAB {
            (0..self.items.len()).filter(|&i| self.archived(&self.items[i])).collect()
        } else {
            (0..self.items.len())
                .filter(|&i| {
                    let it = &self.items[i];
                    let in_tab = match tab {
                        ALL_TAB => true,
                        OPEN_TAB => it.open,
                        p => it.project == p,
                    };
                    in_tab && !self.archived(it)
                })
                .collect()
        };

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
        let (key, mtime, file) =
            (self.items[i].key.clone(), self.items[i].mtime, self.items[i].file.clone());
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
                let d = crate::parse::detail(&file);
                let _ = tx.send(Msg::Detail { key, mtime, detail: Box::new(d) });
            });
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (key, mtime, file);
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
        if let Some(live) = self.live.get(&id) {
            // The guard ↵ already uses: there is no safe way to put text into the stdin of a
            // `claude` someone is sitting in front of.
            let where_ = running_where(live);
            self.say(format!("that session is running — {where_} · answer it there"));
        } else if self.sending.contains(&id) {
            self.say("still waiting on the last reply".into());
        } else if !self.reply_ok {
            // Once per run: this spends tokens from a list, with no turn-by-turn to watch.
            self.reply_ok = true;
            self.say(format!(
                "^r sends a turn to \"{}\" and spends tokens — ^r again to write it",
                first_words(&name, 4)
            ));
        } else {
            self.draft = Some((id, String::new()));
        }
    }

    fn rebuild_tabs(&mut self) {
        let active = self.tabs.get(self.p_idx).cloned();
        self.tabs = model::tabs_for(&self.items, &self.archive);
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
        flash_until: None,
        deep: None,
        search_gen: 0,
        details: HashMap::new(),
        detail_inflight: HashSet::new(),
        live: crate::live::scan(),
        confirm: None,
        draft: None,
        sending: HashSet::new(),
        reply_ok: false,
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
        }
        term.draw(|f| draw(f, app))?;

        // Live refresh: rescan every 2s on a worker so input never stalls behind the scan.
        if !refreshing && last_refresh.elapsed() >= REFRESH {
            refreshing = true;
            last_refresh = Instant::now();
            let tx2 = tx.clone();
            let extra = app.deep.as_ref().map(|d| d.files.clone()).unwrap_or_default();
            std::thread::spawn(move || {
                let items = model::load(&extra);
                // Same worker: one `ps` per refresh, off the input path.
                let _ = tx2.send(Msg::Items(items, crate::live::scan()));
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
                    app.live = live;
                    absorb_items(app, new_items);
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
                        None => app.say("content search failed".into()),
                        Some(files) => {
                            let root = discover::projects_root();
                            let keys = files
                                .iter()
                                .filter_map(|f| {
                                    f.strip_prefix(&root).ok().map(|r| r.to_string_lossy().into_owned())
                                })
                                .collect();
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
                    app.say(if ok {
                        format!("↩ replied · {}", first_words(&text, 8))
                    } else {
                        format!("reply failed · {}", first_words(&text, 10))
                    });
                }
            }
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
        app.say(format!("↩ {freed} archived session{s} back — active again"));
    }
    app.tabs = model::tabs_for(&app.items, &app.archive);
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

    // "↵ again to open it twice" means the *very next* key. Moving, typing or switching tabs is
    // not consent to start a second process on a live transcript.
    if k.code != KeyCode::Enter {
        app.confirm = None;
    }

    if ctrl && k.code == KeyCode::Char('c') {
        return Ok(Flow::Quit);
    }
    if app.help {
        app.help = false; // any key closes the overlay
        return Ok(Flow::Continue);
    }

    match k.code {
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
                app.say("searching…".into());
                std::thread::spawn(move || {
                    let root = discover::projects_root();
                    let files = search::content_search(&term_q, &root);
                    let _ = tx.send(Msg::Search { gen, query: term_q, files });
                });
            }
        }
        KeyCode::Char('a') if ctrl => {
            if let Some(i) = app.selected() {
                let (key, id) = (app.items[i].key.clone(), app.items[i].id.clone());
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
        KeyCode::Enter => {
            if let Some(i) = app.selected() {
                let (cwd, id, name) = (
                    app.items[i].cwd.clone(),
                    app.items[i].id.clone(),
                    app.items[i].name.clone(),
                );
                // Already running? Resuming would point a second `claude` at the same transcript
                // and both would append to it. Go to the session instead — and if we can't find
                // its window, say where it is and make the duplicate an explicit second ↵.
                let running = app.live.get(&id).cloned();
                if let EnterAction::GoToRunning = enter_action(
                    running.is_some(),
                    app.confirm.as_deref() == Some(id.as_str()),
                ) {
                    let live = running.expect("GoToRunning implies a live process");
                    if focus_enabled() && resume::focus_window_titled(&name) {
                        let short: String = name.chars().take(40).collect();
                        app.say(format!("↗ focused \"{short}\" — already running"));
                    } else {
                        app.confirm = Some(id.clone());
                        app.say(format!(
                            "already running ({}) — ↵ again to open it twice",
                            running_where(&live)
                        ));
                    }
                    return Ok(Flow::Continue);
                }
                app.confirm = None;
                // Ghostty: open in a NEW window and keep sessio running as a launcher.
                if resume::in_ghostty() {
                    if let Some(dir) = cwd.as_deref() {
                        if resume::ghostty_launch(std::path::Path::new(dir), &id) {
                            let short: String = name.chars().take(40).collect();
                            app.say(format!("↗ opened \"{short}\" in a new window"));
                            return Ok(Flow::Continue);
                        }
                    }
                }
                hand_over(term, cwd.as_deref(), &id);
            }
        }
        KeyCode::Char('o') if ctrl => {
            if let Some(i) = app.selected() {
                let (cwd, id) = (app.items[i].cwd.clone(), app.items[i].id.clone());
                hand_over(term, cwd.as_deref(), &id);
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
            app.say("reply discarded".into());
        }
        KeyCode::Enter => {
            let body = text.trim().to_string();
            app.draft = None;
            if body.is_empty() {
                app.say("nothing to send".into());
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
    app.say(format!("⏳ sending to \"{}\" …", first_words(&app.items[i].name, 4)));

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

/// Replace sessio with `claude --resume`. The terminal is restored *first* — `exec` never
/// returns, so there is no later opportunity to undo raw mode.
#[cfg(not(target_arch = "wasm32"))]
fn hand_over(term: &mut Terminal<CrosstermBackend<Stdout>>, cwd: Option<&str>, id: &str) -> ! {
    restore(term);
    let path = cwd.map(std::path::Path::new);
    let err = resume::resume_in_place(path, id);
    println!(
        "\nCouldn't launch claude ({err}). Run it yourself:\n  {}\n",
        resume::manual_command(path, id)
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

    // The panel is carved off the left first, so everything below measures itself against the
    // width that is actually left rather than the terminal's.
    let side = sidebar(app, cols);
    let cols = cols.saturating_sub(side + SIDE_GAP);

    // The chrome is a fixed height: header, search, and the one-row session tab strip. Nothing
    // here is derived from what the highlighted session contains, so the preview under it never
    // moves as you walk the tabs.
    let chrome = 1 + 1 + TAB_ROW + usize::from(app.draft.is_some());
    let preview_box = rows.saturating_sub(chrome);
    let base = sel.map(|i| preview(app, &app.items[i], cols, 0).len()).unwrap_or(0);
    let reply_max = preview_box.saturating_sub(base).max(1);
    let prev = sel.map(|i| preview(app, &app.items[i], cols, reply_max)).unwrap_or_default();

    let mut lines: Vec<Line> = Vec::with_capacity(rows + 4);
    lines.push(header(app, cols));
    lines.push(query_line(app, view.len()));
    lines.push(session_tabs(app, &view, cols));
    if let Some((_, text)) = &app.draft {
        lines.push(compose_line(text));
    }
    lines.extend(prev);

    join_side(side_panel(app, side, rows), lines, side)
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
        segs.push(Seg { p: 1, t: "↵ new-window", accent: false });
        segs.push(Seg { p: 2, t: "^o same-window", accent: false });
    } else {
        segs.push(Seg { p: 1, t: "↵ resume", accent: false });
    }
    segs.push(Seg { p: 2, t: "^r reply", accent: false });
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

    // A flash is why you pressed the key; the hints are always there. So the message is budgeted
    // first and the bar shrinks around it — otherwise "already running (pid …)" is clipped to
    // "already runni" and the keypress looks like it did nothing.
    let flash = sanitize(&app.flash);
    let flash_w = if flash.is_empty() { 0 } else { UnicodeWidthStr::width(flash.as_str()) + 2 };

    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, seg) in fit_segments(&segs, cols.saturating_sub(flash_w))
        .into_iter()
        .enumerate()
    {
        if i > 0 {
            spans.push(Span::styled(SEP, dim()));
        }
        let style = if seg.accent { Style::default().fg(theme::ACCENT) } else { dim() };
        spans.push(Span::styled(seg.t, style));
    }
    if !flash.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(flash, Style::default().fg(theme::ACTIVE)));
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
    let natural = (widest + 2).clamp(SIDE_MIN, SIDE_MAX);
    natural.min(cols.saturating_sub(SIDE_GAP + BODY_MIN)).max(SIDE_MIN)
}

/// The project panel: a label, then one row per tab. When there are more projects than rows the
/// list scrolls to keep the selected one on screen, and the label says where you are in it.
fn side_panel(app: &App, w: usize, rows: usize) -> Vec<Line<'static>> {
    let body = rows.saturating_sub(1);
    let n = app.tabs.len();
    let off =
        if n <= body { 0 } else { app.p_idx.saturating_sub(body / 2).min(n - body) };
    // The key bar drops `↑↓ project` while the panel is up, so the panel has to carry it.
    let lead = if w >= 14 { " ↑↓ projects" } else { " projects" };
    let label =
        if n > body { format!("{lead} {}/{n}", app.p_idx + 1) } else { lead.to_string() };

    let mut lines = vec![Line::from(Span::styled(fit_width(&label, w), dim()))];
    for r in 0..body {
        let Some(name) = app.tabs.get(off + r) else {
            lines.push(Line::from("")); // hold the column open to the full height
            continue;
        };
        let style = if off + r == app.p_idx { panel_selected() } else { dim() };
        lines.push(Line::from(Span::styled(
            fit_width(&format!(" {}", sanitize(name)), w),
            style,
        )));
    }
    lines
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
            Span::styled(SEARCH_ICON, Style::default().fg(theme::NAMED)),
            Span::styled(" content", Style::default().fg(theme::NAMED)),
        ],
        None => vec![Span::styled(SEARCH_ICON, Style::default().fg(theme::ACCENT))],
    };
    spans.push(Span::raw(" "));
    spans.push(Span::raw(sanitize(&app.q)));
    spans.push(Span::styled("▏", dim()));
    if app.deep.is_some() {
        let plural = if matches == 1 { "" } else { "es" };
        spans.push(Span::styled(
            format!("  {matches} match{plural}"),
            Style::default().fg(theme::NAMED),
        ));
    }
    Line::from(spans)
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
        ("◆", Style::default().fg(theme::NAMED).add_modifier(Modifier::BOLD))
    } else if app.live.contains_key(&it.id) {
        ("◉", Style::default().fg(theme::ACTIVE))
    } else if age < ACTIVE_MS {
        ("●", Style::default().fg(theme::ACTIVE))
    } else if age < RECENT_MS {
        ("●", Style::default().fg(theme::RECENT))
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
        Some(s) => st.bg(s.bg.unwrap_or(theme::TAB_SEL)).add_modifier(Modifier::BOLD),
        None => st,
    };
    let mut spans = Vec::new();
    let (dot, dot_style) = dot_for(app, it);
    if dot != " " {
        spans.push(Span::styled(dot.to_string(), on(dot_style)));
    }
    if it.open {
        spans.push(Span::styled("▸", on(Style::default().fg(theme::NAMED))));
    }
    spans
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

    // A cell is dot + label + the rule that gives the tab an edge. One tab may not take the whole
    // strip, however long its title, or there is nothing left to steer by.
    let cell = |slot: usize| -> (String, usize) {
        let it = &app.items[view[slot]];
        let label = tab_label(it, slot == cur);
        let label = fit_width(&label, UnicodeWidthStr::width(label.as_str()).min(TAB_MAX));
        // label + the rule that closes the tab + whichever status marks it actually carries.
        let w = UnicodeWidthStr::width(label.as_str())
            + 1
            + spans_width(&tab_marks(app, it, None));
        (label, w)
    };

    // Grow outwards from the focused tab, alternating sides, while the row still has room.
    let overflows = (0..n).map(|i| cell(i).1).sum::<usize>() > cols;
    let budget = cols.saturating_sub(if overflows { MARKERS } else { 0 });
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

    let mut spans: Vec<Span<'static>> = Vec::new();
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
    let mut head = vec![Span::styled(title.clone(), Style::default().fg(theme::ACCENT))];
    if let Some(live) = app.live.get(&it.id) {
        let right = format!("◉ running · {}", running_where(live));
        let used = UnicodeWidthStr::width(title.as_str()) + UnicodeWidthStr::width(right.as_str());
        if used + 2 <= w {
            head.push(Span::raw(" ".repeat(w - used)));
            head.push(Span::styled("◉ running", Style::default().fg(theme::ACTIVE)));
            head.push(Span::styled(format!(" · {}", running_where(live)), dim()));
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

    if it.open {
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled("▸ pick up", Style::default().fg(theme::NAMED)),
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
                Style::default().fg(theme::NAMED),
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
            Style::default().fg(theme::REPLY).add_modifier(Modifier::BOLD),
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
            let mut body: Vec<Line> = rl.into_iter().take(reply_max).collect();
            if total > reply_max {
                body.push(Line::from(Span::styled("… ⇥ for full", dim())));
            }
            let ts = detail.and_then(|d| d.reply_ts.as_deref()).map(since).unwrap_or_default();
            thread.extend(gutter_block(
                &format!("reply {ts}"),
                Style::default().fg(theme::REPLY),
                body,
            ));
        }
    }

    lines.extend(recap_block);
    lines.extend(thread);
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
    let key = Style::default().fg(theme::ACCENT);
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
        Line::from(vec![Span::styled("^r", key), Span::raw("     reply to the session without opening it — sends one turn and stays in the list")]),
        Line::from(vec![Span::styled("⇥ ^e", key), Span::raw("   expand / collapse the reply preview")]),
        Line::from(vec![Span::styled("↵", key), Span::raw("      resume (◉ = already running: ↵ says where, ↵ again opens it twice)")]),
        Line::from(vec![Span::styled("^o", key), Span::raw("     resume in this window (replaces sessio)")]),
        Line::from(vec![Span::styled("?", key), Span::raw("      toggle this help")]),
        Line::from(vec![Span::styled("esc", key), Span::raw("    clear search, then quit")]),
        Line::from(vec![Span::styled("^c", key), Span::raw("     quit")]),
        Line::from(""),
        Line::from(vec![
            Span::styled("◉", Style::default().fg(theme::ACTIVE)),
            Span::raw("      a claude process is attached to this session right now"),
        ]),
        Line::from(vec![
            Span::styled("●", Style::default().fg(theme::ACTIVE)),
            Span::raw("      written in the last 5 minutes ("),
            Span::styled("●", Style::default().fg(theme::RECENT)),
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
            Seg { p: 1, t: "↵ new-window", accent: false },
            Seg { p: 2, t: "^o same-window", accent: false },
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
        let projects: Vec<&str> = tabs.iter().copied().filter(|t| *t != ALL_TAB && *t != OPEN_TAB).collect();
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
            flash_until: None,
            deep: None,
            search_gen: 0,
            details: HashMap::new(),
            detail_inflight: HashSet::new(),
            live: crate::live::LiveMap::new(),
            confirm: None,
            draft: None,
            sending: HashSet::new(),
            reply_ok: false,
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

    #[test]
    fn a_wide_terminal_puts_the_projects_down_the_side() {
        let mut app = fixture(&real_tabs());
        let rows = frame(&mut app, 136, 26);
        assert!(rows[0].starts_with(" ↑↓ projects"), "panel is labelled: {:?}", rows[0]);
        // One project per row, in order, down the left edge.
        for (i, name) in real_tabs().iter().enumerate() {
            assert!(
                rows[i + 1].trim_start().starts_with(name),
                "row {} should hold {name}: {:?}",
                i + 1,
                rows[i + 1]
            );
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
        for cols in [40u16, 60, 80, 100] {
            let mut app = fixture(&real_tabs());
            let rows = frame(&mut app, cols, 26);
            assert!(rows[0].starts_with(" "), "{cols} cols: panel label first: {:?}", rows[0]);
            assert!(rows[0].contains("projects"), "{cols} cols: panel is labelled: {:?}", rows[0]);
            for (i, name) in real_tabs().iter().enumerate() {
                let head: String = name.chars().take(3).collect();
                assert!(
                    rows[i + 1].trim_start().starts_with(&head),
                    "{cols} cols: row {} should hold {name}: {:?}",
                    i + 1,
                    rows[i + 1]
                );
            }
        }
    }

    /// On a narrow window the panel gives up its columns before the dashboard beside it does.
    #[test]
    fn the_panel_shrinks_before_the_dashboard_does() {
        let app = fixture(&real_tabs());
        let natural = sidebar(&app, 300);
        for cols in 0..300 {
            let w = sidebar(&app, cols);
            assert!((SIDE_MIN..=SIDE_MAX).contains(&w), "{cols} cols gave a {w}-wide panel");
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
        // Sizes worth eyeballing: override with SESSIO_DUMP_COLS=90,181
        let sizes: Vec<(u16, u16)> = match std::env::var("SESSIO_DUMP_COLS") {
            Ok(v) => v.split(',').filter_map(|c| c.trim().parse().ok()).map(|c| (c, 34)).collect(),
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
            Span::styled("code", Style::default().fg(theme::CODE)),
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
        assert!(narrow.contains("↵ new-window"), "resume is the point: {narrow}");
        assert!(!narrow.contains("^f search-in-text"), "p5 sheds first: {narrow}");
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
            flash_until: None,
            deep: None,
            search_gen: 0,
            details: HashMap::new(),
            detail_inflight: HashSet::new(),
            live: crate::live::LiveMap::new(),
            confirm: None,
            draft: None,
            sending: HashSet::new(),
            reply_ok: false,
            tx,
        }
    }

    /// xterm-256 to CSS. The palette is a formula, not a table: 16 fixed colours, then a
    /// 6×6×6 cube, then a 24-step grey ramp.
    fn css(c: ratatui::style::Color) -> Option<String> {
        use ratatui::style::Color as C;
        let hex = |r: u8, g: u8, b: u8| format!("#{r:02x}{g:02x}{b:02x}");
        let indexed = |i: u8| -> String {
            const BASE: [(u8, u8, u8); 16] = [
                (0, 0, 0), (128, 0, 0), (0, 128, 0), (128, 128, 0),
                (0, 0, 128), (128, 0, 128), (0, 128, 128), (192, 192, 192),
                (128, 128, 128), (255, 0, 0), (0, 255, 0), (255, 255, 0),
                (0, 0, 255), (255, 0, 255), (0, 255, 255), (255, 255, 255),
            ];
            const STEP: [u8; 6] = [0, 95, 135, 175, 215, 255];
            match i {
                0..=15 => { let (r, g, b) = BASE[i as usize]; hex(r, g, b) }
                16..=231 => {
                    let n = i - 16;
                    hex(STEP[(n / 36) as usize], STEP[((n % 36) / 6) as usize], STEP[(n % 6) as usize])
                }
                _ => { let v = 8 + 10 * (i - 232); hex(v, v, v) }
            }
        };
        Some(match c {
            C::Reset => return None,
            C::Rgb(r, g, b) => hex(r, g, b),
            C::Indexed(i) => indexed(i),
            C::Black => indexed(0),
            C::Red => indexed(9),
            C::Green => indexed(10),
            C::Yellow => indexed(11),
            C::Blue => indexed(12),
            C::Magenta => indexed(13),
            C::Cyan => indexed(14),
            C::White => indexed(15),
            C::Gray => indexed(7),
            C::DarkGray => indexed(8),
            C::LightRed => indexed(9),
            C::LightGreen => indexed(10),
            C::LightYellow => indexed(11),
            C::LightBlue => indexed(12),
            C::LightMagenta => indexed(13),
            C::LightCyan => indexed(14),
        })
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
