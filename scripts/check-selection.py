"""Copy a message longer than the screen with the mouse, in a real terminal.

Scrolling while the button is held keeps the selection, whether by the wheel
or by holding the pointer past the conversation's edge, and the copy holds
the lines no longer on screen. Once copied, the selection stays highlighted
and scrolls with its text until a key is pressed. Content arriving under a
still pointer never turns a click into a copy.
"""
import base64
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import struct
import subprocess
import termios
import time

from terminal_screen import Screen

ROWS, COLS = 24, 100
LINES = 40
FIRST = "Line 01 of a long pasted message."
LAST = f"Line {LINES} of a long pasted message."
# The demo's welcome message opens the history.
HISTORY_START = "This example uses a scripted host"
COPY = re.compile(rb"\x1b\]52;c;([A-Za-z0-9+/=]*)\x07")


def require(condition, message):
    """Fail the check explicitly; unlike assert, this survives python -O."""
    if not condition:
        raise SystemExit(f"Check failed: {message}")


metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], text=True
))
binary = Path(metadata["target_directory"]) / "debug" / "order-support"
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
process = subprocess.Popen([str(binary), "--no-color", "--no-animation"],
                           stdin=slave, stdout=slave, stderr=slave)
captured = bytearray()
screen = Screen(ROWS, COLS)


def pump(seconds):
    """Read what the terminal writes for `seconds`."""
    deadline = time.monotonic() + seconds
    while (remaining := deadline - time.monotonic()) > 0:
        if not select.select([master], [], [], remaining)[0]:
            continue
        try:
            data = os.read(master, 65536)
        except OSError as error:
            if error.errno == errno.EIO:
                raise RuntimeError("Terminal closed early") from error
            raise
        if not data:
            raise RuntimeError("Terminal closed early")
        captured.extend(data)
        screen.feed(data)


def wait_until(condition, what):
    deadline = time.monotonic() + 10
    while not condition():
        if time.monotonic() >= deadline:
            raise RuntimeError(f"Missing {what}; screen:\n{screen.text()}")
        pump(.05)


def wait_for(marker):
    wait_until(lambda: screen.shows(marker), repr(marker))


def mouse(code, column, row, release=False):
    """Send an SGR mouse report; columns and rows count from 1."""
    os.write(master, f"\x1b[<{code};{column};{row}{'m' if release else 'M'}".encode())


PRESS, DRAG, WHEEL_UP, WHEEL_DOWN = 0, 32, 64, 65


def wheel_until(marker, code, column, row):
    """Turn the wheel one notch at a time until the screen shows `marker`."""
    for _ in range(80):
        if screen.shows(marker):
            return
        mouse(code, column, row)
        pump(.05)
    raise RuntimeError(f"Scrolling never showed {marker!r}; screen:\n{screen.text()}")


def copied_since(offset):
    match = COPY.search(captured, offset)
    return base64.b64decode(match.group(1)).decode() if match else None


def wait_for_copy(offset):
    wait_until(lambda: copied_since(offset) is not None, "a clipboard write")
    return copied_since(offset)


def highlighted(marker):
    """Whether the line showing `marker` shows it in reverse video."""
    row = screen.row_of(marker)
    return row is not None and marker in screen.reversed_text(row)


def check_copy(text):
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    require(FIRST in lines and LAST in lines, f"The copy lacks an end: {text!r}")
    require(sum(line.startswith("Line ") for line in lines) == LINES, f"Lines are missing: {text!r}")


try:
    wait_for("scripted/balanced")
    message = "\n\n".join(f"Line {n:02} of a long pasted message." for n in range(1, LINES + 1))
    os.write(master, b"\x1b[200~" + message.encode() + b"\x1b[201~\r")
    wait_for("Ctrl+Y allow once")
    os.write(master, b"\x19")
    wait_for("Execution Completed")

    # The wheel alone, button held and pointer still: the end follows what
    # the pointer shows once the conversation scrolled.
    middle = ROWS // 2
    wheel_until(FIRST, WHEEL_UP, 10, middle)
    start = screen.row_of(FIRST)
    mouse(PRESS, 1, start)
    mouse(WHEEL_DOWN, 1, start)
    wait_until(lambda: screen.row_of(FIRST) != start, "the wheel scrolling under a held button")
    offset = len(captured)
    mouse(PRESS, 1, start, release=True)
    text = wait_for_copy(offset)
    require(text.lstrip().startswith(FIRST) and "Line 02 of" in text,
            f"The wheel did not extend the selection: {text!r}")

    # The wheel, button held: press on the first line, scroll to the last.
    wheel_until(FIRST, WHEEL_UP, 10, middle)
    start = screen.row_of(FIRST)
    mouse(PRESS, 1, start)
    wheel_until(LAST, WHEEL_DOWN, 1, start)
    end = screen.row_of(LAST)
    mouse(DRAG, COLS - 1, end)
    pump(.1)
    require(not screen.shows(FIRST), "The first line should have scrolled off screen")
    offset = len(captured)
    mouse(PRESS, COLS - 1, end, release=True)
    check_copy(wait_for_copy(offset))

    # The copied text stays highlighted; the wheel moves it with its text.
    near_end = f"Line {LINES - 2} of a long pasted message."
    wait_until(lambda: highlighted(near_end), "the copied text highlighted")
    before = screen.row_of(near_end)
    mouse(WHEEL_UP, 10, middle)
    wait_until(lambda: (screen.row_of(near_end) or before) > before, "the wheel scrolling after the copy")
    wait_until(lambda: highlighted(near_end), "the highlight following its text")
    os.write(master, b"\x1b[C")
    wait_until(lambda: not highlighted(near_end), "a key clearing the highlight")

    # The edge, button held: from the last line, hold the pointer above the
    # history until it scrolls to its start, then release there.
    wheel_until(LAST, WHEEL_DOWN, 10, middle)
    end = screen.row_of(LAST)
    mouse(PRESS, COLS - 1, end)
    mouse(DRAG, 1, 1)
    wait_for(HISTORY_START)
    pump(.2)
    offset = len(captured)
    mouse(PRESS, 1, 1, release=True)
    text = wait_for_copy(offset)
    check_copy(text)
    require(HISTORY_START in text, f"The copy should start at the history's start: {text!r}")

    # A still click while a reply streams in below it, with a wheel notch that
    # cannot scroll (the view is at the latest activity): a click, no copy.
    os.write(master, b"\x1b[1;5FAnother question\r")
    wait_for("Ctrl+Y allow once")
    pump(.6)  # An approval accepts answers once on screen for half a second.
    os.write(master, b"\x19")
    wait_until(lambda: not screen.shows("Ctrl+Y allow once"), "the approval closing")
    row = screen.row_of("Another question")
    seen = screen.lines()[row - 1]
    offset = len(captured)
    mouse(PRESS, 10, row)
    wait_until(lambda: "Working" not in screen.text() and screen.shows("Execution Completed"),
               "the second reply")
    mouse(WHEEL_DOWN, 10, row)
    pump(.2)
    mouse(PRESS, 10, row, release=True)
    pump(.3)
    require(screen.lines()[row - 1] != seen, "The history did not move under the pointer")
    require(copied_since(offset) is None, f"A still click copied: {copied_since(offset)!r}")

    os.write(master, b"\x04")
    require(process.wait(timeout=5) == 0, "The terminal did not exit cleanly")
finally:
    if process.poll() is None:
        process.kill()
        process.wait(timeout=5)
    os.close(master)
    os.close(slave)

print("Selection checks passed: wheel and edge scrolling while held, off-screen copy, highlight kept after the copy, still clicks while content arrives.")
