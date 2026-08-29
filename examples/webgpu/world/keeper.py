#!/usr/bin/env python3
# The keeper: makes Docker agree with the store.
#
# People make valleys by writing a definition into OxiMem through the game
# server's /api/make; this walks those definitions every few seconds and
# starts or stops containers to match. It runs on the host and it is the only
# thing that talks to Docker — the game servers face the network, and a
# network-facing process with the Docker socket in reach is one request-
# parsing bug away from being root on the box.
#
# It is deliberately boring: no daemon library, no state file, no memory of
# its own. The store is the desired state, `docker ps` is the actual state,
# and one pass reconciles the two. Kill it at any moment and nothing is lost;
# start two by mistake and they agree with each other because they read the
# same truth.
#
#   keeper.py            one reconciling pass, then exit (for cron or by hand)
#   keeper.py --watch    a pass every ten seconds, forever (for systemd)
#
# Author: Baris AKIN

import json
import socket
import subprocess
import sys
import time

MEM = ("127.0.0.1", 6390)
IMAGE = "rill-valley:latest"
PORT_BASE = 8300          # /valley42 listens on 8342
PREFIX = "uvalley-"       # user valleys; the four house ones are valley-*
DATA = "/root/rill-valley/udata"
MEM_FOR_VALLEY = "127.0.0.1:6390"

# The twelve fields of a definition, in the order door.rill writes them.
FIELDS = ["name", "owner", "players", "time", "gravity", "pines",
          "crystals", "grass", "meteors", "match", "report", "made"]


def resp(sock_file, *args):
    """One RESP command, one parsed reply. The protocol is four line-shapes
    and a byte count; a client library would be more code than this."""
    out = b"*%d\r\n" % len(args)
    for a in args:
        b = a.encode() if isinstance(a, str) else a
        out += b"$%d\r\n%s\r\n" % (len(b), b)
    sock_file.write(out)
    sock_file.flush()
    return read_reply(sock_file)


def read_reply(f):
    line = f.readline()
    kind, rest = line[:1], line[1:].strip()
    if kind == b"+":
        return rest.decode()
    if kind == b"-":
        raise RuntimeError(rest.decode())
    if kind == b":":
        return int(rest)
    if kind == b"$":
        n = int(rest)
        if n < 0:
            return None
        body = f.read(n + 2)[:n]
        return body.decode()
    if kind == b"*":
        n = int(rest)
        if n < 0:
            return None
        return [read_reply(f) for _ in range(n)]
    raise RuntimeError("unreadable reply " + repr(line))


def wanted():
    """id -> definition, read fresh from the store. SCAN, not KEYS — the same
    lesson the valley directory learned: pre-0.43.2 KEYS missed whole types,
    and a scan is the kinder walk anyway."""
    s = socket.create_connection(MEM, timeout=5)
    f = s.makefile("rwb")
    out = {}
    cursor = "0"
    while True:
        reply = resp(f, "SCAN", cursor, "MATCH", "vdef:*", "COUNT", "100")
        cursor, keys = reply[0], reply[1]
        for k in keys:
            rec = resp(f, "GET", k)
            if not rec:
                continue
            parts = rec.split(":")
            if len(parts) != len(FIELDS):
                continue          # not ours; leave whatever it is alone
            out[int(k.split(":")[1])] = dict(zip(FIELDS, parts))
        if cursor == "0":
            break
    f.close()
    s.close()
    return out


def running():
    """id -> container name, from Docker."""
    ps = subprocess.run(
        ["docker", "ps", "--filter", "name=" + PREFIX, "--format", "{{.Names}}"],
        capture_output=True, text=True, check=True)
    out = {}
    for name in ps.stdout.split():
        tail = name[len(PREFIX):]
        if tail.isdigit():
            out[int(tail)] = name
    return out


def start(vid, d):
    port = PORT_BASE + vid
    name = PREFIX + str(vid)
    subprocess.run(["docker", "rm", "-f", name],
                   capture_output=True, check=False)
    subprocess.run(["mkdir", "-p", f"{DATA}-{vid}"], check=True)
    # Every argument below came out of the store, where door.rill wrote it —
    # and door.rill clamped every number on the way in. The keeper still owes
    # the box a last look, because the store is writable by more hands than
    # one: a definition with a shell metacharacter anywhere in it is simply
    # not started. (exec, not shell, so this is belt and braces.)
    flat = "".join(d.values())
    if any(c in flat for c in ";|&$`\\\"'<>\n"):
        print(f"  vdef:{vid} has characters no valley needs; skipped")
        return
    args = [
        "docker", "run", "-d", "--name", name,
        "--restart", "unless-stopped", "--network", "host",
        "-v", f"{DATA}-{vid}:/data",
        "--memory", "256m",
        "--log-opt", "max-size=10m", "--log-opt", "max-file=3",
        IMAGE, str(port),
        "meteors=" + ("on" if d["meteors"] == "1" else "off"),
        "mem=" + MEM_FOR_VALLEY,
        "name=" + d["name"],
        "path=/valley" + str(vid),
        "players=" + d["players"],
        "time=" + d["time"],
        "gravity=" + d["gravity"],
        "pines=" + d["pines"],
        "crystals=" + d["crystals"],
        "grass=" + d["grass"],
        "match=" + d["match"],
        "report=" + d["report"],
    ]
    subprocess.run(args, capture_output=True, check=True)
    print(f"  up   {name}  :{port}  {d['name']} ({d['owner']})")


def stop(vid, name):
    subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=True)
    print(f"  down {name}")


def reconcile():
    want = wanted()
    have = running()
    for vid in sorted(want.keys() - have.keys()):
        start(vid, want[vid])
    for vid in sorted(have.keys() - want.keys()):
        stop(vid, have[vid])


if __name__ == "__main__":
    if "--watch" in sys.argv:
        while True:
            try:
                reconcile()
            except Exception as e:
                print("keeper: " + str(e), file=sys.stderr)
            time.sleep(10)
    else:
        reconcile()
