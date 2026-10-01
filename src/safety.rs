//! Transcript content is untrusted display data. Port of legacy/lib/safety.mjs.
//!
//! This is a security control, not formatting: transcripts contain text an attacker can
//! influence, and raw escape sequences reaching the terminal enable OSC-52 clipboard writes
//! and cursor manipulation. Keep only printable characters plus line feeds.

/// Strip C0 controls except `\n` (U+000A), plus DEL and the C1 range.
/// Mirrors the JS regex `/[\u0000-\u0009\u000B-\u001F\u007F-\u009F]/g`.
pub fn sanitize(value: &str) -> String {
    value
        .chars()
        .filter(|c| {
            let n = *c as u32;
            !((n <= 0x09) || (0x0B..=0x1F).contains(&n) || (0x7F..=0x9F).contains(&n))
        })
        .collect()
}

/// `validPath` in the JS: accept the value only if sanitizing leaves it unchanged.
/// A path that needed stripping is treated as absent rather than silently repaired.
pub fn valid_path(value: &str) -> Option<String> {
    if sanitize(value) == value {
        Some(value.to_string())
    } else {
        None
    }
}

/// `String.prototype.trim()` semantics, which are *not* Rust's `str::trim`.
///
/// ECMAScript's WhiteSpace production includes U+FEFF (ZWNBSP); Unicode's `White_Space`
/// property dropped it in 4.0.1, so `str::trim` leaves it behind. Real transcripts contain it
/// — text pasted from the web arrives with trailing BOMs — and the oracle caught exactly this
/// on 1 of 101 sessions.
///
/// The reverse gap (Rust trims U+0085 NEL, JS does not) cannot bite us: `sanitize` strips the
/// whole U+007F–U+009F range before any of this runs.
pub fn js_trim(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}')
}

#[cfg(test)]
mod tests {
    use super::*;

    // The vector from test/sessio.test.mjs:17 — keep both implementations honest.
    #[test]
    fn removes_control_sequences() {
        let raw = "safe\x1b]52;c;secret\x07 text\r\nnext\u{0}";
        assert_eq!(sanitize(raw), "safe]52;c;secret text\nnext");
    }

    #[test]
    fn keeps_line_feeds_only() {
        assert_eq!(sanitize("a\nb\tc\rd"), "a\nbcd");
    }

    // Regression: the oracle caught this on a real transcript whose pasted web content ended
    // with "\n\u{FEFF} \u{FEFF}". str::trim leaves those; JS trim removes them.
    #[test]
    fn js_trim_strips_zwnbsp_like_javascript() {
        assert_eq!(js_trim("TERMINATOR\n\u{FEFF} \u{FEFF}"), "TERMINATOR");
        assert_eq!(js_trim("\u{FEFF}hello\u{FEFF}"), "hello");
        assert_eq!(js_trim("  spaced  "), "spaced");
        assert_eq!(js_trim("\u{FEFF}\u{FEFF}"), "");
    }

    #[test]
    fn valid_path_rejects_control_characters() {
        assert_eq!(valid_path("/Users/me/work"), Some("/Users/me/work".into()));
        assert_eq!(valid_path("/Users/me\x07/work"), None);
    }
}
