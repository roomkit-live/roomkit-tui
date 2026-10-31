"""A minimal VT screen model for the terminal checks.

The checks assert what a terminal would display, not the bytes written: a
renderer may rewrite only the cells that change, so a word on screen need not
appear contiguously in the output stream. Raw sequences (OSC reports, mode
changes, colors) are still checked on the captured bytes.
"""
import re
import unicodedata

_SEQUENCE = re.compile(
    rb"\x1b\[([0-9;:?<=>]*)[ -/]*([@-~])"  # CSI
    rb"|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)"  # OSC, ended by BEL or ST
    rb"|\x1b[()][0-9A-Za-z]"  # character set
    rb"|\x1b[=>78DEHMc]"  # other two-byte controls
)
# Long enough for any sequence the renderers write; a longer one is malformed.
_MAX_PENDING = 4096


class Screen:
    def __init__(self, rows, cols):
        self.rows, self.cols = rows, cols
        self.grid = self._blank()
        self.row = self.col = 0
        self.saved = None
        self.pending = b""
        # Every distinct screen text, for checks that something never showed.
        self.history = [""]

    def _blank(self):
        return [[" "] * self.cols for _ in range(self.rows)]

    def resize(self, rows, cols):
        """A resized terminal is repainted in full by the application."""
        self.rows, self.cols = rows, cols
        self.grid = self._blank()
        self.row = self.col = 0

    def feed(self, data):
        data, self.pending = self.pending + bytes(data), b""
        index = 0
        while index < len(data):
            byte = data[index]
            if byte == 0x1B:
                match = _SEQUENCE.match(data, index)
                if match is None:
                    if len(data) - index < _MAX_PENDING:
                        self.pending = data[index:]
                        break
                    index += 1
                    continue
                self._control(match)
                index = match.end()
            elif byte == 0x0D:
                self.col, index = 0, index + 1
            elif byte == 0x0A:
                self.row, index = min(self.row + 1, self.rows - 1), index + 1
            elif byte < 0x20 or byte == 0x7F:
                index += 1
            else:
                size = 1 if byte < 0x80 else 2 if byte >> 5 == 0b110 else 3 if byte >> 4 == 0b1110 else 4
                if index + size > len(data):
                    self.pending = data[index:]
                    break
                self._put(data[index:index + size].decode("utf-8", "replace"))
                index += size
        text = self.text()
        if text != self.history[-1]:
            self.history.append(text)

    def _put(self, char):
        if unicodedata.combining(char) or char in "‍️":
            # Joins the previous character's cell.
            column = self.col - 1
            while column > 0 and self.grid[self.row][column] == "":
                column -= 1
            if column >= 0:
                self.grid[self.row][column] += char
            return
        width = 2 if unicodedata.east_asian_width(char) in "WF" else 1
        if self.col + width > self.cols:
            return
        self.grid[self.row][self.col] = char
        if width == 2:
            self.grid[self.row][self.col + 1] = ""
        self.col += width

    def _control(self, match):
        final = match.group(2)
        if final is None:
            return
        raw = match.group(1).decode()
        if raw.startswith("?"):
            if raw[1:] == "1049" and final in b"hl":
                self._alternate(final == b"h")
            return
        params = [int(p) if p.isdigit() else 0 for p in raw.split(";")] if raw else []
        first = params[0] if params else 0
        if final in b"Hf":
            row = params[0] if params else 1
            col = params[1] if len(params) > 1 else 1
            self.row = min(max(row, 1), self.rows) - 1
            self.col = min(max(col, 1), self.cols) - 1
        elif final == b"J":
            self._erase_screen(first)
        elif final == b"K":
            self._erase_line(first)
        elif final in b"ABCD":
            step = max(first, 1)
            if final == b"A":
                self.row = max(self.row - step, 0)
            elif final == b"B":
                self.row = min(self.row + step, self.rows - 1)
            elif final == b"C":
                self.col = min(self.col + step, self.cols - 1)
            else:
                self.col = max(self.col - step, 0)
        elif final == b"G":
            self.col = min(max(first, 1), self.cols) - 1

    def _alternate(self, enter):
        if enter:
            self.saved = (self.grid, self.row, self.col)
            self.grid = self._blank()
        elif self.saved is not None:
            grid, self.row, self.col = self.saved
            self.saved = None
            # The main screen keeps its size only if the terminal did not change.
            if len(grid) == self.rows and len(grid[0]) == self.cols:
                self.grid = grid
            else:
                self.grid = self._blank()

    def _erase_screen(self, mode):
        if mode in (2, 3):
            self.grid = self._blank()
            return
        rows = range(self.row + 1, self.rows) if mode == 0 else range(0, self.row)
        for row in rows:
            self.grid[row] = [" "] * self.cols
        self._erase_line(mode)

    def _erase_line(self, mode):
        line = self.grid[self.row]
        span = {0: range(self.col, self.cols), 1: range(0, self.col + 1)}.get(mode, range(self.cols))
        for column in span:
            line[column] = " "

    def lines(self):
        return ["".join(row).rstrip() for row in self.grid]

    def text(self):
        return "\n".join(self.lines())

    def shows(self, marker):
        """Whether the current screen shows `marker`: text, bytes or a pattern."""
        if isinstance(marker, re.Pattern):
            text = self.text()
            return marker.search(text.encode() if isinstance(marker.pattern, bytes) else text) is not None
        return _text(marker) in self.text()

    def ever(self, marker):
        """Whether any screen so far showed `marker`."""
        return any(_text(marker) in text for text in self.history)

    def row_of(self, marker):
        """The 1-based row of the last line showing `marker`, or None."""
        found = [index for index, line in enumerate(self.lines()) if _text(marker) in line]
        return found[-1] + 1 if found else None


def _text(marker):
    return marker.decode() if isinstance(marker, bytes) else marker
