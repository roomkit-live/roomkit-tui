"""Exercise a real POSIX terminal, with deadlines and terminal-mode restoration."""

import errno
import base64
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

def require(condition, message):
    """Fail the check explicitly; unlike assert, this survives python -O."""
    if not condition:
        raise SystemExit(f"Check failed: {message}")


metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], text=True
))
binary = Path(metadata["target_directory"]) / "debug" / "order-support"


def scenario(allow, enhanced=False, escape=False):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))
    original = termios.tcgetattr(slave)
    args = [str(binary)]
    if enhanced:
        args += ["--no-color", "--no-animation", "--brand", "TerminalFixture", "--command-prefix", "$"]
    else:
        args += ["--native-selection"]
    process = subprocess.Popen(args, stdin=slave, stdout=slave, stderr=slave)
    captured = bytearray()

    def wait_for(marker, after=0):
        deadline = time.monotonic() + 10
        def found():
            if isinstance(marker, re.Pattern):
                plain = re.sub(rb"\x1b\[[0-9;:]*[A-Za-z]", b"", captured[after:])
                return marker.search(plain) is not None
            return marker in captured[after:]
        while not found():
            if time.monotonic() >= deadline:
                raise RuntimeError(f"Terminal did not display {marker!r}; output: {bytes(captured[-2000:])!r}")
            ready, _, _ = select.select([master], [], [], max(0, deadline - time.monotonic()))
            if not ready:
                continue
            try:
                chunk = os.read(master, 65536)
            except OSError as error:
                if error.errno == errno.EIO:
                    raise RuntimeError(f"Terminal exited before displaying {marker!r}") from error
                raise
            if not chunk:
                raise RuntimeError("Terminal output closed unexpectedly")
            captured.extend(chunk)

    def click(marker):
        index = captured.rfind(marker)
        positions = re.findall(rb"\x1b\[(\d+);(\d+)H", captured[:index])
        if index < 0 or not positions:
            raise RuntimeError("No visible tool disclosure target")
        row = positions[-1][0]
        os.write(master, b"\x1b[<0;10;" + row + b"M\x1b[<0;10;" + row + b"m")

    try:
        wait_for(b"scripted/balanced")
        if enhanced:
            os.write(master, b"$mo")
            wait_for(b"Choose a simulated model")
            os.write(master, b"\r")
            wait_for(b"Choices")
            os.write(master, b"\x1b[B\r")
            wait_for(b"$model fast")
            if b"scripted/fast" in captured:
                raise RuntimeError("Selecting a menu item unexpectedly executed it")
            os.write(master, b"\r")
            wait_for(b"scripted/fast")
        os.write(master, (b"$" if enhanced else b"/") + b"order-status 42\r")
        wait_for(b"Ctrl+Y allow once")
        if enhanced:
            # Even with animation disabled, the observed timer advances while
            # waiting for a human; no artificial sleep is needed by the fixture.
            wait_for(re.compile(rb"Waiting for your decision \xc2\xb7 [1-9]\d*s"))
        os.write(master, b"\x19" if allow else (b"\x1b" if escape else b"\x03"))
        wait_for(b"Execution Completed" if allow else b"Execution Interrupted")
        wait_for(re.compile(rb"Execution (?:Completed|Interrupted) \xc2\xb7 \d+\.\ds"))
        if allow:
            wait_for(b"approved")
            wait_for(b"Read record 42")
        else:
            require(b"approved" not in captured and b"denied" not in captured, "An interruption is not a human approval or denial")
        if enhanced:
            offset = len(captured)
            click(b"F2 expand")
            wait_for(b"F2 collapse", after=offset)
            wait_for(b"args:")
            offset = len(captured)
            click(b"F2 collapse")
            wait_for(b"F2 expand", after=offset)
            # Keyboard and mouse control the same per-tool state.
            offset = len(captured)
            os.write(master, b"\x1bOQ")
            wait_for(b"F2 collapse", after=offset)
            # Select exactly the custom application name in the first row.
            os.write(master, b"\x1b[<0;2;1M\x1b[<32;16;1M\x1b[<0;16;1m")
            expected = b"\x1b]52;c;" + base64.b64encode(b"TerminalFixture") + b"\x07"
            wait_for(expected)
            if re.search(rb"\x1b\[(?:38|48);", captured):
                raise RuntimeError("Monochrome mode emitted palette colors")
        # Resize the real terminal before detaching.
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 12, 60, 0, 0))
        if escape:
            os.write(master, b"Draft to clear")
            wait_for(b"Draft to clear")
            offset = len(captured)
            os.write(master, b"\x03")
            wait_for(b"Press Ctrl+C again to exit", offset)
            require(process.poll() is None, "First Ctrl+C exited")
            # Pasting must disarm exit and the previous draft must be gone.
            offset = len(captured)
            os.write(master, b"\x1b[200~New draft\x1b[201~")
            wait_for(b"New draft", offset)
            offset = len(captured)
            os.write(master, b"\x03")
            wait_for(b"Press Ctrl+C again to exit", offset)
            require(process.poll() is None, "Paste did not disarm exit")
            os.write(master, b"\x03")
        else:
            os.write(master, b"\x04")
        code = process.wait(timeout=5)
        if code != 0:
            raise RuntimeError(f"Terminal exited with {code}")
        while select.select([master], [], [], 0)[0]:
            captured.extend(os.read(master, 65536))
        if termios.tcgetattr(slave) != original:
            raise RuntimeError("Terminal modes were not restored")
        if b"\x1b[?1049l" not in captured or b"\x1b[?2004l" not in captured:
            raise RuntimeError("Alternate screen or bracketed paste was not restored")
        if enhanced and b"\x1b[?1006l" not in captured:
            raise RuntimeError("Mouse capture was not restored")
        if not enhanced and b"\x1b[?1006h" in captured:
            raise RuntimeError("Native selection unexpectedly enabled mouse capture")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
        os.close(master)
        os.close(slave)


scenario(True)
scenario(False)
scenario(False, escape=True)
scenario(True, enhanced=True)
print("Terminal checks passed: menus, observed run timing, approval, Esc/Ctrl+C interruption, double-press exit, tool clicks/F2, mouse copy, monochrome, resize and mode restoration.")
