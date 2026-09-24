//! GitHub Copilot CLI sessions, read-only, next to Claude Code's.
//!
//! Copilot keeps one folder per session under `$COPILOT_HOME/session-state/<id>/` (default
//! `~/.copilot`), with a small `workspace.yaml` (cwd, branch, name) and an `events.jsonl` of typed
//! events. Only the few event types that feed the list and the preview are read:
//!
//! * `session.start` — `data.context.cwd` / `branch`, when the workspace file lacks them
//! * `user.message` — `data.content`; a message with a `data.source` (a sub-agent's task, an
//!   autopilot continuation) is not something the user typed and is skipped
//! * `assistant.message` — `data.content`; empty ones are tool calls, and ones carrying
//!   `parentToolCallId` belong to a sub-agent, so both are skipped
//! * `session.task_complete` — `data.summary`, the recap
//! * `session.compaction_complete` — `data.summaryContent`, the fallback summary
//!
//! A folder without `events.jsonl` never had a conversation and is not listed. Nothing here
//! writes under the Copilot folder.

use serde_json::Value;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::discover::{mtime_ms, Row, Source};
use crate::parse::{line_str, Detail, Head, Tail, TAIL_WINDOW};
use crate::safety::{js_trim, sanitize, valid_path};

/// Archive/search keys for Copilot sessions live under this prefix, so they can never collide
/// with a Claude key (`<project-dir>/<id>.jsonl`).
pub const KEY_PREFIX: &str = "copilot/";
const EVENTS: &str = "events.jsonl";
const WORKSPACE: &str = "workspace.yaml";
/// `workspace.yaml` is a few hundred bytes; anything past this is not one we understand.
const WORKSPACE_MAX: u64 = 64 * 1024;

/// `$COPILOT_HOME/session-state`, defaulting to `~/.copilot/session-state` as Copilot CLI does.
pub fn root() -> PathBuf {
    let base = std::env::var_os("COPILOT_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".copilot")))
        .unwrap_or_else(|| PathBuf::from(".copilot"));
    base.join("session-state")
}

/// One row per session folder that has an `events.jsonl`. A missing root is the empty state.
pub fn scan(root: &Path) -> Vec<Row> {
    let mut rows = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return rows;
    };
    for entry in entries.flatten() {
        let id = entry.file_name().to_string_lossy().into_owned();
        let file = entry.path().join(EVENTS);
        let Ok(meta) = fs::metadata(&file) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        rows.push(Row {
            key: format!("{KEY_PREFIX}{id}"),
            id,
            mtime: mtime_ms(&meta),
            size: meta.len(),
            file,
            dir: String::new(),
            source: Source::Copilot,
        });
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.mtime));
    rows
}

/// The key of a file given its path relative to the session-state root: only a session's own
/// `<id>/events.jsonl` is one.
pub fn key_for(rel: &Path) -> Option<String> {
    let mut parts = rel.components();
    let id = parts.next()?.as_os_str().to_str()?;
    let name = parts.next()?.as_os_str();
    (name == EVENTS && parts.next().is_none()).then(|| format!("{KEY_PREFIX}{id}"))
}

/// A folder name for grouping, derived from the cwd the way Claude Code names its project
/// folders, so a Copilot session lands in the same project as Claude sessions from that folder.
pub fn dir_for(cwd: Option<&str>) -> String {
    match cwd {
        Some(c) => c.chars().map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' }).collect(),
        None => "copilot".into(),
    }
}

// ---------- workspace.yaml ----------

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub name: Option<String>,
    /// Whether the user named the session (`/rename`); otherwise Copilot generated the name.
    pub user_named: bool,
}

pub fn workspace(events: &Path) -> Workspace {
    let Some(dir) = events.parent() else {
        return Workspace::default();
    };
    let mut text = String::new();
    let Ok(f) = File::open(dir.join(WORKSPACE)) else {
        return Workspace::default();
    };
    if f.take(WORKSPACE_MAX).read_to_string(&mut text).is_err() {
        return Workspace::default();
    }
    parse_workspace(&text)
}

pub fn parse_workspace(text: &str) -> Workspace {
    let map = yaml_top_level(text);
    let get = |k: &str| map.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
    let nonempty = |v: Option<&str>| v.map(js_trim).filter(|s| !s.is_empty()).map(str::to_string);
    Workspace {
        cwd: nonempty(get("cwd")).and_then(|c| valid_path(&c)),
        branch: nonempty(get("branch")).map(|b| sanitize(&b)),
        name: nonempty(get("name")).map(|n| sanitize(&n)),
        user_named: get("user_named") == Some("true"),
    }
}

/// The top-level `key: value` pairs of a flat YAML mapping — the only shape `workspace.yaml` has.
/// Handles plain, single- and double-quoted scalars and `|`/`>` block scalars (a long name is
/// written as `name: |-` followed by indented lines). Nested mappings are skipped.
fn yaml_top_level(text: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if line.starts_with([' ', '\t', '#', '-']) || line.trim().is_empty() {
            continue;
        }
        let Some((key, rest)) = line.split_once(':') else { continue };
        let rest = rest.trim();
        // The indented lines that follow belong to this key.
        let start = i;
        while i < lines.len() && (lines[i].starts_with([' ', '\t']) || lines[i].trim().is_empty()) {
            i += 1;
        }
        let block = &lines[start..i];
        let value = if rest.starts_with('|') || rest.starts_with('>') {
            let indent = block
                .iter()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.len() - l.trim_start().len())
                .min()
                .unwrap_or(0);
            let body: Vec<&str> = block.iter().map(|l| l.get(indent..).unwrap_or("")).collect();
            let joined = if rest.starts_with('>') { body.join(" ") } else { body.join("\n") };
            if rest.contains('-') {
                joined.trim_end().to_string()
            } else {
                format!("{}\n", joined.trim_end())
            }
        } else if let Some(q) = rest.strip_prefix('"') {
            unquote_double(q.strip_suffix('"').unwrap_or(q))
        } else if let Some(q) = rest.strip_prefix('\'') {
            q.strip_suffix('\'').unwrap_or(q).replace("''", "'")
        } else {
            // A plain scalar may continue on indented lines, folded with spaces.
            std::iter::once(rest)
                .chain(block.iter().map(|l| l.trim()).filter(|l| !l.is_empty()))
                .collect::<Vec<_>>()
                .join(" ")
        };
        out.push((key.trim().to_string(), value));
    }
    out
}

fn unquote_double(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

// ---------- events.jsonl ----------

fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    path.iter().try_fold(v, |v, k| v.get(k))?.as_str()
}

/// Something the user typed: a `user.message` with text and no `source` (sub-agent tasks and
/// autopilot continuations carry one).
fn human_prompt(o: &Value) -> Option<String> {
    if str_at(o, &["type"]) != Some("user.message") {
        return None;
    }
    let data = o.get("data")?;
    if data.get("source").is_some_and(|s| !s.is_null()) {
        return None;
    }
    let text = sanitize(data.get("content")?.as_str()?);
    let t = js_trim(&text);
    (!t.is_empty()).then(|| t.to_string())
}

/// The main agent's visible answer: skips tool-call-only turns and sub-agent messages.
fn assistant_reply(o: &Value) -> Option<String> {
    if str_at(o, &["type"]) != Some("assistant.message") {
        return None;
    }
    let data = o.get("data")?;
    if data.get("parentToolCallId").is_some_and(|p| !p.is_null()) {
        return None;
    }
    let text = sanitize(data.get("content")?.as_str()?);
    let t = js_trim(&text);
    (!t.is_empty()).then(|| t.to_string())
}

fn task_summary(o: &Value) -> Option<String> {
    if str_at(o, &["type"]) != Some("session.task_complete") {
        return None;
    }
    let text = sanitize(str_at(o, &["data", "summary"])?);
    let t = js_trim(&text);
    (!t.is_empty()).then(|| t.to_string())
}

/// A compaction summary, minus the `<overview>`-style tag lines Copilot wraps its sections in.
fn compaction_summary(o: &Value) -> Option<String> {
    if str_at(o, &["type"]) != Some("session.compaction_complete") {
        return None;
    }
    let raw = sanitize(str_at(o, &["data", "summaryContent"])?);
    let kept: Vec<&str> = raw
        .lines()
        .filter(|l| {
            let t = l.trim();
            !(t.starts_with('<')
                && t.ends_with('>')
                && t[1..t.len() - 1]
                    .trim_start_matches('/')
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        })
        .collect();
    let text = kept.join("\n");
    let t = js_trim(&text);
    (!t.is_empty()).then(|| t.to_string())
}

fn ts(o: &Value) -> Option<String> {
    str_at(o, &["timestamp"]).map(str::to_string)
}

/// Cheap `contains` gate before `serde_json`, as in the Claude parser: tool output lines run to
/// hundreds of kilobytes and are never parsed.
fn is(line: &str, ty: &str) -> bool {
    line.contains(&format!("\"type\":\"{ty}\""))
}

fn apply_workspace_title(ws: &Workspace, custom: &mut Option<String>, ai: &mut Option<String>) {
    if let Some(n) = &ws.name {
        if ws.user_named {
            *custom = Some(n.clone());
        } else {
            *ai = Some(n.clone());
        }
    }
}

/// Light read for the list: the workspace file, then `events.jsonl` only up to the first prompt.
pub fn head(events: &Path) -> Head {
    let ws = workspace(events);
    let mut h = Head { cwd: ws.cwd.clone(), branch: ws.branch.clone(), ..Head::default() };
    apply_workspace_title(&ws, &mut h.custom, &mut h.ai);

    let Ok(file) = File::open(events) else {
        return h;
    };
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut buf = Vec::with_capacity(8192);
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = line_str(&buf);
        if is(&line, "session.start") && (h.cwd.is_none() || h.branch.is_none()) {
            if let Ok(o) = serde_json::from_str::<Value>(&line) {
                if h.cwd.is_none() {
                    h.cwd = str_at(&o, &["data", "context", "cwd"]).and_then(valid_path);
                }
                if h.branch.is_none() {
                    h.branch = str_at(&o, &["data", "context", "branch"])
                        .filter(|b| !b.is_empty())
                        .map(sanitize);
                }
            }
        } else if is(&line, "user.message") {
            if let Ok(o) = serde_json::from_str::<Value>(&line) {
                if let Some(text) = human_prompt(&o) {
                    h.first = Some(text);
                    h.first_ts = ts(&o);
                    break;
                }
            }
        }
    }
    h
}

/// The last ~64KB: the latest task summary, as the list-level recap. Copilot sessions are never
/// marked open from their transcript — the pick-up heuristics are tuned to Claude's — though the
/// git-WIP flag still applies by folder.
pub fn tail(events: &Path) -> Tail {
    let mut t = Tail::default();
    let Ok(mut file) = File::open(events) else {
        return t;
    };
    let Ok(meta) = file.metadata() else {
        return t;
    };
    let start = meta.len().saturating_sub(TAIL_WINDOW);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return t;
    }
    let mut raw = Vec::with_capacity(TAIL_WINDOW as usize);
    if file.take(TAIL_WINDOW).read_to_end(&mut raw).is_err() {
        return t;
    }
    for chunk in raw.split(|b| *b == b'\n') {
        let line = line_str(chunk);
        if is(&line, "session.task_complete") {
            if let Ok(o) = serde_json::from_str::<Value>(&line) {
                if let Some(s) = task_summary(&o) {
                    t.recap = Some(s);
                    t.recap_ts = ts(&o);
                }
            }
        }
    }
    t
}

/// Full read for the preview of the highlighted session.
pub fn detail(events: &Path) -> Detail {
    let ws = workspace(events);
    let mut d = Detail { cwd: ws.cwd.clone(), branch: ws.branch.clone(), ..Detail::default() };
    apply_workspace_title(&ws, &mut d.custom, &mut d.ai);

    let Ok(file) = File::open(events) else {
        return d;
    };
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut buf = Vec::with_capacity(8192);
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = line_str(&buf);
        let parse = || serde_json::from_str::<Value>(&line).ok();
        if is(&line, "user.message") {
            if let Some(o) = parse() {
                if let Some(text) = human_prompt(&o) {
                    d.count += 1;
                    if d.first.is_none() {
                        d.first = Some(text.clone());
                        d.first_ts = ts(&o);
                    }
                    d.last = Some(text);
                    d.last_ts = ts(&o);
                }
            }
        } else if is(&line, "assistant.message") {
            if let Some(o) = parse() {
                if let Some(text) = assistant_reply(&o) {
                    d.reply = Some(text);
                    d.reply_ts = ts(&o);
                }
            }
        } else if is(&line, "session.task_complete") {
            if let Some(o) = parse() {
                if let Some(s) = task_summary(&o) {
                    d.recap = Some(s);
                    d.recap_ts = ts(&o);
                }
            }
        } else if is(&line, "session.compaction_complete") {
            if let Some(o) = parse() {
                if let Some(s) = compaction_summary(&o) {
                    d.summary = Some(s);
                    d.summary_ts = ts(&o);
                }
            }
        } else if d.cwd.is_none() && is(&line, "session.start") {
            if let Some(o) = parse() {
                d.cwd = str_at(&o, &["data", "context", "cwd"]).and_then(valid_path);
                if d.branch.is_none() {
                    d.branch = str_at(&o, &["data", "context", "branch"])
                        .filter(|b| !b.is_empty())
                        .map(sanitize);
                }
            }
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sessio-copilot-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn session(root: &Path, id: &str, yaml: &str, events: &[Value]) -> PathBuf {
        let dir = root.join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(WORKSPACE), yaml).unwrap();
        let body: String = events.iter().map(|e| format!("{e}\n")).collect();
        fs::write(dir.join(EVENTS), body).unwrap();
        dir.join(EVENTS)
    }

    fn ev(ty: &str, data: Value, ts: &str) -> Value {
        json!({"type": ty, "data": data, "id": "x", "timestamp": ts, "parentId": null})
    }

    #[test]
    fn reads_the_workspace_including_a_block_scalar_name() {
        let ws = parse_workspace(
            "id: abc\ncwd: /tmp/proj\ngit_root: /tmp/proj\nbranch: main\nname: |-\n  Design brief: a new logo\n\n  second para...\nuser_named: false\ncreated_at: 2026-09-24T11:33:54.706Z\n",
        );
        assert_eq!(ws.cwd.as_deref(), Some("/tmp/proj"));
        assert_eq!(ws.branch.as_deref(), Some("main"));
        assert_eq!(ws.name.as_deref(), Some("Design brief: a new logo\n\nsecond para..."));
        assert!(!ws.user_named);

        let ws = parse_workspace("cwd: \"/tmp/a \\\"b\\\"\"\nname: 'it''s mine'\nuser_named: true\n");
        assert_eq!(ws.cwd.as_deref(), Some("/tmp/a \"b\""));
        assert_eq!(ws.name.as_deref(), Some("it's mine"));
        assert!(ws.user_named);

        assert_eq!(parse_workspace("id: x\ncwd: /\n").name, None);
    }

    #[test]
    fn scan_lists_only_folders_with_events_and_keys_them_under_copilot() {
        let root = tmp("scan");
        session(&root, "s1", "cwd: /tmp\n", &[]);
        fs::create_dir_all(root.join("empty")).unwrap();
        fs::write(root.join("empty").join(WORKSPACE), "cwd: /tmp\n").unwrap();
        let rows = scan(&root);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "s1");
        assert_eq!(rows[0].key, "copilot/s1");
        assert_eq!(rows[0].source, Source::Copilot);
        assert!(scan(&root.join("missing")).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn keys_only_a_sessions_own_events_file() {
        assert_eq!(key_for(Path::new("abc/events.jsonl")).as_deref(), Some("copilot/abc"));
        assert_eq!(key_for(Path::new("abc/files/x.jsonl")), None);
        assert_eq!(key_for(Path::new("abc/other.jsonl")), None);
        assert_eq!(key_for(Path::new("events.jsonl")), None);
    }

    #[test]
    fn head_tail_and_detail_read_what_the_preview_needs() {
        let root = tmp("parse");
        let f = session(
            &root,
            "sid",
            "id: sid\ncwd: /tmp/proj\nbranch: feat\nname: Fix the login\nuser_named: false\n",
            &[
                ev("session.start", json!({"context": {"cwd": "/elsewhere", "branch": "x"}}), "2026-09-01T10:00:00.000Z"),
                ev("user.message", json!({"content": "  fix the login bug  "}), "2026-09-01T10:00:01.000Z"),
                ev("assistant.message", json!({"content": "", "toolRequests": []}), "2026-09-01T10:00:02.000Z"),
                ev("user.message", json!({"content": "sub-agent task", "source": "agent-sid"}), "2026-09-01T10:00:03.000Z"),
                ev("assistant.message", json!({"content": "sub-agent says", "parentToolCallId": "c1"}), "2026-09-01T10:00:04.000Z"),
                ev("assistant.message", json!({"content": "Fixed it."}), "2026-09-01T10:00:05.000Z"),
                ev("session.compaction_complete", json!({"summaryContent": "<overview>\nWe fixed login.\n</overview>"}), "2026-09-01T10:00:06.000Z"),
                ev("user.message", json!({"content": "", "source": "autopilot"}), "2026-09-01T10:00:07.000Z"),
                ev("user.message", json!({"content": "now add a test"}), "2026-09-01T10:00:08.000Z"),
                ev("assistant.message", json!({"content": "Test added."}), "2026-09-01T10:00:09.000Z"),
                ev("session.task_complete", json!({"summary": "Fixed login and added a test.", "success": true}), "2026-09-01T10:00:10.000Z"),
                ev("session.shutdown", json!({}), "2026-09-01T10:00:11.000Z"),
            ],
        );

        let h = head(&f);
        assert_eq!(h.first.as_deref(), Some("fix the login bug"));
        assert_eq!(h.first_ts.as_deref(), Some("2026-09-01T10:00:01.000Z"));
        assert_eq!(h.cwd.as_deref(), Some("/tmp/proj"), "workspace.yaml wins over session.start");
        assert_eq!(h.branch.as_deref(), Some("feat"));
        assert_eq!(h.ai.as_deref(), Some("Fix the login"));
        assert_eq!(h.custom, None);

        let t = tail(&f);
        assert_eq!(t.recap.as_deref(), Some("Fixed login and added a test."));
        assert!(!t.open);

        let d = detail(&f);
        assert_eq!(d.count, 2, "sub-agent and autopilot messages are not prompts");
        assert_eq!(d.last.as_deref(), Some("now add a test"));
        assert_eq!(d.reply.as_deref(), Some("Test added."));
        assert_eq!(d.recap.as_deref(), Some("Fixed login and added a test."));
        assert_eq!(d.summary.as_deref(), Some("We fixed login."));
        assert_eq!(d.ai.as_deref(), Some("Fix the login"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn falls_back_to_session_start_without_a_workspace_cwd() {
        let root = tmp("start");
        let f = session(
            &root,
            "sid",
            "id: sid\n",
            &[
                ev("session.start", json!({"context": {"cwd": "/tmp/from-start", "branch": "dev"}}), "2026-09-01T10:00:00.000Z"),
                ev("user.message", json!({"content": "hello"}), "2026-09-01T10:00:01.000Z"),
            ],
        );
        let h = head(&f);
        assert_eq!(h.cwd.as_deref(), Some("/tmp/from-start"));
        assert_eq!(h.branch.as_deref(), Some("dev"));
        assert_eq!(h.ai, None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn groups_by_cwd_like_claude_names_its_folders() {
        assert_eq!(dir_for(Some("/Users/me/code/my.app")), "-Users-me-code-my-app");
        assert_eq!(dir_for(None), "copilot");
    }
}
