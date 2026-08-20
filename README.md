# sessio

**Find and resume your past [Claude Code](https://claude.com/claude-code) sessions from the terminal.**

🌐 **[Website](https://theanhgen.github.io/sessio/)** · 📦 **[npm](https://www.npmjs.com/package/sessio)**

`sessio` is a fast, self-contained TUI that reads your local Claude Code transcripts and lets you jump back into any past session — the right one, in the right directory — without hunting through `claude --resume` output. Project tabs, type-to-filter, full-text search, a live-updating list, and a preview of where each session left off.

> The command you type is `sessions`. The npm package is named `sessio` (Latin for "a sitting / session") because `sessions` was taken.

```
←→ project · ↑↓ move · type to filter · ^f search-in-text · ^a archive · ⇥ expand-reply · ↵ resume · ^o same-window · ? help · esc quit · live
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

- **Project tabs** — sessions grouped by their working directory; `←`/`→` to switch, or `All`. The list keeps a fixed height and the preview always starts on the same row, so switching tabs changes the text and nothing else.
- **`⏸ open` tab** — "pick up where you left off": surfaces unfinished sessions (Claude ended asking/proposing and you didn't answer, a prompt got no reply, or the folder has uncommitted git changes). Open sessions are marked with an amber `▸` in any view.
- **Type to filter** — instantly narrows by title, project, or first prompt. `^w` (or `⌥⌫`) rubs out a word, `^u` (which is what `⌘⌫` sends) clears the query. Literal matches are shown first; if none exist, sessio falls back to fuzzy subsequence matching.
- **`^f` full-text search** — greps the full transcript body for a term, across *all* sessions on disk.
- **`^a` archive** — hides a session you're done with from every tab; press again to unarchive. Archived sessions collect in a `🗄 archived` tab (you can still resume from there). **A session you work in again comes back out on its own** — archiving records when you hid it, and anything written to afterwards is un-hidden on the next refresh. This is a sessio-local declutter list only — the transcript files are never touched, so `claude --resume` still works and Claude's own cleanup still applies.
- **Live refresh** — the list updates every 2s, so a session you're actively running floats to the top with a green dot (🟢 active <5 min, 🟠 recent <24h). A `◉` instead of `●` means a `claude` process is attached to that session *right now*. Only sessions started with `--resume` carry their id where the outside world can see it, so a session you began as a bare `claude` shows no `◉` — sessio guards what it can prove rather than guessing.
- **Preview** — for the highlighted session: title, project, prompt count, git branch, Claude's **recap** (the goal / state / whose-move paragraph it writes when you leave a session; the compact summary is shown when there is no recap), first/last typed prompt, and Claude's last reply rendered as markdown (including fenced code blocks). `⇥` expands the reply. A session whose recap says the next move is yours is marked as open.
- **`↵` resume** — runs `claude --resume <id>` in that session's original working directory. If the session is **already running** (`◉`), sessio stops rather than pointing a second `claude` at the same transcript: it raises that window when the session is the one its window is currently showing, and otherwise names the pid and tty so you can find it yourself. Either way the duplicate takes an explicit second `↵`. Raising a session sitting in a **background tab is not possible** — no terminal exposes its tabs to the outside world, Ghostty included — so with several sessions per window, expect the pid-and-tty answer more often than a raise. Under **Ghostty**, it opens the session in a **new window** (`ghostty +new-window`) and leaves sessio running as a launcher, so you can fire off several sessions; in any other terminal it hands over the current window as before. Press **`^o`** instead to resume in **this** window (replacing sessio) even under Ghostty — the escape hatch when you don't want a new window.
- **`?` help** — a full keybinding overlay; any key closes it.
- **Explicit updates** — `sessions --update` checks npm and updates a writable global install. Launching sessio never mutates your global install or a git checkout.

## Keys

| Key | Action |
|---|---|
| `←` / `→` | switch project tab |
| `↑` / `↓` | move selection (`↓` reveals more) |
| type | fuzzy-filter (ranked) by name / project / first prompt |
| `^w` / `⌥⌫` | delete the last word of the query |
| `^u` / `⌘⌫` | clear the whole query |
| `^f` | full-text search the current query across all transcripts |
| `^a` | archive / unarchive the selected session (sessio-local hide only) |
| `⇥` / `^e` | expand / collapse the reply preview |
| `↵` | resume the selected session in its directory — or, if it's already running, go to it instead of opening it twice (Ghostty: new window) |
| `^o` | resume in **this** window, replacing sessio (Ghostty escape hatch) |
| `?` | toggle the help overlay |
| `esc` | clear content search, then quit |
| `^c` | quit |

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
