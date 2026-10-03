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


SCENARIO = {"edit_done": False, "steer_started": False, "steer_done": False,
            "ask_done": False, "fetch_done": False, "sub_done": False,
            "bg_started": False, "bg_done": False, "bg_result_seen": False,
            "todo_write_seen": False, "todo_read_seen": False,
            "bgbash_started": False, "bgbash_result_seen": False,
            "feedback_seen": False,
            "kill_started": False, "kill_status_seen": False, "kill_requested": False,
            "kill_id": "", "killed_seen": False,
            "plan_started": False, "plan_deny_seen": False,
            "patch_done": False, "cont_done": False,
            "subcont_done": False, "subcont_sub_seen": False,
            "sub_fresh": False, "sub_sid": "",
            "resume_done": False}


def sse_response(handler, chunks, finish="stop"):
    handler.send_response(200)
    handler.send_header("Content-Type", "text/event-stream")
    handler.end_headers()
    for c in chunks:
        data = json.dumps({"choices": [{"delta": c}]})
        handler.wfile.write(f"data: {data}\n\n".encode())
    fr = json.dumps({"choices": [{"delta": {}, "finish_reason": finish}]})
    handler.wfile.write(f"data: {fr}\n\n".encode())
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

    def do_GET(self):
        html = ("<html><head><style>.x{color:red}</style></head>"
                "<body><h1>Page</h1><p>hello <b>fetch</b> &amp; bye</p>"
                "<script>bad()</script></body></html>")
        body = html.encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        msgs = body.get("messages", [])
        last_user = next((m["content"] for m in reversed(msgs) if m["role"] == "user"), "")
        has_file_block = "[file: README.md]" in last_user
        tool_msgs = [m for m in msgs if m["role"] == "tool"]

        def respond(chunks, handler=None, finish="stop"):
            handler = handler or self
            if body.get("stream") is False:
                data = json.dumps({"choices": [
                    {"message": {"role": c.get("role", "assistant"), "content": c.get("content", "")},
                     "finish_reason": finish} for c in chunks
                ]})
                raw = data.encode()
                handler.send_response(200)
                handler.send_header("Content-Type", "application/json")
                handler.send_header("Content-Length", str(len(raw)))
                handler.end_headers()
                handler.wfile.write(raw)
                return
            sse_response(handler, chunks, finish)

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
            respond(chunks)
            return
        if not SCENARIO["steer_started"] or not SCENARIO["steer_done"]:
            if last_user.endswith("start long task"):
                SCENARIO["steer_started"] = True
                time.sleep(1.5)
                chunks = tool_call_chunks("bash", json.dumps({"command": "rm -rf /tmp/hiderola-e2e-nothing"}))
                respond(chunks)
            elif not SCENARIO["steer_done"]:
                denied = any("denied by permissions" in m["content"] for m in tool_msgs)
                check("permission deny reached the model", denied)
                steered = "second" in last_user
                check("steered message reached the model", steered)
                SCENARIO["steer_done"] = True
                respond([{"role": "assistant", "content": "STEER_DENY_OK"}])
            else:
                respond([{"role": "assistant", "content": "EDIT_OK"}])
            return
        if not SCENARIO["ask_done"]:
            answered = any("User has answered" in m["content"] for m in tool_msgs)
            if answered:
                SCENARIO["ask_done"] = True
                chunks = [{"role": "assistant", "content": "ASK_OK"}]
            else:
                chunks = tool_call_chunks("question", json.dumps({
                    "questions": [{
                        "question": "Which color?",
                        "header": "color",
                        "options": [{"label": "red"}, {"label": "blue"}],
                    }]
                }))
            respond(chunks)
            return
        if not SCENARIO["fetch_done"]:
            fetched = any("hello **fetch**" in m["content"] for m in tool_msgs)
            if fetched:
                SCENARIO["fetch_done"] = True
                chunks = [{"role": "assistant", "content": "FETCH_OK"}]
            else:
                chunks = tool_call_chunks("webfetch", json.dumps({
                    "url": f"http://127.0.0.1:{PORT}/page"
                }))
            respond(chunks)
            return
        is_sub = bool(msgs) and msgs[0]["role"] == "system" and "subagent" in msgs[0]["content"]
        if is_sub:
            user_msgs = [m for m in msgs if m["role"] == "user"]
            prompt = user_msgs[-1]["content"] if user_msgs else ""
            if len(user_msgs) > 1:
                check("continued subagent kept history",
                      "deep research task" in user_msgs[0]["content"])
                SCENARIO["subcont_sub_seen"] = True
                respond([{"role": "assistant", "content": "SUB2_DONE: continued with history"}])
                return
            text = "SUB_DONE: found 3 files"
            if "count bg files" in prompt:
                text = "SUB_BG: bg task result 42"
            elif "deep research task" in prompt:
                text = "SUB1_DONE: research part 1"
            respond([{"role": "assistant", "content": text}])
            return
        if not SCENARIO["sub_done"]:
            sub_result = any("SUB_DONE" in m["content"] for m in tool_msgs)
            if sub_result:
                SCENARIO["sub_done"] = True
                chunks = [{"role": "assistant", "content": "SUBAGENT_OK"}]
            else:
                chunks = tool_call_chunks("subagent", json.dumps({
                    "description": "explore",
                    "prompt": "count files",
                }))
            respond(chunks)
            return
        if "Background task bg-1" in last_user:
            SCENARIO["bg_done"] = True
            SCENARIO["bg_result_seen"] = "SUB_BG: bg task result 42" in last_user
            respond([{"role": "assistant", "content": "BG_DONE_OK"}])
            return
        if not SCENARIO["bg_started"]:
            started = any("started in background" in m["content"] for m in tool_msgs)
            if started:
                SCENARIO["bg_started"] = True
                chunks = [{"role": "assistant", "content": "BG_SPAWNED"}]
            else:
                chunks = tool_call_chunks("subagent", json.dumps({
                    "description": "bg explore",
                    "prompt": "count bg files",
                    "background": True,
                }))
            respond(chunks)
            return
        if not SCENARIO["todo_read_seen"]:
            rendered = any("[~] step one" in m["content"] and "[ ] step two" in m["content"]
                           for m in tool_msgs)
            if not SCENARIO["todo_write_seen"]:
                if rendered:
                    check("todowrite rendered list reached the model", rendered)
                    SCENARIO["todo_write_seen"] = True
                    chunks = tool_call_chunks("todoread", json.dumps({}))
                else:
                    chunks = tool_call_chunks("todowrite", json.dumps({"todos": [
                        {"content": "step one", "status": "in_progress", "priority": "high"},
                        {"content": "step two", "status": "pending", "priority": "medium"},
                    ]}))
            else:
                read = any("[~] step one" in m["content"] for m in tool_msgs)
                check("todoread returned the list", read)
                SCENARIO["todo_read_seen"] = True
                chunks = [{"role": "assistant", "content": "TODO_OK"}]
            respond(chunks)
            return
        if not SCENARIO["bgbash_started"]:
            started = any("moved to the background" in m["content"] for m in tool_msgs)
            if started:
                SCENARIO["bgbash_started"] = True
                chunks = [{"role": "assistant", "content": "BG_BASH_SPAWNED"}]
            else:
                chunks = tool_call_chunks("bash", json.dumps({
                    "command": "echo BG_BASH_OUT",
                    "background": True,
                }))
            respond(chunks)
            return
        if last_user.startswith("Background task bg-") and "BG_BASH_OUT" in last_user:
            SCENARIO["bgbash_result_seen"] = True
            respond([{"role": "assistant", "content": "BG_BASH_OK"}])
            return
        if not SCENARIO["feedback_seen"]:
            fb = any("rejected the bash tool call" in m["content"]
                     and "do it differently" in m["content"] for m in tool_msgs)
            if fb:
                SCENARIO["feedback_seen"] = True
                chunks = [{"role": "assistant", "content": "FEEDBACK_OK"}]
            else:
                chunks = tool_call_chunks("bash", json.dumps({
                    "command": "touch feedback_probe.txt",
                }))
            respond(chunks)
            return
        last_tool = tool_msgs[-1]["content"] if tool_msgs else ""
        if not SCENARIO["kill_requested"]:
            if not SCENARIO["kill_started"]:
                if last_tool.startswith("Command moved to the background"):
                    SCENARIO["kill_started"] = True
                    SCENARIO["kill_id"] = re.search(r"\(task (bg-\d+)\)", last_tool).group(1)
                    time.sleep(1.2)
                    chunks = tool_call_chunks("task_status", json.dumps({"id": SCENARIO["kill_id"]}))
                    respond(chunks)
                else:
                    chunks = tool_call_chunks("bash", json.dumps({
                        "command": "sleep 0.7; echo LIVE_OUT_LINE; sleep 30",
                        "background": True,
                    }))
                    respond(chunks)
                return
            if not SCENARIO["kill_status_seen"]:
                live = "output so far" in last_tool and "LIVE_OUT_LINE" in last_tool
                check("live output visible via task_status", live)
                SCENARIO["kill_status_seen"] = True
                chunks = tool_call_chunks("task_kill", json.dumps({"id": SCENARIO["kill_id"]}))
                respond(chunks)
                return
            killed_msg = f"task {SCENARIO['kill_id']} killed" in last_tool
            check("task_kill reported killed", killed_msg)
            SCENARIO["kill_requested"] = True
            if "was killed" in last_user:
                SCENARIO["killed_seen"] = True
                respond([{"role": "assistant", "content": "KILL_OK"}])
            else:
                respond([{"role": "assistant", "content": "KILL_REQUESTED"}])
            return
        if SCENARIO["kill_requested"] and not SCENARIO["killed_seen"]:
            if last_user.startswith("Background task bg-") and "was killed" in last_user:
                SCENARIO["killed_seen"] = True
                respond([{"role": "assistant", "content": "KILL_OK"}])
                return
        if "make a plan" in last_user and not SCENARIO["plan_started"]:
            SCENARIO["plan_started"] = True
            chunks = tool_call_chunks("write_file", json.dumps({
                "path": "plan_probe.txt",
                "content": "should be denied",
            }))
            respond(chunks)
            return
        if SCENARIO["plan_started"] and not SCENARIO["plan_deny_seen"]:
            denied = any("plan mode is active" in m["content"] for m in tool_msgs)
            check("plan mode denied the write", denied)
            SCENARIO["plan_deny_seen"] = True
            respond([{"role": "assistant", "content": "PLAN_OK: step 1 read code, step 2 fix bug"}])
            return
        if not SCENARIO["patch_done"]:
            res = any("Success. Updated the following files" in m["content"] for m in tool_msgs)
            if res:
                check("apply_patch result reached the model", res)
                SCENARIO["patch_done"] = True
                chunks = [{"role": "assistant", "content": "PATCH_OK"}]
            else:
                patch = ("*** Begin Patch\n"
                         "*** Add File: patch_new.txt\n"
                         "+patch line one\n"
                         "*** Update File: app.txt\n"
                         "@@\n"
                         " value = 2\n"
                         "-name = demo\n"
                         "+name = patched\n"
                         "*** End Patch")
                chunks = tool_call_chunks("apply_patch", json.dumps({"patch": patch}))
            respond(chunks)
            return
        if not SCENARIO["cont_done"]:
            if "Continue from where you left off" in last_user:
                check("continue prompt reached the model", True)
                SCENARIO["cont_done"] = True
                chunks = [{"role": "assistant", "content": "CONT_OK"}]
            else:
                chunks = [{"role": "assistant", "content": "PARTIAL_ANALYSIS "}]
            respond(chunks, finish="length" if "Continue from" not in last_user else "stop")
            return
        if not SCENARIO["subcont_done"]:
            sub2 = any("SUB2_DONE" in m["content"] for m in tool_msgs)
            if sub2:
                SCENARIO["subcont_done"] = True
                chunks = [{"role": "assistant", "content": "SUBCONT_OK"}]
            elif SCENARIO["sub_sid"]:
                chunks = tool_call_chunks("subagent", json.dumps({
                    "description": "continue research",
                    "prompt": "now summarize it",
                    "session_id": SCENARIO["sub_sid"],
                }))
            elif SCENARIO["sub_fresh"]:
                last = tool_msgs[-1]["content"] if tool_msgs else ""
                mm = re.search(r"\(subagent session: (s-[0-9a-z-]+);", last)
                if mm:
                    SCENARIO["sub_sid"] = mm.group(1)
                    chunks = tool_call_chunks("subagent", json.dumps({
                        "description": "continue research",
                        "prompt": "now summarize it",
                        "session_id": SCENARIO["sub_sid"],
                    }))
                else:
                    chunks = [{"role": "assistant", "content": "SUBCONT_NO_SID"}]
            else:
                SCENARIO["sub_fresh"] = True
                chunks = tool_call_chunks("subagent", json.dumps({
                    "description": "research",
                    "prompt": "deep research task",
                }))
            respond(chunks)
            return
        if not SCENARIO["resume_done"]:
            if len(msgs) > 3 and "after resume" in last_user and any(
                    "check @README.md" in m["content"] for m in msgs if m["role"] == "user"):
                check("resumed session kept history", True)
                SCENARIO["resume_done"] = True
            chunks = [{"role": "assistant", "content": "RESUME_OK"}]
            respond(chunks)
            return
        respond([{"role": "assistant", "content": "ALL_DONE"}])


def strip(s):
    return ANSI.sub("", s)


def run_pty():
    bin_path = os.path.abspath("target/debug/hi-derola")
    env = dict(os.environ)
    cfg_dir = os.path.join(WORKDIR, ".config")
    os.makedirs(os.path.join(cfg_dir, "hi-derola"), exist_ok=True)
    env["XDG_CONFIG_HOME"] = cfg_dir
    env["HI_DEROLA_SESSIONS_DIR"] = os.path.join(WORKDIR, "sessions")
    with open(os.path.join(cfg_dir, "hi-derola", "config.toml"), "w") as f:
        f.write(
            "[provider]\n"
            'type = "openai"\n'
            'model = "mock-1"\n'
            f'base_url = "http://127.0.0.1:{PORT}"\n'
            'api_key = "test"\n'
            "stream = true\n"
            "\n[permissions]\n"
            "\n[[permissions.rules]]\n"
            'tool = "bash"\n'
            'pattern = "rm *"\n'
            'permission = "deny"\n'
        )

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
    deadline = time.time() + 150
    sent = 0
    t_sent3 = 0.0

    def type_str(s):
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
            type_str("check @README.md and fix app.txt")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 1
            time.sleep(0.3)
        elif sent == 1 and ("y/n" in plain or "run edit" in plain.lower()):
            time.sleep(0.3)
            os.write(fd, b"y")
            sent = 2
        elif sent == 2 and "EDIT_OK" in plain:
            time.sleep(0.3)
            type_str("start long task")
            time.sleep(0.3)
            os.write(fd, b"\r")
            t_sent3 = time.time()
            sent = 3
        elif sent == 3 and "thinking" in plain and time.time() - t_sent3 > 0.6:
            type_str("second")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 4
        elif sent == 4 and "STEER_DENY_OK" in plain:
            time.sleep(0.3)
            type_str("ask me")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 5
        elif sent == 5 and "Which color?" in plain:
            time.sleep(0.4)
            type_str("blue")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 6
        elif sent == 6 and "ASK_OK" in plain:
            time.sleep(0.3)
            type_str("fetch page")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 7
        elif sent == 7 and "FETCH_OK" in plain:
            time.sleep(0.3)
            type_str("spawn sub")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 8
        elif sent == 8 and "SUBAGENT_OK" in plain:
            time.sleep(0.3)
            type_str("spawn bg sub")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 9
        elif sent == 9 and "BG_DONE_OK" in plain:
            time.sleep(0.3)
            type_str("write todos")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 10
        elif sent == 10 and "TODO_OK" in plain:
            time.sleep(0.3)
            type_str("run bg bash")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 11
        elif sent == 11 and "run bash?" in plain:
            time.sleep(0.4)
            os.write(fd, b"y")
            sent = 12
        elif sent == 12 and "BG_BASH_OK" in plain:
            time.sleep(0.3)
            type_str("test feedback")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 13
        elif sent == 13 and "feedback_probe" in plain:
            time.sleep(0.4)
            os.write(fd, b"f")
            time.sleep(0.3)
            type_str("do it differently")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 14
        elif sent == 14 and "FEEDBACK_OK" in plain:
            time.sleep(0.3)
            type_str("kill bg task")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 15
        elif sent == 15 and "run bash?" in plain:
            time.sleep(0.4)
            os.write(fd, b"y")
            sent = 16
        elif sent == 16 and "KILL_OK" in plain:
            time.sleep(0.4)
            type_str("/plan")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 17
        elif sent == 17 and "plan mode on" in plain:
            time.sleep(0.4)
            type_str("make a plan")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 18
        elif sent == 18 and "PLAN_OK" in plain:
            time.sleep(0.4)
            type_str("/plan")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 19
        elif sent == 19 and "plan mode off" in plain:
            time.sleep(0.4)
            type_str("apply a patch")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 20
        elif sent == 20 and "run apply_patch" in plain:
            time.sleep(0.4)
            os.write(fd, b"y")
            sent = 21
        elif sent == 21 and "PATCH_OK" in plain:
            time.sleep(0.4)
            type_str("test continuation")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 22
        elif sent == 22 and "CONT_OK" in plain:
            time.sleep(0.4)
            type_str("spawn research")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 23
        elif sent == 23 and "SUBCONT_OK" in plain:
            time.sleep(0.4)
            type_str("/sessions")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 24
        elif sent == 24 and "saved sessions" in plain:
            time.sleep(0.4)
            type_str("/clear")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 25
        elif sent == 25 and "new session" in plain:
            time.sleep(0.4)
            type_str("/resume")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 26
        elif sent == 26 and "resumed" in plain:
            time.sleep(0.4)
            type_str("after resume")
            time.sleep(0.3)
            os.write(fd, b"\r")
            sent = 27
        elif sent == 27 and "RESUME_OK" in plain:
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
check("edit + apply_patch kept CRLF", disk == b"value = 2\r\nname = patched\r\nend\r\n")
check("queued note shown", "queued: will steer the current run" in plain)
check("steer note shown", "steer: second" in plain)
check("deny note shown", "denied by permissions" in plain)
check("steer+deny answer rendered", "STEER_DENY_OK" in plain)
check("question options rendered", "Which color?" in plain and "blue" in plain)
check("question answered", "ASK_OK" in plain)
check("webfetch markdown returned", "FETCH_OK" in plain)
check("subagent ran and returned", "SUBAGENT_OK" in plain)
check("subagent progress note", "subagent started: explore" in plain)
check("background task started note", "background task bg-1 started: bg explore" in plain)
check("background task finished note", "background task bg-1 finished: bg explore" in plain)
check("background result reached the model", SCENARIO["bg_result_seen"])
check("background answer rendered", "BG_DONE_OK" in plain)
check("todo list rendered in ui", "[~] step one" in plain)
check("todo answer rendered", "TODO_OK" in plain)
check("background bash started note", "background task bg-2 started: echo BG_BASH_OUT" in plain)
check("background bash result reached the model", SCENARIO["bgbash_result_seen"])
check("background bash answer rendered", "BG_BASH_OK" in plain)
check("rejection feedback reached the model", SCENARIO["feedback_seen"])
check("feedback answer rendered", "FEEDBACK_OK" in plain)
check("live output reached the model", SCENARIO["kill_status_seen"])
check("kill note shown", "killed" in plain)
check("killed notification reached the model", SCENARIO["killed_seen"])
check("plan mode note shown", "plan mode on" in plain)
check("plan mode denial reached the model", SCENARIO["plan_deny_seen"])
check("plan mode blocked the write", not os.path.exists(os.path.join(WORKDIR, "plan_probe.txt")))
check("plan answer rendered", "PLAN_OK" in plain)
check("apply_patch confirm shown", "run apply_patch" in plain)
check("apply_patch result rendered", "PATCH_OK" in plain)
check("apply_patch wrote new file", os.path.exists(os.path.join(WORKDIR, "patch_new.txt")))
check("continue prompt reached the model", SCENARIO["cont_done"])
check("continuation answer rendered", "CONT_OK" in plain)
check("subagent continuation kept history", SCENARIO["subcont_sub_seen"])
check("continued subagent answer rendered", "SUBCONT_OK" in plain)
check("sessions listed", "saved sessions" in plain)
check("session resumed", "resumed" in plain)
check("resumed history reached the model", SCENARIO["resume_done"])
check("resume answer rendered", "RESUME_OK" in plain)

shutil.rmtree(WORKDIR, ignore_errors=True)
with open("/tmp/hiderola-e2e-log.txt", "w") as f:
    f.write(raw)
print("WORKDIR", WORKDIR)
sys.exit(1 if failures else 0)
