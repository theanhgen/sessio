//! The dashboard. Port of the picker, preview and event loop (bin/sessio.mjs:360-782).

use std::collections::{HashMap, HashSet};
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
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
}

const REFRESH: Duration = Duration::from_secs(2);
/// How long a flash stays on screen. It has to be a duration, not a frame: this loop redraws on
/// every 120ms input poll, and a message cleared after one frame — as the JS reference does, where
/// a frame is a keypress or the 2s tick — was gone before anyone could read it.
const FLASH: Duration = Duration::from_secs(5);
const MIN_LIST: usize = 3;
/// Rows the preview keeps whatever the list wants, so ↓ can never squeeze it to nothing.
const MIN_PREVIEW: usize = 8;
const PAGE: usize = 12;

fn dim() -> Style {
    Style::default().fg(theme::DIM)
}

enum Msg {
    Items(Vec<Item>, crate::live::LiveMap),
    Detail { key: String, mtime: i64, detail: Box<Detail> },
    Search { gen: u64, query: String, files: Option<HashSet<PathBuf>> },
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
    off: usize,
    p_idx: usize,
    limit: usize,
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
    tx: Sender<Msg>,
}

impl App {
    /// Say something back about the key just pressed, and keep saying it long enough to be read.
    fn say(&mut self, msg: String) {
        self.flash = msg;
        self.flash_until = Some(Instant::now() + FLASH);
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
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let d = crate::parse::detail(&file);
            let _ = tx.send(Msg::Detail { key, mtime, detail: Box::new(d) });
        });
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
        self.off = 0;
        self.limit = PAGE;
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
    let mut app = App {
        items,
        archive,
        tabs,
        q: String::new(),
        cur: 0,
        off: 0,
        p_idx: 0,
        limit: PAGE,
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
        tx: tx.clone(),
    };

    let mut term = setup()?;
    let result = event_loop(&mut term, &mut app, &rx, &tx);
    restore(&mut term);
    result
}

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

fn restore(term: &mut Terminal<CrosstermBackend<Stdout>>) {
    let _ = disable_raw_mode();
    let _ = execute!(term.backend_mut(), LeaveAlternateScreen);
    let _ = term.show_cursor();
}

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
            }
        }
    }
}

/// Swap in a refreshed list, preserving the highlighted session, active tab and search.
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

enum Flow {
    Continue,
    Quit,
}

fn handle_key(
    term: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    k: KeyEvent,
) -> io::Result<Flow> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);

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
                app.off = 0;
                app.ensure_detail();
            }
        }
        KeyCode::Tab => app.expand = !app.expand,
        KeyCode::Char('e') if ctrl => app.expand = !app.expand,
        KeyCode::Up => {
            app.cur = app.cur.saturating_sub(1);
            app.ensure_detail();
        }
        KeyCode::Down => {
            let len = app.view().len();
            if app.cur + 1 < len {
                app.cur += 1;
                if app.cur >= app.limit {
                    app.limit += PAGE; // reveal more
                }
            }
            app.ensure_detail();
        }
        KeyCode::Left => {
            if !app.tabs.is_empty() {
                app.p_idx = (app.p_idx + app.tabs.len() - 1) % app.tabs.len();
            }
            app.reset_position();
            app.ensure_detail();
        }
        KeyCode::Right => {
            if !app.tabs.is_empty() {
                app.p_idx = (app.p_idx + 1) % app.tabs.len();
            }
            app.reset_position();
            app.ensure_detail();
        }
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
                    if resume::focus_window_titled(&name) {
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
fn drop_word(q: &str) -> usize {
    let sep = |c: char| c.is_whitespace() || matches!(c, '/' | '-' | '_' | '.' | ':' | ',');
    let trimmed = q.trim_end_matches(sep);
    match trimmed.rfind(sep) {
        Some(i) => i + trimmed[i..].chars().next().map_or(1, char::len_utf8),
        None => 0,
    }
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
fn running_where(live: &crate::live::Live) -> String {
    if live.tty.is_empty() {
        format!("pid {}", live.pid)
    } else {
        format!("pid {} · {}", live.pid, live.tty)
    }
}

/// Replace sessio with `claude --resume`. The terminal is restored *first* — `exec` never
/// returns, so there is no later opportunity to undo raw mode.
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

fn draw(f: &mut ratatui::Frame, app: &mut App) {
    let area = f.area();
    let cols = area.width as usize;
    let rows = area.height as usize;

    if app.help {
        f.render_widget(Paragraph::new(help_lines()), area);
        return;
    }

    let view = app.view();
    if app.cur >= view.len() {
        app.cur = view.len().saturating_sub(1);
    }
    let sel = view.get(app.cur).copied();

    let tabs = tab_bar(app, cols);

    // The split is decided by the terminal, never by what is in the tab or what the highlighted
    // session's preview happens to contain. Deriving it from content — the number of rows this
    // tab holds, how long this reply is — moved the separator and everything under it every time
    // you pressed ←/→, which reads as the UI wandering on its own.
    let chrome = 1 + tabs.len() + 1; // header, tab bar, query
    let body = rows.saturating_sub(chrome + 1); // and the always-present "↓ more" row
    let ceiling = body.saturating_sub(MIN_PREVIEW).max(MIN_LIST);
    let want = if app.expand { MIN_LIST } else { app.limit };
    let slots = want.clamp(MIN_LIST, ceiling).min(body.max(1));
    app.limit = app.limit.min(ceiling); // ↓ may not grow the list past its share

    let preview_box = body.saturating_sub(slots);
    let base = sel.map(|i| preview(app, &app.items[i], cols, 0).len()).unwrap_or(0);
    let reply_max = preview_box.saturating_sub(base).max(1);
    let prev = sel.map(|i| preview(app, &app.items[i], cols, reply_max)).unwrap_or_default();

    if app.cur < app.off {
        app.off = app.cur;
    }
    if slots > 0 && app.cur >= app.off + slots {
        app.off = app.cur - slots + 1;
    }

    let mut lines: Vec<Line> = Vec::with_capacity(rows + 4);
    lines.push(header(app, cols));
    lines.extend(tabs);
    lines.push(query_line(app, view.len()));

    let mut shown = 0;
    for (n, &i) in view.iter().skip(app.off).take(slots).enumerate() {
        lines.push(row_line(app, &app.items[i], app.off + n == app.cur, cols));
        shown += 1;
    }
    if shown == 0 {
        lines.push(Line::from(Span::styled("  no match", dim())));
        shown = 1;
    }
    for _ in shown..slots {
        lines.push(Line::from("")); // hold the row open so nothing below it shifts
    }
    // The show-more row is always reserved, for the same reason.
    let below = view.len() as i64 - (app.off + slots) as i64;
    lines.push(if below > 0 {
        Line::from(Span::styled(
            format!(" ↓ {below} more — press ↓ to reveal"),
            dim(),
        ))
    } else {
        Line::from("")
    });
    lines.extend(prev);

    f.render_widget(Paragraph::new(lines), area);
}

/// One hint in the key bar. `p` is how expendable it is: the bar sheds the highest `p` first.
struct Seg {
    p: u8,
    t: &'static str,
    accent: bool,
}

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
    // The full bar is ~136 columns under Ghostty. Anything narrower would be clipped mid-word by
    // the paragraph, so drop the least essential hints instead. `? help` is p0 — it reveals
    // everything that was dropped — and the resume key is p1 because it is the whole point.
    let mut segs: Vec<Seg> = vec![
        Seg { p: 3, t: "←→ project", accent: false },
        Seg { p: 3, t: "↑↓ move", accent: false },
        Seg { p: 4, t: "type", accent: false },
    ];
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

fn tab_bar(app: &App, cols: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut cur: Vec<Span> = Vec::new();
    let mut w = 0usize;
    for (i, p) in app.tabs.iter().enumerate() {
        let label = sanitize(p);
        let vis = UnicodeWidthStr::width(label.as_str()) + 2;
        if w + vis > cols && !cur.is_empty() {
            lines.push(Line::from(std::mem::take(&mut cur)));
            w = 0;
        }
        let style = if i == app.p_idx {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            dim()
        };
        cur.push(Span::styled(format!(" {label} "), style));
        w += vis;
    }
    if !cur.is_empty() {
        lines.push(Line::from(cur));
    }
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines
}

fn query_line(app: &App, matches: usize) -> Line<'static> {
    let mut spans = match &app.deep {
        Some(_) => vec![Span::styled("content›", Style::default().fg(theme::NAMED))],
        None => vec![Span::styled("›", Style::default().fg(theme::ACCENT))],
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

fn row_line(app: &App, it: &Item, selected: bool, cols: usize) -> Line<'static> {
    let age = model::now_ms() - it.mtime;
    // A filled ring means a `claude` process is attached right now, which is a stronger claim
    // than the recency dot: the transcript was written recently vs. the session is *open*.
    let (dot, dot_style) = if app.live.contains_key(&it.id) {
        ("◉", Style::default().fg(theme::ACTIVE))
    } else if age < ACTIVE_MS {
        ("●", Style::default().fg(theme::ACTIVE))
    } else if age < RECENT_MS {
        ("●", Style::default().fg(theme::RECENT))
    } else {
        (" ", Style::default())
    };
    let open_marker = if it.open { "▸" } else { " " };
    let meta = format!(
        "{} · {} ago{}",
        sanitize(&it.project),
        ago(it.mtime),
        it.branch.as_deref().map(|b| format!(" · {}", sanitize(b))).unwrap_or_default()
    );
    let name = sanitize(it.display_name());

    let mut spans = vec![
        Span::styled(dot.to_string(), dot_style),
        Span::styled(open_marker.to_string(), Style::default().fg(theme::NAMED)),
    ];
    if selected {
        let bar = fit_width(&format!("{name}  {meta} "), cols.saturating_sub(2));
        spans.push(Span::styled(bar, Style::default().add_modifier(Modifier::REVERSED)));
    } else {
        spans.push(Span::raw(name));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(meta, dim()));
    }
    Line::from(spans)
}

fn preview(app: &App, it: &Item, width: usize, reply_max: usize) -> Vec<Line<'static>> {
    let w = width.max(1);
    let mut lines: Vec<Line> = vec![Line::from(Span::styled("─".repeat(w), dim()))];

    let kind = if it.custom.is_some() {
        Span::styled("named", Style::default().fg(theme::NAMED))
    } else if it.ai.is_some() {
        Span::styled("auto-named", dim())
    } else {
        Span::styled("unnamed", dim())
    };
    lines.push(Line::from(vec![
        Span::styled(sanitize(it.display_name()), Style::default().fg(theme::ACCENT)),
        Span::raw("  "),
        kind,
    ]));

    let prompts = match it.prompt_count() {
        Some(c) => format!(" · {c} prompt{}", if c == 1 { "" } else { "s" }),
        None => String::new(),
    };
    lines.push(Line::from(Span::styled(
        format!(
            "{} · {} ago{prompts} · {}{}",
            sanitize(&it.project),
            ago(it.mtime),
            size_fmt(it.size),
            it.branch.as_deref().map(|b| format!(" · {}", sanitize(b))).unwrap_or_default()
        ),
        dim(),
    )));

    if it.open {
        lines.push(Line::from(vec![
            Span::styled("▸ pick up", Style::default().fg(theme::NAMED)),
            Span::styled(
                format!(" · {}", it.open_why().unwrap_or("unfinished")),
                dim(),
            ),
        ]));
    }
    if app.archived(it) {
        lines.push(Line::from(Span::styled(
            "🗄 archived · hidden from other tabs · ^a to unarchive",
            dim(),
        )));
    }
    if let Some(d) = &app.deep {
        lines.push(Line::from(Span::styled(
            format!("✓ contains \"{}\"", sanitize(&d.query)),
            Style::default().fg(theme::NAMED),
        )));
    }

    if let Some(live) = app.live.get(&it.id) {
        let where_ = if live.tty.is_empty() {
            format!("pid {}", live.pid)
        } else {
            format!("pid {} · {}", live.pid, live.tty)
        };
        lines.push(Line::from(vec![
            Span::styled("◉ running", Style::default().fg(theme::ACTIVE)),
            Span::styled(format!(" · {where_} · ↵ goes to it"), dim()),
        ]));
    }

    let detail = it.detail.as_ref();

    // The recap is newer, shorter and says whose move it is — prefer it over the compact
    // summary. The full read supersedes the tail read once it lands.
    let recap = detail
        .and_then(|d| d.recap.as_deref())
        .or(it.recap.as_deref());
    let recap_ts = detail
        .and_then(|d| d.recap_ts.as_deref())
        .or(it.recap_ts.as_deref());

    if let Some(recap) = recap {
        let mark = recap_ts
            .map(|t| format!(" · {}{}", stamp(t), rel(t)))
            .unwrap_or_default();
        let mut spans = vec![Span::styled("recap", Style::default().fg(theme::REPLY))];
        if !mark.is_empty() {
            spans.push(Span::styled(mark, dim()));
        }
        lines.push(Line::from(spans));
        for l in md_lines(recap, w.saturating_sub(2)).into_iter().take(4) {
            lines.push(quote(l));
        }
    } else if let Some(summary) = detail.and_then(|d| d.summary.as_ref()) {
        let stamp = detail
            .and_then(|d| d.summary_ts.as_deref())
            .map(|t| format!(" · {}{}", stamp(t), rel(t)))
            .unwrap_or_default();
        lines.push(Line::from(Span::styled(format!("summary{stamp}"), dim())));
        for l in md_lines(summary, w.saturating_sub(2)).into_iter().take(4) {
            lines.push(quote(l));
        }
    }

    lines.push(Line::from(Span::styled(
        format!(
            "first · {}{}",
            it.first_ts.as_deref().map(stamp).unwrap_or_default(),
            it.first_ts.as_deref().map(rel).unwrap_or_default()
        ),
        dim(),
    )));
    for l in wrap_plain(it.first.as_deref().unwrap_or(""), w, 2) {
        lines.push(Line::from(l));
    }

    if detail.is_none() {
        lines.push(Line::from(Span::styled("…", dim()))); // detail still loading
    }

    if let Some(d) = detail {
        if d.count > 1 {
            let stamp_s = d
                .last_ts
                .as_deref()
                .map(|t| format!("{}{}", stamp(t), rel(t)))
                .unwrap_or_default();
            lines.push(Line::from(Span::styled(format!("last · {stamp_s}"), dim())));
            for l in wrap_plain(d.last.as_deref().unwrap_or(""), w, 2) {
                lines.push(Line::from(l));
            }
        }
        if let Some(reply) = &d.reply {
            if reply_max > 0 {
                let stamp_s = d
                    .reply_ts
                    .as_deref()
                    .map(|t| format!("{}{}", stamp(t), rel(t)))
                    .unwrap_or_default();
                lines.push(Line::from(vec![
                    Span::styled("reply", Style::default().fg(theme::REPLY)),
                    Span::styled(format!(" · {stamp_s}"), dim()),
                ]));
                let rl = md_lines(reply, w.saturating_sub(2));
                let total = rl.len();
                for l in rl.into_iter().take(reply_max) {
                    lines.push(quote(l));
                }
                if total > reply_max {
                    lines.push(quote(Line::from(Span::styled("… ⇥ for full", dim()))));
                }
            }
        }
    }
    lines
}

/// Blockquote gutter, marking rendered markdown apart from plain prompt text.
fn quote(l: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::styled("│", dim()), Span::raw(" ")];
    spans.extend(l.spans);
    Line::from(spans)
}

fn help_lines() -> Vec<Line<'static>> {
    let key = Style::default().fg(theme::ACCENT);
    let mut v = vec![
        Line::from(vec![
            Span::styled("sessio", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(" — keys", dim()),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled("← →", key), Span::raw("    switch project tab")]),
        Line::from(vec![Span::styled("↑ ↓", key), Span::raw("    move selection (↓ reveals more)")]),
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
        Line::from(vec![Span::styled("⇥ ^e", key), Span::raw("   expand / collapse the reply preview")]),
        Line::from(vec![Span::styled("↵", key), Span::raw("      resume — or go to it if ◉ (Ghostty: new window, sessio stays open)")]),
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

fn ago(ms: i64) -> String {
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

/// `12 Aug 09:31` in local time.
fn stamp(iso: &str) -> String {
    use chrono::{DateTime, Local};
    match iso.parse::<DateTime<chrono::Utc>>() {
        Ok(t) => t.with_timezone(&Local).format("%-d %b %H:%M").to_string(),
        Err(_) => String::new(),
    }
}

/// The bracketed relative age beside an absolute timestamp: ` [5m]`.
/// The JS prints no "ago" here — the word appears only in the metadata line.
fn rel(iso: &str) -> String {
    match crate::parse::parse_iso_ms(iso) {
        Some(ms) => format!(" [{}]", ago(ms)),
        None => String::new(),
    }
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
    let clean = sanitize(text);
    let words: Vec<&str> = clean.split_whitespace().collect();
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for w in &words {
        let cand = if cur.is_empty() { w.to_string() } else { format!("{cur} {w}") };
        if UnicodeWidthStr::width(cand.as_str()) > width {
            lines.push(std::mem::take(&mut cur));
            cur = w.to_string();
        } else {
            cur = cand;
        }
        if lines.len() >= max_lines {
            break;
        }
    }
    if !cur.is_empty() && lines.len() < max_lines {
        lines.push(cur);
    }
    let full = words.join(" ");
    if lines.len() == max_lines && full.len() > lines.join(" ").len() {
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
            Seg { p: 3, t: "←→ project", accent: false },
            Seg { p: 3, t: "↑↓ move", accent: false },
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
    #[test]
    fn the_key_bar_never_exceeds_the_terminal_width() {
        for cols in 8..200 {
            let w = UnicodeWidthStr::width(rendered(cols).as_str());
            assert!(w <= cols, "{cols} cols rendered {w}");
        }
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
        let l = crate::live::Live { pid: 68227, tty: "ttys013".into() };
        assert_eq!(running_where(&l), "pid 68227 · ttys013");
        let headless = crate::live::Live { pid: 7, tty: String::new() };
        assert_eq!(running_where(&headless), "pid 7");
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
