"""Copy a message longer than the screen with the mouse, in a real terminal.

Scrolling while the button is held keeps the selection, whether by the wheel
or by holding the pointer past the conversation's edge, and the copy holds
the lines no longer on screen. Once copied, the selection stays highlighted
and scrolls with its text until a key is pressed. Content arriving under a
still pointer never turns a click into a copy.
"""
import base64
import re

from terminal_pty import Terminal, binary, require

ROWS, COLS = 24, 100
LINES = 40
FIRST = "Line 01 of a long pasted message."
LAST = f"Line {LINES} of a long pasted message."
# The demo's welcome message opens the history.
HISTORY_START = "This example uses a scripted host"
# The demo streams this reply as a draft, then stores it under another id.
REPLY = "Order 42 is ready"
REPLY_END = "slash command."
COPY = re.compile(rb"\x1b\]52;c;([A-Za-z0-9+/=]*)\x07")
PRESS, DRAG, WHEEL_UP, WHEEL_DOWN = 0, 32, 64, 65

terminal = Terminal([binary("order-support"), "--no-color", "--no-animation"], rows=ROWS, cols=COLS)


def wheel_until(marker, code, column, row):
    """Turn the wheel one notch at a time until the screen shows `marker`."""
    for _ in range(80):
        if terminal.screen.shows(marker):
            return
        terminal.mouse(code, column, row)
        terminal.pump(.05)
    raise RuntimeError(f"Scrolling never showed {marker!r}; {terminal.context()}")


def copied_since(offset):
    match = COPY.search(terminal.output, offset)
    return base64.b64decode(match.group(1)).decode() if match else None


def wait_for_copy(offset):
    terminal.wait_until(lambda: copied_since(offset) is not None, "a clipboard write")
    return copied_since(offset)


def highlighted(marker):
    """Whether the line showing `marker` shows it in reverse video."""
    row = terminal.screen.row_of(marker)
    return row is not None and marker in terminal.screen.reversed_text(row)


def check_copy(text):
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    require(FIRST in lines and LAST in lines, f"The copy lacks an end: {text!r}")
    require(sum(line.startswith("Line ") for line in lines) == LINES, f"Lines are missing: {text!r}")


with terminal:
    terminal.wait_for("scripted/balanced")
    message = "\n\n".join(f"Line {n:02} of a long pasted message." for n in range(1, LINES + 1))
    terminal.send(b"\x1b[200~" + message.encode() + b"\x1b[201~\r")
    terminal.wait_for("Ctrl+Y allow once")
    terminal.send(b"\x19")

    # Selected while the reply streams, released once it is stored under its
    # own id: the selection follows the draft into the stored message.
    terminal.wait_for(REPLY)
    row = terminal.screen.row_of(REPLY)
    column = terminal.screen.lines()[row - 1].index(REPLY) + 1
    terminal.mouse(PRESS, column, row)
    terminal.mouse(DRAG, COLS - 1, row)
    require(not terminal.screen.ever(REPLY_END), "The reply was stored before the selection began")
    terminal.wait_for("Execution Completed")
    offset = len(terminal.output)
    terminal.mouse(PRESS, COLS - 1, row, release=True)
    text = wait_for_copy(offset)
    require(text.startswith(REPLY), f"The copy lost the finalized reply: {text!r}")

    # The wheel alone, button held and pointer still: the end follows what
    # the pointer shows once the conversation scrolled.
    middle = ROWS // 2
    wheel_until(FIRST, WHEEL_UP, 10, middle)
    start = terminal.screen.row_of(FIRST)
    terminal.mouse(PRESS, 1, start)
    terminal.mouse(WHEEL_DOWN, 1, start)
    terminal.wait_until(lambda: terminal.screen.row_of(FIRST) != start, "the wheel scrolling under a held button")
    offset = len(terminal.output)
    terminal.mouse(PRESS, 1, start, release=True)
    text = wait_for_copy(offset)
    require(text.lstrip().startswith(FIRST) and "Line 02 of" in text,
            f"The wheel did not extend the selection: {text!r}")

    # The wheel, button held: press on the first line, scroll to the last.
    wheel_until(FIRST, WHEEL_UP, 10, middle)
    start = terminal.screen.row_of(FIRST)
    terminal.mouse(PRESS, 1, start)
    wheel_until(LAST, WHEEL_DOWN, 1, start)
    end = terminal.screen.row_of(LAST)
    terminal.mouse(DRAG, COLS - 1, end)
    terminal.pump(.1)
    require(not terminal.screen.shows(FIRST), "The first line should have scrolled off screen")
    offset = len(terminal.output)
    terminal.mouse(PRESS, COLS - 1, end, release=True)
    check_copy(wait_for_copy(offset))

    # The copied text stays highlighted; the wheel moves it with its text.
    near_end = f"Line {LINES - 2} of a long pasted message."
    terminal.wait_until(lambda: highlighted(near_end), "the copied text highlighted")
    before = terminal.screen.row_of(near_end)
    terminal.mouse(WHEEL_UP, 10, middle)
    terminal.wait_until(lambda: (terminal.screen.row_of(near_end) or before) > before, "the wheel scrolling after the copy")
    terminal.wait_until(lambda: highlighted(near_end), "the highlight following its text")
    terminal.send(b"\x1b[C")
    terminal.wait_until(lambda: not highlighted(near_end), "a key clearing the highlight")

    # The edge, button held: from the last line, hold the pointer above the
    # history until it scrolls to its start, then release there.
    wheel_until(LAST, WHEEL_DOWN, 10, middle)
    end = terminal.screen.row_of(LAST)
    terminal.mouse(PRESS, COLS - 1, end)
    terminal.mouse(DRAG, 1, 1)
    terminal.wait_for(HISTORY_START)
    terminal.pump(.2)
    offset = len(terminal.output)
    terminal.mouse(PRESS, 1, 1, release=True)
    text = wait_for_copy(offset)
    check_copy(text)
    require(HISTORY_START in text, f"The copy should start at the history's start: {text!r}")

    # A still click while a reply streams in below it, with a wheel notch that
    # cannot scroll (the view is at the latest activity): a click, no copy.
    terminal.send(b"\x1b[1;5FAnother question\r")
    terminal.wait_for("Ctrl+Y allow once")
    terminal.pump(.6)  # An approval accepts answers once on screen for half a second.
    terminal.send(b"\x19")
    # The card's own choices, not its hint, say it is gone: the hint changes
    # to "Waiting for host decision" while the card is still shown.
    terminal.wait_until(lambda: not terminal.screen.shows("Allow once"), "the approval closing")
    row = terminal.screen.row_of("Another question")
    seen = terminal.screen.lines()[row - 1]
    offset = len(terminal.output)
    terminal.mouse(PRESS, 10, row)
    terminal.wait_until(
        lambda: "Working" not in terminal.screen.text() and terminal.screen.shows("Execution Completed"),
        "the second reply",
    )
    terminal.mouse(WHEEL_DOWN, 10, row)
    terminal.pump(.2)
    terminal.mouse(PRESS, 10, row, release=True)
    terminal.pump(.3)
    require(terminal.screen.lines()[row - 1] != seen, "The history did not move under the pointer")
    require(copied_since(offset) is None, f"A still click copied: {copied_since(offset)!r}")

    terminal.send(b"\x04")
    terminal.exited()

print("Selection checks passed: a selection across a finalized draft, wheel and edge scrolling while held, off-screen copy, highlight kept after the copy, still clicks while content arrives.")
