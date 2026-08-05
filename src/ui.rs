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
const MIN_LIST: usize = 3;
const DEFAULT_LIST: usize = 6;
const PAGE: usize = 12;

fn dim() -> Style {
    Style::default().fg(theme::DIM)
}

enum Msg {
    Items(Vec<Item>),
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
    deep: Option<Deep>,
    search_gen: u64,
    details: HashMap<String, (i64, Detail)>,
    detail_inflight: HashSet<String>,
    tx: Sender<Msg>,
}

impl App {
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
    let archive = Archive::load();
    let items = model::load(&[]);
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
        deep: None,
        search_gen: 0,
        details: HashMap::new(),
        detail_inflight: HashSet::new(),
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
        term.draw(|f| draw(f, app))?;
        app.flash.clear(); // shown for one frame only

        // Live refresh: rescan every 2s on a worker so input never stalls behind the scan.
        if !refreshing && last_refresh.elapsed() >= REFRESH {
            refreshing = true;
            last_refresh = Instant::now();
            let tx2 = tx.clone();
            let extra = app.deep.as_ref().map(|d| d.files.clone()).unwrap_or_default();
            std::thread::spawn(move || {
                let items = model::load(&extra);
                let _ = tx2.send(Msg::Items(items));
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
                Msg::Items(new_items) => {
                    refreshing = false;
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
                        None => app.flash = "content search failed".into(),
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
                app.flash = "searching…".into();
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
        KeyCode::Backspace => {
            app.q.pop();
            app.deep = None;
            app.search_gen += 1;
            app.reset_position();
            app.ensure_detail();
        }
        KeyCode::Enter => {
            if let Some(i) = app.selected() {
                let (cwd, id, name) = (
                    app.items[i].cwd.clone(),
                    app.items[i].id.clone(),
                    app.items[i].name.clone(),
                );
                // Ghostty: open in a NEW window and keep sessio running as a launcher.
                if resume::in_ghostty() {
                    if let Some(dir) = cwd.as_deref() {
                        if resume::ghostty_launch(std::path::Path::new(dir), &id) {
                            let short: String = name.chars().take(40).collect();
                            app.flash = format!("↗ opened \"{short}\" in a new window");
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
            app.deep = None;
            app.search_gen += 1;
            app.reset_position();
            app.ensure_detail();
        }
        _ => {}
    }
    Ok(Flow::Continue)
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
    // Reply is capped so the preview never scrolls the frame off; ⇥ expands it to fill,
    // keeping at least MIN_LIST rows of list.
    let base = sel.map(|i| preview(app, &app.items[i], cols, 0).len()).unwrap_or(0);
    let reserved = if app.expand { MIN_LIST } else { DEFAULT_LIST };
    let budget = rows as i64
        - (1 + tabs.len() as i64 + 1 + 1)
        - base as i64
        - 2
        - reserved as i64;
    let reply_max = budget.max(1) as usize;
    let prev = sel.map(|i| preview(app, &app.items[i], cols, reply_max)).unwrap_or_default();

    let overhead = 1 + tabs.len() + 1 + prev.len() + 1;
    let max_fit = rows.saturating_sub(overhead).max(3);
    if app.limit > max_fit {
        app.limit = max_fit;
    }
    // Never hide an ACTIVE (green) session behind "show more".
    let now = model::now_ms();
    let active_count = view.iter().filter(|&&i| now - app.items[i].mtime < ACTIVE_MS).count();
    let win = app.limit.max(active_count).min(max_fit).min(view.len());
    if app.cur < app.off {
        app.off = app.cur;
    }
    if win > 0 && app.cur >= app.off + win {
        app.off = app.cur - win + 1;
    }

    let mut lines: Vec<Line> = Vec::with_capacity(rows + 4);
    lines.push(header(app));
    lines.extend(tabs);
    lines.push(query_line(app, view.len()));

    for (n, &i) in view.iter().skip(app.off).take(win).enumerate() {
        lines.push(row_line(app, &app.items[i], app.off + n == app.cur, cols));
    }
    let below = view.len() as i64 - (app.off + win) as i64;
    if below > 0 {
        lines.push(Line::from(Span::styled(
            format!(" ↓ {below} more — press ↓ to reveal"),
            dim(),
        )));
    }
    lines.extend(prev);

    f.render_widget(Paragraph::new(lines), area);
}

fn header(app: &App) -> Line<'static> {
    let hint = if search::rg_path().is_some() { "^f search-in-text · " } else { "" };
    let arch = if app.tabs.get(app.p_idx).map(String::as_str) == Some(ARCHIVED_TAB) {
        "^a unarchive · "
    } else {
        "^a archive · "
    };
    let exp = if app.expand { "⇥ collapse · " } else { "⇥ expand-reply · " };
    let res = if resume::in_ghostty() { "↵ new-window · ^o same-window · " } else { "↵ resume · " };
    let mut spans = vec![Span::styled(
        format!("←→ project · ↑↓ move · type · {hint}{arch}{exp}{res}? help · esc quit · "),
        dim(),
    )];
    spans.push(Span::styled("live", Style::default().fg(theme::ACCENT)));
    if !app.flash.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            sanitize(&app.flash),
            Style::default().fg(theme::ACTIVE),
        ));
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
    let (dot, dot_style) = if age < ACTIVE_MS {
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
    let _ = app;

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
                format!(" · {}", it.open_why.as_deref().unwrap_or("unfinished")),
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

    let detail = it.detail.as_ref();

    if let Some(summary) = detail.and_then(|d| d.summary.as_ref()) {
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
        Line::from(vec![Span::styled("^a", key), Span::raw("     archive / unarchive the selected session")]),
        Line::from(vec![Span::styled("⇥ ^e", key), Span::raw("   expand / collapse the reply preview")]),
        Line::from(vec![Span::styled("↵", key), Span::raw("      resume (Ghostty: new window, sessio stays open; else in place)")]),
        Line::from(vec![Span::styled("^o", key), Span::raw("     resume in this window (replaces sessio)")]),
        Line::from(vec![Span::styled("?", key), Span::raw("      toggle this help")]),
        Line::from(vec![Span::styled("esc", key), Span::raw("    clear search, then quit")]),
        Line::from(vec![Span::styled("^c", key), Span::raw("     quit")]),
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
}
