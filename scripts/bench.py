#!/usr/bin/env python3
"""Measure source vs skeleton size for one or more repos.

Usage:
    cargo build --release
    python3 scripts/bench.py /path/to/repo [...more repos]

Requires `tiktoken` (pip install tiktoken) for token counts; falls back to a
bytes/4 estimate without it.
"""
import json
import os
import queue
import subprocess
import sys
import threading

BIN = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "target", "release", "semantic_skeletonizer",
)

try:
    import tiktoken
    ENC = tiktoken.get_encoding("o200k_base")
    def tokens(text: str) -> int:
        return len(ENC.encode(text, disallowed_special=()))
    TOKEN_NOTE = "tiktoken o200k_base"
except ImportError:
    def tokens(text: str) -> int:
        return len(text.encode()) // 4
    TOKEN_NOTE = "estimated at 4 bytes/token (install tiktoken for exact counts)"


def bench(root: str):
    proc = subprocess.Popen(
        [BIN, "--root", root],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL, text=True,
    )
    q = queue.Queue()
    threading.Thread(target=lambda: [q.put(l) for l in proc.stdout], daemon=True).start()

    def rpc(rid, method, params):
        proc.stdin.write(json.dumps(
            {"jsonrpc": "2.0", "id": rid, "method": method, "params": params}) + "\n")
        proc.stdin.flush()
        while True:
            m = json.loads(q.get(timeout=300))
            if m.get("id") == rid:
                return m

    rpc(1, "initialize", {"protocolVersion": "2025-06-18"})
    res = rpc(2, "resources/read", {"uri": "skeleton://project/global"})
    graph = json.loads(res["result"]["contents"][0]["text"])
    proc.terminate()

    src_bytes = skel_bytes = src_tok = skel_tok = 0
    for key, skel in graph.items():
        try:
            src = open(os.path.join(root, key), encoding="utf-8", errors="replace").read()
        except OSError:
            continue
        sk = json.dumps(skel)
        src_bytes += len(src.encode())
        skel_bytes += len(sk.encode())
        src_tok += tokens(src)
        skel_tok += tokens(sk)
    return len(graph), src_bytes, skel_bytes, src_tok, skel_tok


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    if not os.path.exists(BIN):
        sys.exit(f"release binary not found at {BIN}; run: cargo build --release")
    print(f"tokens: {TOKEN_NOTE}\n")
    print(f"{'repo':20} {'files':>6} {'src bytes':>12} {'skel bytes':>12} "
          f"{'src tokens':>12} {'skel tokens':>12} {'token savings':>14}")
    for repo in sys.argv[1:]:
        n, sb, kb, st, kt = bench(repo)
        savings = 100 * (1 - kt / st) if st else 0.0
        print(f"{os.path.basename(os.path.normpath(repo)):20} {n:>6} {sb:>12,} {kb:>12,} "
              f"{st:>12,} {kt:>12,} {savings:>13.1f}%")


if __name__ == "__main__":
    main()
