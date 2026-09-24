//! The website demo and the terminal read keys the same way.
//!
//! `demo::key_on` is the page's key handler; `key_step` is the terminal's, minus the part that
//! starts processes. Both run here on two copies of the demo's own fixture, pressed with the same
//! keys, and must draw the same frame after every key — at the demo's desktop size and at its
//! narrowest. Where the browser deliberately cannot do what the terminal does (`↵`, `^o`, `^n`,
//! sending a reply, `^k`, following, `^g`), the check is instead that the terminal asks for the
//! action and the page says `browser demo: …` in the warning tone.
//!
//! Nothing is launched: `key_step` only returns what `handle_key` would go on to do.

use super::*;

/// The demo's layouts: desktop and the narrowest it lays out for.
const SIZES: &[(usize, usize)] = &[(104, 26), (60, 18)];

fn event(name: &str) -> KeyEvent {
    let none = KeyModifiers::NONE;
    let (code, mods) = match name {
        "ArrowUp" => (KeyCode::Up, none),
        "ArrowDown" => (KeyCode::Down, none),
        "ArrowLeft" => (KeyCode::Left, none),
        "ArrowRight" => (KeyCode::Right, none),
        "Enter" => (KeyCode::Enter, none),
        "Escape" => (KeyCode::Esc, none),
        "PageUp" => (KeyCode::PageUp, none),
        "PageDown" => (KeyCode::PageDown, none),
        "Tab" => (KeyCode::Tab, none),
        "Backspace" => (KeyCode::Backspace, none),
        "M-Backspace" => (KeyCode::Backspace, KeyModifiers::ALT),
        _ if name.starts_with('^') && name.chars().count() == 2 => {
            (KeyCode::Char(name.chars().nth(1).unwrap()), KeyModifiers::CONTROL)
        }
        _ if name.chars().count() == 1 => (KeyCode::Char(name.chars().next().unwrap()), none),
        _ => panic!("no key named {name:?}"),
    };
    KeyEvent::new(code, mods)
}

fn text(app: &mut App, cols: usize, rows: usize) -> Vec<String> {
    frame_lines(app, cols, rows)
        .into_iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect()
}

/// The two handlers side by side, on identical apps.
struct Pair {
    page: App,
    term: App,
    cols: usize,
    rows: usize,
    _pin: pin::Guard,
}

impl Pair {
    fn new(cols: usize, rows: usize) -> Pair {
        // As the page has it: ripgrep is never missing, and there is no Ghostty.
        let pin = pin::set(pin::Env { rg: true, ghostty: false, issues: None });
        let mut p = Pair { page: demo::build(), term: demo::build(), cols, rows, _pin: pin };
        p.same("the first frame");
        p
    }

    /// Press `name` in both. Returns (page quit, terminal step).
    fn press(&mut self, name: &str) -> (bool, Step) {
        // Each side draws first, as it would have before the key arrived: `PgUp`/`PgDn` step
        // from the window the last frame drew.
        let _ = text(&mut self.page, self.cols, self.rows);
        let _ = text(&mut self.term, self.cols, self.rows);
        let quit = demo::key_on(&mut self.page, name);
        let step = key_step(&mut self.term, event(name));
        (quit, step)
    }

    /// Press each key and require the same frame, and the same quit, after every one.
    fn keys(&mut self, names: &[&str]) {
        for &name in names {
            let (quit, step) = self.press(name);
            assert_eq!(quit, step == Step::Quit, "{name}: quit on one side only ({step:?})");
            assert!(
                matches!(step, Step::Continue | Step::Quit),
                "{name}: the terminal asked for {step:?}, which the page does not do — test it with `differs`"
            );
            self.same(name);
        }
    }

    fn same(&mut self, after: &str) {
        let (a, b) = (text(&mut self.page, self.cols, self.rows), text(&mut self.term, self.cols, self.rows));
        if a != b {
            let n = a.iter().zip(&b).position(|(x, y)| x != y).unwrap_or(0);
            panic!(
                "after {after:?} at {}x{} the page and the terminal draw row {n} differently:\n  page: {:?}\n  term: {:?}",
                self.cols, self.rows, a.get(n), b.get(n)
            );
        }
    }

    /// A key the browser cannot carry out: the terminal asks for `want`, the page says so in the
    /// warning tone and changes nothing else.
    fn differs(&mut self, name: &str, want: Step) {
        let before = self.page.q.clone();
        let (quit, step) = self.press(name);
        assert!(!quit, "{name}: the page does not quit");
        assert_eq!(step, want, "{name}: what the terminal goes on to do");
        assert!(self.page.flash.starts_with("browser demo: "), "{name}: {:?}", self.page.flash);
        assert_eq!(self.page.flash_tone, Tone::Warning, "{name}: never the success tone");
        assert_eq!(self.page.q, before, "{name}: nothing typed");
        // Level the two again, so later keys compare like with like.
        self.page.flash.clear();
        self.term.flash.clear();
    }
}

fn each_size(f: impl Fn(&mut Pair)) {
    for &(cols, rows) in SIZES {
        f(&mut Pair::new(cols, rows));
    }
}

#[test]
fn navigation_reads_the_same() {
    each_size(|p| {
        p.keys(&["ArrowDown", "ArrowDown", "ArrowRight", "ArrowRight", "ArrowLeft", "ArrowUp"]);
        p.keys(&["ArrowUp", "ArrowUp", "ArrowUp", "ArrowDown", "ArrowRight", "ArrowRight", "ArrowRight"]);
    });
}

#[test]
fn any_key_closes_help_and_does_nothing_else() {
    each_size(|p| {
        for k in ["x", "ArrowDown", "ArrowRight", "Escape", "Enter", "^r", "^a", "PageDown", "?"] {
            p.keys(&["?"]);
            assert!(p.term.help && p.page.help, "? opens help");
            let q = p.term.q.clone();
            let (tab, cur) = (p.term.p_idx, p.term.cur);
            p.keys(&[k]);
            assert!(!p.term.help && !p.page.help, "{k} closes help");
            assert_eq!((p.term.q.clone(), p.term.p_idx, p.term.cur), (q, tab, cur), "{k} only closed it");
        }
    });
}

#[test]
fn page_keys_scroll_only_the_reply() {
    each_size(|p| {
        // The long reply is in cli-tool; walk to it the way a reader would.
        p.keys(&["r", "e", "l", "e", "a", "s", "e"]);
        p.keys(&["PageDown", "PageDown", "PageDown", "PageUp", "PageDown"]);
        p.keys(&["Tab", "PageDown", "^e", "PageUp"]);
        p.keys(&["^u", "ArrowRight", "PageDown", "ArrowLeft"]);
    });
}

#[test]
fn filtering_and_its_word_keys_read_the_same() {
    each_size(|p| {
        p.keys(&["c", "a", "c", "h", "e", " ", "l", "a", "y"]);
        p.keys(&["Backspace", "M-Backspace", "^w", "x", "^u"]);
        p.keys(&["k", "u", "b", "e", "r", "n", "e", "t", "e", "s"]); // no match: the empty state
        p.keys(&["^u"]);
    });
}

#[test]
fn search_states_and_esc_read_the_same() {
    each_size(|p| {
        // ^f with nothing typed asks for a word on both.
        p.keys(&["^f"]);
        // Results in hand (the page finds them itself; the terminal's come from ripgrep, so
        // they are handed to it here): esc goes back to filtering, a second esc quits.
        p.keys(&["l", "i", "m", "i", "t", "e", "r"]);
        let _ = demo::key_on(&mut p.page, "^f");
        let keys = p.page.deep.as_ref().expect("the page searched").keys.clone();
        let step = key_step(&mut p.term, event("^f"));
        let Step::TextSearch { gen, query } = step else { panic!("^f asks for a search: {step:?}") };
        let items = p.term.items.clone();
        p.term.finish_text_search(gen, query, Ok(HashSet::new()), |_| items);
        p.term.deep.as_mut().unwrap().keys = keys;
        p.same("^f with results");
        assert!(p.term.in_text_search() && p.page.in_text_search());
        p.keys(&["ArrowRight", "ArrowDown", "ArrowUp"]);
        p.keys(&["Escape"]);
        assert!(!p.term.in_text_search() && !p.page.in_text_search(), "esc leaves text search");
        assert_eq!(p.term.q, "limiter", "and keeps the query to filter by");
        p.keys(&["^u", "Escape"]); // and now esc quits, on both
    });
}

#[test]
fn the_composer_owns_the_keyboard_on_both() {
    each_size(|p| {
        // The first session is waiting (running): both refuse with where it runs.
        p.keys(&["^r"]);
        assert!(p.term.draft.is_none());
        // A session nobody runs: the token warning, then the composer.
        p.keys(&["ArrowRight", "ArrowRight", "ArrowRight", "^r", "^r"]);
        assert!(p.term.draft.is_some() && p.page.draft.is_some(), "the composer is open");
        let (tab, cur) = (p.term.p_idx, p.term.cur);
        p.keys(&["y", "e", "s", " ", "g", "o", "?", "ArrowDown", "ArrowRight", "Tab", "PageDown", "^a"]);
        assert_eq!((p.term.p_idx, p.term.cur), (tab, cur), "nothing behind the composer moved");
        assert!(!p.term.help, "? typed rather than opened help");
        p.keys(&["Backspace", "M-Backspace", "^w", "t", "e", "s", "t", "s", "^u", "a", "b"]);
        p.keys(&["Escape"]); // discarded, on both
        assert!(p.term.draft.is_none() && p.page.draft.is_none());
        p.keys(&["^r", "Enter"]); // an empty composer sends nothing
        p.keys(&["^r", "o", "k"]);
        let id = p.term.draft.as_ref().unwrap().0.clone();
        p.differs("Enter", Step::Send { id, body: "ok".into() });
    });
}

#[test]
fn archive_reads_the_same_both_ways() {
    each_size(|p| {
        p.keys(&["ArrowRight", "^a"]);
        p.keys(&["ArrowDown", "ArrowDown", "ArrowDown", "ArrowDown", "ArrowDown", "ArrowDown"]);
        p.keys(&["ArrowUp", "ArrowUp", "ArrowUp", "ArrowUp", "ArrowUp", "ArrowUp"]);
        p.keys(&["ArrowUp", "^a", "ArrowDown"]);
    });
}

#[test]
fn what_the_browser_cannot_do_it_says_and_the_terminal_does() {
    each_size(|p| {
        p.differs("Enter", Step::Resume { new_window: false });
        p.differs("^o", Step::Resume { new_window: true });
        p.differs("^n", Step::NewSession);
        p.keys(&["^c"]);
    });
}
