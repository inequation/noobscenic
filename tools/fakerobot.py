#!/usr/bin/env python3
"""A stand-in for the robot's channel-C UDP server, for testing `rehome.py`.

It answers the commands documented in ../doc/reverse-engineering/PROTOCOL.md section C
with the documented response shapes, and prints what it was asked to do. It exists so
the re-home tool can be exercised end to end without the vacuum — the same role
`simdev` plays for the Rust server.

    ./fakerobot.py --port 9123        # or omit --port to pick one like the robot does
    ./rehome.py -t 127.0.0.1 discover

Config writes are kept in memory and reflected back by getCfg, so a full `rehome` run
can be verified.
"""

from __future__ import annotations

import argparse
import base64
import json
import random
import socket
import sys

STATE = {
    "sn": "FAKE0123456789",
    "staName": "",
    "staPwd": "",
    "staIp": "192.168.78.1",
    "staMac": "00:11:22:33:44:55",
    "url": "https://mobile.proscenic.cn/",
    "gateway_ip": "",
    "gateway_port": 0,
    "wifi_mode": "ap",
}

LOG_BLOB = b"fake device log package\n" * 64


def handle(msg: dict) -> dict:
    if "req" in msg:
        return handle_log(msg)

    cmd = msg.get("cmd")
    if cmd == "getID":
        return {"cmd": cmd, "result": "ok", "type": "ipfromapp"}
    if cmd == "getSn":
        return {"cmd": cmd, "result": "ok", "sn": STATE["sn"]}
    if cmd == "checkPwd":
        return {"cmd": cmd, "result": "ok", "code": 2 if STATE["wifi_mode"] == "sta" else 0}
    if cmd == "getCfg":
        return {
            "cmd": cmd,
            "result": "ok",
            "staName": STATE["staName"],
            "staPwd": STATE["staPwd"],
            "staIp": STATE["staIp"],
            "staMac": STATE["staMac"],
            "url": STATE["url"],
            "ip": STATE["gateway_ip"],
            "port": STATE["gateway_port"],
        }
    if cmd == "getWifi":
        return {
            "cmd": cmd,
            "result": "ok",
            "wifi_list": [
                {"ssid": "my-wifi", "rssi": -42, "encrypt": "wpa2"},
                {"ssid": "neighbour", "rssi": -78, "encrypt": "wpa2"},
            ],
        }
    if cmd == "setUrl":
        if "url" in msg:
            STATE["url"] = msg["url"]
        elif "ip" in msg and "port" in msg:
            STATE["gateway_ip"] = msg["ip"]
            STATE["gateway_port"] = msg["port"]
        else:
            return {"cmd": cmd, "result": "fail", "code": -1}
        return {"cmd": cmd, "result": "ok"}
    if cmd == "setSta":
        pwd = msg.get("staPwd", msg.get("pwd", ""))
        if not 8 <= len(pwd) <= 64:  # the real handler validates this
            return {"cmd": cmd, "result": "fail", "code": -1}
        STATE["staName"] = msg.get("ssid", "")
        STATE["staPwd"] = pwd
        STATE["wifi_mode"] = "sta"
        return {"cmd": cmd, "result": "ok", "code": 2}
    if cmd == "setAp":
        STATE["wifi_mode"] = "ap"
        return {"cmd": cmd, "result": "ok"}
    if cmd == "applyCfg":
        return {"cmd": cmd, "result": "ok", "code": 1}
    if cmd == "resetWifi":
        STATE["staName"] = STATE["staPwd"] = ""
        STATE["wifi_mode"] = "ap"
        return {"cmd": cmd, "result": "ok"}
    return {"result": "invalue cmd"}


def handle_log(msg: dict) -> dict:
    if msg.get("req") == "rmLog":
        return {"req": "rmLog", "result": "ok"}
    offset = int(msg.get("offset", 0))
    chunk = LOG_BLOB[offset : offset + 512]
    package = base64.b64encode(chunk).decode("ascii")
    return {
        "req": "getLog",
        "result": "ok",
        "package": package,
        "pLen": len(chunk),
        "b64Len": len(package),
        "totalLen": len(LOG_BLOB),
    }


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Fake Proscenic channel-C endpoint.")
    parser.add_argument("--bind", default="127.0.0.1")
    parser.add_argument("--port", type=int, help="default: random in 9000-9999, like the robot")
    parser.add_argument("--quiet", "-q", action="store_true")
    args = parser.parse_args(argv)

    port = args.port if args.port else random.randrange(9000, 10000)
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.bind((args.bind, port))
    print("fake robot listening on %s:%d" % (args.bind, port), flush=True)

    try:
        while True:
            data, addr = sock.recvfrom(65535)
            text = data.decode("utf-8", "replace")
            try:
                msg = json.loads(text)
            except json.JSONDecodeError:
                if not args.quiet:
                    print("<- %s:%d  (not JSON) %r" % (addr[0], addr[1], text[:120]), flush=True)
                continue
            reply = handle(msg)
            if not args.quiet:
                print("<- %s  -> %s" % (text, json.dumps(reply)), flush=True)
            sock.sendto(json.dumps(reply).encode("utf-8"), addr)
    except KeyboardInterrupt:
        return 0
    finally:
        sock.close()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
