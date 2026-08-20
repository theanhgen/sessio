//! Transcript parsing. Ports `head()` and `tail()` from bin/sessio.mjs:72-174.
//!
//! Both walk the JSONL line by line with a cheap `contains` prefilter before paying for
//! `serde_json`, exactly like the gated `string.includes` in the JS — that gating is what keeps
//! multi-megabyte tool-result lines from being parsed at all.

use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::safety::{js_trim, sanitize, valid_path};

/// Read at least this many lines before an early exit is allowed (JS: `n >= 400`).
const MIN_LINES: usize = 400;
/// Bytes of the file end scanned by `tail()`.
const TAIL_WINDOW: u64 = 65536;

#[derive(Debug, Default, Clone)]
pub struct Head {
    pub first: Option<String>,
    pub first_ts: Option<String>,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub custom: Option<String>,
    pub ai: Option<String>,
}

/// Why a session counts as "open".
///
/// A discriminant rather than a string, because the JS compared the *rendered label* against a
/// different literal than the one it wrote — which silently disabled the 3-day decay for the
/// whole life of the feature. A variant cannot drift from its own label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenReason {
    /// You typed something and Claude never answered.
    Unanswered,
    /// Claude's away-recap says the next move is yours.
    Recap,
    /// Claude ended on a question or proposal you didn't answer.
    CallToAction,
    /// The session's folder has uncommitted changes.
    GitWip,
}

impl OpenReason {
    pub fn label(self) -> &'static str {
        match self {
            OpenReason::Unanswered => "your prompt got no reply",
            OpenReason::Recap => "recap says your move",
            OpenReason::CallToAction => "Claude asked / proposed next",
            OpenReason::GitWip => "uncommitted changes",
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Tail {
    pub open: bool,
    pub reason: Option<OpenReason>,
    pub recap: Option<String>,
    pub recap_ts: Option<String>,
}

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// Claude writes an away-recap when you leave a session: `{type: "system", subtype:
/// "away_summary"}` with a goal / state / whose-move paragraph in `content`. Returns it with the
/// trailing UI hint stripped, which is noise in a preview.
fn away_recap(o: &Value) -> Option<String> {
    if str_field(o, "type") != Some("system") || str_field(o, "subtype") != Some("away_summary") {
        return None;
    }
    let raw = sanitize(o.get("content").and_then(Value::as_str)?);
    let trimmed = raw.trim_end();
    let body = trimmed
        .strip_suffix("(disable recaps in /config)")
        .unwrap_or(trimmed);
    let text = js_trim(body);
    (!text.is_empty()).then(|| text.to_string())
}

/// A recap *states* who owes the next move, where `cta` only pattern-matches a question mark.
pub fn recap_says_your_move(recap: &str) -> bool {
    recap.to_lowercase().contains("next action is yours")
}

/// Which `promptSource` values count as something a human actually asked.
///
/// Gating on "typed" alone hid every background session: the task panel records its prompt as
/// "queued", and a session with no first prompt is dropped from the list entirely. "system" is
/// the one source that is not the user speaking.
fn is_human_prompt(o: &Value) -> bool {
    matches!(
        str_field(o, "promptSource"),
        Some("typed" | "queued" | "suggestion_accepted")
    )
}

/// Light head read for the list: first typed prompt, cwd, branch, and any early title.
/// Stops once 400 lines are in and first+cwd are known, so giant transcripts aren't fully read.
pub fn head(path: &Path) -> Head {
    let mut h = Head::default();
    let Ok(file) = File::open(path) else {
        return h;
    };
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut buf = Vec::with_capacity(8192);
    let mut n = 0usize;

    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = line_str(&buf);
        n += 1;

        // NB: an else-if chain in the JS — a title line is never also checked for a prompt.
        if line.contains("-title\"") {
            if let Ok(o) = serde_json::from_str::<Value>(&line) {
                match str_field(&o, "type") {
                    Some("custom-title") => {
                        if let Some(t) = str_field(&o, "customTitle").filter(|t| !t.is_empty()) {
                            h.custom = Some(sanitize(t)); // last one wins
                        }
                    }
                    Some("ai-title") => {
                        if h.ai.is_none() {
                            if let Some(t) = str_field(&o, "aiTitle").filter(|t| !t.is_empty()) {
                                h.ai = Some(sanitize(t)); // first one wins
                            }
                        }
                    }
                    _ => {}
                }
            }
        } else if line.contains("\"promptSource\":\"") {
            if let Ok(o) = serde_json::from_str::<Value>(&line) {
                if str_field(&o, "type") == Some("user") && is_human_prompt(&o) {
                    if let Some(msg) = o.get("message").filter(|m| !m.is_null()) {
                        // Only a *string* content counts; array content is tool output.
                        let text = msg
                            .get("content")
                            .and_then(Value::as_str)
                            .map(sanitize)
                            .unwrap_or_default();
                        // `startsWith` is checked on the untrimmed text, as in the JS.
                        if !js_trim(&text).is_empty() && !text.starts_with('<') {
                            if h.first.is_none() {
                                h.first = Some(js_trim(&text).to_string());
                                h.first_ts = str_field(&o, "timestamp").map(str::to_string);
                            }
                            if h.cwd.is_none() {
                                if let Some(c) = str_field(&o, "cwd").filter(|c| !c.is_empty()) {
                                    h.cwd = valid_path(c);
                                }
                            }
                            if h.branch.is_none() {
                                if let Some(b) = str_field(&o, "gitBranch").filter(|b| !b.is_empty())
                                {
                                    h.branch = Some(sanitize(b));
                                }
                            }
                        }
                    }
                }
            }
        } else if h.cwd.is_none() && line.contains("\"cwd\":\"") {
            if let Ok(o) = serde_json::from_str::<Value>(&line) {
                if let Some(c) = str_field(&o, "cwd").filter(|c| !c.is_empty()) {
                    h.cwd = valid_path(c);
                }
            }
        }

        if n >= MIN_LINES && h.first.is_some() && h.cwd.is_some() {
            break;
        }
    }
    h
}

/// Cheap tail read (last ~64KB) to tell whether a session is "open": who spoke last, and — if
/// Claude — whether it ended on a question or proposal you didn't answer.
pub fn tail(path: &Path) -> Tail {
    let mut t = Tail::default();
    let Ok(mut file) = File::open(path) else {
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

    let mut reply: Option<String> = None;
    let mut reply_ts: Option<String> = None;
    let mut user_ts: Option<String> = None;

    // The first line of a mid-file window is usually a fragment; it simply fails to parse and is
    // skipped, matching the JS try/catch.
    for chunk in raw.split(|b| *b == b'\n') {
        let line = line_str(chunk);
        if line.contains("\"away_summary\"") {
            if let Ok(o) = serde_json::from_str::<Value>(&line) {
                if let Some(text) = away_recap(&o) {
                    t.recap = Some(text); // the last one in the window wins
                    if let Some(ts) = str_field(&o, "timestamp") {
                        t.recap_ts = Some(ts.to_string());
                    }
                }
            }
        } else if line.contains("\"type\":\"assistant\"") && line.contains("\"type\":\"text\"") {
            if let Ok(o) = serde_json::from_str::<Value>(&line) {
                if str_field(&o, "type") == Some("assistant") {
                    if let Some(blocks) = o
                        .get("message")
                        .and_then(|m| m.get("content"))
                        .and_then(Value::as_array)
                    {
                        let joined = blocks
                            .iter()
                            .filter(|b| str_field(b, "type") == Some("text"))
                            .map(|b| str_field(b, "text").unwrap_or(""))
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        let text = sanitize(&joined);
                        if !js_trim(&text).is_empty() {
                            reply = Some(js_trim(&text).to_string());
                            if let Some(ts) = str_field(&o, "timestamp") {
                                reply_ts = Some(ts.to_string());
                            }
                        }
                    }
                }
            }
        } else if line.contains("\"promptSource\":\"") {
            if let Ok(o) = serde_json::from_str::<Value>(&line) {
                let text = o
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(Value::as_str)
                    .map(sanitize)
                    .unwrap_or_default();
                if str_field(&o, "type") == Some("user")
                    && is_human_prompt(&o)
                    && !js_trim(&text).is_empty()
                    && !text.starts_with('<')
                {
                    if let Some(ts) = str_field(&o, "timestamp") {
                        user_ts = Some(ts.to_string());
                    }
                }
            }
        }
    }

    // JS: `T(x) = x ? new Date(x).getTime() : 0`, and an unparseable date yields NaN, which makes
    // every comparison false. `None` from `parse_iso_ms` models exactly that.
    let unanswered = match &user_ts {
        Some(u) => {
            let tr = reply_ts.as_deref().and_then(parse_iso_ms).unwrap_or(0);
            matches!(parse_iso_ms(u), Some(tu) if tu > tr)
        }
        None => false,
    };
    // A recap that predates Claude's last reply has been overtaken by it, so it only counts when
    // it is at least as new. `T(null) = 0` and an unparseable stamp yields NaN (all comparisons
    // false), exactly as in the JS.
    let tr = reply_ts.as_deref().and_then(parse_iso_ms).unwrap_or(0);
    let recap_is_current = match &t.recap_ts {
        Some(rt) => matches!(parse_iso_ms(rt), Some(x) if x >= tr),
        None => tr == 0,
    };
    let recap_open = t
        .recap
        .as_deref()
        .is_some_and(recap_says_your_move)
        && recap_is_current;

    if unanswered {
        t.open = true;
        t.reason = Some(OpenReason::Unanswered);
    } else if recap_open {
        t.open = true;
        t.reason = Some(OpenReason::Recap);
    } else if let Some(r) = &reply {
        if crate::cta::looks_like_call_to_action(r) {
            t.open = true;
            t.reason = Some(OpenReason::CallToAction);
        }
    }
    t
}

/// Decode a raw line as UTF-8 (lossy, matching Node's stream decoding) and drop a trailing CR.
fn line_str(bytes: &[u8]) -> String {
    let mut s = String::from_utf8_lossy(bytes).into_owned();
    if s.ends_with('\n') {
        s.pop();
    }
    if s.ends_with('\r') {
        s.pop();
    }
    s
}

/// Epoch milliseconds from an ISO-8601 UTC timestamp (`2026-08-05T16:09:02.072Z`) — the only
/// shape Claude writes. Anything else returns `None`, which reproduces JS `NaN` comparison
/// semantics rather than guessing at an offset.
pub fn parse_iso_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || !s.ends_with('Z') {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let millis = if b[19] == b'.' {
        let frac: String = s[20..s.len() - 1].chars().take(3).collect();
        format!("{frac:0<3}").parse::<i64>().ok()?
    } else {
        0
    };
    Some(days_from_civil(y, mo, d) * 86_400_000 + h * 3_600_000 + mi * 60_000 + sec * 1000 + millis)
}

/// Howard Hinnant's civil-date algorithm: days since 1970-01-01.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_timestamps() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00.000Z"), Some(0));
        // Cross-checked against Date.parse in node.
        assert_eq!(parse_iso_ms("2026-08-05T16:09:02.072Z"), Some(1785946142072));
        assert_eq!(parse_iso_ms("2026-08-05T16:09:02Z"), Some(1785946142000));
    }

    #[test]
    fn rejects_shapes_we_do_not_model() {
        assert_eq!(parse_iso_ms(""), None);
        assert_eq!(parse_iso_ms("2026-08-05T16:09:02+02:00"), None);
        assert_eq!(parse_iso_ms("not a date"), None);
    }

    #[test]
    fn strips_crlf() {
        assert_eq!(line_str(b"hello\r\n"), "hello");
        assert_eq!(line_str(b"hello\n"), "hello");
        assert_eq!(line_str(b"hello"), "hello");
    }

    #[test]
    fn away_recap_is_extracted_without_the_config_hint() {
        let line = serde_json::json!({
            "type": "system",
            "subtype": "away_summary",
            "content": "Goal was X; that's done. Next action is yours: ship it. (disable recaps in /config)",
        });
        let got = away_recap(&line).expect("recap");
        assert_eq!(got, "Goal was X; that's done. Next action is yours: ship it.");
    }

    #[test]
    fn other_system_entries_are_not_recaps() {
        for v in [
            serde_json::json!({"type": "system", "subtype": "hook", "content": "nope"}),
            serde_json::json!({"type": "assistant", "subtype": "away_summary", "content": "nope"}),
            serde_json::json!({"type": "system", "subtype": "away_summary", "content": "   "}),
        ] {
            assert!(away_recap(&v).is_none(), "{v}");
        }
    }

    #[test]
    fn recap_flags_a_session_only_when_the_move_is_yours() {
        assert!(recap_says_your_move("... Next action is yours: email HR."));
        assert!(recap_says_your_move("NEXT ACTION IS YOURS - do the thing"));
        assert!(!recap_says_your_move("Goal was X; done, nothing pending."));
    }
}

/// Full read for the DETAIL panel of the highlighted session only (lazy). Port of `detail()`
/// at bin/sessio.mjs:92-146.
///
/// Two deliberate inconsistencies with `head()` are preserved because the JS has them:
/// `ai` is last-wins here but first-wins in `head()`, and `reply_ts` is *cleared* when a later
/// reply carries no timestamp, where `tail()` keeps the previous value.
#[derive(Debug, Default, Clone)]
pub struct Detail {
    pub cwd: Option<String>,
    pub first: Option<String>,
    pub last: Option<String>,
    pub first_ts: Option<String>,
    pub last_ts: Option<String>,
    pub count: usize,
    pub custom: Option<String>,
    pub ai: Option<String>,
    pub branch: Option<String>,
    pub summary: Option<String>,
    pub summary_ts: Option<String>,
    pub reply: Option<String>,
    pub reply_ts: Option<String>,
    pub recap: Option<String>,
    pub recap_ts: Option<String>,
}

pub fn detail(path: &Path) -> Detail {
    let mut d = Detail::default();
    let Ok(file) = File::open(path) else {
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

        if line.contains("\"away_summary\"") {
            // Claude's away-recap; the last one in the file wins.
            if let Ok(o) = serde_json::from_str::<Value>(&line) {
                if let Some(text) = away_recap(&o) {
                    d.recap = Some(text);
                    d.recap_ts = str_field(&o, "timestamp").map(str::to_string);
                }
            }
            continue;
        }

        if line.contains("-title\"") {
            if let Ok(o) = serde_json::from_str::<Value>(&line) {
                match str_field(&o, "type") {
                    Some("custom-title") => {
                        if let Some(t) = str_field(&o, "customTitle").filter(|t| !t.is_empty()) {
                            d.custom = Some(sanitize(t));
                        }
                    }
                    Some("ai-title") => {
                        if let Some(t) = str_field(&o, "aiTitle").filter(|t| !t.is_empty()) {
                            d.ai = Some(sanitize(t)); // last wins, unlike head()
                        }
                    }
                    _ => {}
                }
            }
            continue;
        }

        // Auto-compact recap; keep the latest and strip the boilerplate preamble.
        if line.contains("\"isCompactSummary\":true") {
            if let Ok(o) = serde_json::from_str::<Value>(&line) {
                // Exact field, not a mention of the word somewhere in the payload.
                if o.get("isCompactSummary") == Some(&Value::Bool(true)) {
                    let content = o.get("message").and_then(|m| m.get("content"));
                    let t = match content {
                        Some(Value::String(s)) => s.clone(),
                        Some(Value::Array(a)) => a
                            .iter()
                            .map(|x| str_field(x, "text").unwrap_or(""))
                            .collect::<Vec<_>>()
                            .join("\n\n"),
                        _ => String::new(),
                    };
                    if !t.is_empty() {
                        let body = match t.find("Summary:") {
                            Some(i) => &t[i + 8..],
                            None => &t[..],
                        };
                        d.summary = Some(js_trim(&sanitize(body)).to_string());
                        d.summary_ts = str_field(&o, "timestamp").map(str::to_string);
                    }
                }
            }
            continue;
        }

        if line.contains("\"type\":\"assistant\"") && line.contains("\"type\":\"text\"") {
            if let Ok(o) = serde_json::from_str::<Value>(&line) {
                if str_field(&o, "type") == Some("assistant") {
                    if let Some(blocks) = o
                        .get("message")
                        .and_then(|m| m.get("content"))
                        .and_then(Value::as_array)
                    {
                        let joined = blocks
                            .iter()
                            .filter(|b| str_field(b, "type") == Some("text"))
                            .map(|b| str_field(b, "text").unwrap_or(""))
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        let text = sanitize(&joined);
                        if !js_trim(&text).is_empty() {
                            d.reply = Some(js_trim(&text).to_string());
                            d.reply_ts = str_field(&o, "timestamp").map(str::to_string);
                        }
                        if d.cwd.is_none() {
                            if let Some(c) = str_field(&o, "cwd").filter(|c| !c.is_empty()) {
                                d.cwd = valid_path(c);
                            }
                        }
                    }
                }
            }
            continue;
        }

        if line.contains("\"promptSource\":\"") {
            if let Ok(o) = serde_json::from_str::<Value>(&line) {
                if str_field(&o, "type") == Some("user") && is_human_prompt(&o) {
                    if let Some(msg) = o.get("message").filter(|m| !m.is_null()) {
                        let text = msg
                            .get("content")
                            .and_then(Value::as_str)
                            .map(sanitize)
                            .unwrap_or_default();
                        if !js_trim(&text).is_empty() && !text.starts_with('<') {
                            d.count += 1;
                            if d.first.is_none() {
                                d.first = Some(js_trim(&text).to_string());
                                d.first_ts = str_field(&o, "timestamp").map(str::to_string);
                            }
                            d.last = Some(js_trim(&text).to_string());
                            d.last_ts = str_field(&o, "timestamp").map(str::to_string);
                            if d.cwd.is_none() {
                                if let Some(c) = str_field(&o, "cwd").filter(|c| !c.is_empty()) {
                                    d.cwd = valid_path(c);
                                }
                            }
                            if d.branch.is_none() {
                                if let Some(b) =
                                    str_field(&o, "gitBranch").filter(|b| !b.is_empty())
                                {
                                    d.branch = Some(sanitize(b));
                                }
                            }
                        }
                    }
                }
            }
            continue;
        }

        if d.cwd.is_none() && line.contains("\"cwd\":\"") {
            if let Ok(o) = serde_json::from_str::<Value>(&line) {
                if let Some(c) = str_field(&o, "cwd").filter(|c| !c.is_empty()) {
                    d.cwd = valid_path(c);
                }
            }
        }
    }
    d
}
