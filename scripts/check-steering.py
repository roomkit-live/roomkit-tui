"""Write to the model while it works (RMK-1161), on a real pseudo-terminal.

roomkit-chat runs the real engine against a local Chat Completions fixture.
The fixture holds the model's first answer, a tool call, open while the
person types: the message goes to the running turn, the terminal says so,
and the model reads it at its next step.
"""

import json
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from terminal_pty import DEADLINE, Terminal, binary, require

STEERED = "Use the second ledger"


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


server = ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
threading.Thread(target=server.serve_forever, daemon=True).start()
try:
    with tempfile.TemporaryDirectory() as directory:
        env = {
            "ROOMKIT_BASE_URL": f"http://127.0.0.1:{server.server_port}/v1",
            "ROOMKIT_MODEL": "fixture",
            "ROOMKIT_API_KEY": "",
        }
        args = [binary("roomkit-chat"), "--database", Path(directory) / "chat.db", "--no-animation"]
        with Terminal(args, env=env) as terminal:
            terminal.wait_for(b"fixture")
            terminal.send(b"Look up record 42\r")
            terminal.wait_for(b"Checking the record.")
            terminal.send(STEERED.encode() + b"\r")
            terminal.wait_for("↳ sent to the running turn".encode())
            Fixture.release.set()
            terminal.wait_for(b"Record 42 is in the second ledger.")
            require(len(Fixture.requests) == 2, f"{len(Fixture.requests)} model requests, not 2")
            require(said(Fixture.requests[1], STEERED), "The turn's next step did not read the message")
            require(not terminal.screen.ever(b"An execution is active"), "The message was refused")
            terminal.send(b"\x04")
            terminal.exited()
finally:
    Fixture.release.set()
    server.shutdown()
print("Steering checks passed: a message typed during a tool round goes to the running turn, which reads it at its next step.")
