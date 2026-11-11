"""Browse complete inputs with real arrow keys, including commands and newlines."""
from terminal_pty import Terminal, binary

UP, DOWN = b"\x1b[A", b"\x1b[B"

with Terminal([binary("assistant-demo"), "--no-animation", "--native-selection"]) as terminal:
    terminal.wait_for(b"Ready")

    def current_line():
        # The renderer puts the terminal cursor in the input being edited.
        return "".join(terminal.screen.grid[terminal.screen.row]).strip(" >›❯")

    def expect_line(text):
        terminal.wait_until(lambda: current_line() == text, f"input line {text!r}")

    def submit(text):
        terminal.send(b"\x1b[200~" + text.encode() + b"\x1b[201~")
        expect_line(text.split("\n")[-1])
        terminal.send(b"\r")
        expect_line("")

    # Help accepts arguments without running a model or asking permission.
    submit("/help")
    submit("/help\nHISTORY_SECOND_LINE")
    terminal.send(b"UNFINISHED_DRAFT")
    expect_line("UNFINISHED_DRAFT")
    for key, expected in [
        (UP, "HISTORY_SECOND_LINE"),
        (UP, "/help"),
        (DOWN, "HISTORY_SECOND_LINE"),
        (DOWN, "UNFINISHED_DRAFT"),
    ]:
        terminal.send(key)
        expect_line(expected)
    terminal.send(b"\x04")
    terminal.exited()

print("Input history PTY passed: whole multiline entries, commands, Up/Down and draft restoration.")
