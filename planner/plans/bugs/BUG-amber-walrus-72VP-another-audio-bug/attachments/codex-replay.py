"""Replays Video Studio's Codex handshake against the real `codex app-server` in a disposable
folder, accepts every approval, logs each message with its time, and prints whether each command
and file change Codex started was reported finished (`item/completed`). BUG-amber-walrus-72VP.

Usage:
  python3 -I codex-replay.py <empty-folder> "<prompt>" <log.jsonl> [seconds]

Environment:
  REPLAY_MODEL=gpt-6.1-sol          model for the thread (default: Codex's own default)
  REPLAY_MCP_OFF=computer-use,node_repl
                                    MCP servers to turn off, as the app does for the ones it finds
                                    in ~/.codex/config.toml (only ones defined there: Codex
                                    rejects the thread for any other)
  CODEX_HOME=<dir>                  Codex's own setting: run with another config folder (copy
                                    auth.json into it to stay signed in)

The thread is started as the app starts it: approvalPolicy untrusted, the workspace-write
sandbox with only the folder writable, /tmp and $TMPDIR excluded, no network.
"""
import json, os, select, subprocess, sys, threading, time

ws = os.path.abspath(sys.argv[1])
prompt = sys.argv[2]
log = open(sys.argv[3], "w")
limit = float(sys.argv[4]) if len(sys.argv) > 4 else 240
t0 = time.time()

p = subprocess.Popen(["codex", "app-server"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=subprocess.DEVNULL, cwd=ws, text=True, bufsize=1)
lock = threading.Lock()


def rec(direction, msg):
    log.write(json.dumps({"t": round(time.time() - t0, 3), "dir": direction, "msg": msg}) + "\n")
    log.flush()


def send(msg):
    with lock:
        rec("out", msg)
        p.stdin.write(json.dumps(msg) + "\n")
        p.stdin.flush()


sandbox = {"excludeSlashTmp": True, "excludeTmpdirEnvVar": True, "networkAccess": False,
           "type": "workspaceWrite", "writableRoots": [ws]}
mcp_off = {n: {"enabled": False} for n in os.environ.get("REPLAY_MCP_OFF", "").split(",") if n}
params = {
    "approvalPolicy": "untrusted", "approvalsReviewer": "user", "cwd": ws,
    "config": {"mcp_servers": mcp_off, "web_search": "disabled",
               "sandbox_workspace_write": {"exclude_slash_tmp": True, "exclude_tmpdir_env_var": True,
                                           "network_access": False, "writable_roots": [ws]}},
}
if os.environ.get("REPLAY_MODEL"):
    params["model"] = os.environ["REPLAY_MODEL"]

send({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "video_studio", "title": "replay", "version": "0.1.0"}}})
send({"method": "initialized"})
send({"id": 3, "method": "thread/start", "params": params})

items = {}
done = False
while time.time() - t0 < limit and not done:
    ready, _, _ = select.select([p.stdout], [], [], 1.0)
    if not ready:
        continue
    line = p.stdout.readline()
    if not line:
        break
    try:
        m = json.loads(line)
    except ValueError:
        continue
    rec("in", m)
    meth = m.get("method")
    if not meth and "error" in m:
        print(f"request {m.get('id')} failed: {m['error'].get('message')}")
        break
    if m.get("id") == 3 and "result" in m:
        r = m["result"]
        print("codex", r["thread"].get("cliVersion"), "model", r.get("model"), "service tier", r.get("serviceTier"))
        send({"id": 4, "method": "turn/start", "params": {
            "threadId": r["thread"]["id"], "cwd": ws, "approvalPolicy": "untrusted", "approvalsReviewer": "user",
            "sandboxPolicy": sandbox, "input": [{"type": "text", "text": prompt}]}})
    elif meth and "id" in m:
        # a server request: accept approvals, decline anything else
        decision = "accept" if meth.endswith("requestApproval") else "decline"
        send({"id": m["id"], "result": {"decision": decision}})
    elif meth == "mcpServer/startupStatus/updated" and m["params"].get("status") == "ready":
        print("MCP server ready:", m["params"].get("name"))
    elif meth in ("item/started", "item/completed"):
        it = m["params"]["item"]
        if it.get("type") in ("fileChange", "commandExecution"):
            what = it.get("command") or ", ".join(os.path.relpath(c["path"], ws) for c in it.get("changes", []))
            items.setdefault(it["id"], [it["type"], what[:60], None, None])
            slot = 2 if meth == "item/started" else 3
            items[it["id"]][slot] = (round(time.time() - t0, 2), it.get("status"))
    elif meth == "turn/completed":
        done = True

p.terminate()
print("turn completed:", done, "after", round(time.time() - t0, 1), "s")
for k, (kind, what, started, completed) in items.items():
    print(f"{kind:16} {what:60} started {started}  completed {completed or 'NEVER'}")
