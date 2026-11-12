"""Verify native link destinations on the actual PTY output, as iTerm2 receives them."""

import re

from terminal_pty import Terminal, binary, require

GUIDE = b"https://example.com/guide?view=full&lang=en"
BARE = b"https://example.com/~person/guide_(terminal)"
AUTH = (b"https://example.com/authorize?state=" + b"x" * 240
        + b"&scope=openid+profile&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fcallback")
INLINE = b"https://example.com/inline?token=" + b"i" * 150
BLOCK = b"https://example.com/block?state=" + b"b" * 150 + b"&scope=openid+profile"
NOTICE = b"https://example.com/console/" + b"n" * 60 + b"/settings/privacy#regions"


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
    destination(terminal, AUTH)
    require(any(target == AUTH and b"Open sign-in page" in text for target, text in links(terminal.output)),
            "The conversation sign-in label did not carry its destination")
    require(any(target == GUIDE and b"Open guide" in text for target, text in links(terminal.output)),
            "The Markdown label did not carry its destination")
    require(b"\x1b[?1006h" in terminal.output, "The scenario must run with application mouse capture")
    offset = len(terminal.output)
    terminal.send(b"/connect\r")
    terminal.wait_for(b"Return URL")
    destination(terminal, AUTH, offset)
    require(any(target == AUTH and b"Open sign-in page" in text for target, text in links(terminal.output[offset:])),
            "The same sign-in label lost its destination in the form")
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
    terminal.wait_for(b"Use /connect to open the form.")
    # Code keeps its text, and every wrapped row links the entire URL: without
    # it the terminal's own detection opens only the row clicked.
    offset = len(terminal.output)
    terminal.send(b"/code\r")
    terminal.wait_for(b"Inline")
    for url in (INLINE, BLOCK):
        destination(terminal, url, offset)
        fragments = [text for target, text in links(terminal.output[offset:]) if target == url]
        require(len(fragments) > 1, f"The code URL {url[:30]!r} did not wrap into linked rows")
        require(all(text and text in url for text in fragments),
                f"A linked code fragment is not part of {url[:30]!r}")
    # A notice wraps below the activity row: each row links the entire URL,
    # and copying the notice with the mouse leaves it on screen.
    offset = len(terminal.output)
    terminal.send(b"/notice\r")
    terminal.wait_for(b"use it.", after=offset)
    destination(terminal, NOTICE, offset)
    fragments = {text for target, text in links(terminal.output[offset:]) if target == NOTICE}
    require(len(fragments) > 1, "The notice URL did not wrap into linked rows")
    require(all(text and text in NOTICE for text in fragments),
            "A linked notice fragment is not part of its URL")
    row = 1 + next(index for index, text in enumerate(terminal.screen.text().split("\n"))
                   if "Select Global" in text)
    offset = len(terminal.output)
    terminal.mouse(0, 2, row)
    terminal.mouse(32, 30, row + 1)
    terminal.mouse(0, 30, row + 1, release=True)
    terminal.wait_for(b"\x1b]52;c;", after=offset)
    terminal.send(b"\x1b")
    terminal.pump(0.5)
    require("Select Global" in terminal.screen.text(), "Copying the notice removed it")
    terminal.send(b"\x04")
    terminal.exited()

print("Link PTY passed: Markdown labels, bare URLs, code, notices, wrapped form links, mouse capture, resize and selection retain exact native destinations; a copied notice stays.")
