#!/usr/bin/env python3
"""Drive the sessio TUI through a pty and assert it renders and exits cleanly.

The TUI is the one part the JSON oracle cannot cover, so this checks what would actually break
a user: it starts, paints, survives every key binding, says what each key did, redraws at
different terminal sizes, and — most importantly — restores the terminal on every exit path.

It never sees your real history and never spends a token. Every run gets a throwaway HOME with
synthetic transcripts in ~/.claude/projects, an empty ~/.copilot, a registry row for one decoy
"waiting" process (`sleep` run through a symlink named `claude`), SESSIO_NOTIFY=0 and
SHELL=/usr/bin/false. `claude`, `copilot`, `gh`, `git`, `open`, `xdg-open` and `osascript` are
stubs first on PATH that record the call and fail; the run fails if any stub was called.

Output is drained continuously on a background thread until EOF, because once the child exits
the pty primary returns EIO and any restore sequence written on the way out would be lost.

Usage: scripts/smoke-tui.py [path-to-binary]   (default target/release/sessio)
"""
import fcntl
import json
import os
import pty
import re
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time

BIN = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/release/sessio")
ANSI = re.compile(r"\x1b\[[0-9;?]*[a-zA-Z]|\x1b[()][B0]|\x1b[=>]")
ENTER_ALT, LEAVE_ALT = "\x1b[?1049h", "\x1b[?1049l"

# Set by make_home(): the environment every Session runs in, and where stubs record calls.
ENV = {}
HOME = ""
CALLS = ""

SESSIONS = [
    # id, project, age in seconds, title, first prompt, reply
    ("11111111-0000-4000-8000-000000000001", "web-app", 30, "fix login redirect loop",
     "the login page redirects to itself", "The loop is in the session middleware."),
    ("22222222-0000-4000-8000-000000000002", "api", 120, "add rate limiting to auth routes",
     "add a limiter to the auth routes", "Added a token-bucket limiter.\n\nWant me to wire it into reset too?"),
    ("33333333-0000-4000-8000-000000000003", "api", 3000, "refactor the cache layer",
     "split the cache in two", "Working on the write-behind queue."),
    ("44444444-0000-4000-8000-000000000004", "cli-tool", 9000, "ship the release workflow",
     "write a release workflow",
     "The release workflow, step by step.\n\n" + "\n".join(f"{n}. step {n} of the release" for n in range(1, 80))),
    ("55555555-0000-4000-8000-000000000005", "日本語-プロジェクト", 20000, "修复登录重定向循环 🔁 認証モジュール",
     "ログインのリダイレクトが止まらない", "修正しました 🎉"),
    ("66666666-0000-4000-8000-000000000006", "docs", 200000, "write onboarding docs",
     "draft a getting-started page", "Merged."),
]
WAITING = SESSIONS[0][0]


def write_stub(bindir, name):
    path = os.path.join(bindir, name)
    with open(path, "w") as f:
        f.write(f'#!/bin/sh\necho "{name} $*" >> "{CALLS}"\nexit 1\n')
    os.chmod(path, 0o755)


def make_home():
    """A throwaway HOME holding only synthetic sessions; returns the decoy process."""
    global HOME, CALLS
    root = os.path.realpath(tempfile.mkdtemp(prefix="sessio-smoke-"))
    HOME = os.path.join(root, "home")
    CALLS = os.path.join(root, "calls.log")
    projects = os.path.join(HOME, ".claude", "projects")
    now = time.time()
    for sid, project, age, title, first, reply in SESSIONS:
        # Folders that do not exist: nothing for git or gh to ask about.
        cwd = f"/nonexistent/code/{project}"
        d = os.path.join(projects, cwd.replace("/", "-"))
        os.makedirs(d, exist_ok=True)
        lines = [
            {"type": "ai-title", "aiTitle": title},
            {"type": "user", "promptSource": "typed", "cwd": cwd, "gitBranch": "main",
             "timestamp": "2026-09-01T10:00:00.000Z", "message": {"role": "user", "content": first}},
            {"type": "assistant", "timestamp": "2026-09-01T10:01:00.000Z",
             "message": {"content": [{"type": "text", "text": reply}]}},
        ]
        f = os.path.join(d, f"{sid}.jsonl")
        with open(f, "w") as fh:
            fh.write("".join(json.dumps(line, ensure_ascii=False, separators=(",", ":")) + "\n" for line in lines))
        os.utime(f, (now - age, now - age))
    os.makedirs(os.path.join(HOME, ".copilot"), exist_ok=True)

    bindir = os.path.join(root, "stubs")
    os.makedirs(bindir)
    for name in ["claude", "copilot", "gh", "git", "open", "xdg-open", "osascript", "notify-send"]:
        write_stub(bindir, name)

    # One running session, stopped on a question: `sleep` behind a symlink named `claude` (a
    # symlink, not a copy: macOS kills a copied system binary for its broken signature).
    decoy_dir = os.path.join(root, "decoy")
    os.makedirs(decoy_dir)
    decoy_bin = os.path.join(decoy_dir, "claude")
    os.symlink(shutil.which("sleep") or "/bin/sleep", decoy_bin)
    decoy = subprocess.Popen([decoy_bin, "600"])
    reg = os.path.join(HOME, ".claude", "sessions")
    os.makedirs(reg)
    with open(os.path.join(reg, f"{decoy.pid}.json"), "w") as fh:
        json.dump({"pid": decoy.pid, "sessionId": WAITING, "status": "waiting",
                   "waitingFor": "input needed"}, fh)

    keep = {k: v for k, v in os.environ.items() if k in ("LANG", "LC_ALL", "LC_CTYPE", "TMPDIR", "USER", "LOGNAME")}
    ENV.clear()
    ENV.update(keep)
    ENV.update({
        "HOME": HOME,
        "PATH": f"{bindir}:/usr/bin:/bin:/usr/sbin:/sbin",
        "TERM": "xterm-256color",
        "SHELL": "/usr/bin/false",
        "SESSIO_NOTIFY": "0",
        "LANG": keep.get("LANG", "en_US.UTF-8"),
    })
    # Not set, so nothing reaches for Ghostty, a relocated config or the real ~/.copilot:
    # GHOSTTY_RESOURCES_DIR, TERM_PROGRAM, CLAUDE_CONFIG_DIR, COPILOT_HOME, SESSIO_FOCUS.
    return root, decoy


class Session:
    def __init__(self, cols, rows):
        primary, secondary = pty.openpty()
        fcntl.ioctl(secondary, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.proc = subprocess.Popen(
            [BIN], stdin=secondary, stdout=secondary, stderr=secondary,
            close_fds=True, preexec_fn=os.setsid, env=ENV, cwd=HOME,
        )
        os.close(secondary)
        self.fd = primary
        self.buf = bytearray()
        self._lock = threading.Lock()
        self._drain = threading.Thread(target=self._pump, daemon=True)
        self._drain.start()

    def _pump(self):
        while True:
            try:
                chunk = os.read(self.fd, 65536)
            except OSError:
                return  # EIO once the child closes its side
            if not chunk:
                return
            with self._lock:
                self.buf += chunk

    def text(self):
        with self._lock:
            return self.buf.decode("utf-8", errors="replace")

    def first_paint(self, timeout=30.0):
        """The output once the first frame is up: the alternate screen and the key bar's `quit`
        (or the too-small notice), a cold scan included. Polled, not slept, so a loaded CI runner
        gets the time it needs and a quick one does not wait for nothing."""
        deadline = time.time() + timeout
        while time.time() < deadline and self.proc.poll() is None:
            t = self.text()
            plain = ANSI.sub("", t)
            if ENTER_ALT in t and ("quit" in plain or "window too small" in plain):
                time.sleep(0.2)  # the rest of that frame
                return self.text()
            time.sleep(0.1)
        return self.text()

    def send(self, data):
        if self.proc.poll() is not None:
            return False
        try:
            os.write(self.fd, data)
            return True
        except OSError:
            return False

    def close(self):
        if self.proc.poll() is None:
            try:
                os.killpg(os.getpgid(self.proc.pid), signal.SIGKILL)
            except ProcessLookupError:
                pass
            self.proc.wait(timeout=3)
        self._drain.join(timeout=1.0)
        try:
            os.close(self.fd)
        except OSError:
            pass


def check(cond, label, failures):
    print(("  ok   " if cond else "  FAIL ") + label)
    if not cond:
        failures.append(label)
    return cond


SGR = re.compile(r"\x1b\[[0-9;]*m")
CUP = re.compile(r"\x1b\[(\d+);(\d+)H")


def render(raw, cols, rows):
    """Replay the output into a screen grid, honouring absolute cursor moves."""
    screen = [[" "] * cols for _ in range(rows)]
    r = c = i = 0
    while i < len(raw):
        m = CUP.match(raw, i)
        if m:
            r, c = int(m.group(1)) - 1, int(m.group(2)) - 1
            i = m.end()
            continue
        m = SGR.match(raw, i) or ANSI.match(raw, i)
        if m:
            i = m.end()
            continue
        ch = raw[i]
        if ch == "\n":
            r, c = r + 1, 0
        elif ch == "\r":
            c = 0
        elif ch == "\x1b":
            i += 1
            continue
        elif 0 <= r < rows and 0 <= c < cols:
            screen[r][c] = ch
            c += 1
        i += 1
    return ["".join(x).rstrip() for x in screen]


def run_case(cols, rows, keys, label, quits_itself=False, expect=()):
    """`expect`: each entry is a string, or a tuple of alternatives, that the screen must show
    after the keys (the whole output is searched, so a message that has since expired counts)."""
    print(f"[{label}] {cols}x{rows}")
    failures = []
    s = Session(cols, rows)
    try:
        first = s.first_paint()
        check(len(first) > 0, "paints an initial frame", failures)
        check(ENTER_ALT in first, "enters the alternate screen", failures)
        plain = ANSI.sub("", first)
        check(any(w in plain for w in ("project", "quit", "window too small")),
              "renders the header hints", failures)

        for k in keys:
            s.send(k)
            time.sleep(0.2)
        wants = [w if isinstance(w, tuple) else (w,) for w in expect]

        def seen():
            # The stream, for a message that has come and gone, and the screen it adds up to,
            # since a redraw sends only the cells that changed and can split a phrase.
            raw = s.text()
            return ANSI.sub("", raw) + "\n" + "\n".join(render(raw, cols, rows))

        # Polled: a loaded runner may take a while to get through the keys.
        deadline = time.time() + 10
        text = seen()
        while time.time() < deadline and not all(any(a in text for a in alts) for alts in wants):
            time.sleep(0.2)
            text = seen()
        time.sleep(0.3)
        for alts in wants:
            check(any(a in text for a in alts), f"shows {' or '.join(repr(a) for a in alts)}", failures)

        if quits_itself:
            s.proc.wait(timeout=10)
        else:
            check(s.proc.poll() is None, "still running before quit", failures)
            s.send(b"\x03")  # ^c
            s.proc.wait(timeout=10)

        time.sleep(0.3)  # let the drain thread see the final bytes
        check(s.proc.returncode == 0, f"exit status 0 (got {s.proc.returncode})", failures)
        check(LEAVE_ALT in s.text(), "leaves the alternate screen", failures)
    except subprocess.TimeoutExpired:
        check(False, "exits within the timeout", failures)
    finally:
        s.close()
    return failures


DOWN, UP, LEFT, RIGHT = b"\x1b[B", b"\x1b[A", b"\x1b[D", b"\x1b[C"
PGUP, PGDN = b"\x1b[5~", b"\x1b[6~"
ESC = b"\x1b"


def ctrl(c):
    return bytes([ord(c) & 0x1f])


def rule_row(raw, cols, rows):
    """1-indexed screen row of the preview's ─── separator, or None."""
    for n, line in enumerate(render(raw, cols, rows), 1):
        if "─" * 10 in line:  # right of the project panel since it became permanent
            return n
    return None


def layout_is_stable(cols=110, rows=40, steps=8):
    """Walking the projects and sessions must not move the preview.

    The split used to be derived from content — how many sessions the tab holds, how long the
    highlighted reply is — so every move shifted the separator and everything under it.
    """
    print(f"[layout holds still across projects and sessions] {cols}x{rows}")
    failures = []
    s = Session(cols, rows)
    try:
        s.first_paint()
        seen = []
        for n in range(steps):
            s.send(DOWN if n % 2 else RIGHT)
            time.sleep(0.5)
            seen.append(rule_row(s.text(), cols, rows))
        found = [r for r in seen if r is not None]
        check(len(found) >= 2, f"the preview rule is drawn ({seen})", failures)
        check(len(set(found)) <= 1, f"rule stays on one row (saw {sorted(set(found))})", failures)
        s.send(b"\x03")
        s.proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        check(False, "exits within the timeout", failures)
    finally:
        s.close()
    return failures


CASES = [
    (80, 24, [DOWN, DOWN, UP, RIGHT, LEFT], "navigate", False,
     ["⌂ everything", "web-app"]),
    (104, 26, [], "a running session waiting on you is announced", False,
     ["◆ waiting", "waiting on you"]),
    (80, 24, [b"?", b"x"], "help overlay opens and any key closes", False,
     ["keys and marks"]),
    (80, 24, [b"\t", b"\t", ctrl("e")], "expand / collapse reply", False, []),
    # PgDn / PgUp scroll the latest reply, past its end and back past its top.
    (80, 24, [b"r", b"e", b"l", b"e", b"a", b"s", b"e"] + [PGDN] * 8 + [PGUP] * 10 + [RIGHT, PGDN, LEFT],
     "scroll the reply", False, ["PgDn", "step 1 of the release"]),
    (60, 18, [PGDN] * 8 + [b"\t", PGDN, PGUP], "scroll the reply, narrow", False, []),
    (120, 40, [DOWN] * 15, "deep scroll reveals more", False, []),
    (60, 15, [DOWN, b"\t"], "narrow terminal", False, []),
    (40, 10, [DOWN, RIGHT], "below the minimum size", False, ["window too small"]),
    (200, 60, [DOWN, RIGHT], "wide terminal", False, []),
    (80, 24, [b"c", b"a", b"c", b"h", b"e", b"\x7f"], "type-to-filter then backspace", False,
     ["filter", "match"]),
    (80, 24, [b"k", b"u", b"b", b"e", b"r", b"n", b"e", b"t", b"e", b"s"], "a filter with no match", False,
     ["no matches"]),
    # ^w / ⌥⌫ rub out a word, ^u (what ⌘⌫ sends) clears the query.
    (80, 24, [b"m", b"y", b" ", b"b", b"i", b"t", ctrl("w"), b"\x1b\x7f", ctrl("u")],
     "word-delete and clear-query", False, []),
    # ^f needs ripgrep, which a CI runner may not have: either outcome is a state it names.
    (80, 24, [b"l", b"i", b"m", b"i", b"t", b"e", b"r", ctrl("f")], "text search, then esc back", False,
     [("search in text", "needs ripgrep")]),
    # ^r warns about tokens, then opens the composer; esc discards. Nothing is ever sent.
    (80, 24, [RIGHT, RIGHT, ctrl("r"), ctrl("r"), b"h", b"i", ESC], "reply composer opens and discards", False,
     ["spends tokens", "reply to", "discarded"]),
    (80, 24, [RIGHT, RIGHT, ctrl("t")], "follow refuses a session nobody runs", False,
     ["nothing to follow"]),
    (80, 24, [ctrl("g")], "issues without a GitHub remote", False, ["no GitHub remote"]),
    (80, 24, [RIGHT, RIGHT, ctrl("k")], "end-stale refuses and says why", False, ["can't end"]),
    (80, 24, [RIGHT, ctrl("a")], "archive says how to undo it", False, ["archived", "^a"]),
    # esc with no active content search quits — a distinct exit path, so assert it restores too.
    (80, 24, [b"z", ESC], "esc quits and restores the terminal", True, []),
]

if __name__ == "__main__":
    if not os.path.exists(BIN):
        sys.exit(f"binary not found: {BIN} (cargo build --release)")
    root, decoy = make_home()
    all_failures = []
    try:
        all_failures += layout_is_stable()
        for case in CASES:
            all_failures += run_case(*case)
        print("[isolation]")
        called = open(CALLS).read().strip() if os.path.exists(CALLS) else ""
        check(not called, f"no claude, copilot, gh, git or browser was started ({called!r})", all_failures)
        check(os.path.exists(os.path.join(HOME, ".claude", ".sessio", "archived.json")),
              "^a wrote its archive under the synthetic HOME", all_failures)
    finally:
        decoy.kill()
        decoy.wait()
        shutil.rmtree(root, ignore_errors=True)
    print()
    if not all_failures:
        print(f"TUI SMOKE: all {len(CASES) + 2} cases passed")
    else:
        print(f"TUI SMOKE: {len(all_failures)} assertion(s) FAILED")
        for f in dict.fromkeys(all_failures):
            print(f"  - {f}")
        sys.exit(1)
