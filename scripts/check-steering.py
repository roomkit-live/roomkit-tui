"""Write to the model while it works, on a real pseudo-terminal.

roomkit-chat runs the real engine against a local Chat Completions fixture.
The fixture holds the model's first answer, a tool call, open while the
person types:

- Enter (RMK-1161): the message goes to the running turn, the terminal says
  so, and the model reads it at its next step;
- Ctrl+S, or Ctrl+Enter as a terminal with the kitty keyboard protocol
  reports it (RMK-1285): the turn is interrupted, and the message leaves
  once it stopped, in a turn of its own. The terminal's loop tries again
  while the host holds the room (RMK-1303, against a scripted host).
"""

import json
import re
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from terminal_pty import DEADLINE, Terminal, binary, require

STEERED = "Use the second ledger"
CTRL_S = b"\x13"
CTRL_ENTER = b"\x1b[13;5u"


class Fixture(BaseHTTPRequestHandler):
    """Chat Completions: the first answer waits for `release`, the second
    answers. Each request is kept, for the check to read."""

    requests = []
    release = threading.Event()

    def log_message(self, *_):
        pass

    def chunk(self, delta, finish=None):
        choice = {"index": 0, "delta": delta, "finish_reason": finish}
        self.wfile.write(b"data: " + json.dumps({"choices": [choice]}).encode() + b"\n\n")
        self.wfile.flush()

    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        Fixture.requests.append(json.loads(body))
        first = len(Fixture.requests) == 1
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        try:
            self.answer(first)
        except (BrokenPipeError, ConnectionResetError):
            pass  # An interrupted turn stopped reading its answer.

    def answer(self, first):
        if first:
            self.chunk({"content": "Checking the record. "})
            Fixture.release.wait(DEADLINE)
            call = {"index": 0, "id": "lookup-1", "type": "function",
                    "function": {"name": "lookup_record", "arguments": "{\"reference\":\"42\"}"}}
            self.chunk({"tool_calls": [call]})
            self.chunk({}, "tool_calls")
        else:
            self.chunk({"content": "Record 42 is in the second ledger."})
            self.chunk({}, "stop")
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


def said(request, text):
    """Whether the model was asked with `text` in any of its messages."""
    return any(text in json.dumps(message) for message in request["messages"])


def chat(port, directory):
    """roomkit-chat against the fixture, its turn holding its first answer."""
    Fixture.requests = []
    Fixture.release = threading.Event()
    env = {
        "ROOMKIT_BASE_URL": f"http://127.0.0.1:{port}/v1",
        "ROOMKIT_MODEL": "fixture",
        "ROOMKIT_API_KEY": "",
    }
    args = [binary("roomkit-chat"), "--database", Path(directory) / "chat.db", "--no-animation"]
    terminal = Terminal(args, env=env)
    terminal.wait_for(b"fixture")
    # The kitty keyboard protocol is asked for, so Ctrl+Enter is told apart.
    terminal.wait_for(b"\x1b[>1u")
    terminal.send(b"Look up record 42\r")
    terminal.wait_for(b"Checking the record.")
    return terminal


def handed_to_the_running_turn(port, directory):
    with chat(port, directory) as terminal:
        terminal.send(STEERED.encode() + b"\r")
        terminal.wait_for("↳ sent to the running turn".encode())
        Fixture.release.set()
        terminal.wait_for(b"Record 42 is in the second ledger.")
        require(len(Fixture.requests) == 2, f"{len(Fixture.requests)} model requests, not 2")
        require(said(Fixture.requests[1], STEERED), "The turn's next step did not read the message")
        require(not terminal.screen.ever(b"An execution is active"), "The message was refused")
        terminal.send(b"\x04")
        terminal.exited()


def sent_now(port, directory, key):
    with chat(port, directory) as terminal:
        terminal.send(STEERED.encode() + key)
        terminal.wait_for(b"Execution Interrupted")
        terminal.wait_for(b"Record 42 is in the second ledger.")
        require(len(Fixture.requests) == 2, f"{len(Fixture.requests)} model requests, not 2")
        asked = Fixture.requests[1]
        require(said(asked, STEERED), "The message did not reach the model")
        require(not said(asked, "lookup-1"), "The interrupted turn went on to its call")
        require(not terminal.screen.ever("↳ sent to the running turn".encode()),
                "The message was handed to the turn it should have cut")
        terminal.send(b"\x04")
        terminal.exited()


def tried_again_while_the_host_holds_the_room():
    """RMK-1303, against a scripted host: it refuses the message twice with no
    run to show. The terminal's own loop sends it again, and the draft stays in
    the composer until the host takes it."""
    with Terminal([binary("busy-host")]) as terminal:
        terminal.wait_for(b"Enter send")
        terminal.send(STEERED.encode() + CTRL_S)
        terminal.wait_for(b"Sends once the work stops")
        require(terminal.screen.shows(STEERED.encode()), "The draft left the composer")
        terminal.wait_for(re.compile(rb"Taken after 2 refusals, \d+ ms apart"))
        waited = int(re.search(r"refusals, (\d+) ms", terminal.screen.text()).group(1))
        require(waited >= 380, f"Two retries within {waited} ms, sooner than 200 ms apart")
        terminal.wait_until(lambda: not terminal.screen.shows(STEERED.encode()),
                            "the composer emptied once the host took the draft")
        terminal.send(b"\x04")
        terminal.exited()


server = ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
threading.Thread(target=server.serve_forever, daemon=True).start()
try:
    for check in (
        handed_to_the_running_turn,
        lambda port, directory: sent_now(port, directory, CTRL_S),
        lambda port, directory: sent_now(port, directory, CTRL_ENTER),
    ):
        with tempfile.TemporaryDirectory() as directory:
            check(server.server_port, directory)
        Fixture.release.set()
    tried_again_while_the_host_holds_the_room()
finally:
    Fixture.release.set()
    server.shutdown()
print("Steering checks passed: Enter hands a message to the running turn, which reads it at its next "
      "step; Ctrl+S and Ctrl+Enter cut the turn and send the message once it stopped, trying again "
      "while the host holds the room.")
