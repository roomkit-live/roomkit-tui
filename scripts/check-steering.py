"""Write to the model while it works, on a real pseudo-terminal.

roomkit-chat runs the real engine against a local Chat Completions fixture.
The fixture holds the model's first answer, a tool call, open while the
person types:

- Enter (RMK-1161): the message goes to the running turn, the terminal says
  so, and the model reads it at its next step. What the turn writes until then
  goes above the message, what follows goes below it (RMK-1317);
- Ctrl+S, or Ctrl+Enter as a terminal with the kitty keyboard protocol
  reports it (RMK-1285): the turn is interrupted, and the message leaves
  once it stopped, in a turn of its own. The terminal's loop tries again
  while the host holds the room (RMK-1303, against a scripted host). On an
  empty composer, it cuts the turn for the message handed to it (RMK-1311), or
  ends its wait at the message's own turn, should the cut come first (RMK-1371,
  a real engine holding the message's delivery).
"""

import json
import re
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from terminal_pty import DEADLINE, Terminal, binary, require

STEERED = "Use the second ledger"
# The terminal says a message went to the turn as the host commits it, and its
# delivery follows (RMK-1371). Nothing on the screen says when it reached the
# turn, so a check that lets the turn go on gives it this many seconds.
DELIVERY = 0.2
CTRL_S = b"\x13"
CTRL_ENTER = b"\x1b[13;5u"


class Fixture(BaseHTTPRequestHandler):
    """Chat Completions: the first answer waits for `release`, the second
    answers. Each request is kept, for the check to read."""

    requests = []
    release = threading.Event()
    silent_until_released = False
    next_answer_thinks = False
    next_answer = threading.Event()

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
            if Fixture.silent_until_released:
                Fixture.release.wait(DEADLINE)
            self.chunk({"content": "Checking the record. "})
            Fixture.release.wait(DEADLINE)
            call = {"index": 0, "id": "lookup-1", "type": "function",
                    "function": {"name": "lookup_record", "arguments": "{\"reference\":\"42\"}"}}
            self.chunk({"tool_calls": [call]})
            self.chunk({}, "tool_calls")
        elif Fixture.next_answer_thinks and len(Fixture.requests) == 2:
            Fixture.next_answer.wait(DEADLINE)
            for blank in ("\n", "\n"):
                self.chunk({"content": blank})
                time.sleep(0.05)  # Apart in time, or the engine joins them.
            self.chunk({"content": "Going on without it."})
            self.chunk({}, "stop")
        else:
            self.chunk({"content": "Record 42 is in the second ledger."})
            self.chunk({}, "stop")
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


def said(request, text):
    """Whether the model was asked with `text` in any of its messages."""
    return any(text in json.dumps(message) for message in request["messages"])


def chat(port, directory, silent=False):
    """roomkit-chat against the fixture, its turn holding its first answer,
    before any word of it when `silent`."""
    Fixture.requests = []
    Fixture.release = threading.Event()
    Fixture.silent_until_released = silent
    Fixture.next_answer_thinks = False
    Fixture.next_answer = threading.Event()
    env = {
        "ROOMKIT_BASE_URL": f"http://127.0.0.1:{port}/v1",
        "ROOMKIT_MODEL": "fixture",
        "ROOMKIT_API_KEY": "",
    }
    args = [binary("roomkit-chat"), "--database", Path(directory) / "chat.db", "--no-animation"]
    # Tall enough to keep the whole conversation in view.
    terminal = Terminal(args, rows=60, env=env)
    terminal.wait_for(b"fixture")
    # The kitty keyboard protocol is asked for, so Ctrl+Enter is told apart.
    terminal.wait_for(b"\x1b[>1u")
    terminal.send(b"Look up record 42\r")
    terminal.wait_for(b"Working" if silent else b"Checking the record.")
    return terminal


def handed_to_the_running_turn(port, directory):
    with chat(port, directory) as terminal:
        terminal.send(STEERED.encode() + b"\r")
        terminal.wait_for("↳ sent to the running turn".encode())
        terminal.pump(DELIVERY)
        Fixture.release.set()
        terminal.wait_for(b"Record 42 is in the second ledger.")
        written, message = terminal.screen.row_of(b"Checking the record."), terminal.screen.row_of(STEERED.encode())
        require(written and message and written < message,
                f"The message (row {message}) is not below what the answer had written (row {written})")
        require(len(Fixture.requests) == 2, f"{len(Fixture.requests)} model requests, not 2")
        require(said(Fixture.requests[1], STEERED), "The turn's next step did not read the message")
        require(not terminal.screen.ever(b"An execution is active"), "The message was refused")
        terminal.send(b"\x04")
        terminal.exited()


def read_at_the_next_stop(port, directory):
    """RMK-1317: sent before the model's first word, the message stays below
    what the turn writes without it, and what follows its next stop goes below
    it, as the model reads it there."""
    with chat(port, directory, silent=True) as terminal:
        terminal.send(STEERED.encode() + b"\r")
        terminal.wait_for("↳ sent to the running turn".encode())
        terminal.pump(DELIVERY)
        Fixture.release.set()
        terminal.wait_for(b"Checking the record.")
        terminal.wait_for(b"Record 42 is in the second ledger.")
        written = terminal.screen.row_of(b"Checking the record.")
        message = terminal.screen.row_of(STEERED.encode())
        answer = terminal.screen.row_of(b"Record 42 is in the second ledger.")
        require(written and message and answer and written < message < answer,
                f"Written at row {written}, message at {message}, answer at {answer}")
        terminal.wait_for("↳ read by the turn".encode())
        require(len(Fixture.requests) == 2, f"{len(Fixture.requests)} model requests, not 2")
        require(not said(Fixture.requests[0], STEERED), "The first request knew the message")
        require(said(Fixture.requests[1], STEERED), "The turn's next step did not read the message")
        terminal.send(b"\x04")
        terminal.exited()


def blanks_of_the_next_answer_read_nothing(port, directory):
    """RMK-1317: handed once the calls ended, the model's next answer is already
    being written without the message, however many blanks open it. The stop
    after that answer reads it."""
    with chat(port, directory) as terminal:
        Fixture.next_answer_thinks = True
        Fixture.release.set()
        terminal.wait_until(lambda: len(Fixture.requests) == 2, "the model's next answer")
        terminal.send(STEERED.encode() + b"\r")
        terminal.wait_for("↳ sent to the running turn".encode())
        terminal.pump(DELIVERY)
        Fixture.next_answer.set()
        terminal.wait_for(b"Going on without it.")
        terminal.wait_for(b"Record 42 is in the second ledger.")
        written = terminal.screen.row_of(b"Going on without it.")
        message = terminal.screen.row_of(STEERED.encode())
        answer = terminal.screen.row_of(b"Record 42 is in the second ledger.")
        require(written and message and answer and written < message < answer,
                f"Written at row {written}, message at {message}, answer at {answer}")
        require(len(Fixture.requests) == 3, f"{len(Fixture.requests)} model requests, not 3")
        require(not said(Fixture.requests[1], STEERED), "The second request knew the message")
        require(said(Fixture.requests[2], STEERED), "The turn's next step did not read the message")
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


def cut_for_a_message_already_handed(port, directory):
    """RMK-1311: the handed message's line names the key. Ctrl+S on the empty
    composer cuts the turn, and a turn of its own answers the message, which is
    in the conversation once and shows once."""
    with chat(port, directory) as terminal:
        terminal.send(STEERED.encode() + b"\r")
        terminal.wait_for("sent to the running turn · Ctrl+S to cut it now".encode())
        require(not terminal.screen.shows(b"Ctrl+S send now"), "The key stayed on the row below")
        terminal.send(CTRL_S)
        terminal.wait_for(b"Execution Interrupted")
        terminal.wait_for(b"Record 42 is in the second ledger.")
        terminal.wait_for("answered in its own turn".encode())
        require(len(Fixture.requests) == 2, f"{len(Fixture.requests)} model requests, not 2")
        read = sum(STEERED == message.get("content") for message in Fixture.requests[1]["messages"])
        require(read == 1, f"The model read the message {read} times")
        require(terminal.screen.text().count(STEERED) == 1, "The message shows twice")
        require(not terminal.screen.ever(b"cut the turn"), "The instruction showed")
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


def cut_before_the_message_reached_the_turn():
    """RMK-1371: the cut comes between the message's commit and its delivery, so
    the message gets a turn of its own. The terminal has asked for an answer,
    which the host refused while it held the message. That turn ends its asking:
    the person sees one answer, however long they wait."""
    with Terminal([binary("cut-before-delivery")], rows=60) as terminal:
        terminal.wait_for(b"Start a conversation")
        terminal.send(b"Look up record 42\r")
        terminal.wait_for(b"Checking")
        terminal.send(STEERED.encode() + b"\r")
        terminal.wait_for("sent to the running turn · Ctrl+S to cut it now".encode())
        terminal.send(CTRL_S)
        terminal.wait_for(b"Its own answer")
        terminal.wait_for("answered in its own turn".encode())
        terminal.pump(1)  # Five times as long as the terminal waits to try again.
        require(not terminal.screen.ever(b"Answered twice"), "The model answered twice")
        terminal.send(b"\x04")
        terminal.exited()


server = ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
threading.Thread(target=server.serve_forever, daemon=True).start()
try:
    for check in (
        handed_to_the_running_turn,
        read_at_the_next_stop,
        blanks_of_the_next_answer_read_nothing,
        lambda port, directory: sent_now(port, directory, CTRL_S),
        lambda port, directory: sent_now(port, directory, CTRL_ENTER),
        cut_for_a_message_already_handed,
    ):
        with tempfile.TemporaryDirectory() as directory:
            check(server.server_port, directory)
        Fixture.release.set()
    tried_again_while_the_host_holds_the_room()
    cut_before_the_message_reached_the_turn()
finally:
    Fixture.release.set()
    server.shutdown()
print("Steering checks passed: Enter hands a message to the running turn, which reads it at its next "
      "step; Ctrl+S and Ctrl+Enter cut the turn and send the message once it stopped, trying again "
      "while the host holds the room; on an empty composer, for the message handed to the turn, "
      "which is answered once when the cut comes before the turn took it.")
