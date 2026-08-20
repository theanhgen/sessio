# Changelog

## 1.0.0-alpha.2 - 2026-08-20

Prerelease. Published under the `alpha` dist-tag — `npm i -g sessio` still installs 0.3.x.
Install it with `npm i -g sessio@alpha`.

- Rewrote sessio in Rust and ship it as a prebuilt binary per platform; the npm package is now
  a launcher that picks the right one. Node is needed to install, not to run. The original JS
  implementation is kept in `legacy/` as the reference `scripts/oracle.sh` diffs the port
  against, row for row.
- Added live-session detection: a `◉` marks a session with a `claude` process attached right
  now, and `↵` on one goes to that session — raising its window where it can be found, and
  otherwise naming the pid and tty — instead of pointing a second `claude` at the same
  transcript. A deliberate duplicate takes a second `↵`.
- Show Claude's away-recap in the preview, falling back to the compact summary when there is
  none, and mark a session open when the recap says the next move is yours. A recap that
  predates Claude's last reply no longer counts.
- Made archiving self-releasing: a session written to after you archived it comes back on the
  next refresh, so a hidden session you pick up again does not stay hidden.
- Added `^w` / `⌥⌫` to rub out a word of the query and `^u` (what `⌘⌫` sends) to clear it.
- Fixed the list height and the preview's first row so switching project tabs changes the text
  and nothing else.
- Made the key bar shed hints from the most expendable end rather than overflow a narrow
  terminal.
- Release fixes: derive the npm dist-tag from the version, so a prerelease can no longer take
  `latest` by default, and delete the superseded `publish.yml`, which raced the real release
  workflow on the same tags and could publish a launcher before the binaries it pins exist.

## 0.3.1 - 2026-07-10

- Made updates explicit with `sessions --update`; launching sessio no longer
  changes a checkout or global installation.
- Sanitized transcript-derived terminal output and hardened Ghostty resume
  argument handling.
- Made full-text results include sessions beyond the 300-item browse cap.
- Made the metadata cache path-keyed, private, atomic, and pruned on refresh.
- Added empty-home, search-cap, cache, and terminal-safety regression tests.
