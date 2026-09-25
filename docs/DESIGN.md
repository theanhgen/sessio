# sessio design specification

The shared visual and interaction contract for the terminal dashboard (`src/ui.rs`, `src/md.rs`),
the colour roles behind it (`src/theme.rs`) and the website (`docs/index.html`). Later design work
(#21–#27) builds on it: change this file in the same pull request as a change to what it describes.

## Principles

1. **The terminal owns the surface.** sessio never paints a background except the two selections.
   Body text is the terminal's own foreground. Dark mode is not a prerequisite.
2. **Colour carries state, never decoration.** Anything coloured is saying one of the roles below.
3. **Never colour alone.** Every state also has a glyph or a word, so a monochrome terminal, a
   colour-blind reader or a greyscale screenshot loses nothing.
4. **Keys mean one thing everywhere.** A key does not change meaning with window size or layout.
5. **Nothing moves under you.** The chrome is a fixed height, and a state changes what a row says,
   never which row says it. Feedback has its own region; it never displaces the hints or the preview.

## Colour roles

One set of names across the code and the site. In Rust each role is a constant plus a style
function in `src/theme.rs` (`theme::ATTENTION` / `theme::attention()`). In CSS it is a custom
property of the same name. No other file may name a colour: `theme::tests::no_colour_outside_the_theme`
fails on any `Color::`, `.fg(` or `.bg(` outside `src/theme.rs`.

| Role | Means | Terminal (`src/theme.rs`) | Website variable | Dark | Light |
|---|---|---|---|---|---|
| Surface | The background. Never painted. | terminal default (`Color::Reset`, not set) | `--surface` (also `--surface-raised`, `--surface-sunken` for cards, kbd, the terminal bar) | `#0b0d10` | `#fbfbfc` |
| Primary text | Body text, info feedback | `TEXT` = terminal default fg | `--text` | `#e6e9ee` | `#16181c` |
| Secondary / dim | Hints, metadata, rules, unselected rows | `DIM` = ANSI bright black (`DarkGray`) | `--dim` (and `--faint`, a site-only tertiary step) | `#8a93a0` | `#4a525e` |
| Accent | The thing you are looking at: session title, key names, a hint that is switched on | `ACCENT` = ANSI cyan | `--accent` | `#a882d5` | `#6d3fa3` |
| Selection | The focused session tab (plum) and the selected project (neutral) | `TAB_SEL` = 54, `PANEL_SEL` = 238, text `ON_SEL` = 255 | `--selection`, `--on-selection` | `#6b4a8f` / `#f4effa` | `#6b4a8f` / `#ffffff` |
| Code | Inline and fenced code in markdown | `CODE` = ANSI cyan | `--code` | `#5fd7d7` | `#0b6e75` |
| Attention | Your move: waiting (`◆`), unfinished (`▶`), content-search hits, warnings | `ATTENTION` = ANSI yellow | `--attention` | `#e0b341` | `#8a6100` |
| Running | A `claude` is attached (`◉`), or written in the last 5 minutes (`●`) | `RUNNING` = ANSI green | `--running` | `#4ec96a` | `#1b6e31` |
| Recent | Written in the last 24 hours (`○`) | `RECENT` = xterm 202 | `--recent` | `#e88a2b` | `#a4520c` |
| Voice | Claude's own words: recap and reply labels | `VOICE` = xterm 98 | `--voice` | `#af87ff` | `#6d3fa3` |
| Agent tag | `copilot` on another agent's sessions | `AGENT_TAG` = xterm 68 | (not used on the site) | — | — |
| Success | An action you asked for happened | `SUCCESS` = ANSI green | `--success` | `#4ec96a` | `#1b6e31` |
| Error | An action you asked for failed | `ERROR` = ANSI red | `--error` | `#f07178` | `#b3261e` |

Accent is cyan in the terminal and plum on the site. The site takes the selection hue as its only
brand colour; the terminal keeps cyan because it is an ANSI colour the user's theme already tunes.

The named ANSI roles (dim, accent, code, attention, running, success, error) are remapped by the
terminal theme, which is what keeps them legible on light and dark backgrounds alike. The fixed
xterm-256 roles (recent, voice, agent tag, the selections) are not remapped, so they are chosen to
hold up on both: see the contrast tables.

### Selection

- Two selections are on screen at once and must never look the same: the project panel row takes
  `PANEL_SEL` (neutral grey), the session tab `TAB_SEL` (plum) **and bold**.
- Neither uses reverse video (`the_panel_and_the_tab_strip_highlight_differently`).
- Status glyphs on the focused tab keep their own colour and take only the selection's background
  and weight (`theme::on_selection`), so `◉` and `○` stay different colours when focused.

### Feedback tones

Flashes (the message in the feedback row at the bottom of the frame) have a `theme::Tone`. They used to be green
whatever they said, including failures.

| Tone | Colour | Mark | Used for |
|---|---|---|---|
| Info | primary text | none | neutral acknowledgement: `reply to "x" discarded`, `following "x" …`, `your failed reply to "x" is back …` (a running `^f` says `searching…` on the query row instead) |
| Pending | primary text | `⏳ ` added by the renderer | started, not finished: `sending to "x"…`, `ending "x" (pid N)…`, `issues still loading…`. The result replaces it |
| Success | success (green) | the message's own `↗` / `↩` / `✓` / `🗄` | resumed or switched to, reply landed, archived / unarchived, session ended, an archived session back |
| Warning | attention (yellow) | none: worded as what to press next or why not | refused preconditions (`"x" is running (pid · tty · status) — …`, `still sending to "x"`, `^f needs ripgrep`), `↵ again` / `^o again` / `^k again` confirmations, the token warning, "waiting on you" |
| Error | error (red) | `✗ ` added by the renderer | reply failed, window or browser could not open, search or issues failed, `^k` could not end |

Only something that happened is green. In progress (pending), refused (warning) and failed (error)
never are (`theme::tests::an_error_is_marked_without_colour`,
`ui::tests::session_feedback_names_its_session_and_is_never_falsely_green`).

### Consequences

What each action says, and what it leaves behind. `"x"` is the session's **target**: the first
words of its title, quoted, at most `TARGET_MAX = 32` columns (`target()`).

- **Every message about a session names it.** Refusals, warnings, progress and results all carry
  the target, so a message still says which session it means after `←→` has moved on, and a result
  that arrives later (a reply, a `^k`, a session un-archiving itself) names the one it is about,
  not the one highlighted (`a_reply_names_its_session_when_it_lands_after_you_moved`). Messages
  about a folder (`^g`, `^n`) name the folder or repo instead.
- **Reply (`^r`).** The first `^r` of a run warns that it spends tokens and names the target; the
  composer row reads ` ↳ reply to "x" ` so you see what you answer while you type. `↵` sends:
  `⏳ sending to "x"…` in the feedback row, and until the answer lands the session's tab carries
  `⏳` and its preview a short `⏳ sending "…"` row, wherever you are looking in between; the
  feedback row carries the explanation. The result: `↩ "x" replied · …` (success) or `✗ reply to
  "x" failed · why · your text is kept: ^r on it to retry` (error), and the preview keeps `✗ reply
  failed · why · ^r retries` until you act (the reason stays, since the flash does not). The next `^r` on that session restores the
  failed text into the composer; `↵` sends it again, `esc` discards it for good. **Nothing is
  ever resent on its own.** While a reply is in flight, `^r` on that session and a second send are
  refused (`still sending to "x" — one reply at a time`). A running session is refused with where
  it runs: `"x" is running (pid · tty · status) — answer it in that terminal`.
- **Archive (`^a`).** `🗄 archived "x" · to restore it: ↑↓ to 🗄
  archived, then ^a`; the preview there says `🗄 archived · hidden from every other tab · ^a
  restores it`; unarchiving says `↩ unarchived "x" · back in its project and ⌂ everything`. A
  session written to again comes back on its own: `↩ "x" is back from 🗄 archived — written to again`.
- **Running sessions (`↵`, `^o`).** Under Ghostty 1.3+ sessio switches to the session's own
  terminal by tty: `↗ switched to "x"'s Ghostty terminal — already running (pid · tty · status)`.
  Otherwise it does not move you anywhere and says so in terms you can act on: `"x" is already
  running (pid · tty · status) — go to that terminal, or ↵ again to open it twice`. Only
  `SESSIO_FOCUS=1` tries raising a Ghostty window by title, and says `raised`, never `focused`.
  No message implies that a window outside Ghostty was focused.
- **Consent** (`↵ again`, `^o again`, `^k again`) is armed by its warning and withdrawn by any
  other key, by the flash expiring (`FLASH`), and by a background result taking the feedback row
  (`App::report`), so it never stays armed behind a message that no longer asks
  (`consent_expires_and_anything_else_withdraws_it`).
- **`^o` outside Ghostty** is a warning: `^o needs Ghostty (it asks Ghostty for the window) — ↵
  resumes "x" here`. `^n` outside Ghostty replaces sessio in this window, like `↵`.

## Status vocabulary

Each state has exactly one glyph and one word. `every_status_reads_apart_without_colour` checks the
glyphs are pairwise distinct.

| Glyph | Word | Where | Meaning | Role |
|---|---|---|---|---|
| `◆` | waiting | dot, `◆ waiting` tab, key bar `◆ N waiting on you`, preview `◆ waiting on you · <what for> · pid · tty` | a running session is stopped on a question or permission prompt | attention, bold |
| `◉` | running | dot, preview `◉ running · busy · pid · tty` (or `idle`, or whatever status the registry reports) | a `claude` process is attached right now | running |
| `●` | active | dot, preview `● active · 2m ago · not running` | transcript written in the last 5 minutes, nothing attached | running |
| `○` | recent | dot, preview `○ recently updated · 3h ago · not running` | written in the last 24 hours, nothing attached | recent |
| (none) | — | dot, preview `updated 3d ago · not running` | older than 24 hours | — |
| `▶` | unfinished | tab mark, preview `▶ unfinished · <reason>` | the reason (`open_reason`): `your prompt got no reply`, `recap says your move`, `Claude asked / proposed next` (only for 3 days), `uncommitted changes` (git WIP, on the folder's newest session) | attention |
| `⏸` | open | tab `⏸ open` | the collection of unfinished sessions | dim / selection |
| `🗄` | archived | tab `🗄 archived`, preview line | hidden locally by `^a`; comes back when written to again | dim |
| `⌂` | everything | tab `⌂ everything` | all sessions not archived | dim / selection |
| `copilot` | — | tab and preview tag | written by GitHub Copilot CLI | agent tag |
| `stale` | stale | preview `◉ running · stale · idle Nd · pid · tty`, key bar `^k end-stale` | running but idle for more than 48 hours; `^k` can end it | dim |
| `◌` | ended | follow header | the followed session stopped running | attention |
| `⏳` | sending | tab mark, preview `⏳ sending "…"`, pending feedback | a `^r` reply is on its way to this session | primary text |
| `✓` | contains | preview `✓ contains "…"` | matched a `^f` content search | attention |
| `⚑` | issues | preview line, `^g` list | open GitHub issues for the folder's repo | attention / dim |

The waiting dot outranks running, which outranks recency: a session shows one dot, the most urgent.

### State summary

The preview says the highlighted session's state in words, on fixed rows: the row under the title
(`CHROME + 2`) is always the state, and an unfinished session's `▶ unfinished · <reason>` is always
the row after it. Exactly one of waiting on you, running, active, recently updated or not running, then
the pid and tty of a process. Process presence and transcript recency stay apart: a transcript
written a minute ago with nothing attached says `not running`, and a stale process says `running`.

- The parts are joined by ` · ` and **wrap between parts**, continuation rows indented two
  columns, so a narrow window keeps the pid and tty (the route to the running window) instead of
  cutting them. Only a single part wider than the row ends in `…`.
- Below the minimum size the same summary, two rows at most, follows the highlighted session.
- It is presentation only: it reads `live` and `open_reason` and decides nothing. The dot, the
  `⏸ open` and `◆ waiting` tabs and `sessions --json` classify on their own
  (`presentation_does_not_change_classification`).
- The key bar's `◆ N waiting on you` is the exact number the `◆ waiting` tab holds (waiting,
  listed, not archived), never "several".

### Help overlay

`?` shows the keys, then the status legend grouped by what each mark is evidence of: **process**
(`◆ waiting`, `◉ running`, `stale`), **transcript** (`●` `○`, not running), **unfinished** (`▶`
and its four reasons) and **agent** (`copilot`). It fits 80x24; a shorter window ends on
`… a taller window shows the rest`. The `↵` line says what happens on a running session under
Ghostty (switch to its terminal by tty) and elsewhere (say its pid · tty); it must not promise to
focus or raise a window.

## Keybinding invariants

Verified against `handle_key` and `compose_key` in `src/ui.rs`.

| Key | Action |
|---|---|
| `↑` `↓` | previous / next project (in the `^g` list: previous / next issue) |
| `←` `→` | previous / next session tab |
| type | filter by title, project, first prompt (literal first, fuzzy fallback) |
| `⌫` | delete a character of the query |
| `^w`, `⌥⌫` | delete the last word of the query |
| `^u`, `⌘⌫` | clear the query |
| `^f` | full-text search of the query across every transcript on disk; without ripgrep or with an empty query it says why instead |
| `^a` | archive / unarchive the highlighted session, naming it and how to undo it |
| `^r` | reply without opening (first press warns it spends tokens; Claude only; refused while running or while a reply to it is in flight; restores a failed reply's text) |
| `⇥`, `^e` | give the latest reply more room (hide the first / last prompts), or back |
| `PgUp` `PgDn` | scroll the latest reply a page (its window's height less one line); nothing else moves |
| `↵` | resume here; on a running session switch to its window under Ghostty, else say where and require a second `↵` |
| `^o` | resume in a new Ghostty window, keeping sessio open (same running guard as `↵`) |
| `^n` | new session in the highlighted session's folder (new window under Ghostty) |
| `^t` | follow / unfollow a running session's tail, read-only |
| `^g` | show / hide the folder's GitHub issues; `↵` opens one, `esc` or `^g` goes back |
| `^k` | end a running session idle > 48h; a second `^k` on the same pid confirms |
| `?` | help; any key closes it |
| `esc` | leave the `^f` text search (results, a search still running, or a failed one), otherwise quit (in the composer: discard the draft) |
| `^c` | quit, from anywhere except the composer |

Invariants:

- **Projects are a column, sessions are a row**: `↑↓` always moves projects and `←→` always moves
  sessions, at every window size and in every layout.
- **Plain printable keys always type.** Every command is a control key, `⇥`, `↵`, `esc` or `?`,
  so a filter can contain any word. New commands take a `^` binding.
- **Consent is the very next key, and the same key.** `↵ again` / `^o again` and `^k again` are
  armed by the warning and disarmed by any other key.
- **The composer owns the keyboard** until `↵` sends or `esc` discards. Nothing behind it moves.
- **Only the keys the issues list uses are borrowed** while it is up (`↑↓`, `↵`, `esc`, `^g`);
  `←→` still walks the sessions.
- **Scrolling reads, it never navigates.** `PgUp` `PgDn` move only the latest reply's window: the
  project and session stay put, a different session (by `←→`, `↑↓`, typing or a search) starts at
  the top of its reply, and a live refresh keeps the reader's line. While `^t` or `^g` owns the
  preview they do nothing. Nothing else here used the page keys; the arrows keep their meaning.
- The website demo reads keys the way `handle_key` and `compose_key` do (`ui::demo::key`): any
  key closes help, the composer owns the keyboard, `esc` leaves `^f` and otherwise quits — which
  on the page means leaving the demo. `^f` searches the fixtures' text and `^a` archives on the
  page only. What a browser cannot do (`↵`, `^o`, `^n`, sending a `^r` reply, `^k`, `^t` on a
  running session, `^g`) answers `browser demo: …` in the warning tone with what the key does in a
  terminal; it never prints the terminal's success message. `⇥` is never taken, so Tab always
  moves focus on; `^e` expands instead.

## Layout, spacing and truncation

- **Regions.** The frame is fixed regions, each starting on a fixed row whatever is selected,
  typed or said:

  | Region | Where | Rows |
  |---|---|---|
  | Key bar | body row 0 (`KEYBAR_ROW`) | 1 |
  | Query | body row 1 (`QUERY_ROW`) | 1 |
  | Project context | body row 2 (`CONTEXT_ROW`): the selected tab's whole name, `N sessions · M in 24h`, then its folder (`~`-shortened, cut from the left, left off when the cut would reach the folder's own name) or, for a collection, what it collects. Name first, counts next, place last. The one row that names the scope. | 1 |
  | Session strip | body row 3 (`STRIP_ROW`), never wraps, leads with the position `3/18` | 1 |
  | Reply composer | under the strip while `^r` is open | 0 or 1 |
  | Preview | from row `CHROME = 4` (5 with the composer) to the feedback region | the rest |
  | Feedback | the bottom row, the full terminal width, under the panel too | 1, or 2 while a message needs it; blank when idle |
  | Project panel | left column, from row 0 down to the feedback region | all but feedback |

  The preview's first row never depends on the highlighted project or session
  (`the_regions_hold_their_rows_at_every_supported_size`). A long message takes the feedback
  region's second row from the bottom of the preview and the panel, never from the top.
- **Every row is cut to its region** (`clip`): a row wider than its region ends in `…` at the
  region's edge, measured in display columns, so a CJK or emoji title, a long branch or a deep path
  cannot push the panel's rule or run past the terminal
  (`unicode_titles_and_long_paths_stay_in_their_regions`).
- **Minimum size**: `MIN_COLS = 50` x `MIN_ROWS = 12`, the panel at its narrowest plus a body that
  holds a readable strip, and the chrome plus a few lines of preview and the feedback row. Below it
  the frame is `window too small (need 50x12)`, then any feedback, the composer if open, the
  selected tab and position, the highlighted session and its state summary (two rows at most,
  so a running one keeps its pid and tty), and the keys, each cut to the width. Every
  key keeps its meaning; only the drawing gives up
  (`below_the_minimum_the_frame_says_so_and_keeps_the_essentials`).
- **Supported sizes** held by fixtures: 60x18, 80x24, 104x26 (the website demo), 160x40, the
  minimum itself and sizes below it.
- **Project panel** on the left at every size, separated by ` │ ` (`SIDE_GAP = 3`). Width is the
  widest name + 2 + the count columns, clamped to `SIDE_MIN = 10`…`SIDE_MAX = 22` (plus counts),
  and it gives up columns before the body drops under `BODY_MIN = 80`. Names are cut by `clip`,
  ending in `…`, never wrapped. The old wrapping project strip must not return.
- **Counts**: `24h` and `all`, right-aligned, each at least 3 wide, `·` for zero in the 24h column.
  Dropped whole when the name would get fewer than `COUNT_NAME_MIN = 12` columns.
- **Panel groups**: a dim `╌` rule sits wherever the kind of tab changes: collections
  (`⌂ everything`, `⏸ open`, `◆ waiting`), then projects, then `🗄 archived`.
- **Panel scrolling**: centred on the selection (rules included); the label then shows `N/total`.
- **Session strip**: the focused tab shows its whole title up to `TAB_MAX = 44` columns, others
  their first two words, ending in `…` when words were dropped (the `copilot` tag is not one of
  them). It leads with the position (`3/18`), grows outwards from the focused tab
  and shows `‹N` / `+N›` for hidden tabs, holding `MARKERS = 12` columns for them. The focused tab
  always fits: on a narrow strip its title is cut with `…`, never the tab. The status dot survives
  at any width.
- **Key bar shedding**: hints drop from the highest priority number down until the bar fits (the
  flash no longer competes for it). A key that acts on the highlighted session (`←→`, `↵`, `^o`,
  `^r`, `^a`, `⇥`, `^t follow`, `^g`) is offered only while there is one, and `^r` only on a
  Claude session:

  | Priority | Hints |
  |---|---|
  | 0 (never shed) | `◆ N waiting on you` (exact count), `? help` |
  | 1 | `↵ resume` |
  | 2 | `^o new-window` (Ghostty only), `^r reply`, `esc quit` (`esc back-to-filter` in text search) |
  | 3 | `←→ session`, `^k end-stale` (only when it would act) |
  | 4 | `type`, `⇥ expand-reply` / `⇥ collapse`, `^g issues`, `^t unfollow` |
  | 5 | `^f search-in-text` (only with ripgrep), `^a archive` / `unarchive`, `^t follow`, `live` |

  `↑↓` is not in the bar: the panel's own label carries it. While the `^g` list is up the bar is
  replaced by that list's keys.
- **Preview hierarchy**, one column, top to bottom by what you act on:

  | Rows | Content | Shed in a short window |
  |---|---|---|
  | rule, title | the session's name (`copilot` tag first) | never |
  | state | `CHROME + 2` onward, then `▶ unfinished · <reason>` (see State summary) | never |
  | location | `project · branch · N prompts` | 5th |
  | flags | `✓ contains "…"`, `🗄 archived …`, `⚑` issues | never; issues 3rd |
  | file facts | `tokens in … · out … · cache w … · r … · 12K file · auto-named`: what the file is, below what it is about; `auto-named` goes before the row is cut | 1st |
  | recap | Claude's recap (or the compact summary), italic, at most `RECAP_MAX = 6` rows, ending ` …` when cut | rows past `RECAP_MIN = 2` 6th, the rest last |
  | first, last | the conversation's two ends, 2 rows each; hidden by `⇥` | first 2nd, last 4th |
  | reply | Claude's latest reply, whole, in a scrolling window over the rest of the box | never |

  Blocks are shed until the reply keeps `REPLY_MIN = 4` rows (three lines and its indicator);
  the recap's first rows outlast that and the reply goes down to one line and the indicator first.
- **Labels**: a preview at least `GUTTER_MIN = 72` columns wide puts `recap` `first` `last`
  `reply` right-aligned in a `GUTTER = 12` column, content lined up after it. Narrower, the label
  leads its first row (`recap 3h: Goal…`), or takes a row of its own when the text opens on a fence,
  table, heading or list, and the text gets the full width. Prose measure is the width (minus the
  gutter when there is one), capped at `MEASURE_MAX = 140`.
- **Reply window**: a reply that fits is shown whole with no indicator. One that does not gets
  the rest of the box less one row, and that row says where the window is, parts dropped from the
  end (never cut) to fit: `↓ 40 more lines · PgDn scrolls · ⇥ more room` at the top,
  `lines 20–38 of 88 · ↓ 50 more · PgUp PgDn` in the middle, `lines 69–88 of 88 · end of reply ·
  PgUp` at the end (`⇥ more room` only while collapsed). With inline labels the label scrolls
  away with the reply's first row, so a scrolled window opens on `reply 9m: ↑ 12 lines above`
  (one row less of reply) rather than reading on from the recap as if it were part of it. It never says "full": `⇥` gives the reply
  more rows, it does not un-cut it; scrolling is what reaches every line.
- **Loading and missing**: before the transcript is read the reply's place says
  `reading transcript…`; once read, `no reply yet` and (Claude sessions only) `no recap yet`. A
  refresh that finds the session written again keeps showing the previous read until the new one
  lands, so the reply neither blanks nor jumps.
- **Markdown**: code is hard-wrapped at the measure, never cut; headings wrap like prose; a table
  whose columns do not fit side by side at their natural widths is set as one wrapped
  `header: cell · header: cell` line per row instead of cutting cells.
- Times are one format everywhere: `22m`, `3h`, `2d`.
- **Feedback** lasts `FLASH = 5s` and the list refreshes every `REFRESH = 2s`.

## Search states

The query row and an empty list share one `QueryState`, so they cannot disagree. The row is
`🔍 <query>▏  <mode> · <found>`; the mode is dim, the count takes the role in the table. A filter
covers the selected tab, which the context row under it names, so the query row does not repeat
it; text search, which reaches past the tab, says `all sessions` (or the tab it is counting in). Typing filters the `CAP = 300` newest sessions in the selected tab by title, project and
first prompt; `^f` reads every transcript on disk (Claude and Copilot), past the cap, and loads
what it finds.

| State | Query row | Session strip (empty list) | Advice under it |
|---|---|---|---|
| No sessions at all | `type to filter` | `no sessions yet` | where sessio reads from; start `claude` or `copilot` |
| Empty tab, no query | `type to filter` | `no sessions here` | `↑↓` picks another project |
| Filter, literal | `filter · N matches` | — | — |
| Filter, fuzzy fallback | `filter · N fuzzy matches · none exact` (dim) | — | — |
| Filter, none | `filter · no matches` (attention) | `nothing in <tab> matches "q"` (attention) | what filtering looks at and the cap; `^f` instead, or `brew install ripgrep`; `^w` / `^u` |
| Searching | `search in text · all sessions · searching…` | `searching every transcript for "q"…` | — |
| Search failed | `… · ✗ failed · esc back to filter` (error) | `✗ text search failed` (error) | the reason; `^f` retries, `esc` back |
| Text results | `search in text · all sessions · N matches` (attention; `N of T` on another tab) | — | — |
| Text, none | `… · no text matches` (attention) | `no session's text contains "q"` (attention) | what was searched; `esc` back, or edit and `^f` |
| Text, none in this tab | `search in text · <tab> · 0 of T matches` | `no text match in <tab>` | `T elsewhere · ↑↓ to ⌂ everything`; `esc` back |

- A failure also flashes `✗ text search failed · <reason>`; missing ripgrep and `^f` on an empty
  query flash a warning and change no state. Ordinary filtering never depends on ripgrep.
- While `^f` owns the list (running, failed or showing results) the key bar's `esc quit` reads
  `esc back-to-filter`, and the 🔍 takes the attention colour.
- **Stale results never land.** Every query edit, `^f` and `esc` bumps `search_gen`; a result is
  taken only under the generation it started with
  (`a_stale_search_never_overwrites_newer_query_state`).
- **Every string from a transcript** passes through `sanitize` before it is drawn.

## Brand

The icon is the dashboard in miniature: the selected session row in `--selection` plum, its
running `●` in `--running` green, the `s` of sessio and a title bar in `--on-selection`, between
two quieter rows (`○` marks and bars barely lighter than the `--surface-raised` tile). One icon
serves light and dark: it is a dark tile either way, like the demo's terminal window.

- **Type:** Fraunces SemiBold, for the `s` and the name, and nowhere else. The site loads it from
  Google Fonts for the header; the icon and logo files carry it as outlines.
- **Spacing:** the `s` is placed by its ink, not its advance: dot, 5 units, `s`, 5 units, bar, the
  group centred in the row and the letter's body centred on the row's middle line (100-unit tile).
- **Files:** `docs/brand/` — `icon.svg`, `icon-{32,64,180,256,512,1024}.png`,
  `logo-{dark,light}.svg` (icon and name, for dark and light backgrounds), `social.png`
  (1200x630 link preview). `scripts/brand.swift` draws all of them from one description; change
  the mark there, never by hand. The binary embeds `icon-256.png` for notifications.

## Contrast

WCAG 2.x ratios, computed from the sRGB values (4.5:1 is AA for body text; 3:1 applies only to
large text, at least 24px regular or 18.66px bold, and to glyphs and other non-text marks).

### Website (`docs/index.html`)

Type: the headline and the logo are Fraunces SemiBold; the headline's promise is drawn as the
dashboard's selected row (plum, led by the running dot). Body text is the system sans; code and
labels the system monospace. Everything smaller than body text takes one of three sizes,
`--fs-s` 12px, `--fs-m` 13.5px, `--fs-l` 15px. Buttons animate colour only and press to 97%.

The page follows `prefers-color-scheme`. Terminal windows (`.term`) stay dark in both schemes: the
demo draws the palette a dark terminal shows, and `.term` re-declares the dark roles for
everything inside it.

The page runs in this order: the task, install (copying is the primary action, and a refused copy
selects the command and says so), the demo, find / understand / continue, the CLI and agent skill,
keys, requirements. The demo is entered with **Try the demo** (or by tabbing to it) and left with
`esc`, **Exit demo** or `Tab`; while it has focus the whole window (title bar included) wears an accent ring and says `keys go to the
demo`. It lays the frame out for its container — 104x26 on a desktop, down to 60x18 — and never
shrinks the text below 12px; narrower than 60 columns it scrolls sideways inside its window, never
the page. If the wasm fails to load, the screen says so and the rest of the page is unaffected. The demo's screen is set in Fira Code, served from `docs/fonts/` as a subset that keeps the
geometric shapes (Google's subsets drop them), so every status mark is drawn at one size and
height; ligatures are off. After each key a screen reader hears the feedback row, or else a
one-line summary (`demo::summary`: the project and the highlighted session). Every non-ASCII glyph
the demo draws is boxed to its terminal cells (`1ch` or `2ch`), because the fallback font that
draws it has its own width and would push the columns after it. The header (logo and section
links) stays at the top while the page scrolls; on a phone its links scroll sideways in one row.

| Variable | Dark | on surface | on raised | on sunken | Light | on surface | on raised | on sunken |
|---|---|---|---|---|---|---|---|---|
| `--text` | `#e6e9ee` | 15.99 | 15.04 | 14.20 | `#16181c` | 17.19 | 15.87 | 14.76 |
| `--dim` | `#8a93a0` | 6.27 | 5.90 | 5.56 | `#4a525e` | 7.63 | 7.05 | 6.56 |
| `--faint` | `#7a8390` | 5.07 | 4.77 | 4.51 | `#5b636f` | 5.87 | 5.42 | 5.04 |
| `--accent` | `#a882d5` | 6.32 | 5.94 | 5.61 | `#6d3fa3` | 7.03 | 6.49 | 6.03 |
| `--code` | `#5fd7d7` | 11.29 | 10.63 | 10.03 | `#0b6e75` | 5.80 | 5.35 | 4.98 |
| `--attention` | `#e0b341` | 9.91 | 9.32 | 8.80 | `#8a6100` | 5.36 | 4.95 | 4.60 |
| `--running` | `#4ec96a` | 9.16 | 8.62 | 8.14 | `#1b6e31` | 6.11 | 5.64 | 5.25 |
| `--recent` | `#e88a2b` | 7.51 | 7.06 | 6.67 | `#a4520c` | 5.36 | 4.95 | 4.60 |
| `--voice` | `#af87ff` | 7.16 | 6.74 | 6.36 | `#6d3fa3` | 7.03 | 6.49 | 6.03 |
| `--success` | `#4ec96a` | 9.16 | 8.62 | 8.14 | `#1b6e31` | 6.11 | 5.64 | 5.25 |
| `--error` | `#f07178` | 6.80 | 6.40 | 6.04 | `#b3261e` | 6.32 | 5.84 | 5.43 |

Buttons: `--on-selection` on `--selection` is 6.19 (dark) and 7.00 (light); the primary button's
hover (`--surface` on `--accent`) is 6.32 and 7.03. Every pair passes AA.

`--faint` replaced the old `--dimmer` (`#5c6470`), which was 3.25:1 on the page and 3.06:1 on
cards: below AA for the kicker, section headings, table headings, footer and demo note it set.

### Terminal

The ANSI roles depend on the user's theme; the values below are the xterm defaults the website
demo draws (bright variants), on a black terminal, a white one, and the demo's `#121519`.

| Role | xterm value | on black | on white | on demo |
|---|---|---|---|---|
| `DIM` | `#808080` | 5.32 | 3.95 | 4.64 |
| `ACCENT`, `CODE` | `#00ffff` | 16.75 | 1.25 | 14.60 |
| `ATTENTION` | `#ffff00` | 19.56 | 1.07 | 17.05 |
| `RUNNING`, `SUCCESS` | `#00ff00` | 15.30 | 1.37 | 13.34 |
| `ERROR` | `#ff0000` | 5.25 | 4.00 | 4.58 |
| `RECENT` (202) | `#ff5f00` | 6.89 | 3.05 | 6.01 |
| `VOICE` (98) | `#875fd7` | 4.65 | 4.52 | 4.05 (the demo draws `#af87ff`: 6.74) |
| `AGENT_TAG` (68) | `#5f87d7` | 5.93 | 3.54 | 5.17 |

The "on white" column for the ANSI roles is the raw xterm value and does not describe a real light
theme, which remaps them (a light theme's yellow is dark). That is a manual check. The fixed roles
are ours: they were 208 / 141 / 75, at 2.41 / 2.72 / 2.32 on white, and now clear 3:1 on black and
white (`fixed_colours_survive_a_light_terminal`). `RECENT` marks a glyph, so 3:1 is its bar.
`VOICE` and `AGENT_TAG` label short text at the terminal's size, which is not large text, so 3:1
is only the floor the test holds them to: `AGENT_TAG` on white (3.54) is below AA. The website
demo, whose text is 12.5px, draws `VOICE` as `#af87ff` (`theme::demo_rgb`,
`the_demo_voice_clears_aa_on_its_background`); the terminal keeps xterm 98.

Fixed pairs, independent of the terminal theme (`selection_text_clears_aa_on_both_selections`,
`status_colours_stay_legible_on_the_focused_tab`):

| Pair | Ratio |
|---|---|
| `ON_SEL` on `PANEL_SEL` (`#eeeeee` on `#444444`) | 8.39 |
| `ON_SEL` on `TAB_SEL` (`#eeeeee` on `#5f0087`) | 9.86 |
| `ATTENTION` / `RUNNING` / `RECENT` / `AGENT_TAG` on `TAB_SEL` | 10.65 / 8.33 / 3.75 / 3.23 |

## Checks

Automated (in `cargo test`):

- `theme::tests::no_colour_outside_the_theme`: no colour outside `src/theme.rs`.
- `theme::tests::selection_text_clears_aa_on_both_selections`,
  `status_colours_stay_legible_on_the_focused_tab`, `fixed_colours_survive_a_light_terminal`: the
  fixed pairs above.
- `theme::tests::an_error_is_marked_without_colour`.
- `theme::tests::the_demo_voice_clears_aa_on_its_background`: the demo's `VOICE` on its screen.
- `ui::tests::every_status_reads_apart_without_colour`: every status glyph distinct.
- `ui::tests::a_failed_action_does_not_flash_green`: errors carry `✗` and not the success colour.
- #25, consequences: `a_target_is_short_quoted_and_never_wider_than_its_budget`,
  `a_reply_names_its_session_when_it_lands_after_you_moved`,
  `the_composer_names_the_session_it_answers`, `a_reply_in_flight_stays_marked_until_it_lands`,
  `a_second_submit_is_refused_while_one_is_in_flight`,
  `a_failed_reply_keeps_its_text_for_a_deliberate_retry`,
  `a_running_session_refusal_says_where_it_runs`, `archiving_says_what_happened_and_how_to_undo_it`,
  `consent_expires_and_anything_else_withdraws_it`,
  `session_feedback_names_its_session_and_is_never_falsely_green`.
- `ui::tests::every_state_keeps_the_same_rows`: normal, waiting, error and empty frames share rows.
- `ui::tests::the_regions_hold_their_rows_at_every_supported_size`,
  `feedback_takes_its_own_row_and_leaves_the_key_bar_alone`,
  `the_selected_project_and_session_stay_visible_while_lists_overflow`,
  `below_the_minimum_the_frame_says_so_and_keeps_the_essentials`,
  `unicode_titles_and_long_paths_stay_in_their_regions`: the region layout at 60x18, 80x24,
  104x26, 160x40 and below the minimum, with width assertions on every row.
- `ui::tests::the_panel_and_the_tab_strip_highlight_differently`: no reverse video.
- `ui::tests::the_preview_says_each_state_in_the_same_place`: the state fixtures (waiting, busy,
  idle, recent-but-not-running, unanswered prompt, git WIP) and the row each lands on;
  `narrow_layouts_keep_the_state_and_the_route_to_it` (pid and tty at every size and below the
  minimum); `presentation_does_not_change_classification`;
  `the_key_bar_counts_the_sessions_waiting_on_you`; `the_key_bar_offers_only_keys_that_can_act`;
  `help_groups_the_status_legend`.
- `ui::tests::the_query_row_names_the_mode_the_scope_and_the_count`,
  `every_empty_state_reads_differently`, `empty_states_name_the_way_out`,
  `a_stale_search_never_overwrites_newer_query_state`, `a_failed_search_is_an_error_you_can_leave`,
  `missing_ripgrep_explains_itself_and_filtering_still_works`,
  `text_search_on_an_empty_query_says_to_type_first`, `esc_is_labelled_for_what_it_will_do`: the
  search states above.
- `ui::tests::every_reply_line_is_reachable_without_resuming` (every supported size and the
  minimum), `a_cut_reply_says_how_much_is_left_and_how_to_reach_it`,
  `scrolling_never_moves_the_selection_and_a_new_session_starts_at_the_top`,
  `a_refresh_does_not_move_a_reader_off_their_place`,
  `narrow_previews_lead_with_their_labels_and_wide_ones_line_them_up`,
  `the_preview_reads_state_then_recap_then_context_then_reply`, `a_short_window_keeps_what_you_act_on`,
  `loading_and_missing_content_say_so`, `code_tables_and_cjk_stay_inside_the_preview_and_reachable`;
  `md::tests::a_table_too_wide_to_set_keeps_every_cell`, `a_long_heading_wraps_instead_of_being_cut`.
- #27, the release gate:
  - `ui::frames::every_scene_matches_its_golden_frames`: golden text frames (`tests/frames/`) of
    every state above — normal, waiting, running, unfinished, archived, no sessions, filter (literal,
    fuzzy, none), text search (running, failed, results, none), a scrolled reply, the composer with
    a long draft, a reply sending and failed, help, `^t`, `^g` (stubbed issues), a Copilot session,
    CJK and emoji, forty projects, a refresh while reading, concurrent feedback — at every
    supported size, the minimum and below it. The clock, ripgrep, Ghostty and `gh` are pinned, so
    the frames are the same on every machine. `SESSIO_BLESS=1 cargo test frames` rewrites them;
    review the diff. `every_golden_file_has_a_scene`, `no_scene_overflows_any_size`,
    `a_refresh_keeps_the_reader_on_the_same_lines_in_every_size`,
    `concurrent_feedback_keeps_every_message_in_its_place`.
  - `ui::parity`: `ui::demo::key_on` and `key_step` pressed with the same keys on the demo's
    fixture must draw the same frame after every key, at 104x26 and 60x18 — navigation, help
    dismissal, `PgUp` `PgDn` and `⇥` / `^e`, filtering and the word keys, `^f` states and `esc`,
    the composer (it owns the keyboard, `esc` discards, an empty `↵` sends nothing), archive. `↵`,
    `^o`, `^n` and a composer `↵` must answer `browser demo: …` in the warning tone on the page and
    ask for the launch in the terminal (`Step`), which the test never carries out.
  - `ui::guidance::every_overlay_key_is_documented_everywhere`: every key in the `?` overlay is in
    the README key table, `sessions --help`'s `KEYS`, the site's key table and the table above.
  - `scripts/smoke-tui.py` (CI, Linux and macOS): the binary through a pty against a throwaway
    `HOME` of synthetic transcripts with a decoy waiting session; `claude`, `copilot`, `gh`, `git`
    and the browser openers are stubs that fail the run if called.
  - `scripts/check-demo.sh` (CI, after the demo build): `docs/demo/` is not committed, the build
    left a module, and it exports every function `docs/index.html` calls.

Manual, for a release or a change to this file:

1. **Dark terminal**: run `sessions` in a dark theme (e.g. Ghostty's default). The selected
   project (grey) and focused tab (plum, bold) are distinct; `◆ ◉ ● ○ ▶` are all readable.
2. **Light terminal**: switch to a light theme (any light Ghostty theme, or macOS Terminal
   "Basic"). Same checks; in particular yellow `◆`/`▶`, dim hints, and the orange `○` read against
   white.
3. **Monochrome**: take a screenshot and desaturate it: every state in the vocabulary table is
   still told apart by its glyph, and a failed action (e.g. `^o` outside Ghostty is a warning;
   `^r` then `↵` on a session with `claude` missing from `PATH` is an error) shows `✗`.
4. **Website**: open `docs/index.html` with the OS in dark and then light mode. Text is readable in
   both; the terminal windows stay dark in light mode.
5. **Website, desktop and mobile**: at a desktop width the demo lays out 104x26; at phone width
   (about 390px) the page does not scroll sideways, the demo narrows toward 60x18 and scrolls
   inside its own window below that. **Try the demo** gives it the keyboard with a visible ring,
   `esc`, **Exit demo** and `Tab` give it back, and with the wasm blocked the screen says so and
   the rest of the page works.

## Known divergences

For the issue that owns each:

- None open. (#26 fixed the website's key table: `^o` is a new Ghostty window behind the same
  guard, and `^n`, `^t`, `^g`, `^k`, `^c` are listed.)
