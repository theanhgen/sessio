//! What ships beside the binary says the same keys the binary has.
//!
//! The `?` overlay is the source: every key it lists must also be in the README's key table, the
//! `KEYS` block of `sessions --help`, the website's key table and the design spec's keybinding
//! table. A key added to the overlay and nowhere else fails here, naming the documents it is
//! missing from. There is no allowlist: every key the overlay names is documented everywhere.

use super::*;

const README: &str = include_str!("../../README.md");
const CLI: &str = include_str!("../cli.rs");
const SITE: &str = include_str!("../../docs/index.html");
const DESIGN: &str = include_str!("../../docs/DESIGN.md");

/// The keys the `?` overlay names, in order, from the rows above its status legend.
fn overlay_keys() -> Vec<String> {
    let _pin = pin::set(pin::Env { rg: true, ..Default::default() });
    let rows: Vec<String> = help_lines(200, 80)
        .into_iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect();
    let end = rows.iter().position(|r| r.trim().is_empty()).expect("a blank row before the legend");
    let key = regex::Regex::new(r"\^[a-z]|⌥⌫|⌘⌫|PgUp|PgDn|\besc\b|[↑↓←→↵⇥?]").unwrap();
    let mut keys: Vec<String> = Vec::new();
    for r in &rows[1..end] {
        for m in key.find_iter(r) {
            if !keys.iter().any(|k| k == m.as_str()) {
                keys.push(m.as_str().to_string());
            }
        }
    }
    keys
}

/// The text of a markdown section, from its heading to the next heading of the same level.
fn section<'a>(doc: &'a str, heading: &str) -> &'a str {
    let start = doc.find(heading).unwrap_or_else(|| panic!("no {heading:?}"));
    let level = heading.chars().take_while(|&c| c == '#').count();
    let rest = &doc[start + heading.len()..];
    let next = rest
        .match_indices('\n')
        .map(|(i, _)| i + 1)
        .find(|&i| rest[i..].starts_with(&"#".repeat(level)) && !rest[i..].starts_with(&"#".repeat(level + 1)))
        .unwrap_or(rest.len());
    &rest[..next]
}

/// The `KEYS` block of `sessions --help`, split into its tokens.
fn cli_keys() -> Vec<&'static str> {
    let start = CLI.find("\nKEYS").expect("usage() has a KEYS block");
    let block = &CLI[start..];
    let block = &block[..block.find("\",").expect("the usage string ends")];
    block.split(|c: char| c.is_whitespace() || c == '/' || c == '·').filter(|t| !t.is_empty()).collect()
}

#[test]
fn the_overlay_lists_the_keys_it_should() {
    let keys = overlay_keys();
    // A floor, so a regex or layout change cannot quietly make the checks below vacuous.
    for k in ["↑", "←", "^w", "⌥⌫", "^u", "⌘⌫", "^f", "^a", "^r", "^t", "⇥", "^e", "PgUp", "PgDn", "^g", "↵", "^o", "^n", "^k", "?", "esc", "^c"] {
        assert!(keys.iter().any(|x| x == k), "{k} is not in the overlay's keys: {keys:?}");
    }
}

#[test]
fn every_overlay_key_is_documented_everywhere() {
    let readme = section(README, "## Keys");
    let design = section(DESIGN, "## Keybinding invariants");
    let site = &SITE[SITE.find("<table").expect("the site has a key table")..];
    let site = &site[..site.find("</table>").unwrap()];
    let cli = cli_keys();

    let mut missing = Vec::new();
    for k in overlay_keys() {
        let code = format!("`{k}`");
        let kbd = format!("<kbd>{k}</kbd>");
        for (doc, found) in [
            ("README.md ## Keys", readme.contains(&code)),
            ("sessions --help KEYS", cli.contains(&k.as_str())),
            ("docs/index.html key table", site.contains(&kbd)),
            ("docs/DESIGN.md keybinding table", design.contains(&code)),
        ] {
            if !found {
                missing.push(format!("{k} — {doc}"));
            }
        }
    }
    assert!(missing.is_empty(), "keys in the ? overlay that a shipped document leaves out:\n  {}", missing.join("\n  "));
}
