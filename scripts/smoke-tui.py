#!/usr/bin/env python3
"""Drive the sessio TUI through a pty and assert it renders and exits cleanly.

The TUI is the one part the JSON oracle cannot cover, so this checks what would actually break
a user: it starts, paints, survives every key binding, redraws at different terminal sizes, and
— most importantly — restores the terminal on every exit path.

Output is drained continuously on a background thread until EOF, because once the child exits
the pty primary returns EIO and any restore sequence written on the way out would be lost.

Usage: scripts/smoke-tui.py [path-to-binary]
"""
import fcntl
import os
import pty
import re
import signal
import struct
import subprocess
import sys
import termios
import threading
import time

BIN = sys.argv[1] if len(sys.argv) > 1 else "target/release/sessio"
ANSI = re.compile(r"\x1b\[[0-9;?]*[a-zA-Z]|\x1b[()][B0]|\x1b[=>]")
ENTER_ALT, LEAVE_ALT = "\x1b[?1049h", "\x1b[?1049l"


class Session:
    def __init__(self, cols, rows):
        primary, secondary = pty.openpty()
        fcntl.ioctl(secondary, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.proc = subprocess.Popen(
            [BIN], stdin=secondary, stdout=secondary, stderr=secondary,
            close_fds=True, preexec_fn=os.setsid,
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


def run_case(cols, rows, keys, label, quits_itself=False):
    print(f"[{label}] {cols}x{rows}")
    failures = []
    s = Session(cols, rows)
    try:
        time.sleep(1.5)  # first paint (includes a cold scan)
        first = s.text()
        check(len(first) > 0, "paints an initial frame", failures)
        check(ENTER_ALT in first, "enters the alternate screen", failures)
        check("project" in ANSI.sub("", first) or "quit" in ANSI.sub("", first),
              "renders the header hints", failures)

        for k in keys:
            s.send(k)
            time.sleep(0.15)

        if quits_itself:
            s.proc.wait(timeout=4)
        else:
            check(s.proc.poll() is None, "still running before quit", failures)
            s.send(b"\x03")  # ^c
            s.proc.wait(timeout=4)

        time.sleep(0.3)  # let the drain thread see the final bytes
        check(s.proc.returncode == 0, f"exit status 0 (got {s.proc.returncode})", failures)
        check(LEAVE_ALT in s.text(), "leaves the alternate screen", failures)
    except subprocess.TimeoutExpired:
        check(False, "exits within the timeout", failures)
    finally:
        s.close()
    return failures


DOWN, UP, LEFT, RIGHT = b"\x1b[B", b"\x1b[A", b"\x1b[D", b"\x1b[C"

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


def rule_row(raw, cols, rows):
    """1-indexed screen row of the preview's ─── separator, or None."""
    for n, line in enumerate(render(raw, cols, rows), 1):
        if line.startswith("─" * 10):
            return n
    return None


def layout_is_stable(cols=110, rows=40, tabs=8):
    """Walking the project tabs must not move the preview.

    The split used to be derived from content — how many sessions the tab holds, how long the
    highlighted reply is — so every ←/→ shifted the separator and everything under it.
    """
    print(f"[layout holds still across tabs] {cols}x{rows}")
    failures = []
    s = Session(cols, rows)
    try:
        time.sleep(2.0)
        seen = []
        for _ in range(tabs):
            s.send(RIGHT)
            time.sleep(0.5)
            seen.append(rule_row(s.text(), cols, rows))
        found = [r for r in seen if r is not None]
        check(len(found) >= 2, f"the preview rule is drawn ({seen})", failures)
        check(len(set(found)) <= 1, f"rule stays on one row across tabs (saw {sorted(set(found))})",
              failures)
        s.send(b"\x03")
        s.proc.wait(timeout=4)
    except subprocess.TimeoutExpired:
        check(False, "exits within the timeout", failures)
    finally:
        s.close()
    return failures


CASES = [
    (80, 24, [DOWN, DOWN, UP, RIGHT, LEFT], "navigate", False),
    (80, 24, [b"?", b"x"], "help overlay opens and any key closes", False),
    (80, 24, [b"\t", b"\t", b"\x05"], "expand / collapse reply", False),
    (120, 40, [DOWN] * 15, "deep scroll reveals more", False),
    (60, 15, [DOWN, b"\t"], "narrow terminal", False),
    (200, 60, [DOWN, RIGHT], "wide terminal", False),
    (80, 24, [b"s", b"e", b"s", b"\x7f"], "type-to-filter then backspace", False),
    # ^w / ⌥⌫ rub out a word, ^u (what ⌘⌫ sends) clears the query.
    (80, 24, [b"m", b"y", b" ", b"b", b"i", b"t", b"\x17", b"\x1b\x7f", b"\x15"],
     "word-delete and clear-query", False),
    # esc with no active content search quits — a distinct exit path, so assert it restores too.
    (80, 24, [b"z", b"\x1b"], "esc quits and restores the terminal", True),
]

if __name__ == "__main__":
    if not os.path.exists(BIN):
        sys.exit(f"binary not found: {BIN} (cargo build --release)")
    all_failures = []
    all_failures += layout_is_stable()
    for case in CASES:
        all_failures += run_case(*case)
    print()
    if not all_failures:
        print(f"TUI SMOKE: all {len(CASES)} cases passed")
    else:
        print(f"TUI SMOKE: {len(all_failures)} assertion(s) FAILED")
        for f in dict.fromkeys(all_failures):
            print(f"  - {f}")
        sys.exit(1)
