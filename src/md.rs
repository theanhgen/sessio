//! Minimal markdown → ratatui renderer, so replies and summaries look the way Claude renders
//! them. Port of `inlineMd` / `wrapMd` / `renderTable` / `mdLines` (bin/sessio.mjs:368-441).
//!
//! The JS builds one string with embedded ANSI and then has to `stripAnsi` it again to measure
//! width. Styling as spans removes that round trip, and `unicode-width` fixes the JS's use of
//! `.length`, which mis-measures emoji and CJK. Output is therefore *not* byte-identical to the
//! JS — the golden tests below pin this implementation instead.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::ui::theme;

/// An inline run of text with one style.
#[derive(Debug, Clone, PartialEq)]
struct Seg {
    text: String,
    style: Style,
}

fn seg(text: impl Into<String>, style: Style) -> Seg {
    Seg { text: text.into(), style }
}

/// Parse inline markdown: `**bold**`, `__bold__`, `` `code` ``, `[text](url)` → text.
///
/// The JS runs four sequential regex replacements; this is a single left-to-right scan, which
/// behaves the same on real content and nests more sensibly on pathological input.
fn inline(s: &str, base: Style) -> Vec<Seg> {
    let chars: Vec<char> = s.chars().collect();
    let mut out: Vec<Seg> = Vec::new();
    let mut plain = String::new();
    let mut i = 0;

    macro_rules! flush {
        () => {
            if !plain.is_empty() {
                out.push(seg(std::mem::take(&mut plain), base));
            }
        };
    }

    while i < chars.len() {
        // **bold** / __bold__ — non-greedy, must not be empty
        if let Some((inner, next)) = fenced(&chars, i, "**").or_else(|| fenced(&chars, i, "__")) {
            flush!();
            out.push(seg(inner, base.add_modifier(Modifier::BOLD)));
            i = next;
            continue;
        }
        // `inline code`
        if chars[i] == '`' {
            if let Some(end) = (i + 1..chars.len()).find(|&j| chars[j] == '`') {
                if end > i + 1 {
                    flush!();
                    out.push(seg(
                        chars[i + 1..end].iter().collect::<String>(),
                        base.fg(theme::CODE),
                    ));
                    i = end + 1;
                    continue;
                }
            }
        }
        // [text](url) — keep the text, drop the target
        if chars[i] == '[' {
            if let Some((text, next)) = link(&chars, i) {
                flush!();
                out.push(seg(text, base.fg(theme::CODE)));
                i = next;
                continue;
            }
        }
        plain.push(chars[i]);
        i += 1;
    }
    flush!();
    out
}

/// Match `<delim>inner<delim>` starting at `i`, returning the inner text and the index after.
fn fenced(chars: &[char], i: usize, delim: &str) -> Option<(String, usize)> {
    let d: Vec<char> = delim.chars().collect();
    if chars.len() < i + d.len() * 2 + 1 || chars[i..].len() < d.len() || chars[i..i + d.len()] != d[..] {
        return None;
    }
    let start = i + d.len();
    let mut j = start;
    while j + d.len() <= chars.len() {
        if chars[j..j + d.len()] == d[..] {
            if j > start {
                return Some((chars[start..j].iter().collect(), j + d.len()));
            }
            return None;
        }
        j += 1;
    }
    None
}

fn link(chars: &[char], i: usize) -> Option<(String, usize)> {
    let close = (i + 1..chars.len()).find(|&j| chars[j] == ']')?;
    if close == i + 1 || chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let paren = (close + 2..chars.len()).find(|&j| chars[j] == ')')?;
    Some((chars[i + 1..close].iter().collect(), paren + 1))
}

fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn segs_width(segs: &[Seg]) -> usize {
    segs.iter().map(|s| width(&s.text)).sum()
}

/// Word-wrap styled segments to `width`, with distinct first-line and continuation prefixes.
fn wrap_segs(segs: Vec<Seg>, w: usize, first: &str, cont: &str) -> Vec<Line<'static>> {
    let w = w.max(1);
    // Explode into words carrying their style, so a wrap point can fall inside a styled run.
    let mut words: Vec<Seg> = Vec::new();
    for s in segs {
        for part in s.text.split_whitespace() {
            // A word wider than the viewport can never fit on a line of its own, so break it
            // at a character boundary. The JS wraps on whitespace only and lets such a word
            // overflow — which silently clips text that has no spaces at all, i.e. most CJK.
            if width(part) > w {
                let mut rest = part.to_string();
                while width(&rest) > w {
                    let (head, tail) = split_at_width(&rest, w);
                    words.push(seg(head, s.style));
                    rest = tail;
                }
                if !rest.is_empty() {
                    words.push(seg(rest, s.style));
                }
            } else {
                words.push(seg(part, s.style));
            }
        }
    }

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cur: Vec<Seg> = vec![seg(first.to_string(), Style::default())];
    let mut started = false;

    for word in words {
        let extra = if started { 1 } else { 0 };
        if started && segs_width(&cur) + extra + width(&word.text) > w {
            lines.push(to_line(std::mem::take(&mut cur)));
            cur.push(seg(cont.to_string(), Style::default()));
            cur.push(word);
        } else {
            if started {
                cur.push(seg(" ", Style::default()));
            }
            cur.push(word);
            started = true;
        }
    }
    lines.push(to_line(cur));
    lines
}

fn to_line(segs: Vec<Seg>) -> Line<'static> {
    Line::from(
        segs.into_iter()
            .filter(|s| !s.text.is_empty())
            .map(|s| Span::styled(s.text, s.style))
            .collect::<Vec<Span>>(),
    )
}

/// Truncate to an exact display width, appending `…` when it doesn't fit.
fn fit(raw: &str, w: usize) -> String {
    if width(raw) <= w {
        return format!("{raw}{}", " ".repeat(w - width(raw)));
    }
    let mut out = String::new();
    let mut used = 0;
    for c in raw.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + cw > w.saturating_sub(1) {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    let _ = used;
    format!("{out}{}", " ".repeat(w.saturating_sub(width(&out))))
}

/// Render a markdown table. `rows[0]` is the header; the separator row is already dropped.
/// Columns shrink widest-first until the table fits, never below 6, as in the JS.
fn render_table(rows: &[Vec<String>], w: usize) -> Vec<Line<'static>> {
    let ncol = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if ncol == 0 {
        return Vec::new();
    }
    let padded: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            let mut c = r.clone();
            c.resize(ncol, String::new());
            c
        })
        .collect();

    let mut widths: Vec<usize> = (0..ncol)
        .map(|i| {
            padded
                .iter()
                .map(|r| segs_width(&inline(&r[i], Style::default())))
                .max()
                .unwrap_or(1)
                .max(1)
        })
        .collect();

    let gap = 2 * ncol.saturating_sub(1);
    let mut over = (widths.iter().sum::<usize>() + gap) as i64 - w as i64;
    while over > 0 {
        let (mi, _) = widths.iter().enumerate().max_by_key(|(_, v)| **v).unwrap();
        if widths[mi] <= 6 {
            break;
        }
        widths[mi] -= 1;
        over -= 1;
    }

    let mut out = Vec::new();
    out.push(row_line(&padded[0], &widths, Style::default().add_modifier(Modifier::BOLD)));
    let rule: usize = (widths.iter().sum::<usize>() + gap).min(w);
    out.push(Line::from(Span::styled(
        "─".repeat(rule),
        Style::default().fg(theme::DIM),
    )));
    for r in &padded[1..] {
        out.push(row_line(r, &widths, Style::default()));
    }
    out
}

fn row_line(cells: &[String], widths: &[usize], base: Style) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, c) in cells.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        let styled = inline(c, base);
        if segs_width(&styled) <= widths[i] {
            let pad = widths[i] - segs_width(&styled);
            for s in styled {
                spans.push(Span::styled(s.text, s.style));
            }
            if pad > 0 {
                spans.push(Span::raw(" ".repeat(pad)));
            }
        } else {
            // Truncating drops styling, as in the JS.
            spans.push(Span::styled(fit(c, widths[i]), base));
        }
    }
    Line::from(spans)
}

fn is_table_row(s: &str) -> bool {
    let t = s.trim();
    t.starts_with('|') && t.ends_with('|') && t.len() > 1
}

fn is_separator(s: &str) -> bool {
    s.contains('|')
        && s.contains('-')
        && s
            .trim()
            .chars()
            .all(|c| c == '|' || c == '-' || c == ':' || c.is_whitespace())
}

fn cells(s: &str) -> Vec<String> {
    s.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|c| c.trim().to_string())
        .collect()
}

/// Render markdown to styled lines, wrapped to `w`.
pub fn md_lines(text: &str, w: usize) -> Vec<Line<'static>> {
    let w = w.max(1); // a 1-2 column terminal would otherwise loop forever in the hard-wrap
    let raw: Vec<&str> = text.split('\n').collect();
    let mut res: Vec<Line<'static>> = Vec::new();
    let mut i = 0;

    while i < raw.len() {
        let line = raw[i].trim_end();

        // Fenced code: verbatim, so ** and ` inside code aren't mangled.
        if line.trim_start().starts_with("```") {
            let mut j = i + 1;
            while j < raw.len() && !raw[j].trim_start().starts_with("```") {
                let mut code = raw[j].replace('\t', "  ").trim_end().to_string();
                if code.is_empty() {
                    res.push(Line::from(""));
                } else {
                    while !code.is_empty() {
                        let (head, rest) = split_at_width(&code, w);
                        res.push(Line::from(Span::styled(head, Style::default().fg(theme::DIM))));
                        code = rest;
                    }
                }
                j += 1;
            }
            i = j + 1; // skip the closing fence (or run to end if unterminated)
            continue;
        }

        // Table: a row followed by a separator row.
        if is_table_row(line) && i + 1 < raw.len() && is_separator(raw[i + 1]) {
            let mut block = vec![cells(line)];
            let mut j = i + 2;
            while j < raw.len() && is_table_row(raw[j]) {
                block.push(cells(raw[j]));
                j += 1;
            }
            res.extend(render_table(&block, w));
            i = j;
            continue;
        }

        if line.trim().is_empty() {
            res.push(Line::from(""));
            i += 1;
            continue;
        }

        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];

        if let Some(rest) = heading(trimmed) {
            res.push(to_line(inline(rest, Style::default().add_modifier(Modifier::BOLD))));
        } else if let Some(rest) = bullet(trimmed) {
            res.extend(wrap_segs(
                inline(rest, Style::default()),
                w,
                &format!("{indent}• "),
                &format!("{indent}  "),
            ));
        } else if let Some((num, rest)) = ordered(trimmed) {
            let prefix = format!("{indent}{num}. ");
            let cont = " ".repeat(width(&prefix));
            res.extend(wrap_segs(inline(rest, Style::default()), w, &prefix, &cont));
        } else {
            res.extend(wrap_segs(inline(line, Style::default()), w, "", ""));
        }
        i += 1;
    }

    // Collapse consecutive blanks and trim the end, so the height budget isn't spent on nothing.
    let mut out: Vec<Line<'static>> = Vec::new();
    for l in res {
        let blank = line_is_blank(&l);
        if blank && out.last().map(line_is_blank).unwrap_or(true) {
            continue;
        }
        out.push(l);
    }
    while out.last().map(line_is_blank).unwrap_or(false) {
        out.pop();
    }
    out
}

fn line_is_blank(l: &Line<'_>) -> bool {
    l.spans.iter().all(|s| s.content.trim().is_empty())
}

fn split_at_width(s: &str, w: usize) -> (String, String) {
    let mut head = String::new();
    let mut used = 0;
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + cw > w {
            break;
        }
        head.push(c);
        used += cw;
        chars.next();
    }
    if head.is_empty() {
        // A single character wider than the viewport: take it anyway to guarantee progress.
        if let Some(c) = chars.next() {
            head.push(c);
        }
    }
    (head, chars.collect())
}

fn heading(s: &str) -> Option<&str> {
    let hashes = s.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) {
        let rest = &s[hashes..];
        if rest.starts_with(' ') || rest.starts_with('\t') {
            return Some(rest.trim_start());
        }
    }
    None
}

fn bullet(s: &str) -> Option<&str> {
    let mut c = s.chars();
    match c.next() {
        Some('-') | Some('*') | Some('+') => {}
        _ => return None,
    }
    let rest = &s[1..];
    if rest.starts_with(' ') || rest.starts_with('\t') {
        Some(rest.trim_start())
    } else {
        None
    }
}

fn ordered(s: &str) -> Option<(String, &str)> {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = &s[digits.len()..];
    let rest = rest.strip_prefix('.')?;
    if rest.starts_with(' ') || rest.starts_with('\t') {
        Some((digits, rest.trim_start()))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect()
    }

    #[test]
    fn renders_headings_bullets_and_ordered_lists() {
        let out = md_lines("# Title\n\n- one\n- two\n\n1. first\n2. second", 40);
        assert_eq!(
            plain(&out),
            vec!["Title", "", "• one", "• two", "", "1. first", "2. second"]
        );
    }

    #[test]
    fn strips_inline_markup_but_keeps_the_text() {
        let out = md_lines("a **bold** and `code` and [link](http://x)", 80);
        assert_eq!(plain(&out), vec!["a bold and code and link"]);
    }

    #[test]
    fn bold_is_actually_styled() {
        let out = md_lines("**loud**", 80);
        assert!(out[0].spans.iter().any(|s| s.style.add_modifier.contains(Modifier::BOLD)));
    }

    #[test]
    fn fenced_code_is_verbatim() {
        let out = md_lines("```\nlet x = **not bold**;\n```", 80);
        assert_eq!(plain(&out), vec!["let x = **not bold**;"]);
    }

    #[test]
    fn unterminated_fence_runs_to_the_end() {
        let out = md_lines("```\nstill code\nmore code", 80);
        assert_eq!(plain(&out), vec!["still code", "more code"]);
    }

    #[test]
    fn code_hard_wraps_at_width() {
        let out = md_lines("```\nabcdefghij\n```", 4);
        assert_eq!(plain(&out), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn renders_a_table_with_a_rule() {
        let out = md_lines("| a | b |\n|---|---|\n| 1 | 2 |", 40);
        let p = plain(&out);
        assert_eq!(p.len(), 3);
        assert!(p[0].starts_with('a'));
        assert!(p[1].chars().all(|c| c == '─'));
        assert!(p[2].starts_with('1'));
    }

    #[test]
    fn wraps_long_prose_to_width() {
        let out = md_lines("the quick brown fox jumps over the lazy dog", 12);
        assert!(out.len() > 1);
        for l in &out {
            assert!(width(&plain(std::slice::from_ref(l))[0]) <= 12);
        }
    }

    #[test]
    fn collapses_blank_runs_and_trims_the_tail() {
        let out = md_lines("a\n\n\n\nb\n\n\n", 40);
        assert_eq!(plain(&out), vec!["a", "", "b"]);
    }

    #[test]
    fn measures_wide_characters_by_display_width() {
        // The JS used .length here and would fit twice as many CJK cells as actually render.
        let out = md_lines("日本語テキストです", 6);
        for l in &out {
            assert!(width(&plain(std::slice::from_ref(l))[0]) <= 6);
        }
    }

    #[test]
    fn narrow_width_terminates() {
        // Guard against the hard-wrap infinite loop the JS comment at :404 warns about.
        let out = md_lines("```\nwwww\n```", 1);
        assert_eq!(out.len(), 4);
    }
}
