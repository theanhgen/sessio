# Changelog

## 1.0.0-alpha.10 - 2026-08-28

- **A repository too dirty to fit in a pipe was reported clean.** `git status --porcelain` was
  spawned with a piped stdout that nothing read until the child had exited — so once the output
  passed the OS pipe buffer (16 KB and up on macOS), git blocked on write, could never exit, and
  the 2-second timeout classified it as clean. Measured: 12,000 untracked files, 194 KB of
  porcelain, reported clean in exactly the timeout. The repos most likely to trip it are a large
  tree mid-refactor or a directory full of untracked build output — exactly the ones where the
  `⏸ open` tab's "you left work here" is most true. The pipe is now drained while waiting, and the
  result comes back on the same deadline rather than on an unbounded join.
- **A prompt that was one unbroken token overran the preview's measure.** A URL, an absolute path
  or a base64 blob came back as a blank line plus the whole token — 78 columns against a 10-column
  measure — and spilled across the label gutter alpha.8 introduced. Two faults compounded: a line
  was spent on a blank before the oversized word was taken, and the truncation guard compared byte
  lengths, where the joined output is *longer* than the input it came from, so the ellipsis never
  fired. With a trailing word it did fire, which is why the obvious test case passed. Such a word
  is now hard-split at the measure, and whether anything was dropped is tracked as it happens
  instead of inferred by comparing input against output — a comparison that cannot be made exact
  in bytes or in display width, since hard-splitting inserts separators the original never had.
- `wrap_plain` no longer panics on a zero line budget. Nothing reaches it — both call sites pass a
  literal 2 — but it is the same expression as the fix above, and the helper gave no hint that 0
  was forbidden.
- `sessions --update` no longer mistakes a prefix-sibling of the npm root for the global install.
  A package root of `/opt/homebrew/lib/node_modules-old/sessio` matched `…/lib/node_modules` under
  a bare prefix test, and the command would have run `npm i -g` against a checkout it does not own.

## 1.0.0-alpha.9 - 2026-08-21

- **Corrects what alpha.7 and alpha.8 claimed about `↵` on a running session.** The README and the
  website said sessio raises that session's window, and walks the splits when the window is showing
  a different one. It does not: the raise is off unless `SESSIO_FOCUS=1` is set, so what actually
  happens is the guard — sessio names the pid and tty and makes the duplicate an explicit second
  `↵`. The docs described the intent; this describes the behaviour.
- Turned the raise off by default, because it cannot be made to land. Measured on Ghostty by
  reading window z-order before and after: `AXRaise` alone puts the target at position 2 and never
  1, since position 1 is the key window and under Ghostty that is sessio's own, which stays running
  as a launcher. Adding `set frontmost to true` makes macOS promote whatever *it* considers the
  app's main window — neither the target nor the previous front — so every press reshuffles the
  stack and a different unrelated window comes forward. Setting `AXMain` first does not override
  it. That reshuffling was the "it cycles through all the windows" report. `SESSIO_FOCUS=1` opts
  back in for anyone working on the mechanism; the split walk from alpha.7 rides on the same gate.
- Dropped the claim that `↵` opens a new window under Ghostty. `ghostty +new-window` answers
  `+new-window is not supported on this platform` on macOS, so that path has never run there and
  `↵` has always handed over the current window. The claim predates the Rust port.

## 1.0.0-alpha.8 - 2026-08-20

- Rebuilt the preview around a label gutter. `recap`, `first`, `last` and `reply` used to spend a
  whole row each announcing themselves before their content began underneath; the labels now sit
  right-aligned beside the first line of what they label, so every row carries text and all of it
  shares one left edge.
- Capped prose at 90 columns. It was wrapped to the full terminal width, so a 200-column window
  rendered a 198-character measure and the eye had no way back to the start of the next line —
  the layout got worse the more room you gave it. Past the cap, width goes to a second column
  (recap beside the thread) and then to margin.
- Gave the title its own line, with whether the session is running pushed to the far edge, and
  demoted the fact chain to one quiet line beneath: locators first, how the title was come by
  last. One time format throughout — `22m`, `3h` — where three used to compete within four lines.

## 1.0.0-alpha.7 - 2026-08-20

- `↵` on a running session now focuses the right **split**, not just the right window. A window
  reports the title of whichever split has focus and nothing about the others, so a session
  sharing a window was invisible from outside and `↵` landed you in its neighbour. sessio now
  walks the splits with `goto_split:next` and reads the title back after each step, which turns
  it into a search with feedback rather than a guess: it stops on the target, and a window that
  does not hold it wraps back to the split it started on. Gated on ⌘] still being bound to
  `goto_split:next` — a keystroke Ghostty does not claim would be delivered to whatever runs in
  that pane, typing brackets into a live session. Background *tabs* remain out of reach: they
  are not accessibility objects at all, so there is nothing to enumerate or read.

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
