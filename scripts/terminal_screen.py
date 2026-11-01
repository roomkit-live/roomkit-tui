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
        # Reverse video per cell: the monochrome form of a highlight.
        self.inverse = self._flags()
        self.reverse = False
        self.row = self.col = 0
        self.saved = None
        self.pending = b""
        # Every distinct screen text, for checks that something never showed.
        self.history = [""]

    def _blank(self):
        return [[" "] * self.cols for _ in range(self.rows)]

    def _flags(self):
        return [[False] * self.cols for _ in range(self.rows)]

    def resize(self, rows, cols):
        """A resized terminal is repainted in full by the application."""
        self.rows, self.cols = rows, cols
        self.grid = self._blank()
        self.inverse = self._flags()
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
        self.inverse[self.row][self.col] = self.reverse
        if width == 2:
            self.grid[self.row][self.col + 1] = ""
            self.inverse[self.row][self.col + 1] = self.reverse
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
        handler = _CONTROLS.get(final)
        if handler is not None:
            handler(self, params, raw)

    def _position(self, params):
        row = params[0] if params else 1
        col = params[1] if len(params) > 1 else 1
        self.row = min(max(row, 1), self.rows) - 1
        self.col = min(max(col, 1), self.cols) - 1

    def _move(self, final, params):
        step = max(params[0] if params else 0, 1)
        if final == b"A":
            self.row = max(self.row - step, 0)
        elif final == b"B":
            self.row = min(self.row + step, self.rows - 1)
        elif final == b"C":
            self.col = min(self.col + step, self.cols - 1)
        else:
            self.col = max(self.col - step, 0)

    def _column(self, params):
        self.col = min(max(params[0] if params else 0, 1), self.cols) - 1

    def _attributes(self, params):
        """Apply SGR parameters. An extended color (38, 48 or 58) carries its
        operands, never attributes; the colon form keeps them in one parameter."""
        index = 0
        while index < len(params):
            param, index = params[index], index + 1
            if ":" in param:
                continue
            code = int(param) if param.isdigit() else 0
            if code in (38, 48, 58):
                mode = params[index] if index < len(params) else ""
                index += {"5": 2, "2": 4}.get(mode, 1)
            elif code in (0, 27):
                self.reverse = False
            elif code == 7:
                self.reverse = True

    def _alternate(self, enter):
        if enter:
            self.saved = (self.grid, self.inverse, self.row, self.col)
            self.grid, self.inverse = self._blank(), self._flags()
        elif self.saved is not None:
            grid, inverse, self.row, self.col = self.saved
            self.saved = None
            # The main screen keeps its size only if the terminal did not change.
            if len(grid) == self.rows and len(grid[0]) == self.cols:
                self.grid, self.inverse = grid, inverse
            else:
                self.grid, self.inverse = self._blank(), self._flags()

    def _erase_screen(self, mode):
        if mode in (2, 3):
            self.grid, self.inverse = self._blank(), self._flags()
            return
        rows = range(self.row + 1, self.rows) if mode == 0 else range(0, self.row)
        for row in rows:
            self.grid[row] = [" "] * self.cols
            self.inverse[row] = [False] * self.cols
        self._erase_line(mode)

    def _erase_line(self, mode):
        line = self.grid[self.row]
        # A cursor past the last column (after writing it) erases up to that column.
        span = {0: range(self.col, self.cols), 1: range(0, min(self.col + 1, self.cols))}.get(mode, range(self.cols))
        for column in span:
            line[column] = " "
            self.inverse[self.row][column] = False

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

    def reversed_text(self, row):
        """The characters of 1-based `row` shown in reverse video."""
        cells = zip(self.grid[row - 1], self.inverse[row - 1])
        return "".join(char for char, reverse in cells if reverse)

    def row_of(self, marker):
        """The 1-based row of the last line showing `marker`, or None."""
        found = [index for index, line in enumerate(self.lines()) if _text(marker) in line]
        return found[-1] + 1 if found else None


def _text(marker):
    return marker.decode() if isinstance(marker, bytes) else marker


def _first(params):
    return params[0] if params else 0


# The CSI controls the renderers write, by final byte: (screen, params, raw).
_CONTROLS = {
    b"H": lambda screen, params, raw: screen._position(params),
    b"f": lambda screen, params, raw: screen._position(params),
    b"J": lambda screen, params, raw: screen._erase_screen(_first(params)),
    b"K": lambda screen, params, raw: screen._erase_line(_first(params)),
    b"G": lambda screen, params, raw: screen._column(params),
    b"m": lambda screen, params, raw: screen._attributes(raw.split(";")),
    **{final: (lambda final: lambda screen, params, raw: screen._move(final, params))(final)
       for final in (b"A", b"B", b"C", b"D")},
}


def _self_check():
    """Check the attribute parsing the selection checks rely on."""
    cases = [
        (b"\x1b[7mA", "A"),
        (b"\x1b[1;7mA\x1b[27mB", "A"),
        (b"\x1b[7m\x1b[mA", ""),
        (b"\x1b[7m\x1b[0mA", ""),
        # Extended colors carry operands, never attributes.
        (b"\x1b[38;5;7mA", ""),
        (b"\x1b[38;2;7;7;7mA", ""),
        (b"\x1b[7m\x1b[48;2;0;10;20mA", "A"),
        (b"\x1b[7m\x1b[38:5:1mA", "A"),
        (b"\x1b[38;5;7;7mA", "A"),
        (b"\x1b[38;2;0;0;0;7mA", "A"),
    ]
    for sequence, expected in cases:
        screen = Screen(1, 10)
        screen.feed(sequence)
        if screen.reversed_text(1) != expected:
            raise SystemExit(f"Check failed: {sequence!r} shows {screen.reversed_text(1)!r} reversed")
    # Cursor and erase controls place and clear text where a terminal would.
    layouts = [
        ((3, 10), b"\x1b[2;3fab\x1b[Ax\x1b[1;1Hc\x1b[2Bd\x1b[5Ge\x1b[2Cg\x1b[3Df",
         ["c   x", "  ab", " d  ef g"]),
        ((2, 6), b"abcdef\x1b[1;3H\x1b[K", ["ab", ""]),
        ((2, 6), b"abcdef\x1b[1;3H\x1b[1K", ["   def", ""]),
        ((2, 6), b"abcdef\x1b[2K", ["", ""]),
        ((1, 3), b"abc\x1b[1K", [""]),
        ((2, 4), b"abcd\x1b[2;1Hefgh\x1b[1;2H\x1b[J", ["a", ""]),
        ((2, 4), b"abcd\x1b[2;1Hefgh\x1b[2;2H\x1b[1J", ["", "  gh"]),
        ((2, 4), b"abcd\x1b[2J", ["", ""]),
    ]
    for (rows, cols), sequence, expected in layouts:
        screen = Screen(rows, cols)
        screen.feed(sequence)
        if screen.lines() != expected:
            raise SystemExit(f"Check failed: {sequence!r} shows {screen.lines()!r}")
    print("Screen model checks passed: reverse video, extended colors, cursor and erase controls.")


if __name__ == "__main__":
    _self_check()
