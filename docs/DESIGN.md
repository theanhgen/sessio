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
   never which row says it.

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
| Attention | Your move: waiting (`◆`), unfinished (`▸`), content-search hits, warnings | `ATTENTION` = ANSI yellow | `--attention` | `#e0b341` | `#8a6100` |
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

Flashes (the message at the end of the key bar) have a `theme::Tone`. They used to be green
whatever they said, including failures.

| Tone | Colour | Mark | Used for |
|---|---|---|---|
| Info | primary text | none | progress and neutral acknowledgement: `searching…`, `⏳ sending…`, `reply discarded`, `following …` |
| Success | success (green) | the message's own `↗` / `↩` / `✓` | resumed or focused elsewhere, reply landed, session ended, archived sessions back |
| Warning | attention (yellow) | none: worded as what to press next or why not | refused preconditions, `↵ again` / `^k again` confirmations, "waiting on you" |
| Error | error (red) | `✗ ` added by the renderer | reply failed, window or browser could not open, search or issues failed, `^k` could not end |

A typed feedback region, pending state and per-session targets are #25.

## Status vocabulary

Each state has exactly one glyph and one word. `every_status_reads_apart_without_colour` checks the
glyphs are pairwise distinct.

| Glyph | Word | Where | Meaning | Role |
|---|---|---|---|---|
| `◆` | waiting | dot, `◆ waiting` tab, key bar `◆ N waiting on you` | a running session is stopped on a question or permission prompt | attention, bold |
| `◉` | running | dot, preview `◉ running · pid · tty` | a `claude` process is attached right now | running |
| `●` | active | dot | transcript written in the last 5 minutes, nothing attached | running |
| `○` | recent | dot | written in the last 24 hours | recent |
| (none) | — | dot | older than 24 hours | — |
| `▸` | unfinished / pick up | tab mark, preview `▸ pick up · <reason>` | ended on a question, unanswered prompt, or uncommitted git changes | attention |
| `⏸` | open | tab `⏸ open` | the collection of unfinished sessions | dim / selection |
| `🗄` | archived | tab `🗄 archived`, preview line | hidden locally by `^a`; comes back when written to again | dim |
| `⌂` | everything | tab `⌂ everything` | all sessions not archived | dim / selection |
| `copilot` | — | tab and preview tag | written by GitHub Copilot CLI | agent tag |
| `stale` | stale | preview `· stale · idle Nd`, key bar `^k end-stale` | running but idle for more than 48 hours; `^k` can end it | dim |
| `◌` | ended | follow header | the followed session stopped running | attention |
| `✓` | contains | preview `✓ contains "…"` | matched a `^f` content search | attention |
| `⚑` | issues | preview line, `^g` list | open GitHub issues for the folder's repo | attention / dim |

The waiting dot outranks running, which outranks recency: a session shows one dot, the most urgent.

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
| `^f` | full-text search of the query across all transcripts (only with ripgrep and a non-empty query) |
| `^a` | archive / unarchive the highlighted session |
| `^r` | reply without opening (first press warns it spends tokens; Claude only; refused while running) |
| `⇥`, `^e` | expand / collapse the reply preview |
| `↵` | resume here; on a running session switch to its window under Ghostty, else say where and require a second `↵` |
| `^o` | resume in a new Ghostty window, keeping sessio open (same running guard as `↵`) |
| `^n` | new session in the highlighted session's folder (new window under Ghostty) |
| `^t` | follow / unfollow a running session's tail, read-only |
| `^g` | show / hide the folder's GitHub issues; `↵` opens one, `esc` or `^g` goes back |
| `^k` | end a running session idle > 48h; a second `^k` on the same pid confirms |
| `?` | help; any key closes it |
| `esc` | leave content search, otherwise quit (in the composer: discard the draft) |
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
- The website demo supports `↑↓←→`, typing, `⌫`, `⇥`, `^r`, `?` and `esc`; it cannot resume,
  send or open windows (#26 labels this).

## Layout, spacing and truncation

- **Fixed chrome**: row 0 key bar and feedback, row 1 query, row 2 session strip (`TAB_ROW = 1`,
  never wraps), then the reply composer when open, then the preview. The preview's first row does
  not depend on the highlighted session (`every_state_keeps_the_same_rows`).
- **Project panel** on the left at every size, separated by ` │ ` (`SIDE_GAP = 3`). Width is the
  widest name + 2 + the count columns, clamped to `SIDE_MIN = 10`…`SIDE_MAX = 22` (plus counts),
  and it gives up columns before the body drops under `BODY_MIN = 80`. Names are cut by
  `fit_width`, never wrapped. The old wrapping project strip must not return.
- **Counts**: `24h` and `all`, right-aligned, each at least 3 wide, `·` for zero in the 24h column.
  Dropped whole when the name would get fewer than `COUNT_NAME_MIN = 12` columns.
- **Panel scrolling**: centred on the selection; the label then shows `N/total`.
- **Session strip**: the focused tab shows its whole title up to `TAB_MAX = 44` columns, others
  their first two words. It grows outwards from the focused tab and shows `‹N` / `+N›` for hidden
  tabs, holding `MARKERS = 12` columns for them. The status dot survives at any width.
- **Key bar shedding**: the flash is budgeted first, then hints drop from the highest priority
  number down until the bar fits:

  | Priority | Hints |
  |---|---|
  | 0 (never shed) | `◆ N waiting on you`, `? help` |
  | 1 | `↵ resume` |
  | 2 | `^o new-window` (Ghostty only), `^r reply`, `esc quit` |
  | 3 | `←→ session`, `^k end-stale` (only when it would act) |
  | 4 | `type`, `⇥ expand-reply` / `⇥ collapse`, `^g issues`, `^t unfollow` |
  | 5 | `^f search-in-text` (only with ripgrep), `^a archive` / `unarchive`, `^t follow`, `live` |

  `↑↓` is not in the bar: the panel's own label carries it. While the `^g` list is up the bar is
  replaced by that list's keys.
- **Preview**: gutter labels right-aligned in `GUTTER = 12` columns; prose measure is the width
  minus the gutter, capped at `MEASURE_MAX = 140`. Recap and summary at most 6 lines; first and
  last prompts 2 lines each; the reply gets the rest of the height and ends `… ⇥ for full` when
  cut. Times are one format everywhere: `22m`, `3h`, `2d`.
- **Feedback** lasts `FLASH = 5s` and the list refreshes every `REFRESH = 2s`.
- **Every string from a transcript** passes through `sanitize` before it is drawn.

## Contrast

WCAG 2.x ratios, computed from the sRGB values (4.5:1 is AA for body text, 3:1 for large or bold
text and for glyphs).

### Website (`docs/index.html`)

The page follows `prefers-color-scheme`. Terminal windows (`.term`) stay dark in both schemes: the
demo draws the palette a dark terminal shows, and `.term` re-declares the dark roles for
everything inside it.

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
| `VOICE` (98) | `#875fd7` | 4.65 | 4.52 | 4.05 |
| `AGENT_TAG` (68) | `#5f87d7` | 5.93 | 3.54 | 5.17 |

The "on white" column for the ANSI roles is the raw xterm value and does not describe a real light
theme, which remaps them (a light theme's yellow is dark). That is a manual check. The fixed roles
are ours: they were 208 / 141 / 75, at 2.41 / 2.72 / 2.32 on white, and now clear 3:1 on black and
white (`fixed_colours_survive_a_light_terminal`). `RECENT` marks a glyph, and `VOICE` and
`AGENT_TAG` label short bold or tag text, so 3:1 is the bar they are held to.

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
- `ui::tests::every_status_reads_apart_without_colour`: every status glyph distinct.
- `ui::tests::a_failed_action_does_not_flash_green`: errors carry `✗` and not the success colour.
- `ui::tests::every_state_keeps_the_same_rows`: normal, waiting, error and empty frames share rows.
- `ui::tests::the_panel_and_the_tab_strip_highlight_differently`: no reverse video.

Manual, for a release or a change to this file:

1. **Dark terminal**: run `sessions` in a dark theme (e.g. Ghostty's default). The selected
   project (grey) and focused tab (plum, bold) are distinct; `◆ ◉ ● ○ ▸` are all readable.
2. **Light terminal**: switch to a light theme (any light Ghostty theme, or macOS Terminal
   "Basic"). Same checks; in particular yellow `◆`/`▸`, dim hints, and the orange `○` read against
   white.
3. **Monochrome**: take a screenshot and desaturate it: every state in the vocabulary table is
   still told apart by its glyph, and a failed action (e.g. `^o` outside Ghostty is a warning;
   `^r` then `↵` on a session with `claude` missing from `PATH` is an error) shows `✗`.
4. **Website**: open `docs/index.html` with the OS in dark and then light mode. Text is readable in
   both; the terminal windows stay dark in light mode.

## Known divergences

For the issue that owns each:

- The website's key table describes `^o` as "resume in this window, skipping the already-running
  guard". The code opens a new Ghostty window behind the same guard (#23 / #26).
- The website lacks `^n`, `^t`, `^g`, `^k` in its key table (#26 / #27).
- The help overlay's legend covers `◉ ● ○` but not `◆ ▸` (#22 groups the legend).
