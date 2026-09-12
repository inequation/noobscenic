#!/usr/bin/env python3
"""Log anything that connects, on whatever ports you name.

noobscenic listens where we *told* the robot to go. This listens where it might be
going instead — port 80 and 443 for a DNS-override deployment, or any port you
suspect. It answers nothing useful; the point is the log line.

For a TLS connection it parses the **SNI** out of the ClientHello, which names the host
the client thinks it is talking to. That is the single most useful fact when a device
will not say what it is doing: if the robot is still reaching for
`mobile.proscenic.cn`, this prints that hostname.

    sudo ./catchall.py --ports 80,443
    sudo ./catchall.py --ports 80,443,8080,8081 --trace catch.jsonl

Ports below 1024 need root. Pair it with a DNS override pointing the vendor hostname
at this machine; see ../doc/reverse-engineering/REPORT.md section 6.
"""

from __future__ import annotations

import argparse
import json
import socket
import sys
import threading
import time


def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime()) + ".%03dZ" % (
        int(time.time() * 1000) % 1000
    )


def parse_sni(data: bytes) -> str | None:
    """Pull the server_name out of a TLS ClientHello, or None if it is not one."""
    try:
        if len(data) < 45 or data[0] != 0x16:  # not a TLS handshake record
            return None
        # record: type(1) version(2) length(2) | handshake: type(1) length(3) version(2)
        # random(32) then the variable-length fields.
        pos = 5 + 4 + 2 + 32
        pos += 1 + data[pos]  # session id
        pos += 2 + int.from_bytes(data[pos:pos + 2], "big")  # cipher suites
        pos += 1 + data[pos]  # compression methods
        pos += 2  # extensions length
        while pos + 4 <= len(data):
            ext_type = int.from_bytes(data[pos:pos + 2], "big")
            ext_len = int.from_bytes(data[pos + 2:pos + 4], "big")
            body = data[pos + 4:pos + 4 + ext_len]
            if ext_type == 0x0000 and len(body) >= 5:  # server_name
                name_len = int.from_bytes(body[3:5], "big")
                return body[5:5 + name_len].decode("utf-8", "replace")
            pos += 4 + ext_len
    except (IndexError, ValueError):
        return None
    return None


def describe(data: bytes) -> str:
    sni = parse_sni(data)
    if sni:
        return "TLS ClientHello, SNI=%s" % sni
    if data[:1] == b"\x16":
        return "TLS ClientHello (no SNI)"
    text = data.decode("utf-8", "replace")
    first = text.split("\r\n", 1)[0][:200]
    if any(text.startswith(v) for v in ("GET ", "POST ", "PUT ", "HEAD ", "OPTIONS ")):
        host = ""
        for line in text.split("\r\n"):
            if line.lower().startswith("host:"):
                host = "  Host: " + line[5:].strip()
                break
        return "HTTP %s%s" % (first, host)
    return "raw %r" % data[:200]


def serve(port: int, args, lock: threading.Lock) -> None:
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        listener.bind((args.bind, port))
    except OSError as exc:
        with lock:
            print("cannot bind %d: %s" % (port, exc), file=sys.stderr, flush=True)
        return
    listener.listen(16)
    with lock:
        print("listening on %s:%d" % (args.bind, port), flush=True)

    while True:
        try:
            conn, addr = listener.accept()
        except OSError:
            return
        conn.settimeout(args.read_timeout)
        try:
            data = conn.recv(4096)
        except OSError:
            data = b""
        detail = describe(data) if data else "connected, sent nothing"
        with lock:
            print("%s  %s:%d -> :%d  %s" % (now_iso(), addr[0], addr[1], port, detail), flush=True)
            if args.trace:
                with open(args.trace, "a", encoding="utf-8") as handle:
                    handle.write(json.dumps({
                        "ts": now_iso(), "peer": "%s:%d" % addr, "port": port,
                        "len": len(data), "detail": detail,
                        "head": data[:512].decode("utf-8", "replace"),
                    }, ensure_ascii=False) + "\n")
        if args.reply_http and data[:1] != b"\x16":
            body = b'{"code":0,"message":"ok","data":{}}'
            try:
                conn.sendall(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n"
                    b"Content-Length: %d\r\n\r\n%s" % (len(body), body)
                )
            except OSError:
                pass
        conn.close()


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Log whatever connects.")
    parser.add_argument("--ports", default="80,443",
                        help="comma-separated TCP ports (default %(default)s)")
    parser.add_argument("--bind", default="0.0.0.0")
    parser.add_argument("--trace", metavar="FILE", help="append a JSONL record per connection")
    parser.add_argument("--read-timeout", type=float, default=3.0)
    parser.add_argument("--reply-http", action="store_true",
                        help="answer non-TLS connections with the channel-A ok envelope")
    args = parser.parse_args(argv)

    ports = [int(p) for p in args.ports.split(",") if p.strip()]
    lock = threading.Lock()
    threads = [threading.Thread(target=serve, args=(p, args, lock), daemon=True) for p in ports]
    for thread in threads:
        thread.start()
    print("waiting. Ctrl-C to stop.", flush=True)
    try:
        while True:
            time.sleep(3600)
    except KeyboardInterrupt:
        return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
