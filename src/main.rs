//! sessio — find and resume past coding-agent sessions (Claude Code, GitHub Copilot CLI).

mod cli;

use sessio::{model, ui};
use sessio::model::Item;

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

    // A subcommand is the non-interactive CLI; flags alone (or nothing) keep the old behaviour.
    // Checked first, so a `--help` inside a reply's message is the message, not a request.
    if let Some(cmd) = args.first().filter(|a| !a.starts_with('-')) {
        std::process::exit(cli::run(cmd, &args[1..]));
    }

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
        let items = model::load_claude(&[]);
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
    print!("{}", cli::usage());
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
        open_why: it.open_why(),
        hay: Some(&it.hay),
    }
}
