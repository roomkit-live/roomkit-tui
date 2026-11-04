"""A model's reasoning on a real pseudo-terminal (RMK-1325).

roomkit-chat runs the real engine against a local Chat Completions fixture
that writes its reasoning in `reasoning_content`, as a server with a reasoning
parser does, and holds its answer. The terminal shows the reasoning as it comes
and folds it once the answer starts; F2 or a click opens it. Nothing stores it:
the same conversation opened again does not show it.
"""

import json
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from terminal_pty import DEADLINE, Terminal, binary, require

REASONING = b"Weighing the two ledgers."
ANSWER = b"Record 42 is in the second ledger."
F2 = b"\x1bOQ"


class Fixture(BaseHTTPRequestHandler):
    """Chat Completions: reasoning first, its answer once `release` is set."""

    release = threading.Event()

    def log_message(self, *_):
        pass

    def chunk(self, delta, finish=None):
        choice = {"index": 0, "delta": delta, "finish_reason": finish}
        self.wfile.write(b"data: " + json.dumps({"choices": [choice]}).encode() + b"\n\n")
        self.wfile.flush()

    def do_POST(self):
        self.rfile.read(int(self.headers["Content-Length"]))
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        self.chunk({"reasoning_content": REASONING.decode()})
        Fixture.release.wait(DEADLINE)
        self.chunk({"content": ANSWER.decode()})
        self.chunk({}, "stop")
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


def chat(port, directory):
    env = {
        "ROOMKIT_BASE_URL": f"http://127.0.0.1:{port}/v1",
        "ROOMKIT_MODEL": "fixture",
        "ROOMKIT_API_KEY": "",
    }
    args = [binary("roomkit-chat"), "--database", Path(directory) / "chat.db", "--no-animation"]
    terminal = Terminal(args, rows=40, env=env)
    terminal.wait_for(b"fixture")
    return terminal


def click(terminal, row):
    terminal.mouse(0, 10, row)
    terminal.mouse(0, 10, row, release=True)


server = ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
threading.Thread(target=server.serve_forever, daemon=True).start()
try:
    with tempfile.TemporaryDirectory() as directory:
        with chat(server.server_port, directory) as terminal:
            terminal.send(b"Look up record 42\r")
            terminal.wait_for("∴ Thinking".encode())
            terminal.wait_for(REASONING)
            Fixture.release.set()
            terminal.wait_for(ANSWER)
            terminal.wait_for("∴ Thought for".encode())
            terminal.wait_until(lambda: not terminal.screen.shows(REASONING), "the reasoning folded")
            written = terminal.screen.row_of("∴ Thought for".encode())
            answer = terminal.screen.row_of(ANSWER)
            require(written and answer and written < answer,
                    f"The reasoning (row {written}) is not above the answer (row {answer})")
            terminal.send(F2)
            terminal.wait_for(REASONING)
            terminal.send(F2)
            terminal.wait_until(lambda: not terminal.screen.shows(REASONING), "the reasoning closed")
            click(terminal, written)
            terminal.wait_for(REASONING)
            click(terminal, written)
            terminal.wait_until(lambda: not terminal.screen.shows(REASONING), "the click closed it")
            terminal.send(b"\x04")
            terminal.exited()
        with chat(server.server_port, directory) as terminal:
            terminal.wait_for(ANSWER)
            require(not terminal.screen.ever(REASONING), "The reasoning was stored")
            require(not terminal.screen.ever("∴".encode()), "A reasoning block came back")
            terminal.send(b"\x04")
            terminal.exited()
finally:
    Fixture.release.set()
    server.shutdown()
print("Thinking checks passed: the reasoning shows as it comes, folds once the answer starts, opens "
      "with F2 or a click, and the same conversation opened again does not hold it.")
