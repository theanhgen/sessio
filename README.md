# sessio

**Find and resume your past [Claude Code](https://claude.com/claude-code) sessions from the terminal.**

🌐 **[Website](https://theanhgen.github.io/sessio/)** · 📦 **[npm](https://www.npmjs.com/package/sessio)**

`sessio` is a fast, self-contained TUI that reads your local Claude Code transcripts and lets you jump back into any past session — the right one, in the right directory — without hunting through `claude --resume` output. A project panel, browser-style session tabs, type-to-filter, full-text search, live refresh, a preview of where each session left off — and a reply key that answers a session without opening it. Every one of those is also a plain command (`sessions ls`, `find`, `show`, `resume`, `reply`, `archive`) with `--json` for scripts and agents, and an [agent skill](#agent-skill) ships with it.

> The command you type is `sessions`. The npm package is named `sessio` (Latin for "a sitting / session") because `sessions` was taken.

```
↑↓ project · ←→ session · type to filter · ^f search-in-text · ^a archive · ⇥ expand-reply · ^r reply · ^t follow · ↵ resume · ^o new-window · ? help · esc quit · live
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

`^f` searches inside every transcript body via [ripgrep](https://github.com/BurntSushi/ripgrep). Without `rg` installed, everything else works — only content search is disabled. Install it with `brew install ripgrep` / `apt install ripgrep`.

## What it does

- **Projects panel** — sessions grouped by their working directory; `↑`/`↓` to switch, or `⌂ everything`. Launched from inside a project folder (or a subfolder of one), sessio opens on that project. The projects run down the left as a panel at every window size; on a narrow window the panel narrows and abbreviates the names rather than giving way. The chrome is a fixed height, so switching projects changes the text and nothing else.
- **Session tabs** — the sessions in the current project are browser-style tabs on one row, moved with `←`/`→`. The focused tab shows its whole title, the rest show two words, and the strip scrolls around the focused one rather than wrapping.
- **Counts in the project panel** — beside every tab, right-aligned: sessions touched in the last 24 hours (`·` for none), then all of them. The panel drops the numbers before it abbreviates a name.
- **`⏸ open` tab** — "pick up where you left off": surfaces unfinished sessions (Claude ended asking/proposing and you didn't answer, a prompt got no reply, or the folder has uncommitted git changes). Open sessions are marked with an amber `▸` in any view.
- **`◆ waiting` tab** — right below `⏸ open`, while any running session has stopped on a question or permission prompt: exactly those sessions, one `↑↓` away. It disappears again once nothing is waiting.
- **🔍 Type to filter** — instantly narrows by title, project, or first prompt. `^w` (or `⌥⌫`) rubs out a word, `^u` (which is what `⌘⌫` sends) clears the query. Literal matches are shown first; if none exist, sessio falls back to fuzzy subsequence matching.
- **`^f` full-text search** — greps the full transcript body for a term, across *all* sessions on disk.
- **`^a` archive** — hides a session you're done with from every tab; press again to unarchive. Archived sessions collect in a `🗄 archived` tab (you can still resume from there). **A session you work in again comes back out on its own** — archiving records when you hid it, and anything written to afterwards is un-hidden on the next refresh. This is a sessio-local declutter list only — the transcript files are never touched, so `claude --resume` still works and Claude's own cleanup still applies.
- **Live refresh** — the list updates every 2s, so a session you're actively running floats to the top with a green dot (🟢 active <5 min, 🟠 recent <24h). A `◉` instead of `●` means a `claude` process is attached to that session *right now* — every running session, including ones you started as a bare `claude`. sessio reads the registry Claude Code keeps at `~/.claude/sessions/<pid>.json` and cross-checks each row against `ps`, so a row left behind by a crash, or a pid since recycled, is not reported as running.
- **`◆` tells you when a session starts waiting on you** — while the dashboard is open, a running session that flips to `waiting` (a permission prompt, a question) posts a macOS notification titled `sessio`, naming the session and what it is waiting for, and flashes the same line in the status bar. One wait is one notification however long it lasts; several flipping in the same refresh are one notification that counts them. Sessions already waiting when sessio starts are not announced. Off the Mac it rings the terminal bell instead. `SESSIO_NOTIFY=0` turns it off.
- **Preview** — for the highlighted session: title, project, prompt count, git branch, token totals (input, output, cache write, cache read — counted once per API response; no prices), Claude's **recap** (the goal / state / whose-move paragraph it writes when you leave a session; the compact summary is shown when there is no recap), first/last typed prompt, and Claude's last reply rendered as markdown (including fenced code blocks). `⇥` drops the first/last context to give the reply the whole box. A session whose recap says the next move is yours is marked as open.
- **`↵` resume** — runs `claude --resume <id>` in that session's original working directory, replacing sessio in this window. If the session is **already running** (`◉`), sessio stops rather than pointing a second `claude` at the same transcript: it names the pid, tty and what that session is doing (`idle` / `busy` / `waiting` / `shell`) so you can find the window yourself, and opening it a second time takes an explicit second `↵`. **`^o`** opens it in a new window instead (below), behind the same guard.
  <br>**Under Ghostty 1.3+ (macOS), `↵` on a running session switches to it**: Ghostty's AppleScript dictionary reports each terminal's tty, so sessio focuses the exact terminal — its tab and split included — instead of guessing from window titles.
  <br>Elsewhere sessio can also try to *raise* the running session's window by title, but that is **off by default** and gated behind `SESSIO_FOCUS=1`, because it cannot be made to land: measured on Ghostty, `AXRaise` puts the target at z-position 2 and never 1, since position 1 is the key window and that is sessio's own. Adding `set frontmost to true` makes macOS promote whatever it considers the app's main window instead, so each press reshuffles the stack and a different unrelated window surfaces.
  <br>Under Ghostty, **`^o`** opens the session in a **new window** and keeps sessio running as a launcher. On macOS it asks the Ghostty you already have open through its AppleScript dictionary (Ghostty 1.3+), so no second Ghostty is started; the first time, macOS may ask whether Ghostty may control itself. On Linux it uses `ghostty +new-window`. If the window can't be opened, sessio says why and stays put — `↵` still resumes here.
- **`^r` reply without opening** — send one turn to a session and stay in the list. `claude -p --resume` appends to the same transcript, so the answer shows up in the preview on the next refresh. It refuses on a session that is already running (`◉`) — there is no safe way to put text into the stdin of a `claude` you are sitting in front of — and the first `^r` of a run warns that this spends tokens before the second one opens the composer. `esc` discards the draft. It is `^r` rather than a bare `r` because plain letters filter the list.
- **`^t` follow a running session** — pins the preview to the highlighted `◉` session and shows the end of its transcript (your prompts, Claude's text, and the names of the tools it called), newest at the bottom, re-read on every 2s refresh. Read-only: it reads at most the last 256 KB of the transcript and never writes to it or attaches to the `claude` running it. If the session stops, the tail stays on screen marked `◌ ended`. Any move — a project, a session, a keystroke into the filter — or `^t` again stops following. On a session that is not running it says so and does nothing.
- **`?` help** — a full keybinding overlay; any key closes it.
- **Explicit updates** — `sessions --update` checks npm and updates a writable global install. Launching sessio never mutates your global install or a git checkout.

## Keys

| Key | Action |
|---|---|
| `↑` / `↓` | switch project |
| `←` / `→` | move between session tabs (`→` reveals more) |
| type | fuzzy-filter (ranked) by name / project / first prompt |
| `^w` / `⌥⌫` | delete the last word of the query |
| `^u` / `⌘⌫` | clear the whole query |
| `^f` | full-text search the current query across all transcripts |
| `^a` | archive / unarchive the selected session (sessio-local hide only) |
| `^r` | reply to the selected session without opening it |
| `^t` | follow the selected running (`◉`) session's tail, read-only; any move stops following |
| `^g` | open GitHub issues for the session's repo (needs `gh`); `↵` opens one in the browser |
| `⇥` / `^e` | expand / collapse the reply preview |
| `↵` | resume the selected session in its directory, in **this** window, replacing sessio — if it's already running, switches to its window under Ghostty (or says where elsewhere), and a second `↵` opens it twice anyway |
| `^o` | Ghostty only: resume in a **new** window and keep sessio open — the same already-running guard, confirmed with a second `^o` |
| `^n` | start a new `claude` in the selected session's folder — a new window under Ghostty, this window everywhere else |
| `?` | toggle the help overlay |
| `esc` | clear content search, then quit |
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
```

- **Ids** are any prefix only one session has; `ls` prints eight characters.
- **Filters** for `ls` and `find`: `-p`/`--project <name>`, `--here`, `--open`, `--running`,
  `--waiting`, `--archived`, `-n`/`--limit <N>`.
- **`--json`** on `ls`, `find` and `show` prints one JSON document with the state the dashboard
  shows: why a session is open, the process running it and what it is doing, whether it is
  archived, and the transcript's path. `show --json` adds `tokens`
  (`{input, output, cache_write, cache_read}`, or `null` when the transcript has no usage data).
- **Marks** in `ls`: `◆` waiting on you, `◉` running, `▸` unfinished.
- **Uncommitted changes count on the first look.** The dashboard checks git in the background and
  fills the flag in on a later refresh. A command has no later refresh, so it waits for those
  checks (2s at most per repo) before it answers.
- **The keys' guards carry over.** `resume` refuses a session that is already running unless you
  pass `--force`, and needs a terminal; `--print` prints the command instead. `reply` refuses a
  running session, spends tokens like `^r`, and strips control characters from Claude's answer
  before printing it. A message that starts with `-` goes after `--`, and `-` alone reads it from
  stdin.
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
- **Claude Code** installed, with a `claude` binary on your PATH (used to resume)
- **ripgrep** (optional) for `^f` full-text search
- macOS or Linux

## Keep your `sessions` muscle memory

If you already invoke the tool some other way, just alias:

```sh
alias sessions='sessio'   # or point it at the global install
```

## The website demo is the real thing

[theanhgen.github.io/sessio](https://theanhgen.github.io/sessio/) runs sessio itself, compiled to
WebAssembly, against six fixture sessions — the page calls the same `frame_lines()` the terminal
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
