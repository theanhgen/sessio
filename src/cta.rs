//! "Did Claude end on a question or a proposal?" — port of the `CTA` regex at bin/sessio.mjs:150.

use regex::Regex;
use std::sync::OnceLock;

/// Only the tail of the reply is considered, so a question in the middle of a long answer
/// doesn't flag the session.
const WINDOW_CHARS: usize = 400;

fn cta() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?i)\?\s*["')\]]*\s*$|\b(want me to|should i|shall i|do you want|let me know|ready to|next step|proceed\b|which (one|of)|confirm)\b"#,
        )
        .expect("CTA pattern is a compile-time constant")
    })
}

/// JS slices the last 400 UTF-16 code units; this takes the last 400 `char`s. Identical for
/// ASCII, and differs only in how many astral characters (emoji) fit in the window.
pub fn looks_like_call_to_action(reply: &str) -> bool {
    let n = reply.chars().count();
    let start = n.saturating_sub(WINDOW_CHARS);
    let window: String = reply.chars().skip(start).collect();
    cta().is_match(&window)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_trailing_questions() {
        assert!(looks_like_call_to_action("So what do you think?"));
        assert!(looks_like_call_to_action("Ready to ship?)"));
        assert!(looks_like_call_to_action("Want me to start with the parser."));
        assert!(looks_like_call_to_action("Which one should it be"));
    }

    #[test]
    fn ignores_statements() {
        assert!(!looks_like_call_to_action("Done. The tests pass."));
        assert!(!looks_like_call_to_action("I fixed the bug and committed it."));
    }

    #[test]
    fn only_looks_at_the_tail() {
        let long = format!("Should I do it? {}", "x. ".repeat(400));
        assert!(!looks_like_call_to_action(&long));
    }
}
