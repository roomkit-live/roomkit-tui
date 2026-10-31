"""Regenerate assets/screenshot.png from the assistant-demo example.

Drives the example in a pseudo-terminal, renders the final screen to HTML
with the pyte terminal emulator, then to PNG with headless Chrome and
ImageMagick. Development tool only; nothing here runs in CI.

    python3 -m venv /tmp/shot-env && /tmp/shot-env/bin/pip install pyte
    cargo build -p roomkit-tui-examples --bin assistant-demo
    /tmp/shot-env/bin/python scripts/screenshot.py target/debug/assistant-demo /tmp/shot 132 31
    google-chrome --headless=new --disable-gpu --hide-scrollbars \
      --force-device-scale-factor=2 --default-background-color=00000000 \
      --window-size=1220,760 --screenshot=/tmp/shot/raw.png /tmp/shot/03-answer.html
    convert /tmp/shot/raw.png -trim +repage -strip assets/screenshot.png
"""

import errno
import fcntl
import html
import os
import pty
import select
import struct
import subprocess
import sys
import termios
import time

import pyte

binary, out_dir = sys.argv[1], sys.argv[2]
cols = int(sys.argv[3]) if len(sys.argv) > 3 else 140
rows = int(sys.argv[4]) if len(sys.argv) > 4 else 36

screen = pyte.Screen(cols, rows)
stream = pyte.ByteStream(screen)

master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
env = {k: v for k, v in os.environ.items() if k not in ("NO_COLOR",)}
env.update(TERM="xterm-256color", COLORTERM="truecolor")
process = subprocess.Popen(
    [binary, *sys.argv[5:]], stdin=slave, stdout=slave, stderr=slave, env=env
)


def pump(seconds):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        ready, _, _ = select.select([master], [], [], 0.05)
        if not ready:
            continue
        try:
            chunk = os.read(master, 65536)
        except OSError as error:
            if error.errno == errno.EIO:
                return
            raise
        stream.feed(chunk)


def text():
    return "\n".join(screen.display)


def wait_for(marker, timeout=10):
    deadline = time.monotonic() + timeout
    while marker not in text():
        if time.monotonic() >= deadline:
            raise RuntimeError(f"missing {marker!r}:\n{text()}")
        pump(0.1)


def send(data, settle=0.3):
    os.write(master, data)
    pump(settle)


COLORS = {
    "black": "000000", "red": "cd3131", "green": "0dbc79", "brown": "e5e510",
    "blue": "2472c8", "magenta": "bc3fbc", "cyan": "11a8cd", "white": "e5e5e5",
    "brightblack": "666666", "brightred": "f14c4c", "brightgreen": "23d18b",
    "brightbrown": "f5f543", "brightblue": "3b8eea", "brightmagenta": "d670d6",
    "brightcyan": "29b8db", "brightwhite": "ffffff",
}
DEFAULT_FG, DEFAULT_BG = "d4d4d8", "16161e"


def color(value, fallback):
    if value == "default":
        return fallback
    if value in COLORS:
        return COLORS[value]
    if len(value) == 6:
        return value
    return fallback


def render(name):
    lines = []
    for y in range(rows):
        line = screen.buffer[y]
        spans, current, buffer = [], None, []
        for x in range(cols):
            char = line[x]
            if char.data == "":
                continue  # Second cell of a wide character.
            fg = color(char.fg, DEFAULT_FG)
            bg = color(char.bg, DEFAULT_BG)
            if char.reverse:
                fg, bg = bg, fg
            style = (fg, bg, char.bold, char.italics, char.underscore)
            if style != current and buffer:
                spans.append((current, "".join(buffer)))
                buffer = []
            current = style
            buffer.append(char.data)
        if buffer:
            spans.append((current, "".join(buffer)))
        parts = []
        for (fg, bg, bold, italic, underline), chunk in spans:
            css = f"color:#{fg};background:#{bg}"
            if bold:
                css += ";font-weight:700"
            if italic:
                css += ";font-style:italic"
            if underline:
                css += ";text-decoration:underline"
            parts.append(f'<span style="{css}">{html.escape(chunk)}</span>')
        lines.append(f'<div class="row">{"".join(parts)}</div>')
    page = f"""<!doctype html>
<html><head><meta charset="utf-8"><style>
html,body{{margin:0;background:transparent}}
.window{{display:inline-block;margin:20px;border-radius:10px;overflow:hidden;
  background:#{DEFAULT_BG};box-shadow:0 12px 36px rgba(0,0,0,.35);
  border:1px solid #2a2a36}}
.bar{{height:30px;background:#20202a;display:flex;align-items:center;padding:0 12px;gap:8px}}
.dot{{width:12px;height:12px;border-radius:50%}}
.screen{{padding:10px 14px 14px;font:14px/18px 'DejaVu Sans Mono',monospace;white-space:pre}}
.row{{height:18px}}
.row span{{display:inline-block;height:18px;vertical-align:top}}
</style></head><body><div class="window">
<div class="bar"><span class="dot" style="background:#ff5f57"></span>
<span class="dot" style="background:#febc2e"></span>
<span class="dot" style="background:#28c840"></span></div>
<div class="screen">{"".join(lines)}</div></div></body></html>"""
    path = os.path.join(out_dir, f"{name}.html")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(page)
    with open(os.path.join(out_dir, f"{name}.txt"), "w", encoding="utf-8") as handle:
        handle.write(text())
    print("wrote", path)


try:
    wait_for("Ready")
    pump(0.5)
    render("01-welcome")
    send(b"Look up record 42 and summarize it", 0.3)
    send(b"\r", 0.2)
    wait_for("Ctrl+Y")
    pump(0.6)
    render("02-approval")
    # The approval card accepts answers once it could be read.
    send(b"\x19", 0.2)
    wait_for("Execution Completed", 15)
    pump(1.5)
    render("03-answer")
finally:
    try:
        os.write(master, b"\x04")
        process.wait(timeout=5)
    except Exception:
        process.kill()
