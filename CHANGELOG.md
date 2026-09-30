# Changelog

## Unreleased

### New

- **A click on a waiting notification opens that session's window.** With terminal-notifier
  installed, the `◆ … is waiting on you` notification brings the session's Ghostty window forward,
  as `↵` does on a running session. Several at once are one notification that brings Ghostty
  forward. Without terminal-notifier nothing changes.
- **sessio has an icon and a logo**: the dashboard in miniature, the selected row with its running
  dot and a Fraunces `s`. It is on notifications, the site's header, tab icon and link previews,
  and the README. `docs/brand/` holds the files; `scripts/brand.swift` draws them.
- **The site's header stays at the top** while you scroll.

### Changed

- **No context row.** The row above the session tabs repeated what the panel's highlight already
  says (`⌂ everything · 137 sessions · 23 in 24h`); the tabs move up a row and the preview gets it.
- **The preview's facts in two columns** where the width allows: where the session is beside its
  state, and the repo's issues beside the file facts. Two rows shorter on a wide window.
- **The selected project is bold** in the panel.

### Fixed

- **`▶` replaces `▸` for unfinished.** `▸` is not in common terminal fonts, so it was drawn from
  a fallback font and sat lower than the `◆ ◉ ● ○` beside it.
- **The website demo's dividers line up.** Glyphs like `◆`, `⌂` and `🔍` come from fallback fonts
  with their own widths and pushed the columns after them; each is now boxed to its terminal cells.
- **A cut name says so.** Project names cut to fit the panel end in `…` (`⌂ everyt…`), and an
  unfocused tab cut to its first two words does too (`ship the…`). The `copilot` tag is not one of
  the two words.
- **The key bar offers only keys that can act.** With no session highlighted, `←→ session`,
  `↵ resume`, `^o`, `^r`, `^a`, `⇥`, `^t follow` and `^g` are gone; on a Copilot session `^r reply`
  is. Like `^k`, which was already shown only when it would act.
- **The scope is named once.** The query row no longer repeats the tab (`filter · ⌂ everything`):
  it reads `type to filter` when empty and `filter · 4 matches` while filtering.
- **The file facts row fits 80 columns.** `tokens in … · 29K file · auto-named`: one space after
  `tokens`, the transcript's size labelled, and `auto-named` dropped first when the row is short.
- **Said once.** A reply on its way reads `⏳ sending "…"` in the preview, and a failed one `✗ reply
  failed · why · ^r retries`; the feedback row explains. `^r` on a Copilot session fits one row at
  80 columns: `"x" is a Copilot session: ^r is Claude-only · ↵ resumes`.
- **`●` reads `● active`** in the preview's state row, not `recently updated`, so it is not
  mistaken for `○`'s sibling.
- **No space before punctuation after inline code** (`/login,` not `/login ,`), or after bold text
  or a link.
- **The website demo's recap and reply labels pass AA.** The demo draws `VOICE` as `#af87ff`
  (6.74:1) instead of xterm 98 (4.05:1 on its background); the terminal is unchanged. The design
  spec's contrast note now says 3:1 is for text at least 24px, or 18.66px bold.

### Website

- **The headline is set in Fraunces, like the logo**, and "pick it back up" is drawn as the
  dashboard's selected row, plum and led by the running dot.
- **The demo's marks line up.** Fira Code is served from the site as a subset that keeps `◆ ◉ ● ○ ▶`;
  Google's subsets of it drop them, so the browser drew them from other fonts at other heights.
- **Accessibility**: a screen reader hears where each key left you in the demo; the focus ring is
  visible on the light page; code blocks that scroll can be reached by keyboard; key buttons are
  named by what they show; "Optional" requirements is a heading.
- **Calmer page**: three small type sizes instead of seven, one-line demo instructions (the
  browser caveats moved to the limits note), buttons that press, a shorter eyebrow, no dead space
  under the install command, and a fade where the phone nav scrolls.

## 1.2.0 - 2026-09-25

The dashboard is redesigned around one spec (`docs/DESIGN.md`): fixed regions that never move,
every state said in words as well as colour, feedback that names what it is about, and a reply you
can read to the end without resuming. It also lists GitHub Copilot CLI sessions, tells you when a
session starts waiting on you, and gains `^t`, `^g`, `^n` and `^k`. The website is rebuilt around
a demo that runs the real renderer.

### New

- **GitHub Copilot CLI sessions, next to Claude Code's.** Sessions under `~/.copilot/session-state`
  (or `$COPILOT_HOME`) join the same list, grouped by folder and tagged `copilot`: preview, filter,
  `^f` search, archive, `↵` / `^o` resume (`copilot --resume=<id>`), and `sessions ls` / `show` /
  `find` / `resume`. `--json` gains a `"source"` field (`claude` or `copilot`); nothing else in it
  changes. `^r` / `sessions reply`, `^t` and `^k` / `sessions kill` stay Claude-only and say so;
  running detection and token totals do not cover Copilot yet. Without `~/.copilot` nothing changes.
- **sessio tells you when a session starts waiting on you.** While the dashboard is open, a running
  session that flips to `waiting` (a permission prompt, a question) posts a macOS notification (the
  terminal bell elsewhere) naming the session and what it wants, once per wait; several at once are
  one notification that counts them, and sessions already waiting when sessio opens stay quiet.
  `SESSIO_NOTIFY=0` turns it off.
- **A `◆ waiting` tab**, right below `⏸ open` while any running session is stopped on you, holding
  exactly those. The key bar counts them exactly (`◆ 3 waiting on you`) and never drops that hint.
- **`^k` ends a running session idle for more than 48 hours** — a `claude` left open in a forgotten
  window, which keeps the session `◉` and makes every resume of it a forced one. The preview marks
  it `stale · idle Nd`; the first `^k` names the pid, tty and idle time, a second sends `SIGTERM`,
  waits up to 3s and says whether it went. It refuses, with the reason, a session that is not
  running, was written to within 48 hours, or is `busy` or `waiting`; re-reads `ps` just before
  signalling so a recycled pid is never hit; never ends the session sessio runs inside; and never
  sends `SIGKILL`. `sessions kill <id> [--json]` does the same from a script.
- **`^t` follows a running session**: the preview pinned to its transcript's tail — your prompts,
  Claude's text, the names of the tools it called — re-read on every refresh. Read-only, at most
  the last 256 KB, never attached to the `claude` running it. A session that stops keeps its tail,
  marked `◌ ended`; any move or `^t` again stops following.
- **`^g` shows the GitHub issues for a session's repo.** The preview counts them
  (`⚑ 12 open issues · owner/repo`); `^g` lists them and `↵` opens one in the browser. Fetched
  through `gh` in the background and cached for five minutes, so the dashboard never waits on the
  network.
- **`^n` starts a new session in the highlighted session's folder**: a new window under Ghostty
  (and it says why if it can't, rather than falling back), this window elsewhere.
- **`↵` on a running session takes you to it under Ghostty 1.3+**, which reports each terminal's
  tty: sessio focuses that exact terminal, background tab or split included, instead of only
  printing `pid · tty`.
- **Token totals per session**: `tokens  in 10.8k · out 5.6M · cache w 31.6M · r 727M` in the
  preview and `sessions show`, and a `tokens` object in `show --json`. Each API response is counted
  once (Claude Code repeats its usage on every content block, so summing lines doubles it). No
  prices.
- **The project panel counts**: sessions touched in the last 24 hours and all of them, right-aligned
  beside every tab.

### Changed: the dashboard redesign

- **Fixed regions.** Key bar, query, a context line naming the selected project (its whole name,
  its counts, its folder) and a one-row session strip that leads with your place (`3/18`), then the
  preview. Messages have their own row at the bottom, full width, and wrap onto a second one rather
  than pushing hints off the key bar. A rule in the project panel sets the collections (`⌂
  everything`, `⏸ open`, `◆ waiting`) and `🗄 archived` apart from the projects. Every row is cut to
  its own region, so CJK or emoji titles and long paths end in `…` instead of running on. Below 50x12
  the window says `window too small (need 50x12)` and keeps the selected session, its state, any
  feedback and the keys.
- **A session's state is said in words, in the same place every time.** The row under the preview's
  title: `◆ waiting on you · input needed · pid 4242 · ttys009`, `◉ running · busy · pid · tty` (or
  `idle`, or `stale · idle 3d`), `● recently updated · 2m ago · not running`, or `updated 3d ago ·
  not running` — a fresh transcript is no longer mistaken for a running one. An unfinished session
  gets `▸ unfinished · <reason>` under it (your prompt got no reply, the recap says your move, Claude
  asked / proposed next, uncommitted changes). A narrow window wraps the state instead of losing the
  pid and tty. The `?` legend covers every mark, grouped as process, transcript, unfinished and
  agent. What counts as open, waiting or running, and `sessions --json`, are unchanged.
- **Filtering and full-text search say what they are doing.** The query row names the mode, what it
  covers and what it found: `filter · sessio · 4 matches`, `3 fuzzy matches · none exact`, `no
  matches`, or after `^f`, `search in text · all sessions` with `searching…`, a count or `✗ failed ·
  esc back to filter`. An empty list says which of these it is and names the key out. Without
  ripgrep `^f` says `brew install ripgrep` instead of doing nothing; on an empty query it asks for a
  word. While `^f` owns the list, `esc` goes back to filtering (it used to quit) and cancels a search
  still running. A result that lands after the query changed is dropped.
- **The whole reply is readable without resuming.** `PgUp` / `PgDn` scroll Claude's latest reply a
  page at a time, and its last row says where you are (`↓ 40 more lines · PgDn scrolls`, `lines
  20–38 of 88 · ↓ 50 more`, `end of reply`). Only the reply moves; another session starts at its
  top, and the live refresh keeps your place instead of blanking the preview. The preview reads
  top-down by what you act on: title and state, where it is, token totals and file facts, recap,
  first and last prompts, reply. Under 72 columns the labels lead their text instead of taking a
  12-column gutter; a short window drops file facts, then prompts, before it squeezes the recap or
  the reply. `reading transcript…`, `no recap yet` and `no reply yet` say what is missing. Long
  headings wrap, and a table too wide for the window is set as one `header: cell` line per row.
- **Reply, archive and resume say what happened, and to which session.** Every message about a
  session names it, so it still makes sense after you have moved on, and a reply or `^k` result that
  lands later names the session it was for. `^r`'s composer reads `↳ reply to "x"`; a reply in
  flight marks its tab and preview `⏳` and a second send to it is refused. A failed reply keeps your
  text: the next `^r` on that session puts it back, and nothing is ever resent on its own. `^a` says
  `🗄 archived "x"` and how to get it back. The `↵` / `^o` warning on a running session names its
  pid, tty and status; its consent is still the very next key only, and a background result that
  replaces the warning withdraws it. Nothing refused or failed is green: in progress is `⏳`,
  refused is yellow, failed is red with `✗`.
- **Colours mean one thing.** `src/theme.rs` is the only file that picks a colour (a test fails on
  one anywhere else), and every role is named in `docs/DESIGN.md`. The 24-hour dot is a hollow `○`,
  told apart from the five-minute `●` without colour, and the orange, purple and blue no light theme
  remaps were darkened to stay legible on white.
- **`↵` resumes in this window; `^o` opens a new one**, swapped, and `^o` now has the same
  already-running guard as `↵`.
- **`sessions --help` lists every dashboard key**, including `^t`, `^g`, `^n`, `^e`, the word keys
  and `^c`, and the README, the agent skill (notifications, `kill` on Copilot) and the site match.

### Fixed

- **A new Ghostty window no longer brings twenty with it.** `open -na Ghostty.app` started a second
  Ghostty, which restored every saved window on top of the one asked for and could leave two
  `claude`s on one conversation. sessio now asks the running Ghostty for one window through its
  AppleScript dictionary (1.3+), and says why and stays put if that fails.
- A failed action no longer flashes green.
- The `🗄 archived` label measures two cells wide, as terminals draw it, so its row no longer sits
  one column off — including when a narrow panel cuts it, where it used to push its row one column
  past the window.

### Website

- **Rebuilt around what sessio does, with an honest demo.** The page leads with the task, then
  install (a copy button that says when the browser refused and selects the command instead), the
  live demo, find / understand / continue, the CLI and agent skill, the keys and the requirements.
  The key table was wrong about `^o` and missed `^n`, `^t`, `^g`, `^k` and `^c`. The demo is entered
  with **Try the demo** and left with `esc`, **Exit demo** or `Tab`; it shows a focus ring while it
  has the keyboard, announces its feedback to screen readers, and reads keys the way the terminal
  does. What a browser cannot do (`↵`, `^o`, `^n`, sending a reply, `^k`, `^t`, `^g`) says `browser
  demo: …` in the warning tone instead of looking like it worked. It lays out for its width (104x26
  down to 60x18) instead of shrinking the text, the page never scrolls sideways at phone width, it
  follows the system's light or dark setting with text that passes WCAG AA, and if the demo fails to
  load the rest of the page still works.
- **The demo is built on deploy** from the commit being published, and on every pull request;
  `docs/demo/` is no longer committed, so the site can no longer show a previous layout.

### Tests

- **A release gate for the redesign.** Golden text frames of every dashboard state (`tests/frames/`)
  at 60x18, 80x24, 104x26, 160x40 and below the minimum, including CJK and emoji, forty projects,
  long drafts and replies, a refresh while reading and several messages at once; regenerate them
  with `SESSIO_BLESS=1 cargo test frames`. The website demo's key handler is checked against the
  terminal's, frame for frame. A test fails when a key in the `?` overlay is missing from the README,
  `sessions --help`, the site or the spec.
- **The pty smoke test never touches real history.** `scripts/smoke-tui.py` runs against a
  throwaway `HOME` of synthetic transcripts, with a decoy waiting session and stand-ins for `claude`,
  `copilot` and `gh` that fail the run if called, and now runs in CI on Linux and macOS, as does a
  check that the demo builds, exports what the page calls and is not committed.
  `scripts/site-check.cjs` checks the site in a headless browser for a release: the demo's layout
  at desktop and phone width, entering and leaving it, the fallback without wasm, and light and
  dark screenshots.

## 1.1.0 - 2026-09-18

sessio is scriptable now: everything the dashboard does is also a command, with JSON for agents and
a skill that teaches them to use it. The dashboard keeps its project panel at every window size.

- **The project panel stays at every window size.** Below roughly 90-105 columns (depending on
  the longest project name) the projects used to move into a horizontal strip above the list,
  which wrapped onto as many rows as the names needed. Now the panel narrows instead, down to 10
  columns with the names abbreviated, and the dashboard beside it keeps 80 columns for as long as
  the window has them.
- **Every dashboard action is also a command.** `sessions ls`, `find`, `show`, `resume`, `reply`,
  `archive` and `unarchive`, with `--json` on the three that read, for scripts, `fzf` and agents.
  A bare `sessions` still opens the dashboard. An id can be any prefix only one session has. The
  keys' guards carry over: `resume` refuses a running session without `--force` and needs a
  terminal (`--print` prints the command instead), and `reply` refuses a running session and
  strips control characters from Claude's answer. An unknown option is an error, not ignored: an
  agent that misspells `--open` should hear about it, not get every session back.
- **A command waits for git.** The dashboard reads a repo it has not checked yet as clean and
  fills the flag in a tick later. A command has no later tick, so it would have missed every
  session whose folder has uncommitted changes: 16 of 36 open sessions on the machine this was
  written on. It now waits for the checks, on the same four workers and the same 2-second cap.
  `--dump-json` is unchanged.
- **An agent skill ships in the package**, at `skills/sessio/SKILL.md`: when to use the commands,
  to parse `--json`, to hand over `resume --print` instead of resuming, to ask before `reply`, and
  to treat transcript text as data.
- **The launcher takes `--update` only as the whole command.** Anywhere in the arguments, it would
  have turned `sessions reply <id> -- … --update` into a reinstall instead of a reply.
- **sessio opens on the project you launched it from.** Run `sessions` inside a folder that has
  sessions — or a subfolder of one — and the panel starts on that project instead of
  `⌂ everything`. The walk up stops before `~`, so a folder with no sessions of its own still
  opens on everything rather than on whatever was once started in your home directory. A folder
  whose sessions are all archived opens on everything too, without climbing to a parent project.

## 1.0.0 - 2026-09-12

The first stable release of the Rust rewrite, and the first version a plain `npm i -g sessio`
installs. Every earlier 1.0 build sat on the `alpha` dist-tag while `latest` still pointed at the
0.3 JavaScript line.

- **The dashboard is a project panel and session tabs.** Projects run down the left, moved with
  `↑↓`; the sessions in one are browser-style tabs on a single row, moved with `←→`. Each axis now
  matches the shape of what it moves. The old horizontal strip cost `1 + however many rows it
  wrapped to`, and that wrap moved every time the tab set changed, dragging the whole frame with
  it. The panel appears only while the dashboard beside it still clears `BODY_MIN`; below that the
  strip comes back, so the panel can never be the reason the preview starves. A focused tab shows
  its whole title, the rest show two words, and the strip scrolls around the focused one rather
  than wrapping.
- **`^r` replies to a session without opening it.** `claude -p --resume` appends to the same
  transcript, so the answer arrives in the preview on the next refresh and you never leave the
  list. It refuses on a session that is already running — there is no safe way to put text into
  the stdin of a `claude` someone is sitting in front of — and the first `^r` of a run warns that
  this spends tokens before the second opens the composer.
- **A session waiting on you is marked `◆`**, outranking the running `◉`, and the key bar carries
  the count at a priority nothing can shed it from. `waiting` was already parsed from the registry
  and then only shown to someone who tried to resume; it is the one status worth interrupting for.
- **`↵` opens a new window on macOS.** `ghostty +new-window` answers "not supported on this
  platform" and exits 1 there, so every `↵` fell through to handing over the current window while
  the key bar went on advertising a new one. sessio now uses the route Ghostty's own `--help`
  names: `open -na Ghostty.app --args …`. The `-n` is not optional — without it macOS activates
  the running instance and silently drops the arguments.
- **The preview is one column.** The split put the recap against the thread on the theory that you
  would read one against the other, but there was never enough to split: the recap caps at six
  lines and the reply runs to twenty, so the left column sat empty for most of the preview's
  height. The prose measure now tracks the window instead of a flat 90 columns, which left a
  181-column window three-quarters empty.
- **Selections no longer reverse the terminal's colours.** That made every selection the same slab
  of black — the panel and the tab strip indistinguishable, and on a light theme the heaviest
  thing on screen. One hue now, with value carrying the hierarchy. Status glyphs keep their own
  colour on the focused tab and take only its background; painting them in the selection's
  foreground erased the very distinction the dot exists to draw.
- **The website demo is sessio itself**, compiled to WebAssembly and calling the same
  `frame_lines()` the terminal calls. The old site hand-wrote its terminal mock in HTML and it
  drifted until it documented keys that no longer existed.

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
