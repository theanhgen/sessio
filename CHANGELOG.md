# Changelog

## 1.0.0-alpha.6 - 2026-08-20

- Fixed `sessions` doing nothing at all when installed from npm. The launcher only ran when the
  path it was invoked by matched its own, compared without resolving symlinks — and npm installs
  the command as a symlink into `node_modules`, so the check was false for every install and the
  process exited 0 in silence. Importing the module and running the binary directly both worked,
  which is exactly why the tests and the release workflow missed it; both now go through the
  linked command instead, and the release runs the launcher's tests before publishing.

## 1.0.0-alpha.5 - 2026-08-20

Prerelease. Published under the `alpha` dist-tag — `npm i -g sessio` still installs 0.3.x.
Install it with `npm i -g sessio@alpha`.

- Publish the per-platform binaries under plain `sessio-darwin-arm64`-style names instead of an
  `@sessio/*` scope. The scope would have needed an npm organization created for it; the flat
  names are already ours and need no npm-side setup.
- Fixed the flash message lasting about 120ms. It was cleared after one frame, which in the JS
  reference meant a keypress or the 2s tick, but this loop redraws on every 120ms input poll —
  so `already running — ↵ again to open it twice`, the entire answer to pressing `↵` on a live
  session, was repainted away before it could be read and the feature looked dead. Flashes are
  now timed, and the consent they ask for expires with them: `↵ again` means again now.
- Said plainly, in the README and on the site, that a session in a background tab cannot be
  raised. No terminal exposes its tabs — Ghostty's entire external surface is `+new-window`,
  `+new-tab` and `+toggle-quick-terminal` — so with several sessions per window the pid-and-tty
  answer is the common case, not the fallback.
- Build x86_64-apple-darwin by cross-compiling on the arm64 runner. macos-13 is GitHub's last
  Intel image and its queue runs to tens of minutes, which blocked releases outright.
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
