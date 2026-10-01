# Contributing to sessio

sessio is a Rust TUI and CLI, shipped on npm as prebuilt binaries behind a small Node launcher.
You need a stable Rust toolchain; Node 16+ and Python 3 only for the launcher tests and the pty
smoke test.

```sh
git clone https://github.com/theanhgen/sessio && cd sessio
cargo run                 # the dashboard, on your own sessions
cargo run -- ls --json    # any command
```

## Repo map

| Path | What it is |
|---|---|
| `src/` | The program. `main.rs` and `cli.rs` are the entry point and the commands; `ui.rs` draws the dashboard and handles keys; `discover.rs`, `parse.rs`, `copilot.rs` and `model.rs` read transcripts into the session list; `rank.rs` filters and ranks it; `cta.rs` and `git.rs` decide what is unfinished; `live.rs` finds running sessions; `resume.rs`, `kill.rs`, `notify.rs`, `issues.rs` and `search.rs` are the actions; `store.rs` is the archive list; `md.rs` renders replies; `safety.rs` strips control characters from transcript text; `theme.rs` holds the colours; `lib.rs` is the library entry the wasm demo uses. |
| `src/ui/` | Test modules: golden frames, demo parity, shipped guidance. |
| `tests/cli.rs` | The commands, run against a throwaway `HOME`. |
| `tests/frames/` | Golden frames: the dashboard as text, one file per scene and size. |
| `bin/sessio.mjs` | The npm launcher: finds the platform binary and runs it. |
| `test/` | Node tests for the launcher and the legacy helpers. |
| `legacy/` | The original JavaScript implementation, frozen. `scripts/oracle.sh` diffs the Rust build's `--dump-json` against it. Not shipped, not maintained. |
| `skills/sessio/` | The agent skill shipped in the npm package. |
| `docs/` | The site (`index.html`, served by GitHub Pages), `DESIGN.md`, `brand/`, `fonts/`. `docs/demo/` is built, never committed. |
| `scripts/` | `smoke-tui.py` (pty smoke test), `build-demo.sh`, `check-demo.sh`, `install-demo-tools.sh` and `site-check.cjs` (the site's demo), `stage-npm-packages.sh` (release), `oracle.sh` (legacy diff), `brand.swift` (draws the icon and logo), `backup-sessions.sh` and its LaunchAgent (optional, for users). |
| `.github/workflows/` | `ci.yml` (every PR and push to main), `pages.yml` (deploys the site from main), `release.yml` (publishes to npm on a `v*` tag). |

## Build and test

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
- **Shipped guidance.** `ui::guidance` fails when a key in the `?` overlay is missing from the
  README's key table, `sessions --help`, the website or `docs/DESIGN.md`.

## The website demo

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

## Releasing

npm is published by `release.yml`, and only when a `v*` tag is pushed. A merge to `main` updates
the site, not npm.

1. Set the version in `Cargo.toml` and in `package.json` (the package and its five
   `optionalDependencies`), run `cargo build` so `Cargo.lock` follows, and turn the changelog's
   `## Unreleased` into `## X.Y.Z - YYYY-MM-DD`.
2. Merge that as `release: vX.Y.Z`.
3. Tag the merge commit and push the tag: `git tag -a vX.Y.Z -m "sessio X.Y.Z" && git push origin vX.Y.Z`.

The workflow builds every target, publishes the platform packages, then the `sessio` launcher that
pins them.

## Conventions

- Changes to the layout go through `docs/DESIGN.md` and the golden frames in the same pull request.
- User-visible changes get a line under `## Unreleased` in `CHANGELOG.md`.
- `cargo clippy --all-targets -- -D warnings` must pass; CI runs it.
