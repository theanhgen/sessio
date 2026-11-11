//! Full-text search across every transcript body, via ripgrep. Port of `contentSearch`
//! (bin/sessio.mjs:517) and the `RG` probe at :49.
//!
//! Without `rg` on the system everything else still works; only content search is disabled.
//!
//! ripgrep only finds the candidates. A Claude Code transcript also holds what Claude Code
//! injects around the conversation: the skill list, the tool list, the output style, CLAUDE.md,
//! a copy of every API request. That is the same in nearly every session, so a word from it
//! (a skill's name) matched nearly every session. Each candidate is read again, and a match
//! counts only inside the conversation (see `in_conversation`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// Locate ripgrep once. `None` disables `^f` gracefully.
pub fn rg_path() -> Option<&'static str> {
    static RG: OnceLock<Option<String>> = OnceLock::new();
    RG.get_or_init(|| {
        for p in ["rg", "/opt/homebrew/bin/rg", "/usr/local/bin/rg", "/usr/bin/rg"] {
            let ok = Command::new(p)
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                return Some(p.to_string());
            }
        }
        None
    })
    .as_deref()
}

/// Every folder `^f` and `find --text` search: Claude Code's projects and Copilot CLI's
/// session-state.
pub fn roots() -> Vec<PathBuf> {
    vec![crate::discover::projects_root(), crate::copilot::root()]
}

/// What to install when `rg` is missing. The one place the hint is spelled, so the dashboard,
/// its help and `find --text` cannot disagree about it.
pub const INSTALL_HINT: &str = "brew install ripgrep";

/// Grep every transcript body for `term`, returning the matching files, or why it could not.
///
/// `-F` keeps the query literal, so a prompt containing regex metacharacters searches for
/// itself rather than exploding. Runs to completion on a worker thread; the caller drops the
/// result if a newer search has superseded it.
pub fn content_search(term: &str, roots: &[PathBuf]) -> Result<HashSet<PathBuf>, String> {
    let candidates = content_search_with(rg_path(), term, roots)?;
    Ok(keep_conversation(candidates, term))
}

/// The candidates whose match is in the conversation, checked in parallel.
fn keep_conversation(candidates: HashSet<PathBuf>, term: &str) -> HashSet<PathBuf> {
    let files: Vec<PathBuf> = candidates.into_iter().collect();
    let needle = Needle::new(term);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let kept = std::sync::Mutex::new(HashSet::new());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(files.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(f) = files.get(i) else { break };
                if in_conversation(f, &needle) {
                    kept.lock().expect("no worker panics").insert(f.clone());
                }
            });
        }
    });
    kept.into_inner().expect("no worker panics")
}

/// The term, ready for both checks: a case-insensitive literal for the raw line (cheap, so only
/// lines that could match are parsed), and its lowercase form for the text they hold.
struct Needle {
    raw: regex::bytes::Regex,
    lower: String,
}

impl Needle {
    fn new(term: &str) -> Self {
        let raw = regex::bytes::RegexBuilder::new(&regex::escape(term))
            .case_insensitive(true)
            .build()
            .expect("an escaped literal is a valid pattern");
        Needle { raw, lower: term.to_lowercase() }
    }
}

/// Whether `needle` occurs in the conversation of a transcript: the prompts, the agent's replies,
/// its tool calls and their results, the titles, and prompts queued while it was busy. Not in
/// what the agent adds around them: for Claude Code, `attachment` lines (skills, tools,
/// environment, instructions), copies of API requests, `isMeta` lines, `<system-reminder>`
/// blocks and Claude's thinking; for Copilot CLI (`events.jsonl`), `system.*`, `model.*`,
/// `skill.*`, hooks and the session's bookkeeping.
fn in_conversation(path: &Path, needle: &Needle) -> bool {
    let copilot = path.file_name().is_some_and(|n| n == "events.jsonl");
    use std::io::BufRead;
    let Ok(file) = std::fs::File::open(path) else { return false };
    let mut reader = std::io::BufReader::with_capacity(256 * 1024, file);
    let mut buf = Vec::with_capacity(8192);
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => return false,
            Ok(_) => {}
        }
        // A term with characters JSON escapes can't be found in the raw line; parse those all.
        let escapable = needle.lower.contains(['"', '\\']) || needle.lower.chars().any(char::is_control);
        if !escapable && !needle.raw.is_match(&buf) {
            continue;
        }
        let Ok(o) = serde_json::from_slice::<serde_json::Value>(&buf) else { continue };
        let mut texts: Vec<&str> = Vec::new();
        if copilot {
            copilot_text(&o, &mut texts);
        } else {
            conversation_text(&o, &mut texts);
        }
        if texts.iter().any(|t| without_reminders(t).to_lowercase().contains(&needle.lower)) {
            return true;
        }
    }
}

/// The conversation's own strings in one transcript line, in `out`.
fn conversation_text<'a>(o: &'a serde_json::Value, out: &mut Vec<&'a str>) {
    use serde_json::Value;
    let field = |k: &str| o.get(k).and_then(Value::as_str);
    match field("type") {
        Some("user" | "assistant") if o.get("isMeta") != Some(&Value::Bool(true)) => {
            match o.get("message").and_then(|m| m.get("content")) {
                Some(Value::String(s)) => out.push(s),
                Some(Value::Array(blocks)) => {
                    for b in blocks {
                        match b.get("type").and_then(Value::as_str) {
                            Some("text") => out.extend(b.get("text").and_then(Value::as_str)),
                            Some("tool_use") => strings(b.get("input"), out),
                            Some("tool_result") => strings(b.get("content"), out),
                            _ => {} // thinking, images
                        }
                    }
                }
                _ => {}
            }
        }
        Some("attachment") => {
            let a = o.get("attachment");
            if a.and_then(|a| a.get("type")).and_then(Value::as_str) == Some("queued_command") {
                out.extend(a.and_then(|a| a.get("prompt")).and_then(Value::as_str));
            }
        }
        Some("custom-title") => out.extend(field("customTitle")),
        Some("ai-title") => out.extend(field("aiTitle")),
        _ => {}
    }
}

/// The conversation's own strings in one Copilot CLI event, in `out`. A prompt's `content` is
/// what was typed; its `transformedContent` adds the injected context, so it is left out.
fn copilot_text<'a>(o: &'a serde_json::Value, out: &mut Vec<&'a str>) {
    use serde_json::Value;
    let data = o.get("data");
    let get = |k: &str| data.and_then(|d| d.get(k));
    match o.get("type").and_then(Value::as_str) {
        Some("user.message" | "assistant.message") => out.extend(get("content").and_then(Value::as_str)),
        Some("tool.execution_start") => strings(get("arguments"), out),
        Some("tool.execution_complete") => strings(get("result"), out),
        _ => {}
    }
}

/// Every string inside `v`, however deep: a tool's input and a tool result are nested values.
fn strings<'a>(v: Option<&'a serde_json::Value>, out: &mut Vec<&'a str>) {
    use serde_json::Value;
    match v {
        Some(Value::String(s)) => out.push(s),
        Some(Value::Array(a)) => a.iter().for_each(|x| strings(Some(x), out)),
        Some(Value::Object(m)) => m.values().for_each(|x| strings(Some(x), out)),
        _ => {}
    }
}

/// `text` with every `<system-reminder>…</system-reminder>` block taken out: Claude Code puts
/// them inside prompts and tool results, and they carry the same injected context.
fn without_reminders(text: &str) -> std::borrow::Cow<'_, str> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    if !text.contains("<system-reminder>") {
        return std::borrow::Cow::Borrowed(text);
    }
    RE.get_or_init(|| regex::Regex::new(r"(?s)<system-reminder>.*?</system-reminder>").expect("valid"))
        .replace_all(text, "")
}

/// `content_search` with the ripgrep binary passed in, so a missing one can be tested.
fn content_search_with(
    rg: Option<&str>,
    term: &str,
    roots: &[PathBuf],
) -> Result<HashSet<PathBuf>, String> {
    if term.is_empty() {
        return Err("nothing to search for".into());
    }
    let rg = rg.ok_or_else(|| format!("ripgrep (rg) is not installed · {INSTALL_HINT}"))?;
    // A root that does not exist (no Copilot installed) would make rg exit 2 — a failure — so it
    // is left out rather than passed along.
    let present: Vec<&Path> = roots.iter().map(PathBuf::as_path).filter(|r| r.is_dir()).collect();
    if present.is_empty() {
        return Ok(HashSet::new());
    }
    let out = Command::new(rg)
        .args(["-l", "-i", "-F", "--glob", "*.jsonl", "--", term])
        .args(&present)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run rg: {e}"))?;

    // rg exits 1 with no matches — that is an empty result set, not a failure.
    if !out.status.success() && out.status.code() != Some(1) {
        return Err(failure_reason(out.status.code(), &out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// One short line saying why rg failed: its exit code and the first thing it said.
fn failure_reason(code: Option<i32>, stderr: &[u8]) -> String {
    let said = String::from_utf8_lossy(stderr);
    let first = said.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let first: String = first.chars().take(120).collect();
    let code = code.map_or_else(|| "was killed".to_string(), |c| format!("exited {c}"));
    if first.is_empty() {
        format!("rg {code}")
    } else {
        format!("rg {code}: {first}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_term_searches_nothing() {
        assert!(content_search("", &[PathBuf::from("/tmp")]).is_err());
    }

    #[test]
    fn a_missing_ripgrep_says_how_to_install_it() {
        let e = content_search_with(None, "x", &[PathBuf::from("/tmp")]).unwrap_err();
        assert!(e.contains(INSTALL_HINT), "{e}");
    }

    fn transcript(name: &str, lines: &[serde_json::Value]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sessio-convo-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("s.jsonl");
        std::fs::write(&f, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        f
    }

    #[test]
    fn injected_context_is_not_the_conversation() {
        use serde_json::json;
        let f = transcript("injected", &[
            json!({"type": "attachment", "attachment": {"type": "skill_listing", "content": "- card-reconcile: Revolut bills"}}),
            json!({"type": "api-request-blob", "message": {"content": [{"type": "text", "text": "card-reconcile"}]}}),
            json!({"type": "user", "isMeta": true, "message": {"content": "card-reconcile"}}),
            json!({"type": "user", "message": {"content": [{"type": "text",
                "text": "<system-reminder>skills: card-reconcile</system-reminder>hello"}]}}),
            json!({"type": "assistant", "attributionSkill": "card-reconcile",
                "message": {"content": [{"type": "thinking", "thinking": "card-reconcile?"}]}}),
        ]);
        for term in ["card-reconcile", "Revolut"] {
            assert!(!in_conversation(&f, &Needle::new(term)), "{term}");
        }
        assert!(in_conversation(&f, &Needle::new("HELLO")));
    }

    #[test]
    fn a_copilot_session_is_read_the_same_way() {
        use serde_json::json;
        let dir = std::env::temp_dir().join(format!("sessio-convo-{}-copilot", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("events.jsonl");
        let ev = |t: &str, d: serde_json::Value| json!({"type": t, "data": d});
        let lines = [
            ev("system.message", json!({"content": "<skill><name>card-reconcile</name></skill>"})),
            ev("model.messages_snapshot", json!({"messages": [{"content": "card-reconcile"}]})),
            ev("user.message", json!({"content": "hi there", "transformedContent": "card-reconcile hi there"})),
            ev("assistant.message", json!({"content": "Hello back."})),
            ev("tool.execution_start", json!({"arguments": {"command": "cargo build --zebra"}})),
            ev("tool.execution_complete", json!({"result": {"content": "error: unknown flag"}})),
        ];
        std::fs::write(&f, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        assert!(!in_conversation(&f, &Needle::new("card-reconcile")));
        for term in ["hi there", "hello back", "--zebra", "unknown flag"] {
            assert!(in_conversation(&f, &Needle::new(term)), "{term}");
        }
    }

    #[test]
    fn the_conversation_is_found_wherever_it_is() {
        use serde_json::json;
        let f = transcript("found", &[
            json!({"type": "user", "promptSource": "typed", "message": {"content": "fix the Login bug"}}),
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Fixed the ECONNRESET retry."}]}}),
            json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Bash",
                "input": {"command": "grep -r zebra-term src"}}]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result",
                "content": [{"type": "text", "text": "src/a.rs: panicked at Ünïcode"}]}]}}),
            json!({"type": "attachment", "attachment": {"type": "queued_command", "prompt": "also the queued-thing"}}),
            json!({"type": "ai-title", "aiTitle": "Parser refactor"}),
            json!({"type": "user", "message": {"content": "say \"quoted\" back"}}),
        ]);
        for term in ["login", "econnreset", "zebra-term", "ünïcode", "queued-thing", "parser REFACTOR", "\"quoted\""] {
            assert!(in_conversation(&f, &Needle::new(term)), "{term}");
        }
        assert!(!in_conversation(&f, &Needle::new("absent")));
    }

    #[test]
    fn a_failure_says_why() {
        assert_eq!(failure_reason(Some(2), b"\nrg: bad glob\nmore\n"), "rg exited 2: rg: bad glob");
        assert_eq!(failure_reason(Some(2), b""), "rg exited 2");
        assert_eq!(failure_reason(None, b""), "rg was killed");
    }

    #[test]
    fn searches_every_root_and_skips_missing_ones() {
        let Some(_) = rg_path() else {
            return;
        };
        let base = std::env::temp_dir().join(format!("sessio-search-roots-{}", std::process::id()));
        let (claude, copilot) = (base.join("projects"), base.join("session-state"));
        std::fs::create_dir_all(claude.join("p")).unwrap();
        std::fs::create_dir_all(copilot.join("sid")).unwrap();
        let a = claude.join("p").join("a.jsonl");
        let b = copilot.join("sid").join("events.jsonl");
        std::fs::write(&a, "{\"type\":\"user\",\"message\":{\"content\":\"shared-needle\"}}\n").unwrap();
        std::fs::write(&b, "{\"type\":\"user.message\",\"data\":{\"content\":\"shared-needle\"}}\n").unwrap();

        let hits = content_search("shared-needle", &[claude.clone(), copilot.clone()]).unwrap();
        assert!(hits.contains(&a) && hits.contains(&b), "{hits:?}");
        // No ~/.copilot: still a search, not a failure.
        let hits = content_search("shared-needle", &[claude.clone(), base.join("absent")]).unwrap();
        assert_eq!(hits.len(), 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn finds_a_literal_match() {
        let Some(_) = rg_path() else {
            return; // ripgrep is optional; skip rather than fail the suite
        };
        let dir = std::env::temp_dir().join(format!("sessio-search-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.jsonl");
        std::fs::write(&f, "{\"type\":\"user\",\"message\":{\"content\":\"needle(1)\"}}\n").unwrap();

        // A regex metacharacter must be matched literally thanks to -F.
        let roots = [dir.clone()];
        let hits = content_search("needle(1)", &roots).unwrap();
        assert!(hits.contains(&f));
        assert!(content_search("absent", &roots).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
