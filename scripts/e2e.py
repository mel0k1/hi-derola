import json
import os
import pty
import re
import select
import shutil
import signal
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ANSI = re.compile(r"\x1b\[[0-9;?]*[a-zA-Z]|\x1b\][^\x07]*\x07|\x1b[()][B0]")

WORKDIR = tempfile.mkdtemp(prefix="hiderola-e2e-")
PORT = 18971
failures = []


def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond:
        failures.append(name)


def make_crlf_file():
    p = os.path.join(WORKDIR, "app.txt")
    with open(p, "wb") as f:
        f.write(b"value = 1\r\nname = demo\r\nend\r\n")
    return p


SCENARIO = {"edit_done": False}


def sse_response(handler, chunks):
    handler.send_response(200)
    handler.send_header("Content-Type", "text/event-stream")
    handler.end_headers()
    for c in chunks:
        data = json.dumps({"choices": [{"delta": c}]})
        handler.wfile.write(f"data: {data}\n\n".encode())
    handler.wfile.write(b"data: [DONE]\n\n")
    handler.wfile.flush()


def tool_call_chunks(name, args):
    return [
        {"role": "assistant", "content": ""},
        {"tool_calls": [{"index": 0, "id": "call_1", "type": "function",
                         "function": {"name": name, "arguments": ""}}]},
        {"tool_calls": [{"index": 0, "function": {"arguments": args}}]},
        {},
    ]


class Mock(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        msgs = body.get("messages", [])
        self.json_ok = False
        last_user = next((m["content"] for m in reversed(msgs) if m["role"] == "user"), "")
        has_file_block = "[file: README.md]" in last_user
        tool_msgs = [m for m in msgs if m["role"] == "tool"]

        if not SCENARIO["edit_done"]:
            check("mention block reached the model", has_file_block)
            if len(tool_msgs) == 0:
                chunks = tool_call_chunks("edit", json.dumps({
                    "path": "app.txt",
                    "old_str": "value = 1\nname = demo",
                    "new_str": "value = 2\nname = demo",
                }))
            else:
                check("tolerant edit applied", tool_msgs[-1]["content"].startswith("edited app.txt"))
                SCENARIO["edit_done"] = True
                chunks = [{"role": "assistant", "content": "EDIT_OK"}]
            sse_response(self, chunks)
        else:
            sse_response(self, [{"role": "assistant", "content": "EDIT_OK"}])


def strip(s):
    return ANSI.sub("", s)


def run_pty():
    bin_path = os.path.abspath("target/debug/hi-derola")
    env = dict(os.environ)
    cfg_dir = os.path.join(WORKDIR, ".config")
    os.makedirs(os.path.join(cfg_dir, "hi-derola"), exist_ok=True)
    env["XDG_CONFIG_HOME"] = cfg_dir
    with open(os.path.join(cfg_dir, "hi-derola", "config.toml"), "w") as f:
        f.write(
            "[provider]\n"
            'type = "openai"\n'
            'model = "mock-1"\n'
            f'base_url = "http://127.0.0.1:{PORT}"\n'
            'api_key = "test"\n'
            "stream = true\n"
        )
    os.makedirs(os.path.join(cfg_dir, "hi-derola"), exist_ok=True)

    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(WORKDIR)
        os.environ.update(env)
        os.execv(bin_path, ["hi-derola"])
        os._exit(1)

    import fcntl
    import termios
    import struct
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))

    out = ""
    deadline = time.time() + 60
    steps = ["check @README.md and fix app.txt", "y"]
    sent = 0

    def type_str(fd, s):
        for ch in s:
            os.write(fd, ch.encode())
            time.sleep(0.015)

    while time.time() < deadline:
        r, _, _ = select.select([fd], [], [], 0.3)
        if r:
            try:
                data = os.read(fd, 65536)
            except OSError:
                break
            if not data:
                break
            out += data.decode("utf-8", "replace")
        plain = strip(out)
        if sent == 0 and "type a message" in plain:
            time.sleep(0.5)
            type_str(fd, steps[0])
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 1
            time.sleep(0.3)
        elif sent == 1 and ("y/n" in plain or "run edit" in plain.lower()):
            time.sleep(0.3)
            os.write(fd, steps[1].encode())
            sent = 2
        elif sent == 2 and "EDIT_OK" in plain:
            break
    try:
        os.kill(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    os.waitpid(pid, 0)
    os.close(fd)
    return strip(out), out


with open(os.path.join(WORKDIR, "README.md"), "w") as f:
    f.write("readme content for mention\n")
make_crlf_file()

srv = ThreadingHTTPServer(("127.0.0.1", PORT), Mock)
threading.Thread(target=srv.serve_forever, daemon=True).start()

plain, raw = run_pty()
srv.shutdown()

check("final answer rendered", "EDIT_OK" in plain)
check("confirm box appeared", "y/n" in plain or "run edit" in plain.lower())
check("mention note", "attached" in plain)
with open(os.path.join(WORKDIR, "app.txt"), "rb") as f:
    disk = f.read()
check("edit written with CRLF preserved", disk == b"value = 2\r\nname = demo\r\nend\r\n")

shutil.rmtree(WORKDIR, ignore_errors=True)
print("WORKDIR", WORKDIR)
sys.exit(1 if failures else 0)
