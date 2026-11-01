"""A program under test on a real pseudo-terminal, as the checks drive it.

Spawning, resizing, reading under a deadline, mouse reports, and the check
that the program leaves the terminal's modes as it found them. What the
program shows is read through the screen model of terminal_screen.py.
"""
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import termios
import time

from terminal_screen import Screen

DEADLINE = 10  # seconds a check waits for what it expects


def require(condition, message):
    """Fail the check explicitly; unlike assert, this survives python -O."""
    if not condition:
        raise SystemExit(f"Check failed: {message}")


def binary(name):
    """The debug build of example binary `name`."""
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], text=True
    ))
    return Path(metadata["target_directory"]) / "debug" / name


class Terminal:
    """One program on a pseudo-terminal of `rows` by `cols` cells."""

    def __init__(self, args, rows=24, cols=100, env=None):
        self.master, self.slave = pty.openpty()
        self.screen = Screen(rows, cols)
        self.process = None
        self.resize(rows, cols)
        self.original = termios.tcgetattr(self.slave)
        self.output = bytearray()
        self.process = subprocess.Popen(
            [str(arg) for arg in args], stdin=self.slave, stdout=self.slave, stderr=self.slave,
            env={**os.environ, **(env or {})},
        )

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()

    def resize(self, rows, cols):
        """Resize the terminal and tell the program, as a window would. What it
        wrote at the old size is read first, so the screen model never lays it
        out at the new size, and its full repaint at the new size is awaited:
        crossterm 0.29 can leave a key unread when it arrives with the resize
        signal, until the next key comes."""
        running = self.process is not None
        if running:
            self.pump(0.05)
            offset = len(self.output)
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.screen.resize(rows, cols)
        if running:
            os.kill(self.process.pid, signal.SIGWINCH)
            self.wait_for(b"\x1b[2J", after=offset)

    def send(self, data):
        os.write(self.master, data)

    def mouse(self, code, column, row, release=False):
        """Send an SGR mouse report; columns and rows count from 1."""
        self.send(f"\x1b[<{code};{column};{row}{'m' if release else 'M'}".encode())

    def read(self, timeout):
        """Take what the program writes within `timeout` seconds, if anything."""
        if not select.select([self.master], [], [], max(0, timeout))[0]:
            return
        try:
            chunk = os.read(self.master, 65536)
        except OSError as error:
            if error.errno == errno.EIO:
                raise RuntimeError(f"Terminal closed early; {self.context()}") from error
            raise
        if not chunk:
            raise RuntimeError("Terminal output closed unexpectedly")
        self.output.extend(chunk)
        self.screen.feed(chunk)

    def pump(self, seconds):
        """Read for `seconds`, whatever comes."""
        deadline = time.monotonic() + seconds
        while (remaining := deadline - time.monotonic()) > 0:
            self.read(remaining)

    def wait_until(self, condition, what):
        deadline = time.monotonic() + DEADLINE
        while not condition():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeError(f"Missing {what}; {self.context()}")
            self.read(remaining)

    def wait_for(self, marker, after=0):
        """Wait until a frame drawn since the wait began shows `marker`: text,
        bytes or a pattern. Raw sequences (bytes starting with ESC) are
        searched in the output written since `after`."""
        if isinstance(marker, bytes) and marker.startswith(b"\x1b"):
            self.wait_until(lambda: marker in self.output[after:], repr(marker))
        else:
            # Any frame since the wait began counts: a brief hint can be drawn
            # and cleared within one read when the check falls behind.
            since = len(self.screen.history) - 1
            self.wait_until(lambda: self.screen.showed(marker, since), repr(marker))

    def context(self):
        """What a failed wait shows to explain itself."""
        return f"screen:\n{self.screen.text()}\ntail={bytes(self.output[-1000:])!r}"

    def exited(self):
        """The program exits cleanly and restores the terminal's modes. Its
        output is read while it exits: unread, a full pseudo-terminal buffer
        blocks the program's last writes, and it never exits."""
        deadline = time.monotonic() + DEADLINE
        while self.process.poll() is None:
            remaining = deadline - time.monotonic()
            require(remaining > 0, f"The program did not exit; {self.context()}")
            self.read(min(remaining, 0.05))
        require(self.process.returncode == 0, "The program did not exit cleanly")
        while select.select([self.master], [], [], 0)[0]:
            self.output.extend(os.read(self.master, 65536))
        require(termios.tcgetattr(self.slave) == self.original, "Terminal modes were not restored")

    def close(self):
        if self.process is not None and self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=5)
        os.close(self.master)
        os.close(self.slave)
