"""Verify native link destinations on the actual PTY output, as iTerm2 receives them."""

import re

from terminal_pty import Terminal, binary, require

GUIDE = b"https://example.com/guide?view=full&lang=en"
BARE = b"https://example.com/~person/guide_(terminal)"
AUTH = b"https://example.com/authorize?state=" + b"x" * 240 + b"&scope=read"


def links(output):
    # Codex uses BEL-terminated OSC 8, with the complete URL on every fragment.
    return re.findall(rb"\x1b\]8;;([^\x07]+)\x07([^\x1b]*)\x1b\]8;;\x07", output)


def destination(terminal, url, after=0):
    terminal.wait_until(lambda: any(target == url for target, _ in links(terminal.output[after:])),
                        f"native link to {url!r}")


with Terminal([binary("link-host")], rows=30, cols=100) as terminal:
    terminal.wait_for(b"Open guide")
    destination(terminal, GUIDE)
    destination(terminal, BARE)
    require(any(target == GUIDE and b"Open guide" in text for target, text in links(terminal.output)),
            "The Markdown label did not carry its destination")
    require(b"\x1b[?1006h" in terminal.output, "The scenario must run with application mouse capture")
    offset = len(terminal.output)
    terminal.send(b"/connect\r")
    terminal.wait_for(b"Return URL")
    destination(terminal, AUTH, offset)
    require(sum(target == AUTH for target, _ in links(terminal.output[offset:])) > 1,
            "The long form URL did not wrap into multiple linked fragments")
    offset = len(terminal.output)
    terminal.resize(30, 60)
    destination(terminal, AUTH, offset)
    # Selecting form text repaints it; native destinations must survive that paint.
    offset = len(terminal.output)
    terminal.mouse(0, 2, 4)
    terminal.mouse(32, 8, 4)
    terminal.wait_for(b"\x1b[7m", after=offset)
    destination(terminal, AUTH, offset)
    terminal.mouse(0, 8, 4, release=True)
    terminal.send(b"\x1b")
    terminal.wait_for(b"Open guide")
    terminal.send(b"\x04")
    terminal.exited()

print("Link PTY passed: Markdown labels, bare URLs, wrapped form links, mouse capture, resize and selection retain exact native destinations.")
