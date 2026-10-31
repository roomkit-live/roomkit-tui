"""Exercise sidebar visibility, keyboard navigation, resize and terminal cleanup."""
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

def require(condition, message):
    """Fail the check explicitly; unlike assert, this survives python -O."""
    if not condition:
        raise SystemExit(f"Check failed: {message}")


metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], text=True
))
binary = Path(metadata["target_directory"]) / "debug" / "assistant-demo"
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 140, 0, 0))
original = termios.tcgetattr(slave)
process = subprocess.Popen([str(binary), "--no-color", "--no-animation"], stdin=slave, stdout=slave, stderr=slave)
captured = bytearray()


def wait_for(marker, after=0):
    deadline = time.monotonic() + 10
    while marker not in re.sub(rb"\x1b\[[0-9;:]*[A-Za-z]", b"", captured[after:]):
        if time.monotonic() >= deadline:
            raise RuntimeError(f"Missing sidebar output {marker!r}: {bytes(captured[-1500:])!r}")
        if not select.select([master], [], [], .1)[0]:
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


try:
    wait_for(b"F7 hide")
    os.write(master, b"Keep my draft")
    wait_for(b"Keep my draft")
    start = len(captured)
    os.write(master, b"\x1b[18~")  # F7
    wait_for(b"F7 sidebar", start)
    start = len(captured)
    os.write(master, b"\x1b[18~\x1b[19~")  # F7, F8
    wait_for(b"Esc back", start)
    # Navigate to the application's help link and invoke it, retaining the draft.
    os.write(master, b"\x1b[B\r")
    wait_for(b"/inspect")
    wait_for(b"Keep my draft")
    start = len(captured)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 90, 0, 0))
    os.kill(process.pid, __import__("signal").SIGWINCH)
    wait_for(b"F7 sidebar", start)
    start = len(captured)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 140, 0, 0))
    os.kill(process.pid, __import__("signal").SIGWINCH)
    wait_for(b"F7 hide", start)
    os.write(master, b"\x04")
    require(process.wait(timeout=5) == 0, 'process.wait(timeout=5) == 0')
    require(termios.tcgetattr(slave) == original, 'termios.tcgetattr(slave) == original')
finally:
    if process.poll() is None:
        process.kill()
        process.wait(timeout=5)
    os.close(master)
    os.close(slave)
print("Sidebar terminal checks passed: visibility, keyboard actions, draft preservation, resize and restoration.")
