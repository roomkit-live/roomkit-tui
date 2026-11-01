"""Exercise sidebar visibility, keyboard navigation, resize and terminal cleanup."""
import re

from terminal_pty import Terminal, binary, require

# NO_COLOR, not --no-color: the terminal resolves the environment's policy
# for the renderer, which then draws glyphs instead of colors.
with Terminal([binary("assistant-demo"), "--no-animation"], rows=32, cols=140,
              env={"NO_COLOR": "1"}) as terminal:
    terminal.wait_for(b"F7 hide")
    # Without colors, the context breakdown tells its parts apart by glyph.
    terminal.wait_for("█ System prompt".encode())
    terminal.wait_for("▄ Messages".encode())
    require(not terminal.screen.ever("■"), "a colored legend marker without colors")
    terminal.send(b"Keep my draft")
    terminal.wait_for(b"Keep my draft")
    terminal.send(b"\x1b[18~")  # F7
    terminal.wait_for(b"F7 sidebar")
    terminal.send(b"\x1b[18~\x1b[19~")  # F7, F8
    terminal.wait_for(b"Esc back")
    # Navigate to the application's help link and invoke it, retaining the draft.
    terminal.send(b"\x1b[B\r")
    terminal.wait_for(b"/inspect")
    terminal.wait_for(b"Keep my draft")
    terminal.resize(24, 90)
    terminal.wait_for(b"F7 sidebar")
    terminal.resize(32, 140)
    terminal.wait_for(b"F7 hide")
    terminal.send(b"\x04")
    terminal.exited()
    require(not re.search(rb"\x1b\[(?:38|48);", terminal.output), "NO_COLOR emitted a color")
print("Sidebar terminal checks passed: visibility, keyboard actions, draft preservation, resize, uncolored context breakdown and restoration.")
