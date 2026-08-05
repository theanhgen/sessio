//! sessio — find and resume past Claude Code sessions.

mod cta;
mod discover;
mod git;
mod md;
mod model;
mod parse;
mod rank;
mod resume;
mod safety;
mod search;
mod store;
mod ui;

use model::Item;

/// The list row as the oracle compares it. Field names mirror the JS dump exactly.
#[derive(serde::Serialize)]
struct DumpRow<'a> {
    key: &'a str,
    id: &'a str,
    dir: &'a str,
    project: &'a str,
    mtime: i64,
    size: u64,
    name: Option<&'a str>,
    title: Option<&'a str>,
    custom: Option<&'a str>,
    ai: Option<&'a str>,
    first: Option<&'a str>,
    #[serde(rename = "firstTs")]
    first_ts: Option<&'a str>,
    cwd: Option<&'a str>,
    branch: Option<&'a str>,
    open: bool,
    #[serde(rename = "openWhy")]
    open_why: Option<&'a str>,
    hay: Option<&'a str>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return;
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("sessio {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.iter().any(|a| a == "--update") {
        // Updates are handled by the npm launcher, which knows how sessio was installed.
        // A self-updating binary can't tell a cargo install from a brew tap from an npm global.
        eprintln!("Run `npm i -g sessio` to update, or `cargo install --git <repo>` for a source build.");
        std::process::exit(1);
    }
    if args.iter().any(|a| a == "--dump-json") {
        let items = model::load(&[]);
        let dump: Vec<DumpRow> = items.iter().map(to_dump).collect();
        println!("{}", serde_json::to_string(&dump).expect("plain data"));
        return;
    }

    if let Err(e) = ui::run() {
        eprintln!("sessions error: {e}");
        std::process::exit(1);
    }
}

fn print_usage() {
    println!(
        "sessio {} — find and resume past Claude Code sessions.

USAGE:
  sessions                 browse and resume
  sessions --update        update instructions for this install
  sessions --dump-json     print the computed session list (oracle harness)
  sessions --version

KEYS:
  ←/→ project · ↑/↓ move · type to filter · ^f search-in-text
  ^a archive · ⇥ expand-reply · ↵ resume · ^o same-window · ? help · esc quit",
        env!("CARGO_PKG_VERSION")
    );
}

fn to_dump(it: &Item) -> DumpRow<'_> {
    DumpRow {
        key: &it.key,
        id: &it.id,
        dir: &it.dir,
        project: &it.project,
        mtime: it.mtime,
        size: it.size,
        name: Some(&it.name),
        title: it.title.as_deref(),
        custom: it.custom.as_deref(),
        ai: it.ai.as_deref(),
        first: it.first.as_deref(),
        first_ts: it.first_ts.as_deref(),
        cwd: it.cwd.as_deref(),
        branch: it.branch.as_deref(),
        open: it.open,
        open_why: it.open_why.as_deref(),
        hay: Some(&it.hay),
    }
}
