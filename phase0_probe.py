#!/usr/bin/env python3
"""Phase 0 probe: prove a Claude Code session can be created/forked by file writes alone.

Produces two sessions in the current project dir:
  A) a fully synthetic session (built from scratch)
  B) a fork of an existing real session, truncated at a chosen message

Run, then verify with `claude --resume` that both appear and open.
"""
import json, os, sys, uuid
from datetime import datetime, timezone

HOME = os.path.expanduser("~")
PROJDIR = os.path.join(HOME, ".claude", "projects", "-home-amitsinghtanwar")
CWD = "/home/amitsinghtanwar"
VERSION = "2.1.191"

def now_iso():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z"

def new_uuid():
    return str(uuid.uuid4())

def write_session(sid, records):
    path = os.path.join(PROJDIR, f"{sid}.jsonl")
    with open(path, "w") as f:
        for r in records:
            f.write(json.dumps(r) + "\n")
    return path

# ---------------------------------------------------------------- Test A
def build_synthetic():
    sid = new_uuid()
    u1 = new_uuid(); a1 = new_uuid()
    recs = [
        {"type": "mode", "mode": "normal", "sessionId": sid},
        {"type": "permission-mode", "permissionMode": "default", "sessionId": sid},
        {"parentUuid": None, "isSidechain": False, "promptId": new_uuid(),
         "type": "user",
         "message": {"role": "user", "content": "SILY-SYNTHETIC-TEST: this session was created entirely by writing a file, no Claude prompt."},
         "uuid": u1, "timestamp": now_iso(), "permissionMode": "default",
         "origin": {"kind": "human"}, "promptSource": "typed", "userType": "external",
         "entrypoint": "cli", "cwd": CWD, "sessionId": sid, "version": VERSION, "gitBranch": "HEAD"},
        {"parentUuid": u1, "isSidechain": False, "requestId": "req_synthetic_0001",
         "type": "assistant",
         "message": {"model": "claude-opus-4-8", "id": "msg_synthetic_0001", "type": "message",
                     "role": "assistant",
                     "content": [{"type": "text", "text": "This assistant turn was also hand-written into the .jsonl. If you can read this in --resume, sily's premise holds."}],
                     "stop_reason": "end_turn", "stop_sequence": None,
                     "usage": {"input_tokens": 1, "output_tokens": 1}},
         "uuid": a1, "timestamp": now_iso(), "userType": "external", "entrypoint": "cli",
         "cwd": CWD, "sessionId": sid, "version": VERSION, "gitBranch": "HEAD"},
    ]
    path = write_session(sid, recs)
    return sid, path

# ---------------------------------------------------------------- Test B
def fork_existing(src_basename, cut_after_n_msgs=2):
    """Fork a real session: keep header records + first N user/assistant messages,
    rewrite sessionId throughout, write under a new id."""
    src = os.path.join(PROJDIR, src_basename)
    new_sid = new_uuid()
    out = []
    kept_msgs = 0
    for line in open(src):
        line = line.strip()
        if not line:
            continue
        o = json.loads(line)
        t = o.get("type")
        if t in ("mode", "permission-mode"):
            o["sessionId"] = new_sid
            out.append(o)
            continue
        if t in ("user", "assistant"):
            if kept_msgs >= cut_after_n_msgs:
                break
            if "sessionId" in o:
                o["sessionId"] = new_sid
            out.append(o)
            kept_msgs += 1
    path = write_session(new_sid, out)
    return new_sid, path, kept_msgs

if __name__ == "__main__":
    print("Project dir:", PROJDIR)
    sid_a, path_a = build_synthetic()
    print(f"\n[A] Synthetic session: {sid_a}\n    -> {path_a}")

    src = "083fe422-b067-4462-946b-1d60d5d9ded7.jsonl"
    if not os.path.exists(os.path.join(PROJDIR, src)):
        for f in sorted(os.listdir(PROJDIR)):
            if f.endswith(".jsonl"):
                src = f; break
    sid_b, path_b, n = fork_existing(src, cut_after_n_msgs=2)
    print(f"\n[B] Forked from {src}, kept {n} msgs: {sid_b}\n    -> {path_b}")

    print("\nVERIFY: run  claude --resume  and look for:")
    print(f"  - the SILY-SYNTHETIC-TEST session  ({sid_a[:8]})")
    print(f"  - the forked session               ({sid_b[:8]})")
