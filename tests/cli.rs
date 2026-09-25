//! The CLI end to end, against a fixture `CLAUDE_CONFIG_DIR` — never the real one, since
//! `archive` writes the archive list and `reply` spends tokens.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

const A: &str = "aaaa1111-0000-4000-8000-000000000001";
const B: &str = "aaaa2222-0000-4000-8000-000000000002";
const C: &str = "cccc3333-0000-4000-8000-000000000003";

/// Three sessions in two projects, under a throwaway home:
///
/// * A — `alpha`, newest, titled, answered.
/// * B — `beta`, whose folder is a git repo with uncommitted changes.
/// * C — `alpha`, oldest, with a prompt Claude never answered.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let raw = std::env::temp_dir().join(format!("sessio-cli-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&raw);
        fs::create_dir_all(&raw).unwrap();
        // Canonical, because the binary sees the cwd through getcwd: /private/var, not /var.
        let root = raw.canonicalize().unwrap();
        let f = Fixture { root };
        for p in ["alpha/sub", "beta"] {
            fs::create_dir_all(f.work(p)).unwrap();
        }
        let git = Command::new("git").args(["-C"]).arg(f.work("beta")).args(["init", "-q"]).status();
        assert!(matches!(git, Ok(s) if s.success()), "git init failed");
        fs::write(f.work("beta").join("wip.txt"), "unsaved").unwrap();

        let (alpha, beta) = (f.work("alpha"), f.work("beta"));
        f.transcript(A, &alpha, 60, &[
            json!({"type": "ai-title", "aiTitle": "Refactor the parser"}),
            user("please refactor\nthe parser", &alpha, "2026-09-01T10:00:00.000Z"),
            assistant("Done. The needle-in-body word is only here.", "2026-09-01T10:05:00.000Z"),
        ]);
        f.transcript(B, &beta, 3_600, &[
            user("write the docs", &beta, "2026-09-01T09:00:00.000Z"),
            assistant("Written.", "2026-09-01T09:01:00.000Z"),
        ]);
        f.transcript(C, &alpha, 7_200, &[
            user("first\n  question", &alpha, "2026-09-01T08:00:00.000Z"),
            assistant("An answer.", "2026-09-01T08:01:00.000Z"),
            user("and a follow-up nobody answered", &alpha, "2026-09-01T08:02:00.000Z"),
        ]);
        f
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    /// Under the home directory, because `--here` never walks up past it.
    fn work(&self, p: &str) -> PathBuf {
        self.home().join("work").join(p)
    }

    fn transcript(&self, id: &str, cwd: &Path, age_secs: u64, lines: &[Value]) {
        let label = cwd.to_string_lossy().replace('/', "-");
        let dir = self.root.join("claude/projects").join(label);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{id}.jsonl"));
        let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
        fs::write(&file, body).unwrap();
        let when = SystemTime::now() - Duration::from_secs(age_secs);
        fs::File::options().write(true).open(&file).unwrap().set_modified(when).unwrap();
    }

    /// A Copilot CLI session folder: `workspace.yaml` plus `events.jsonl`. Absent unless a test
    /// adds one, so every other test runs with no Copilot at all.
    fn copilot(&self, id: &str, workspace: &str, age_secs: u64, events: &[Value]) {
        let dir = self.root.join("copilot/session-state").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("workspace.yaml"), workspace).unwrap();
        let file = dir.join("events.jsonl");
        let body: String = events.iter().map(|l| format!("{l}\n")).collect();
        fs::write(&file, body).unwrap();
        let when = SystemTime::now() - Duration::from_secs(age_secs);
        fs::File::options().write(true).open(&file).unwrap().set_modified(when).unwrap();
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_sessio"));
        c.args(args)
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude"))
            // Never the real ~/.copilot, even for a developer who has set COPILOT_HOME.
            .env("COPILOT_HOME", self.root.join("copilot"))
            .env("HOME", self.home())
            .current_dir(&self.root);
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }

    /// Run a command that must succeed and return its stdout.
    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(o.status.success(), "{args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(args)).unwrap()
    }

    fn ids(&self, args: &[&str]) -> Vec<String> {
        let v = self.json(args);
        v.as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap().to_string()).collect()
    }

    /// A stand-in `claude` first on PATH, running `script`.
    fn fake_claude(&self, script: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let bin = self.root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("claude");
        fs::write(&exe, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn user(text: &str, cwd: &Path, ts: &str) -> Value {
    json!({
        "type": "user", "promptSource": "typed", "cwd": cwd, "gitBranch": "main",
        "timestamp": ts, "message": {"role": "user", "content": text},
    })
}

fn assistant(text: &str, ts: &str) -> Value {
    json!({"type": "assistant", "timestamp": ts, "message": {"content": [{"type": "text", "text": text}]}})
}

fn cp_event(ty: &str, data: Value, ts: &str) -> Value {
    json!({"type": ty, "data": data, "id": "e", "timestamp": ts, "parentId": null})
}

const D: &str = "dddd4444-0000-4000-8000-000000000004";

/// Adds D: a Copilot session in `alpha`, between A and B in age.
fn with_copilot(f: &Fixture) {
    let alpha = f.work("alpha");
    f.copilot(
        D,
        &format!("id: {D}\ncwd: {}\nbranch: feat\nname: Copilot fixes the lexer\nuser_named: false\n", alpha.display()),
        600,
        &[
            cp_event("session.start", json!({"context": {"cwd": alpha}}), "2026-09-01T11:00:00.000Z"),
            cp_event("user.message", json!({"content": "fix the lexer"}), "2026-09-01T11:00:01.000Z"),
            cp_event("assistant.message", json!({"content": "Lexer fixed; copilot-only-needle."}), "2026-09-01T11:00:02.000Z"),
            cp_event("session.task_complete", json!({"summary": "Fixed the lexer."}), "2026-09-01T11:00:03.000Z"),
        ],
    );
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn ls_json_is_newest_first_with_the_state_of_each_session() {
    let f = Fixture::new("json");
    let v = f.json(&["ls", "--json"]);
    let rows = v.as_array().unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [A, B, C]);

    assert_eq!(rows[0]["title"], "Refactor the parser");
    assert_eq!(rows[0]["project"], "alpha");
    assert_eq!(rows[0]["first_prompt"], "please refactor\nthe parser");
    assert_eq!(rows[0]["branch"], "main");
    assert_eq!(rows[0]["open"], false);
    assert_eq!(rows[0]["running"], Value::Null);
    assert_eq!(rows[0]["archived"], false);
    assert!(rows[0]["transcript"].as_str().unwrap().ends_with(&format!("{A}.jsonl")));
    assert!(rows[0]["modified"].as_str().unwrap().ends_with('Z'));

    // The one-shot load waits for git: a dashboard's first frame would still call this clean.
    assert_eq!(rows[1]["open_reason"], "uncommitted changes");
    assert_eq!(rows[2]["open_reason"], "your prompt got no reply");
}

#[test]
fn ls_prints_one_line_per_session() {
    let f = Fixture::new("plain");
    let out = f.ok(&["ls"]);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "{out}");
    assert!(lines[0].contains("aaaa1111") && !lines[0].contains(A), "short ids: {}", lines[0]);
    assert!(lines[0].contains("alpha") && lines[0].contains("Refactor the parser"));
    assert!(lines[1].starts_with('▶') && lines[2].starts_with('▶'), "open sessions are marked");
    // An untitled session shows its first prompt, and a multi-line one stays on its own row.
    assert!(lines[2].ends_with("  first question"), "{}", lines[2]);
}

#[test]
fn filters_narrow_the_list() {
    let f = Fixture::new("filters");
    assert_eq!(f.ids(&["ls", "--json", "--open"]), [B, C]);
    assert_eq!(f.ids(&["ls", "--json", "-p", "beta"]), [B]);
    assert_eq!(f.ids(&["ls", "--json", "--project=alpha", "-n", "1"]), [A]);
    assert!(f.ids(&["ls", "--json", "--running"]).is_empty());

    let here = f.cmd(&["ls", "--json", "--here"]).current_dir(f.work("alpha/sub")).output().unwrap();
    assert!(here.status.success(), "{}", stderr(&here));
    let ids: Vec<String> = serde_json::from_slice::<Value>(&here.stdout).unwrap().as_array().unwrap()
        .iter().map(|r| r["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids, [A, C], "a subfolder lists its project");

    let empty = f.run(&["ls", "--running"]);
    assert!(empty.status.success() && empty.stdout.is_empty());
    assert!(stderr(&empty).contains("no matching sessions"));

    let nosuch = f.run(&["ls", "-p", "gamma"]);
    assert_eq!(nosuch.status.code(), Some(1));
    assert!(stderr(&nosuch).contains("no project named 'gamma'"));
}

#[test]
fn find_ranks_titles_and_text_search_reads_the_bodies() {
    let f = Fixture::new("find");
    assert_eq!(f.ids(&["find", "--json", "parser"]), [A]);
    assert_eq!(f.ids(&["find", "--json", "docs"]), [B]);
    assert!(f.ids(&["find", "--json", "needle-in-body"]).is_empty(), "plain find reads titles only");

    if sessio::search::rg_path().is_some() {
        assert_eq!(f.ids(&["find", "--json", "--text", "needle-in-body"]), [A]);
    }
}

#[test]
fn show_takes_a_prefix_only_one_session_has() {
    let f = Fixture::new("show");
    let out = f.ok(&["show", "aaaa1"]);
    assert!(out.starts_with("Refactor the parser\n"), "{out}");
    for part in [A, "alpha", "first prompt", "  please refactor\n  the parser", "last reply", "needle-in-body"] {
        assert!(out.contains(part), "missing {part:?} in\n{out}");
    }

    let v = f.json(&["show", "--json", C]);
    assert_eq!(v["prompts"], 2);
    assert_eq!(v["last_prompt"], "and a follow-up nobody answered");
    assert_eq!(v["open_reason"], "your prompt got no reply");

    let ambiguous = f.run(&["show", "aaaa"]);
    assert_eq!(ambiguous.status.code(), Some(1));
    assert!(stderr(&ambiguous).contains("matches 2 sessions"), "{}", stderr(&ambiguous));
    assert_eq!(f.run(&["show", "ffff"]).status.code(), Some(1));
}

#[test]
fn show_totals_tokens_once_per_api_response() {
    let f = Fixture::new("tokens");
    let alpha = f.work("alpha");
    // Two lines of one response (same message id, same usage), then a second response.
    let turn = |id: &str, block: &str, input: u64, output: u64, write: u64, read: u64| {
        json!({"type": "assistant", "timestamp": "2026-09-01T11:01:00.000Z", "message": {
            "id": id, "content": [{"type": block, "text": "ok"}],
            "usage": {"input_tokens": input, "output_tokens": output,
                      "cache_creation_input_tokens": write, "cache_read_input_tokens": read}}})
    };
    let d = "dddd4444-0000-4000-8000-000000000004";
    f.transcript(d, &alpha, 30, &[
        user("count my tokens", &alpha, "2026-09-01T11:00:00.000Z"),
        turn("msg_1", "tool_use", 10, 800, 31_000, 700_000),
        turn("msg_1", "text", 10, 800, 31_000, 700_000),
        turn("msg_2", "text", 800, 5_000, 600_000, 27_000_000),
    ]);

    let v = f.json(&["show", "--json", d]);
    assert_eq!(
        v["tokens"],
        json!({"input": 810, "output": 5_800, "cache_write": 631_000, "cache_read": 27_700_000})
    );
    let out = f.ok(&["show", d]);
    assert!(out.contains("tokens    in 810 · out 5.8k · cache w 631k · r 27.7M"), "{out}");

    // A transcript without usage data has no totals, and no line for them.
    assert_eq!(f.json(&["show", "--json", A])["tokens"], Value::Null);
    assert!(!f.ok(&["show", A]).contains("\n  tokens "));
}

#[test]
fn archive_hides_a_session_and_unarchive_brings_it_back() {
    let f = Fixture::new("archive");
    assert!(f.ok(&["archive", "aaaa1"]).starts_with("archived  aaaa1111"));
    assert_eq!(f.ids(&["ls", "--json"]), [B, C]);
    assert_eq!(f.ids(&["ls", "--json", "--archived"]), [A]);
    assert!(f.root.join("claude/.sessio/archived.json").exists());

    // Explicit, not a toggle: archiving twice must not unarchive.
    assert!(f.ok(&["archive", "aaaa1"]).starts_with("already archived"));
    assert_eq!(f.ids(&["ls", "--json", "--archived"]), [A]);

    assert!(f.ok(&["unarchive", "aaaa1"]).starts_with("unarchived"));
    assert_eq!(f.ids(&["ls", "--json"]), [A, B, C]);
    assert!(f.ok(&["unarchive", "aaaa1"]).starts_with("not archived"));
}

#[test]
fn reply_runs_claude_in_the_sessions_folder_and_cleans_its_answer() {
    let f = Fixture::new("reply");
    let log = f.root.join("claude-args.log");
    let path = f.fake_claude(r#"{ pwd -P; printf '%s\n' "$@"; } > "$FAKE_LOG"; printf 'the answer\033]52;c;eA==\007 ok\n'"#);

    let o = f.cmd(&["reply", "aaaa1", "hello", "there"]).env("PATH", &path).env("FAKE_LOG", &log).output().unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    // The escape sequence is stripped: a reply is transcript text, and could set the clipboard.
    assert_eq!(String::from_utf8(o.stdout).unwrap(), "the answer]52;c;eA== ok\n");
    let seen = fs::read_to_string(&log).unwrap();
    let expected = format!("{}\n-p\n--resume\n{A}\nhello there\n", f.work("alpha").display());
    assert_eq!(seen, expected);
}

#[test]
fn reply_reads_the_message_from_stdin_only_when_asked() {
    use std::io::Write;
    let f = Fixture::new("reply-stdin");
    let log = f.root.join("claude-args.log");
    let path = f.fake_claude(r#"printf '%s\n' "$@" > "$FAKE_LOG"; echo ok"#);
    let mut child = f.cmd(&["reply", C, "-"]).env("PATH", &path).env("FAKE_LOG", &log)
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"from a pipe").unwrap();
    assert!(child.wait_with_output().unwrap().status.success());
    assert!(fs::read_to_string(&log).unwrap().ends_with("from a pipe\n"));
}

#[test]
fn a_failing_claude_is_reported_not_swallowed() {
    let f = Fixture::new("reply-fail");
    let path = f.fake_claude("echo 'boom: not logged in' >&2; exit 3");
    let o = f.cmd(&["reply", "aaaa1", "hi"]).env("PATH", &path).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("boom: not logged in"), "{}", stderr(&o));
    assert!(o.stdout.is_empty());
}

#[test]
fn resume_prints_the_command_and_needs_a_terminal_to_run_it() {
    let f = Fixture::new("resume");
    let expected = format!("cd -- '{}' && claude --resume '{A}'\n", f.work("alpha").display());
    assert_eq!(f.ok(&["resume", "--print", "aaaa1"]), expected);

    let o = f.run(&["resume", "aaaa1"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("needs an interactive terminal"));
    assert!(stderr(&o).contains(&format!("claude --resume '{A}'")));
}

#[test]
fn a_wrong_command_line_exits_2() {
    let f = Fixture::new("usage");
    for args in [
        &["frob"][..],
        &["ls", "parser"],
        &["ls", "--opne"],
        &["show"],
        &["show", "a", "b"],
        &["find"],
        &["reply", "aaaa1"],
        &["archive"],
        &["ls", "--here", "-p", "alpha"],
        &["kill"],
        &["kill", "a", "b"],
        &["kill", "aaaa1", "--force"],
    ] {
        assert_eq!(f.run(args).status.code(), Some(2), "{args:?}");
    }
    assert!(f.ok(&["ls", "--help"]).contains("FILTERS"));
    assert!(f.ok(&["help"]).contains("sessions reply"));
    assert!(f.ok(&["help"]).contains("sessions kill"));
}

/// A process this test owns, dressed as a running `claude` for session `id`: `sleep` run through
/// a symlink named `claude`, plus the registry row Claude Code would have written for it. Never a
/// real claude — the kill tests signal only this. A symlink, not a copy: macOS kills a copied
/// system binary on launch for its broken signature.
struct Decoy {
    child: std::process::Child,
}

impl Decoy {
    fn start(f: &Fixture, id: &str, status: &str) -> Self {
        let bin = f.root.join("decoy");
        fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("claude");
        if !exe.exists() {
            std::os::unix::fs::symlink("/bin/sleep", &exe).unwrap();
        }
        let child = Command::new(&exe).arg("60").spawn().unwrap();
        let reg = f.root.join("claude/sessions");
        fs::create_dir_all(&reg).unwrap();
        let row = json!({"pid": child.id(), "sessionId": id, "status": status});
        fs::write(reg.join(format!("{}.json", child.id())), row.to_string()).unwrap();
        Decoy { child }
    }

    fn running(&mut self) -> bool {
        self.child.try_wait().unwrap().is_none()
    }
}

impl Drop for Decoy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Backdate one fixture transcript.
fn age(f: &Fixture, id: &str, secs: u64) {
    let file = fs::read_dir(f.root.join("claude/projects"))
        .unwrap()
        .flat_map(|d| fs::read_dir(d.unwrap().path()).unwrap())
        .map(|e| e.unwrap().path())
        .find(|p| p.file_name().unwrap().to_string_lossy().starts_with(id))
        .unwrap();
    let when = SystemTime::now() - Duration::from_secs(secs);
    fs::File::options().write(true).open(&file).unwrap().set_modified(when).unwrap();
}

#[test]
fn kill_ends_a_running_session_idle_for_more_than_48h() {
    let f = Fixture::new("kill");
    age(&f, C, 3 * 86_400);
    let mut decoy = Decoy::start(&f, C, "idle");
    let pid = decoy.child.id();

    let v = f.json(&["kill", "--json", "cccc3"]);
    assert_eq!(v["id"], C);
    assert_eq!(v["pid"], pid);
    assert_eq!(v["idle_days"], 3);
    assert_eq!(v["ended"], true);
    assert!(!decoy.running(), "the decoy got SIGTERM");
}

#[test]
fn kill_refuses_anything_but_a_stale_idle_session() {
    let f = Fixture::new("kill-refuse");
    // Not running at all.
    let o = f.run(&["kill", "aaaa1"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("not running"), "{}", stderr(&o));

    // Running, but written to two hours ago.
    let mut recent = Decoy::start(&f, C, "idle");
    let o = f.run(&["kill", "--json", C]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("active 2h ago"), "{}", stderr(&o));
    assert!(o.stdout.is_empty());
    assert!(recent.running(), "left alone");
    drop(recent);

    // Old, but busy or waiting on the user.
    age(&f, B, 5 * 86_400);
    for (status, why) in [("busy", "busy"), ("waiting", "waiting on you")] {
        let mut d = Decoy::start(&f, B, status);
        let o = f.run(&["kill", B]);
        assert_eq!(o.status.code(), Some(1), "{status}");
        assert!(stderr(&o).contains(why), "{status}: {}", stderr(&o));
        assert!(d.running(), "{status}: left alone");
        let _ = fs::remove_file(f.root.join(format!("claude/sessions/{}.json", d.child.id())));
    }
}

#[test]
fn kill_will_not_signal_a_process_that_is_not_claude() {
    // The registry says session C is pid N, but pid N is a plain `sleep`: a recycled pid, or a
    // registry row that lies. The rules pass; the re-check before signalling must not.
    let f = Fixture::new("kill-notclaude");
    age(&f, C, 3 * 86_400);
    let mut child = Command::new("/bin/sleep").arg("60").spawn().unwrap();
    let reg = f.root.join("claude/sessions");
    fs::create_dir_all(&reg).unwrap();
    let row = json!({"pid": child.id(), "sessionId": C, "status": "idle"});
    fs::write(reg.join(format!("{}.json", child.id())), row.to_string()).unwrap();

    let o = f.run(&["kill", C]);
    let alive = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("not a claude process"), "{}", stderr(&o));
    assert!(alive, "the sleep was left alone");
}

#[test]
fn copilot_sessions_merge_into_the_list_by_age_and_say_where_they_came_from() {
    let f = Fixture::new("copilot-ls");
    with_copilot(&f);
    // A folder Copilot made but never talked in is not a session.
    fs::create_dir_all(f.root.join("copilot/session-state/empty-one")).unwrap();

    let v = f.json(&["ls", "--json"]);
    let rows = v.as_array().unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [A, D, B, C]);
    assert_eq!(rows[0]["source"], "claude");
    let d = &rows[1];
    assert_eq!(d["source"], "copilot");
    assert_eq!(d["title"], "Copilot fixes the lexer");
    assert_eq!(d["project"], "alpha", "grouped with the Claude sessions from the same folder");
    assert_eq!(d["branch"], "feat");
    assert_eq!(d["first_prompt"], "fix the lexer");
    assert_eq!(d["running"], Value::Null);
    assert!(d["transcript"].as_str().unwrap().ends_with(&format!("{D}/events.jsonl")));
    assert_eq!(f.ids(&["ls", "--json", "-p", "alpha"]), [A, D, C]);

    let s = f.json(&["show", "--json", "dddd"]);
    assert_eq!(s["source"], "copilot");
    assert_eq!(s["prompts"], 1);
    assert_eq!(s["recap"], "Fixed the lexer.");
    assert_eq!(s["last_reply"], "Lexer fixed; copilot-only-needle.");
    assert!(f.ok(&["show", "dddd"]).contains("  source    copilot\n"));
}

#[test]
fn copilot_sessions_resume_with_copilot_and_refuse_a_reply() {
    let f = Fixture::new("copilot-resume");
    with_copilot(&f);
    let expected = format!("cd -- '{}' && copilot --resume='{D}'\n", f.work("alpha").display());
    assert_eq!(f.ok(&["resume", "--print", "dddd"]), expected);

    let o = f.run(&["resume", "dddd"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains(&format!("copilot --resume='{D}'")), "{}", stderr(&o));

    // Nothing may be launched for a reply: a stand-in claude would record it.
    let log = f.root.join("claude-args.log");
    let path = f.fake_claude(r#"echo called > "$FAKE_LOG""#);
    let o = f.cmd(&["reply", "dddd", "hi"]).env("PATH", &path).env("FAKE_LOG", &log).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("reply is Claude-only"), "{}", stderr(&o));
    assert!(!log.exists(), "claude was not run");

    let o = f.run(&["kill", "dddd"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("kill is Claude-only"), "{}", stderr(&o));
}

#[test]
fn copilot_sessions_are_searched_and_archived_like_the_rest() {
    let f = Fixture::new("copilot-search");
    with_copilot(&f);
    if Command::new("rg").arg("--version").output().is_ok() {
        assert_eq!(f.ids(&["find", "--json", "--text", "copilot-only-needle"]), [D]);
    }
    assert_eq!(f.ids(&["find", "--json", "lexer"]), [D]);

    assert!(f.ok(&["archive", "dddd"]).starts_with("archived  dddd4444"));
    assert_eq!(f.ids(&["ls", "--json"]), [A, B, C]);
    let saved = fs::read_to_string(f.root.join("claude/.sessio/archived.json")).unwrap();
    assert!(saved.contains(&format!("copilot/{D}")), "{saved}");
    assert!(f.ok(&["unarchive", "dddd"]).starts_with("unarchived"));
    assert_eq!(f.ids(&["ls", "--json"]), [A, D, B, C]);
}
