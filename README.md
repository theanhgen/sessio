<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/brand/logo-dark.svg">
    <img src="docs/brand/logo-light.svg" alt="sessio" height="72">
  </picture>
</p>

# sessio

**Find the coding-agent session you left, and pick it back up — [Claude Code](https://claude.com/claude-code) and [GitHub Copilot CLI](https://github.com/features/copilot/cli), from the terminal.**

🌐 **[Website](https://theanhgen.github.io/sessio/)** · 📦 **[npm](https://www.npmjs.com/package/sessio)**

`sessio` is a fast, self-contained TUI that reads your local Claude Code and Copilot CLI transcripts and lets you jump back into any past session — the right one, in the right directory — without hunting through `claude --resume` output. A project panel, browser-style session tabs, type-to-filter, full-text search, live refresh, a preview of where each session left off — and a reply key that answers a session without opening it. Every one of those is also a plain command (`sessions ls`, `find`, `show`, `resume`, `reply`, `archive`, `kill`) with `--json` for scripts and agents, and an [agent skill](#agent-skill) ships with it.

> The command you type is `sessions`. The npm package is named `sessio` (Latin for "a sitting / session") because `sessions` was taken.

```
↑↓ project · ←→ session · type to filter · ^f search-in-text · ^a archive · ⇥ expand-reply · PgUp/PgDn scroll-reply · ^r reply · ^t follow · ^g issues · ^k end-stale · ↵ resume · ^o new-window · ^n new-session · ? help · esc quit · live
```

## Install

```sh
npm install -g sessio
```

This puts a `sessions` command on your PATH (and `sessio` as an alias). Run it:

```sh
sessions
```

### Optional: full-text search

`^f` searches inside every transcript body via [ripgrep](https://github.com/BurntSushi/ripgrep). Without `rg` installed, everything else works, typing still filters, and `^f` says `^f needs ripgrep · brew install ripgrep` instead of searching. Install it with `brew install ripgrep` / `apt install ripgrep`.

## What it does

- **Projects panel** — sessions grouped by their working directory; `↑`/`↓` to switch, or `⌂ everything`. Launched from inside a project folder (or a subfolder of one), sessio opens on that project. The projects run down the left as a panel at every window size; on a narrow window the panel narrows and abbreviates the names rather than giving way. A rule sets the collections (`⌂ everything`, `⏸ open`, `◆ waiting`) apart from the projects, and `🗄 archived` apart from both. Above the session tabs, a context line spells out the selected project's whole name, how many sessions it holds and the folder they live in. The chrome is a fixed height, so switching projects changes the text and nothing else.
- **Session tabs** — the sessions in the current project are browser-style tabs on one row, moved with `←`/`→`. The focused tab shows its whole title, the rest show two words, and the strip scrolls around the focused one rather than wrapping. It leads with your place in it, like `3/18`.
- **Counts in the project panel** — beside every tab, right-aligned: sessions touched in the last 24 hours (`·` for none), then all of them. The panel drops the numbers before it abbreviates a name.
- **`⏸ open` tab** — "pick up where you left off": surfaces unfinished sessions (Claude ended asking/proposing and you didn't answer, a prompt got no reply, Claude's recap says the move is yours, or the folder has uncommitted git changes — marked on that folder's newest session). Open sessions are marked with an amber `▶` in any view, and the preview says which reason applies: `▶ unfinished · your prompt got no reply`.
- **`◆ waiting` tab** — right below `⏸ open`, while any running session has stopped on a question or permission prompt: exactly those sessions, one `↑↓` away. It disappears again once nothing is waiting. The key bar counts them exactly (`◆ 3 waiting on you`) and never drops that hint, however narrow the window.
- **🔍 Type to filter** — instantly narrows the selected tab by title, project, or first prompt, among the 300 most recent sessions. `^w` (or `⌥⌫`) rubs out a word, `^u` (which is what `⌘⌫` sends) clears the query. Literal matches are shown first; if none exist, sessio falls back to fuzzy subsequence matching and says so (`3 fuzzy matches · none exact`). The query row always says which search is on, over what, and what it found: `filter · sessio · 4 matches`.
- **`^f` full-text search** — greps the full transcript body for the query, across *all* sessions on disk (Claude and Copilot), including ones older than the 300 the list browses. The query row turns to `search in text · all sessions` and shows `searching…`, the match count, or `✗ failed` with the reason in the feedback row. `esc` goes back to filtering the same query. An empty list says why — no sessions yet, nothing in this tab, no filter match, no transcript match — and which key gets you out.
- **`^a` archive** — hides a session you're done with from every tab; press again to unarchive. Archived sessions collect in a `🗄 archived` tab (you can still resume from there). The feedback row names the session and says how to get it back (`↑↓ to 🗄 archived, then ^a`). **A session you work in again comes back out on its own** — archiving records when you hid it, and anything written to afterwards is un-hidden on the next refresh. This is a sessio-local declutter list only — the transcript files are never touched, so `claude --resume` still works and Claude's own cleanup still applies.
- **Live refresh** — the list updates every 2s, so a session you're actively running floats to the top with a dot (green `●` written in the last 5 min, orange `○` in the last 24h). A `◉` instead means a `claude` process is attached to that session *right now* — every running session, including ones you started as a bare `claude`. sessio reads the registry Claude Code keeps at `~/.claude/sessions/<pid>.json` and cross-checks each row against `ps`, so a row left behind by a crash, or a pid since recycled, is not reported as running.
- **`◆` tells you when a session starts waiting on you** — while the dashboard is open, a running session that flips to `waiting` (a permission prompt, a question) posts a macOS notification titled `sessio`, naming the session and what it is waiting for, and shows the same line in the feedback row at the bottom of the window. One wait is one notification however long it lasts; several flipping in the same refresh are one notification that counts them. Sessions already waiting when sessio starts are not announced. With [terminal-notifier](https://github.com/julienXX/terminal-notifier) installed (`brew install terminal-notifier`) the notification carries sessio's icon, and **clicking it brings that session's Ghostty window forward**, the same switch `↵` makes; without it the notification comes from AppleScript and a click only opens Script Editor. Off the Mac it rings the terminal bell instead. `SESSIO_NOTIFY=0` turns it off.
- **Feedback row** — what a key did (resumed, refused, failed, `↵ again` to confirm) appears on the bottom row, the full width of the window, instead of pushing hints off the key bar; a long message wraps onto a second row. Every message about a session names it (`"fix login redirect…" is running (pid 4242 · ttys009 · busy) — …`), so it still makes sense after you have moved on, and a result that arrives later — a reply, a `^k` — names the session it is about. Green means it happened; in progress is marked `⏳`, refused is yellow, failed is red with `✗`.
- **Small windows** — the dashboard lays out from 50x12 up. Below that it says `window too small (need 50x12)` and keeps only the highlighted project and session, its state (with the pid and tty of a running one), any feedback, and the keys, which work as usual.
- **Preview** — for the highlighted session: title, then its state in words on the row under it — `◆ waiting on you · input needed · pid 4242 · ttys009`, `◉ running · busy · pid · tty` (or `idle`, or `stale · idle 3d`), `● recently updated · 2m ago · not running`, or `updated 3d ago · not running` — and `▶ unfinished · <reason>` under that when it is open. A fresh transcript and an attached process are told apart: recently written is not running. On a narrow window the state wraps rather than losing the pid and tty. Then where it is (project, git branch, prompt count), then what the file is (token totals — input, output, cache write, cache read, counted once per API response; no prices — the transcript's size, and whether it was named), then Claude's **recap** (the goal / state / whose-move paragraph it writes when you leave a session; the compact summary is shown when there is no recap), the first and last prompt you typed, and Claude's latest reply rendered as markdown (fenced code, tables, CJK). The reply is never cut off for good: when it is taller than the preview, its last row says how much is left (`↓ 40 more lines · PgDn scrolls`, then `lines 20–38 of 88 · ↓ 50 more`), and **`PgUp` / `PgDn`** scroll it a page at a time. Only the reply moves; another session starts at the top, and the 2s refresh keeps your place. `⇥` hides the first/last prompts to give the reply more room. On a narrow window the labels lead their text (`recap: …`, `reply: …`) instead of taking a 12-column gutter, and a short window drops the file facts, then the prompts, before it squeezes the recap or the reply. Until the transcript is read the preview says `reading transcript…`; `no recap yet` and `no reply yet` say what is missing. A table too wide for the window is set as one `header: cell` line per row rather than cut. A session whose recap says the next move is yours is marked as open.
- **`↵` resume** — runs `claude --resume <id>` in that session's original working directory, replacing sessio in this window. If the session is **already running** (`◉`), sessio stops rather than pointing a second `claude` at the same transcript: it names the session, its pid, tty and what it is doing (`idle` / `busy` / `waiting` / `shell`) so you can find the terminal yourself, and opening it a second time takes an explicit second `↵` — the very next key: anything else, or the message running out, withdraws it. **`^o`** opens it in a new window instead (below), behind the same guard.
  <br>**Under Ghostty 1.3+ (macOS), `↵` on a running session switches to it**: Ghostty's AppleScript dictionary reports each terminal's tty, so sessio focuses the exact terminal — its tab and split included — instead of guessing from window titles. **By default, in any other terminal (Terminal, iTerm2, tmux, …) sessio does not move you**: it tells you the pid and tty and leaves finding it to you.
  <br>When the tty switch is unavailable or finds nothing, sessio can also try to *raise* a Ghostty window whose title matches, but that is **off by default** and gated behind `SESSIO_FOCUS=1` (the message then says it *raised* a window, not that you are there), because it cannot be made to land: measured on Ghostty, `AXRaise` puts the target at z-position 2 and never 1, since position 1 is the key window and that is sessio's own. Adding `set frontmost to true` makes macOS promote whatever it considers the app's main window instead, so each press reshuffles the stack and a different unrelated window surfaces.
  <br>Under Ghostty, **`^o`** opens the session in a **new window** and keeps sessio running as a launcher. On macOS it asks the Ghostty you already have open through its AppleScript dictionary (Ghostty 1.3+), so no second Ghostty is started; the first time, macOS may ask whether Ghostty may control itself. On Linux it uses `ghostty +new-window`. If the window can't be opened, sessio says why and stays put — `↵` still resumes here.
- **`^r` reply without opening** — send one turn to a session and stay in the list. `claude -p --resume` appends to the same transcript, so the answer shows up in the preview on the next refresh. It refuses on a session that is already running (`◉`) — there is no safe way to put text into the stdin of a `claude` you are sitting in front of — and the first `^r` of a run warns that this spends tokens before the second one opens the composer. The composer says which session it answers (`↳ reply to "x"`), and while the reply is on its way that session's tab and preview carry `⏳` — one reply at a time per session, so a second send is refused. If it fails, the feedback and the preview say why and your text is kept: the next `^r` on that session puts it back in the composer, and nothing is ever resent on its own. `esc` discards the draft. It is `^r` rather than a bare `r` because plain letters filter the list.
- **`^t` follow a running session** — pins the preview to the highlighted `◉` session and shows the end of its transcript (your prompts, Claude's text, and the names of the tools it called), newest at the bottom, re-read on every 2s refresh. Read-only: it reads at most the last 256 KB of the transcript and never writes to it or attaches to the `claude` running it. If the session stops, the tail stays on screen marked `◌ ended`. Any move — a project, a session, a keystroke into the filter — or `^t` again stops following. On a session that is not running it says so and does nothing.
- **`^k` end a stale session** — a `claude` left open in a forgotten window keeps running and keeps the session `◉`. When one has been running with its transcript untouched for more than 48 hours, the preview says `stale · idle Nd` beside `◉ running`, and `^k` ends it: the first press names the pid, tty and idle time, a second `^k` sends it `SIGTERM`. Anything else is refused with the reason — not running, active within the last 48 hours, `busy` mid-turn, or `waiting` on you. Just before signalling, sessio re-reads `ps` to check the pid is still that session's `claude` (not a recycled pid), and it never ends the session it is itself running inside. It waits up to 3s and says whether the process went; it never escalates to `SIGKILL`.
- **`^g` GitHub issues** — the preview counts the open issues of the highlighted session's repo (`⚑ 12 open issues · owner/repo`); `^g` swaps the preview for the list, `↑↓` walks it, `↵` opens one in the browser, `esc` or `^g` goes back. Fetched through `gh` in the background and cached for five minutes, so the dashboard never waits on the network; a folder without a GitHub `origin` shows nothing.
- **`^n` new session** — starts a fresh `claude` in the highlighted session's folder: a new window under Ghostty (it says why and stays put if it can't), this window everywhere else.
- **`?` help** — the keys, then a status legend grouped by what each mark is evidence of: a process attached (`◆ waiting`, `◉ running`, `stale`), the transcript's age (`●` `○`), unfinished work (`▶` and its reasons), another agent (`copilot`). Any key closes it.
- **Explicit updates** — `sessions --update` checks npm and updates a writable global install. Launching sessio never mutates your global install or a git checkout.

## Sources

sessio lists sessions from two agents in one list, newest first, grouped by folder:

- **Claude Code** — `~/.claude/projects` (or `$CLAUDE_CONFIG_DIR/projects`). Everything above applies.
- **GitHub Copilot CLI** — `~/.copilot/session-state` (or `$COPILOT_HOME/session-state`), read-only.
  These carry a small `copilot` tag on their tab and in the preview. The title is Copilot's own
  session name (else the first prompt); the preview shows the first and last prompt you typed, the
  last reply, and Copilot's task summary as the recap (its compaction summary when there is none).
  `↵` runs `copilot --resume=<id>` in the session's folder and `^o` does the same in a new Ghostty
  window; `^f` searches their `events.jsonl` too, and `^a` archives them like any other.
  Not covered yet: running detection (`◉`), `^r` reply (it says "reply is Claude-only"), `^t`
  follow, `^k` end-stale (it says "^k is Claude-only"), token totals, and the open-session heuristics other than uncommitted changes.

Without a `~/.copilot` folder nothing changes. Both kinds count toward the same 300-session cap.

## Keys

| Key | Action |
|---|---|
| `↑` / `↓` | switch project |
| `←` / `→` | move between session tabs (`→` reveals more) |
| type | filter the selected tab by name / project / first prompt (literal first, fuzzy if none; the newest 300 sessions) |
| `^w` / `⌥⌫` | delete the last word of the query |
| `^u` / `⌘⌫` | clear the whole query |
| `^f` | full-text search the current query across every transcript on disk, past the 300 too (needs ripgrep) |
| `^a` | archive / unarchive the selected session (sessio-local hide only) |
| `^r` | reply to the selected session without opening it |
| `^t` | follow the selected running (`◉`) session's tail, read-only; any move stops following |
| `^g` | open GitHub issues for the session's repo (needs `gh`); `↵` opens one in the browser |
| `⇥` / `^e` | give the latest reply more room (hides the first/last prompts), or back |
| `PgUp` / `PgDn` | scroll the latest reply a page; only the reply moves |
| `↵` | resume the selected session in its directory, in **this** window, replacing sessio — if it's already running, switches to its window under Ghostty (or says where elsewhere), and a second `↵` opens it twice anyway |
| `^o` | Ghostty only: resume in a **new** Ghostty window and keep sessio open — the same already-running guard, confirmed with a second `^o`; outside Ghostty it says it needs Ghostty and does nothing |
| `^n` | start a new `claude` in the selected session's folder — a new window under Ghostty, this window everywhere else |
| `^k` | end a running session idle for more than 48h (not `busy`, not `waiting`) — confirmed with a second `^k` |
| `?` | toggle the help overlay |
| `esc` | leave the `^f` text search, otherwise quit |
| `^c` | quit |

## Commands

Everything the dashboard does is also a command, for scripts, `fzf` and agents. A bare `sessions`
still opens the dashboard.

```sh
sessions ls                        # recent sessions, newest first (20; -n 0 for all)
sessions ls --here --open          # unfinished work in this folder's project
sessions ls --waiting              # running sessions blocked on you
sessions find "login bug"          # rank by title, project and first prompt
sessions find --text ECONNRESET    # search inside every transcript (needs rg)
sessions show 1b6324f5             # recap, first and last prompt, last reply
sessions resume 1b6324f5           # resume in its own folder, in this terminal
sessions reply 1b6324f5 "run the tests again"   # one turn, without opening it
sessions archive 1b6324f5          # hide it; `unarchive` undoes it
sessions kill 1b6324f5             # end its claude, if it has sat idle for more than 48h
```

- **Ids** are any prefix only one session has; `ls` prints eight characters.
- **Filters** for `ls` and `find`: `-p`/`--project <name>`, `--here`, `--open`, `--running`,
  `--waiting`, `--archived`, `-n`/`--limit <N>`.
- **`--json`** on `ls`, `find` and `show` prints one JSON document with the state the dashboard
  shows: why a session is open, the process running it and what it is doing, whether it is
  archived, and the transcript's path. `show --json` adds `tokens`
  (`{input, output, cache_write, cache_read}`, or `null` when the transcript has no usage data).
- **Marks** in `ls`: `◆` waiting on you, `◉` running, `▶` unfinished.
- **Uncommitted changes count on the first look.** The dashboard checks git in the background and
  fills the flag in on a later refresh. A command has no later refresh, so it waits for those
  checks (2s at most per repo) before it answers.
- **The keys' guards carry over.** `resume` refuses a session that is already running unless you
  pass `--force`, and needs a terminal; `--print` prints the command instead. `reply` refuses a
  running session, spends tokens like `^r`, and strips control characters from Claude's answer
  before printing it. A message that starts with `-` goes after `--`, and `-` alone reads it from
  stdin. `kill` follows `^k`'s rules without the second press — running it is the consent — and
  exits `1` with the reason when a session is not running, was active within 48 hours, or is
  `busy` or `waiting`; `--json` prints the pid, idle days and whether the process went.
- Exit status: `0` done, `1` couldn't do it, `2` bad command line.

## Agent skill

[`skills/sessio/SKILL.md`](skills/sessio/SKILL.md) teaches an agent to use those commands: when to
reach for them, to parse `--json`, to hand you `resume --print` rather than resume anything itself,
to ask before `reply`, and to treat transcript text as data rather than instructions. It ships in
the npm package; link it into Claude Code with:

```sh
ln -s "$(npm root -g)/sessio/skills/sessio" ~/.claude/skills/sessio
```

## Update

```sh
sessions --update
```

This is intentionally explicit: normal launches never contact npm or modify
your installation. In a git checkout, it prints the `git pull` command for you
to run rather than changing the checkout itself.

## Requirements

- **Node.js ≥ 16** — only to install from npm. sessio itself is a native binary with no runtime
  dependencies; `cargo install` and the prebuilt archives don't need node at all.
- **Claude Code** or **GitHub Copilot CLI**, with `claude` or `copilot` on your PATH (used to resume)
- **ripgrep** (optional) for `^f` full-text search
- **terminal-notifier** (optional, macOS) so a click on a waiting notification opens that session's window
- macOS or Linux

## Keep your `sessions` muscle memory

If you already invoke the tool some other way, just alias:

```sh
alias sessions='sessio'   # or point it at the global install
```

## The website demo is the real thing

[theanhgen.github.io/sessio](https://theanhgen.github.io/sessio/) runs sessio itself, compiled to
WebAssembly, against eight fixture sessions — the page calls the same `frame_lines()` the terminal
does. It is not a screenshot and not a JavaScript recreation, which is deliberate: the previous
site hand-wrote its terminal mock in HTML and it drifted until it documented keys that no longer
existed.

The Pages deploy builds it from the commit it deploys, so the site cannot fall behind a layout
change, and CI builds it on every pull request. `docs/demo/` is not committed. To preview the site
locally, build it yourself:

```sh
cargo install wasm-bindgen-cli --version "$(grep -A1 'name = "wasm-bindgen"' Cargo.lock | grep version | head -1 | cut -d'"' -f2)"
rustup target add wasm32-unknown-unknown
./scripts/build-demo.sh      # → docs/demo/
```

CI also runs `scripts/check-demo.sh` after the build: it fails if `docs/demo/` was committed or
if the module does not export every function the page calls. For a release, or a change to the
site, `node scripts/site-check.cjs` (needs Playwright) checks the page in a headless browser —
the demo's layout at desktop and phone width, no sideways scroll, entering and leaving the demo,
the fallback when the wasm is blocked — and saves light and dark screenshots of each.

## Development

```sh
cargo clippy --all-targets -- -D warnings
cargo test                                   # unit, golden-frame, parity and CLI tests
npm test                                     # the npm launcher
cargo build --release && npm run smoke       # drive the TUI through a pty
```

None of it reads your sessions or spends a token. The CLI tests and the pty smoke test
(`scripts/smoke-tui.py`) run against a throwaway `HOME` of synthetic transcripts, with stand-ins
for `claude`, `copilot`, `gh` and the browser that fail the run if anything calls them. CI runs
all four on Linux and macOS.

- **Golden frames.** `tests/frames/<scene>.txt` holds the dashboard as text for every key state —
  normal, waiting, running, unfinished, archived, no sessions, each filter and search state, a
  scrolled reply, the composer, a reply sending and failed, help, `^t`, `^g`, a Copilot session,
  CJK and emoji, forty projects, a refresh while reading, several messages at once — at 60x18,
  80x24, 104x26 and 160x40, the 50x12 minimum and below it. A layout change fails
  `ui::frames`; when the change is intended, regenerate them and review the diff:

  ```sh
  SESSIO_BLESS=1 cargo test frames
  git diff tests/frames
  ```
- **Demo parity.** `ui::parity` presses the same keys in the website demo's handler and the
  terminal's and requires the same frame after each: navigation, help, the composer, `PgUp`/`PgDn`,
  filtering and search, archive. What only a terminal can do is checked to say `browser demo: …`.
- **Shipped guidance.** `ui::guidance` fails when a key in the `?` overlay is missing from the key
  table above, `sessions --help`, the website or `docs/DESIGN.md`.

## How it works

`sessio` reads Claude Code's transcript files at `~/.claude/projects/**/*.jsonl` (or under `$CLAUDE_CONFIG_DIR` if you've relocated it), parsing each session's title, prompts, compact summary, and last reply. Only the 300 most-recent sessions are read while browsing; full-text search loads every matching session, including matches older than that cap.

A full cold scan of those 300 takes about 40ms, so there is no metadata cache to go stale — every refresh re-reads from disk. The only state sessio keeps is your archive list, in `~/.claude/.sessio/archived.json`, written owner-only.

> ⚠️ **The `.jsonl` transcript format is undocumented and internal to Claude Code.** It may change without notice. `sessio` parses defensively and degrades gracefully, but a format change on Anthropic's side can break fields until this tool is updated. This project is not affiliated with or endorsed by Anthropic.

## Optional: back up your sessions to iCloud (macOS)

Claude transcripts are your work history and aren't backed up anywhere by default. [`scripts/backup-sessions.sh`](scripts/backup-sessions.sh) rsyncs `~/.claude/projects` into iCloud Drive incrementally (no `--delete`, so a local wipe can't erase your backup).

> **Privacy:** transcript files can contain prompts, code, tool output, and credentials. This optional script uploads them in plaintext to the configured iCloud account. Review the destination and your organisation's data-handling policy before enabling it; use encrypted storage if that is required.

Run it manually, or schedule it with the included LaunchAgent template:

```sh
cp scripts/com.sessio.backup.plist ~/Library/LaunchAgents/
launchctl load ~/Library/LaunchAgents/com.sessio.backup.plist
```

Edit the paths in both files first if your setup differs. On Linux, run the script from `cron` instead.

## License

MIT © 2026 theanhgen
