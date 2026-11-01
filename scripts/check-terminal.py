"""Exercise a real POSIX terminal, with deadlines and terminal-mode restoration."""

import base64
import re

from terminal_pty import Terminal, binary, require

example = binary("order-support")


def scenario(allow, enhanced=False, escape=False):
    args = [example]
    if enhanced:
        args += ["--no-color", "--no-animation", "--brand", "TerminalFixture", "--command-prefix", "$"]
    else:
        args += ["--native-selection"]
    with Terminal(args) as terminal:

        def click(marker):
            row = terminal.screen.row_of(marker)
            if row is None:
                raise RuntimeError("No visible tool disclosure target")
            terminal.mouse(0, 10, row)
            terminal.mouse(0, 10, row, release=True)

        terminal.wait_for(b"scripted/balanced")
        if enhanced:
            terminal.send(b"$mo")
            terminal.wait_for(b"Choose a simulated model")
            terminal.send(b"\r")
            terminal.wait_for(b"Choices")
            terminal.send(b"\x1b[B\r")
            terminal.wait_for(b"$model fast")
            if terminal.screen.ever(b"scripted/fast"):
                raise RuntimeError("Selecting a menu item unexpectedly executed it")
            terminal.send(b"\r")
            terminal.wait_for(b"scripted/fast")
        terminal.send((b"$" if enhanced else b"/") + b"order-status 42\r")
        terminal.wait_for(b"Ctrl+Y allow once")
        if enhanced:
            # Even with animation disabled, the observed timer advances while
            # waiting for a human; no artificial sleep is needed by the fixture.
            terminal.wait_for(re.compile(rb"Waiting for your decision \xc2\xb7 [1-9]\d*s"))
        terminal.send(b"\x19" if allow else (b"\x1b" if escape else b"\x03"))
        terminal.wait_for(b"Execution Completed" if allow else b"Execution Interrupted")
        terminal.wait_for(re.compile(rb"Execution (?:Completed|Interrupted) \xc2\xb7 \d+\.\ds"))
        if allow:
            terminal.wait_for(b"approved")
            terminal.wait_for(b"Read record 42")
        else:
            require(not terminal.screen.ever(b"approved") and not terminal.screen.ever(b"denied"), "An interruption is not a human approval or denial")
        if enhanced:
            offset = len(terminal.output)
            click(b"F2 expand")
            terminal.wait_for(b"F2 collapse", after=offset)
            terminal.wait_for(b"args:")
            offset = len(terminal.output)
            click(b"F2 collapse")
            terminal.wait_for(b"F2 expand", after=offset)
            # Keyboard and mouse control the same per-tool state.
            offset = len(terminal.output)
            terminal.send(b"\x1bOQ")
            terminal.wait_for(b"F2 collapse", after=offset)
            # Select exactly the custom application name in the first row.
            terminal.send(b"\x1b[<0;2;1M\x1b[<32;16;1M\x1b[<0;16;1m")
            expected = b"\x1b]52;c;" + base64.b64encode(b"TerminalFixture") + b"\x07"
            terminal.wait_for(expected)
            if re.search(rb"\x1b\[(?:38|48);", terminal.output):
                raise RuntimeError("Monochrome mode emitted palette colors")
        # Resize the real terminal before detaching.
        terminal.resize(12, 60)
        if escape:
            terminal.send(b"Draft to clear")
            terminal.wait_for(b"Draft to clear")
            terminal.send(b"\x03")
            terminal.wait_for(b"Press Ctrl+C again to exit")
            require(terminal.process.poll() is None, "First Ctrl+C exited")
            # Pasting must disarm exit and the previous draft must be gone.
            terminal.send(b"\x1b[200~New draft\x1b[201~")
            terminal.wait_for(b"New draft")
            terminal.send(b"\x03")
            terminal.wait_for(b"Press Ctrl+C again to exit")
            require(terminal.process.poll() is None, "Paste did not disarm exit")
            terminal.send(b"\x03")
        else:
            terminal.send(b"\x04")
        terminal.exited()
        output = terminal.output
        require(b"\x1b[?1049l" in output and b"\x1b[?2004l" in output,
                "Alternate screen or bracketed paste was not restored")
        require(not enhanced or b"\x1b[?1006l" in output, "Mouse capture was not restored")
        require(enhanced or b"\x1b[?1006h" not in output,
                "Native selection unexpectedly enabled mouse capture")


scenario(True)
scenario(False)
scenario(False, escape=True)
scenario(True, enhanced=True)
print("Terminal checks passed: menus, observed run timing, approval, Esc/Ctrl+C interruption, double-press exit, tool clicks/F2, mouse copy, monochrome, resize and mode restoration.")
