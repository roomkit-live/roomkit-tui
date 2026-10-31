"""Check exact OSC 7501 bytes and lifecycle using real terminal entry points."""

import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import subprocess
import termios
import time

from terminal_screen import Screen

def require(condition, message):
    """Fail the check explicitly; unlike assert, this survives python -O."""
    if not condition:
        raise SystemExit(f"Check failed: {message}")


APPROVAL_ARMING = 0.6  # seconds; the terminal arms decisions after 500 ms

metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], text=True
))
example = Path(metadata["target_directory"]) / "debug" / "order-support"
artifacts = subprocess.check_output([
    "cargo", "test", "--locked", "-p", "roomkit-tui-examples", "--test", "terminal_status",
    "--no-run", "--message-format=json",
], text=True)
fixture = next(row["executable"] for line in artifacts.splitlines()
               if (row := json.loads(line)).get("executable") and row.get("target", {}).get("name") == "terminal_status")
fixture_args = [fixture, "--ignored", "--exact", "status_fixture", "--nocapture"]
osc = re.compile(rb"\x1b\]7501;([^\x1b]*)\x1b\\")


class Terminal:
    def __init__(self, args, term="xterm-256color"):
        self.master, self.slave = pty.openpty()
        self.screen = Screen(24, 100)
        self.resize(24, 100)
        self.original = termios.tcgetattr(self.slave)
        self.output = bytearray()
        self.process = subprocess.Popen(args, stdin=self.slave, stdout=self.slave, stderr=self.slave,
                                        env={**os.environ, "TERM": term, "NO_COLOR": "1"})

    def resize(self, rows, cols):
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.screen.resize(rows, cols)
        if hasattr(self, "process"):
            os.kill(self.process.pid, signal.SIGWINCH)

    def send(self, value):
        os.write(self.master, value)

    def wait(self, predicate):
        deadline = time.monotonic() + 10
        while not predicate():
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([self.master], [], [], max(0, remaining))[0]:
                raise AssertionError(f"Terminal deadline; reports={self.reports()}, screen:\n{self.screen.text()}\ntail={self.output[-1000:]!r}")
            chunk = os.read(self.master, 65536)
            self.output.extend(chunk)
            self.screen.feed(chunk)

    def text(self, text, after=0):
        """Wait until the screen shows `text`."""
        self.wait(lambda: self.screen.shows(text))

    def reports(self):
        return [value.decode("ascii") for value in osc.findall(self.output)]

    def state(self, state, app="status-fixture", after=0):
        pairs = f"state={state}:app={app}"
        self.wait(lambda: pairs in self.reports()[after:])

    def exited(self):
        require(self.process.wait(timeout=5) == 0, 'self.process.wait(timeout=5) == 0')
        while select.select([self.master], [], [], 0)[0]:
            self.output.extend(os.read(self.master, 65536))
        require(termios.tcgetattr(self.slave) == self.original, 'termios.tcgetattr(self.slave) == self.original')
        require(b"\x1b[?1049l" in self.output, 'b"\\x1b[?1049l" in self.output')
        require(not re.search(rb"\x1b\[(?:38|48);", self.output), "NO_COLOR was ignored")

    def close(self):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=5)
        os.close(self.master)
        os.close(self.slave)


terminal = Terminal(fixture_args)
try:
    terminal.state("idle")
    terminal.send(b"/stream-failure\r")
    terminal.state("error")
    terminal.text(b"Execution Failed")
    # A resize changes rendering, but must not dismiss or resend the result.
    before = len(terminal.output)
    terminal.resize(20, 80)
    terminal.text(b"Execution Failed", before)
    require(terminal.reports() == [f"state={state}:app=status-fixture" for state in ("idle", "working", "error")], 'terminal.reports() == [f"state={state}:app=status-fixture" for state in ("idle", "working", "error")]')
    terminal.send(b"/admission-failure\r")
    terminal.state("error", after=3)
    terminal.text(b"Execution Failed", before)
    terminal.send(b"/switch\r")
    terminal.text(b"Status fixture restored")
    terminal.send(b"Inspect record\r")
    terminal.state("blocked:kind=permission")
    terminal.text(b"Approve first action?")
    count = len(terminal.reports())
    # A new approval card ignores answers until it could be read.
    time.sleep(APPROVAL_ARMING)
    terminal.send(b"\x19")
    terminal.text(b"Approve second action?")
    require(len(terminal.reports()) == count, "One open permission must remain blocked")
    time.sleep(APPROVAL_ARMING)
    terminal.send(b"\x19")
    terminal.state("done")
    terminal.text(b"Execution Completed")
    terminal.send(b"/disconnect\r")
    terminal.exited()
    require(terminal.reports() == [f"state={state}:app=status-fixture" for state in (
        "idle", "working", "error", "idle", "working", "error", "idle",
        "working", "blocked:kind=permission", "working", "done", "idle", "clear",
    )], terminal.reports())
finally:
    terminal.close()

# A host disconnect in the same burst as admission still clears the active record.
terminal = Terminal(fixture_args)
try:
    terminal.state("idle")
    terminal.send(b"/disconnect-working\r")
    terminal.exited()
    require(terminal.reports() == [f"state={state}:app=status-fixture" for state in ("idle", "working", "clear")], 'terminal.reports() == [f"state={state}:app=status-fixture" for state in ("idle", "working", "clear")]')
finally:
    terminal.close()

# An existing run-only host works with reporting and animation disabled.
terminal = Terminal([str(example), "--terminal-status", "order-support", "--no-animation"])
try:
    terminal.state("idle", "order-support")
    terminal.send(b"Inspect record\r")
    terminal.state("blocked:kind=permission", "order-support")
    terminal.send(b"\x1b")
    terminal.text(b"Execution Interrupted")
    terminal.send(b"\x04")
    terminal.exited()
    require(terminal.reports() == [f"state={state}:app=order-support" for state in (
        "idle", "working", "blocked:kind=permission", "working", "idle", "clear",
    )], terminal.reports())
finally:
    terminal.close()

for args, term in [([], "xterm-256color"),
                   (["--terminal-status", "example", "--no-terminal-status"], "xterm-256color"),
                   (["--terminal-status", "example"], "dumb")]:
    terminal = Terminal([str(example), *args], term)
    try:
        terminal.text(b"scripted/balanced")
        terminal.send(b"\x04")
        terminal.exited()
        require(not terminal.reports(), "Disabled reporting emitted OSC 7501")
    finally:
        terminal.close()

for args in [[], ["--help"], ["--check"], ["--json"]]:
    result = subprocess.run([str(example), "--terminal-status", "example", *args],
                            input=b"", capture_output=True, timeout=10)
    require(result.returncode == (0 if args else 1), 'result.returncode == (0 if args else 1)')
    require(b"\x1b]7501;" not in result.stdout + result.stderr, "Noninteractive output emitted status")

print("OSC 7501 checks passed: exact transitions, permission settlement, latched results, admission/stream errors, interruption, view switching, cleanup and opt-in output.")
