//! Golden frames: the dashboard's key states, drawn at every supported size and compared as text
//! against `tests/frames/<scene>.txt`.
//!
//! The layout tests in `ui::tests` assert invariants (a region's row, a row's width). These catch
//! everything else a change moves: a hint that vanished, a label that reads differently, a state
//! that stopped saying itself. A difference fails with the first row that changed; when the change
//! is intended, regenerate the files and review the diff like any other:
//!
//! ```sh
//! SESSIO_BLESS=1 cargo test frames
//! git diff tests/frames
//! ```
//!
//! Nothing here touches the machine it runs on. The clock is pinned (`model::TEST_NOW`), and so
//! are ripgrep, Ghostty and the `gh` issues answer (`pin::Env`). The sessions are built in memory:
//! no transcript is read, and no `claude`, `copilot`, `rg`, `gh` or browser is started.

use super::*;
use crate::issues::{Issue, Status};
use crate::parse::{OpenReason, Usage};

/// The pinned clock: 2026-08-29, the demo's.
const NOW: i64 = 1_788_000_000_000;

/// Every supported size, the minimum itself, and one below it on each axis.
const SIZES: &[(usize, usize)] = &[(60, 18), (80, 24), (104, 26), (160, 40), (50, 12), (49, 12), (80, 11)];

/// One session of a scene.
struct S {
    project: &'static str,
    name: &'static str,
    ago_min: i64,
    source: Source,
    open: Option<OpenReason>,
    first: &'static str,
    last: &'static str,
    recap: Option<&'static str>,
    reply: String,
    count: usize,
}

impl S {
    fn new(project: &'static str, name: &'static str, ago_min: i64) -> S {
        S {
            project,
            name,
            ago_min,
            source: Source::Claude,
            open: None,
            first: "look at the failing test",
            last: "go ahead",
            recap: Some("Found the cause and fixed it; the tests pass. Nothing left."),
            reply: "Fixed. The test now passes on both platforms.".into(),
            count: 6,
        }
    }
}

fn item(n: usize, s: &S) -> Item {
    let tokens = (s.source == Source::Claude).then_some(Usage {
        input: 1_200 + n as u64 * 310,
        output: 18_400 + n as u64 * 2_900,
        cache_write: 96_000 + n as u64 * 11_000,
        cache_read: 1_400_000 + n as u64 * 230_000,
    });
    let recap = s.recap.map(str::to_string);
    Item {
        id: format!("{n:04}aaaa-0000-4000-8000-00000000000{}", n % 10),
        key: format!("key{n}"),
        file: PathBuf::from("/nonexistent/sessio-frames.jsonl"),
        mtime: NOW - s.ago_min * 60_000,
        size: 8_000 + n as u64 * 5_300,
        dir: s.project.into(),
        source: s.source,
        first: Some(s.first.into()),
        first_ts: None,
        // Never a real folder: `^g` and git must have nothing to ask about.
        cwd: Some(format!("/nonexistent/code/{}", s.project)),
        branch: Some(if n % 2 == 0 { "main" } else { "feat/limiter" }.into()),
        custom: None,
        ai: Some(s.name.into()),
        open: s.open.is_some(),
        open_reason: s.open,
        recap: recap.clone(),
        recap_ts: None,
        title: Some(s.name.into()),
        name: s.name.into(),
        project: s.project.into(),
        hay: format!("{} {} {}", s.name, s.project, s.first).to_lowercase(),
        detail: Some(Detail {
            count: s.count,
            first: Some(s.first.into()),
            last: Some(s.last.into()),
            reply: Some(s.reply.clone()),
            recap,
            tokens,
            ..Default::default()
        }),
    }
}

fn app_of(sessions: &[S]) -> App {
    let items: Vec<Item> = sessions.iter().enumerate().map(|(n, s)| item(n, s)).collect();
    let (tx, _rx) = mpsc::channel();
    let mut app = App {
        items,
        archive: Archive::default(),
        tabs: Vec::new(),
        q: String::new(),
        cur: 0,
        p_idx: 0,
        expand: false,
        help: false,
        flash: String::new(),
        flash_tone: Tone::Info,
        flash_until: None,
        deep: None,
        text: TextSearch::Idle,
        search_gen: 0,
        details: HashMap::new(),
        detail_inflight: HashSet::new(),
        live: crate::live::LiveMap::new(),
        confirm: None,
        kill_confirm: None,
        draft: None,
        sending: HashMap::new(),
        failed: HashMap::new(),
        reply_ok: false,
        follow: None,
        issues: false,
        issue_cur: 0,
        reply_top: None,
        reply_view: None,
        tx,
    };
    // Every detail is already read, as a refresh would find it cached.
    for it in &app.items {
        if let Some(d) = &it.detail {
            app.details.insert(it.key.clone(), (it.mtime, d.clone()));
        }
    }
    app.tabs = app.tabs_now();
    app
}

/// A reply taller than any preview here, so it scrolls.
fn long_reply() -> String {
    let mut s = String::from("The release workflow, step by step.\n\n## What runs on a tag\n\n");
    for n in 1..=12 {
        s.push_str(&format!("{n}. step {n} of the release, which does one thing and says so\n"));
    }
    s.push_str("\n## Still open\n\n");
    for n in 1..=10 {
        s.push_str(&format!("- open question {n}: whether to sign the binaries before the first report\n"));
    }
    s.push_str("\n```sh\ngit tag v1.2.0-rc.1\ngit push origin v1.2.0-rc.1\n```\n\nWant me to add the signing step?");
    s
}

/// The standard set: a session waiting on you, an unfinished one, a busy one, a long reply, an
/// old one, a Copilot session, an unanswered prompt and an archived session.
fn sessions() -> Vec<S> {
    vec![
        S {
            recap: Some("Found the loop in the expiry middleware. Waiting on permission to run the test suite."),
            reply: "The loop is in `middleware/session.ts`: an expired session redirects to `/login`, \
                    which sits behind the same middleware.\n\nI need to run `npm test` next."
                .into(),
            count: 9,
            ..S::new("web-app", "fix login redirect loop", 1)
        },
        S {
            open: Some(OpenReason::CallToAction),
            first: "where did we get to with the limiter?",
            last: "use seconds, and document it",
            recap: Some("Limiter is in and tested; the Retry-After units need a decision. Your move."),
            reply: "Added a token-bucket limiter; over-limit requests return **429** with a \
                    `Retry-After`.\n\nWant me to wire the same limiter into password reset?"
                .into(),
            count: 14,
            ..S::new("api", "add rate limiting to auth routes", 2)
        },
        S {
            recap: Some("Read-through layer merged; writing the write-behind queue now."),
            reply: "Working on the write-behind queue. The flush on shutdown needs care.".into(),
            count: 22,
            ..S::new("api", "refactor the cache layer", 41)
        },
        S {
            open: Some(OpenReason::Recap),
            first: "write a release workflow that publishes to npm on a tag",
            last: "walk me through what it does before I tag anything",
            recap: Some("Release workflow written and dry-run on a fork. Signing is undecided; the first tag is yours."),
            reply: long_reply(),
            count: 31,
            ..S::new("cli-tool", "ship the release workflow", 180)
        },
        S::new("web-app", "dark mode tokens", 900),
        S {
            source: Source::Copilot,
            first: "why does the macOS test job fail one run in five?",
            last: "pin the runner image",
            recap: Some("The flake is a timing test racing the runner's clock."),
            reply: "Pinned `macos-14`. The timing test is still fragile.".into(),
            ..S::new("cli-tool", "triage flaky CI job", 1400)
        },
        S {
            open: Some(OpenReason::Unanswered),
            last: "and the contributor guide too?",
            recap: None,
            ..S::new("docs", "write onboarding docs", 2600)
        },
        S {
            recap: Some("Spike concluded: not worth it for three services. Archived."),
            ..S::new("docs", "graphql gateway spike", 9000)
        },
    ]
}

fn live(pid: i32, tty: &str, status: &str, waiting_for: &str) -> crate::live::Live {
    crate::live::Live { pid, tty: tty.into(), status: status.into(), waiting_for: waiting_for.into() }
}

/// The standard app: session 0 waiting on you, session 2 busy, session 7 archived.
fn standard() -> App {
    let mut app = app_of(&sessions());
    let (w, b, a) = (app.items[0].clone(), app.items[2].clone(), app.items[7].clone());
    app.live.insert(w.id, live(4242, "ttys009", "waiting", "input needed"));
    app.live.insert(b.id, live(5120, "ttys004", "busy", ""));
    app.archive.toggle(&a.key, &a.id);
    app.tabs = app.tabs_now();
    app
}

fn select_tab(app: &mut App, tab: &str) {
    app.p_idx = app.tabs.iter().position(|t| t == tab).unwrap_or_else(|| panic!("no tab {tab:?} in {:?}", app.tabs));
    app.reset_position();
}

fn select_session(app: &mut App, name: &str) {
    let v = app.view();
    app.cur = v.iter().position(|&i| app.items[i].name == name).unwrap_or_else(|| panic!("{name:?} not in view"));
}

/// Text of each row, trailing blanks dropped.
fn rows(app: &mut App, cols: usize, rows: usize) -> Vec<String> {
    frame_lines(app, cols, rows)
        .into_iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>().trim_end().to_string())
        .collect()
}

type Scene = fn(usize, usize) -> App;

/// Every scene, by the file its frames are kept in.
fn scenes() -> Vec<(&'static str, Scene)> {
    vec![
        ("normal", |_, _| {
            let mut app = standard();
            select_session(&mut app, "dark mode tokens");
            app
        }),
        ("waiting", |_, _| {
            let mut app = standard();
            select_tab(&mut app, WAITING_TAB);
            app
        }),
        ("running", |_, _| {
            let mut app = standard();
            select_session(&mut app, "refactor the cache layer");
            app
        }),
        ("unfinished", |_, _| {
            let mut app = standard();
            select_tab(&mut app, OPEN_TAB);
            select_session(&mut app, "write onboarding docs");
            app
        }),
        ("archived", |_, _| {
            let mut app = standard();
            select_tab(&mut app, ARCHIVED_TAB);
            app
        }),
        ("empty", |_, _| app_of(&[])),
        ("filter", |_, _| {
            let mut app = standard();
            app.q = "cache".into();
            app.requery();
            app
        }),
        ("filter-fuzzy", |_, _| {
            let mut app = standard();
            app.q = "rlt".into();
            app.requery();
            app
        }),
        ("filter-no-match", |_, _| {
            let mut app = standard();
            app.q = "kubernetes".into();
            app.requery();
            app
        }),
        ("search-running", |_, _| {
            let mut app = standard();
            app.q = "limiter".into();
            app.requery();
            app.begin_text_search(true).expect("starts");
            app
        }),
        ("search-failed", |_, _| {
            let mut app = standard();
            app.q = "limiter".into();
            app.requery();
            let (gen, q) = app.begin_text_search(true).expect("starts");
            app.finish_text_search(gen, q, Err("rg exited 2: permission denied".into()), |_| unreachable!());
            app
        }),
        ("search-results", |_, _| {
            let mut app = standard();
            app.q = "limiter".into();
            app.requery();
            let (gen, q) = app.begin_text_search(true).expect("starts");
            let items = app.items.clone();
            let files = items.iter().take(2).map(|it| it.file.clone()).collect();
            // The loader stands in for `model::load`: the sessions the search found, already read.
            app.finish_text_search(gen, q, Ok(files), |_| items);
            let keys = app.items.iter().take(2).map(|it| it.key.clone()).collect();
            app.deep.as_mut().unwrap().keys = keys;
            app
        }),
        ("search-no-match", |_, _| {
            let mut app = standard();
            app.q = "kubernetes".into();
            app.requery();
            let (gen, q) = app.begin_text_search(true).expect("starts");
            let items = app.items.clone();
            app.finish_text_search(gen, q, Ok(HashSet::new()), |_| items);
            app
        }),
        ("reply-scrolled", |cols, rows_| {
            let mut app = standard();
            select_session(&mut app, "ship the release workflow");
            for _ in 0..2 {
                let _ = rows(&mut app, cols, rows_);
                app.scroll_reply(true);
            }
            app
        }),
        ("composer", |_, _| {
            let mut app = standard();
            select_session(&mut app, "dark mode tokens");
            app.reply_ok = true;
            app.begin_reply();
            let id = app.draft.as_ref().unwrap().0.clone();
            let long = "yes — move the remaining colours to tokens too, then check contrast in both themes and \
                        write down anything under 4.5:1 so we can decide per case";
            app.draft = Some((id, long.into()));
            app
        }),
        ("sending", |_, _| {
            let mut app = standard();
            select_session(&mut app, "dark mode tokens");
            let id = app.items[app.selected().unwrap()].id.clone();
            let _job = app.start_sending(&id, "run the tests again").expect("sends");
            app
        }),
        ("reply-failed", |_, _| {
            let mut app = standard();
            select_session(&mut app, "dark mode tokens");
            let (key, id) = {
                let it = &app.items[app.selected().unwrap()];
                (it.key.clone(), it.id.clone())
            };
            let job = app.start_sending(&id, "run the migration").expect("sends");
            app.on_replied(&key, &id, &job.title, job.body, false, "Error: API overloaded, try again later");
            app
        }),
        ("help", |_, _| {
            let mut app = standard();
            app.help = true;
            app
        }),
        ("follow", |_, _| {
            let mut app = standard();
            select_session(&mut app, "refactor the cache layer");
            let it = &app.items[app.selected().unwrap()];
            app.follow = Some(Follow {
                key: it.key.clone(),
                id: it.id.clone(),
                name: it.name.clone(),
                file: it.file.clone(),
                entries: Some(vec![
                    Entry { who: Who::You, text: "go ahead with the write-behind part".into() },
                    Entry { who: Who::Claude, text: "Starting on the queue and its flush.".into() },
                    Entry { who: Who::Tool, text: "Read, Edit, Bash".into() },
                    Entry { who: Who::Claude, text: "The queue is in; the flush on shutdown is next.".into() },
                ]),
                ended: false,
            });
            app.say(Tone::Info, "following \"refactor the cache layer\" — read-only · any move stops it".into());
            app
        }),
        ("issues", |_, _| {
            let mut app = standard();
            select_session(&mut app, "add rate limiting to auth routes");
            // `gh` stubbed: three open issues for this scene only.
            pin::update(|e| e.issues = Some(issues()));
            app.issues = true;
            app
        }),
        ("copilot", |_, _| {
            let mut app = standard();
            select_session(&mut app, "triage flaky CI job");
            app.begin_reply(); // refused: reply is Claude-only
            app
        }),
        ("unicode", |_, _| {
            let mut s = sessions();
            s.truncate(2);
            s.push(S {
                first: "ログインのリダイレクトが止まらない 🔁",
                recap: Some("認証ミドルウェアのループを修正しました。テストは通っています。"),
                reply: "修正しました 🎉\n\n| 列 | 説明 |\n|---|---|\n| 認証 | ミドルウェアを除外 |\n| テスト | 追加済み ✅ |"
                    .into(),
                ..S::new("日本語-プロジェクト-とても長い名前", "修复登录重定向循环 🔁 認証モジュールの長いタイトル", 3)
            });
            s.push(S::new("emoji-🚀-launch", "🚀🚀🚀 ship it 🎉🎉🎉 with a title far too long for any tab", 5));
            let mut app = app_of(&s);
            let n = app.items.len();
            app.items[n - 2].branch = Some("feat/認証-🔁-リダイレクト".into());
            app.items[n - 2].cwd =
                Some("/nonexistent/very/deeply/nested/folder/that/goes/on/日本語-プロジェクト-とても長い名前".into());
            select_session(&mut app, "修复登录重定向循环 🔁 認証モジュールの長いタイトル");
            app
        }),
        ("many-projects", |_, _| {
            let names: Vec<&'static str> = (0..40)
                .map(|n| &*Box::leak(format!("project-{n:02}-with-a-long-name").into_boxed_str()))
                .collect();
            let s: Vec<S> = names.iter().enumerate().map(|(n, &p)| S::new(p, "a session", 10 + n as i64 * 90)).collect();
            let mut app = app_of(&s);
            select_tab(&mut app, "project-31-with-a-long-name");
            app
        }),
        ("refresh-while-reading", |cols, rows_| {
            let mut app = standard();
            select_session(&mut app, "ship the release workflow");
            for _ in 0..2 {
                let _ = rows(&mut app, cols, rows_);
                app.scroll_reply(true);
            }
            refresh(&mut app);
            app
        }),
        ("concurrent-feedback", |_, _| {
            let mut app = standard();
            // A reply on its way to one session…
            select_session(&mut app, "dark mode tokens");
            let id = app.items[app.selected().unwrap()].id.clone();
            let _job = app.start_sending(&id, "run the tests again").expect("sends");
            // …a composer open on another…
            select_session(&mut app, "ship the release workflow");
            app.reply_ok = true;
            app.begin_reply();
            let draft_id = app.draft.as_ref().unwrap().0.clone();
            app.draft = Some((draft_id, "and tag rc.1".into()));
            // …and a notification long enough to take the feedback region's second row.
            app.say(
                Tone::Warning,
                "◆ 2 sessions started waiting on you: \"fix login redirect loop\" (input needed) and \
                 \"refactor the cache layer\" (permission to run npm test)"
                    .into(),
            );
            app
        }),
    ]
}

/// A live refresh as the event loop does it: every session re-listed without its detail, the one
/// being read written to again since.
fn refresh(app: &mut App) {
    let reading = app.selected().map(|i| app.items[i].key.clone());
    let mut fresh = app.items.clone();
    for it in &mut fresh {
        it.detail = None;
        if Some(&it.key) == reading.as_ref() {
            it.mtime += 30_000;
        }
    }
    absorb_items(app, fresh);
}

/// A scene built with nothing stubbed but what it stubs itself.
fn build(scene: Scene, cols: usize, rows_: usize) -> App {
    pin::update(|e| e.issues = None);
    scene(cols, rows_)
}

fn render_scene(scene: Scene) -> String {
    let mut out = String::new();
    for &(cols, rows_) in SIZES {
        let mut app = build(scene, cols, rows_);
        out.push_str(&format!("=== {cols}x{rows_} ===\n"));
        for r in rows(&mut app, cols, rows_) {
            out.push_str(&r);
            out.push('\n');
        }
    }
    out
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/frames")
}

fn pinned() -> (pin::Guard, NowGuard) {
    crate::model::TEST_NOW.with(|t| t.set(Some(NOW)));
    (pin::set(pin::Env { rg: true, ghostty: false, issues: None }), NowGuard)
}

struct NowGuard;
impl Drop for NowGuard {
    fn drop(&mut self) {
        crate::model::TEST_NOW.with(|t| t.set(None));
    }
}

/// What the stubbed `gh` finds for every folder.
fn issues() -> Status {
    let issue = |number: u64, title: &str, labels: &[&str], hours: i64| Issue {
        number,
        title: title.into(),
        url: format!("https://github.com/acme/api/issues/{number}"),
        labels: labels.iter().map(|l| l.to_string()).collect(),
        updated: chrono::DateTime::from_timestamp_millis(NOW - hours * 3_600_000)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    };
    Status::Ready {
        slug: "acme/api".into(),
        issues: vec![
            issue(42, "rate limiter ignores X-Forwarded-For", &["bug"], 3),
            issue(41, "document Retry-After units", &["docs", "good first issue"], 26),
            issue(37, "password reset has no limiter", &[], 120),
        ],
        fetched_ms: NOW - 60_000,
    }
}

#[test]
fn every_scene_matches_its_golden_frames() {
    let _pin = pinned();
    let bless = std::env::var_os("SESSIO_BLESS").is_some_and(|v| v != "0" && !v.is_empty());
    let mut failed = Vec::new();
    for (name, scene) in scenes() {
        let got = render_scene(scene);
        let path = dir().join(format!("{name}.txt"));
        if bless {
            fs_write(&path, &got);
            continue;
        }
        let want = std::fs::read_to_string(&path).unwrap_or_default();
        if got != want {
            let at = got.lines().zip(want.lines()).position(|(g, w)| g != w);
            let (g, w) = match at {
                Some(n) => (got.lines().nth(n).unwrap_or(""), want.lines().nth(n).unwrap_or("")),
                None => ("(length differs)", ""),
            };
            failed.push(format!("{name}: line {}\n    got:  {g:?}\n    want: {w:?}", at.map_or(0, |n| n + 1)));
        }
    }
    assert!(
        failed.is_empty(),
        "frames differ from tests/frames/ — if the change is intended, run \
         `SESSIO_BLESS=1 cargo test frames` and review `git diff tests/frames`:\n{}",
        failed.join("\n")
    );
}

fn fs_write(path: &std::path::Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A golden file whose scene was removed or renamed is a check that no longer runs.
#[test]
fn every_golden_file_has_a_scene() {
    let names: HashSet<String> = scenes().into_iter().map(|(n, _)| format!("{n}.txt")).collect();
    let orphans: Vec<String> = std::fs::read_dir(dir())
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect::<Vec<String>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f.ends_with(".txt") && !names.contains(f))
        .collect();
    assert!(orphans.is_empty(), "tests/frames has files no scene writes: {orphans:?}");
}

/// Every row of every scene fits its terminal, at every size.
#[test]
fn no_scene_overflows_any_size() {
    let _pin = pinned();
    for (name, scene) in scenes() {
        for &(cols, rows_) in SIZES {
            let mut app = build(scene, cols, rows_);
            let lines = frame_lines(&mut app, cols, rows_);
            assert!(lines.len() <= rows_, "{name} at {cols}x{rows_}: {} rows", lines.len());
            for l in &lines {
                let w = spans_width(&l.spans);
                assert!(w <= cols, "{name} at {cols}x{rows_}: a row is {w} wide: {:?}", row_text(l));
            }
        }
    }
}

fn row_text(l: &Line) -> String {
    l.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// A refresh landing while a long reply is scrolled leaves the reader on the same lines.
#[test]
fn a_refresh_keeps_the_reader_on_the_same_lines_in_every_size() {
    let _pin = pinned();
    for &(cols, rows_) in SIZES.iter().filter(|(c, r)| *c >= MIN_COLS && *r >= MIN_ROWS) {
        let mut app = standard();
        select_session(&mut app, "ship the release workflow");
        for _ in 0..2 {
            let _ = rows(&mut app, cols, rows_);
            app.scroll_reply(true);
        }
        let before = rows(&mut app, cols, rows_);
        refresh(&mut app);
        let after = rows(&mut app, cols, rows_);
        let reply = |r: &[String]| r.iter().filter(|l| l.contains("open question") || l.contains("step ")).cloned().collect::<Vec<_>>();
        assert_eq!(reply(&after), reply(&before), "{cols}x{rows_}");
        assert!(!after.join("\n").contains("reading transcript"), "{cols}x{rows_}: blanked");
    }
}

/// Two things said at once — a reply in flight, a draft being written, a notification — each
/// keep their own place: the tab's `⏳`, the composer row, the feedback region.
#[test]
fn concurrent_feedback_keeps_every_message_in_its_place() {
    let _pin = pinned();
    let scene = scenes().into_iter().find(|(n, _)| *n == "concurrent-feedback").unwrap().1;
    for &(cols, rows_) in &[(80, 24), (104, 26), (160, 40)] {
        let mut app = build(scene, cols, rows_);
        let r = rows(&mut app, cols, rows_);
        let text = r.join("\n");
        assert!(text.contains(SENDING_MARK), "{cols}x{rows_}: the in-flight reply is marked\n{text}");
        assert!(text.contains("↳ reply to"), "{cols}x{rows_}: the composer is up\n{text}");
        // The feedback region is the bottom row or two, and nowhere else says the message.
        let said: Vec<usize> = (0..r.len()).filter(|&n| r[n].contains("◆ 2 sessions")).collect();
        assert_eq!(said.len(), 1, "{cols}x{rows_}: said once\n{text}");
        assert!(said[0] >= r.len() - FEEDBACK_MAX, "{cols}x{rows_}: in the feedback region\n{text}");
    }
}
