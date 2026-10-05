#!/usr/bin/env python3
"""Replay the vendor app's own soft-AP pairing sequence against the robot.

This mirrors what "Proscenic Robotic" 1.5.11 (com.baole.blap) does while the
phone sits on the robot's pairing AP. It is NOT the {"cmd":...} UDP protocol on
port 7913 that the firmware analysis found; the app contains no code for that
protocol at all. Sources (decompiled app):

  1. ConnectUtil.sendUdp() (ConnectUtil$6)
       bind local UDP 12002, send b"version:?\\n" to 192.168.4.1:12002,
       wait 1 s per try for a reply containing "version", give up after ~5 s.
       The text after "version" is kept as the robot's version string.
  2. HttpRobotClient.getRobotInfo()
       GET http://<robot-ip>/robot/getRobotInfo.do
           ?ssid=<home ssid>&pwd=<home pass>
           &jDomain=<HTTP API host, no scheme>&jPort=<HTTP API port>
           &sDomain=<IM/push host>&sPort=20008&cleanSTime=5
       Retrofit @Query(encoded=true): the values go out RAW. RobotBaseInterceptor
       then replaces only ! @ $ ^ ( ) with %21 %40 %24 %5E %28 %29.
       Expected reply: {"result":"0","data":{appKey, authCode, deviceId,
       deviceType, errorCode, errorMsg, inModel, netWork, uid, version, ...}}.
       The app only proceeds if data.appKey == 67ce4fabe562405d9492cad9097e09bf.
  3. The app then switches the phone back to the home Wi-Fi and polls the cloud
     (getRobotOnLineState) until the robot shows up. There is no extra commit
     message in this flow: the single HTTP GET carries Wi-Fi creds + both servers.

Robot IP: the app's normal path (lineRobot) always uses 192.168.4.1, whatever
the phone's own address is. Its alternative path (lineRobotWifi) uses the
phone's gateway (<own subnet>.1). The M7 Pro's AP has been seen at
192.168.78.1, so by default this script tries all of: 192.168.4.1, the gateway
of the interface that routes to the robot, and 192.168.78.1.

Nothing here is confirmed against a real unit; earlier scans found no TCP
listener and no open UDP 12002. Expect silence; the point is to see.
"""

from __future__ import annotations

import argparse
import datetime as dt
import http.client
import json
import socket
import sys
import time

APP_PORT_UDP = 12002
APP_APPKEY = "67ce4fabe562405d9492cad9097e09bf"
DEFAULT_TARGETS = ["192.168.4.1", "192.168.78.1"]
URL_ESCAPES = [("!", "%21"), ("@", "%40"), ("$", "%24"), ("^", "%5E"), ("(", "%28"), (")", "%29")]


def log(msg: str) -> None:
    print("%s %s" % (dt.datetime.now().strftime("%H:%M:%S.%f")[:-3], msg), flush=True)


def gateway_guess(probe_ip: str) -> str | None:
    """<own address>.1 on the interface that routes to probe_ip, like lineRobotWifi()."""
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.connect((probe_ip, 9))
        own = s.getsockname()[0]
        s.close()
    except OSError:
        return None
    return own.rsplit(".", 1)[0] + ".1"


def udp_version_probe(targets: list[str], port: int, local_port: int, total: float,
                      per_try: float, broadcast: str | None) -> dict[str, str]:
    """Send version:?\\n and collect every datagram that comes back, from anyone."""
    payload = b"version:?\n"
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
    try:
        sock.bind(("", local_port))
    except OSError as exc:
        log("cannot bind local UDP %d (%s); using an ephemeral port instead" % (local_port, exc))
        sock.bind(("", 0))
    log("UDP: local %s:%d" % sock.getsockname())
    sock.settimeout(per_try)
    dests = list(targets) + ([broadcast] if broadcast else [])
    found: dict[str, str] = {}
    deadline = time.monotonic() + total
    while time.monotonic() < deadline:
        for d in dests:
            try:
                sock.sendto(payload, (d, port))
                log("UDP -> %s:%d %r" % (d, port, payload))
            except OSError as exc:
                log("UDP -> %s:%d send failed: %s" % (d, port, exc))
        end = time.monotonic() + per_try
        while time.monotonic() < end:
            try:
                data, peer = sock.recvfrom(4096)
            except socket.timeout:
                break
            except ConnectionRefusedError:
                log("UDP <- ICMP port unreachable (nothing listening on %d)" % port)
                continue
            except OSError as exc:
                log("UDP <- error: %s" % exc)
                continue
            text = data.decode("utf-8", "replace")
            log("UDP <- %s:%d %r" % (peer[0], peer[1], text))
            if "version" in text:
                # App: substring after "version", trimmed (exact cut not recovered; keep raw too)
                found[peer[0]] = text
        if found:
            break
    sock.close()
    return found


def build_query(a: argparse.Namespace) -> str:
    pairs = [
        ("ssid", a.ssid), ("pwd", a.pwd),
        ("jDomain", a.jdomain), ("jPort", str(a.jport)),
        ("sDomain", a.sdomain), ("sPort", str(a.sport)),
        ("cleanSTime", str(a.clean_stime)),
    ]
    q = "&".join("%s=%s" % (k, v) for k, v in pairs)
    if a.urlencode:
        from urllib.parse import urlencode
        return urlencode(pairs)
    for raw, esc in URL_ESCAPES:  # RobotBaseInterceptor's only escaping
        q = q.replace(raw, esc)
    return q


def redact(text: str, a: argparse.Namespace) -> str:
    if a.show_secrets or not a.pwd:
        return text
    escaped = a.pwd
    for raw, esc in URL_ESCAPES:
        escaped = escaped.replace(raw, esc)
    from urllib.parse import quote_plus
    for form in {a.pwd, escaped, quote_plus(a.pwd)}:
        text = text.replace("pwd=" + form, "pwd=***")
    return text


def http_get_robot_info(host: str, port: int, path: str, timeout: float,
                        a: argparse.Namespace) -> dict | None:
    log("HTTP -> GET http://%s:%d%s" % (host, port, redact(path, a)))
    conn = http.client.HTTPConnection(host, port, timeout=timeout)
    try:
        # putrequest with skip_accept_encoding so the raw path is sent unmodified
        conn.putrequest("GET", path, skip_accept_encoding=True)
        conn.putheader("Accept-Encoding", "gzip")
        conn.putheader("User-Agent", "okhttp/3.10.0")
        conn.putheader("Connection", "Keep-Alive")
        conn.endheaders()
        resp = conn.getresponse()
        body = resp.read()
    except (OSError, http.client.HTTPException) as exc:
        log("HTTP <- %s:%d failed: %s: %s" % (host, port, type(exc).__name__, exc))
        return None
    finally:
        conn.close()
    log("HTTP <- %d %s, %d bytes" % (resp.status, resp.reason, len(body)))
    for k, v in resp.getheaders():
        log("         %s: %s" % (k, v))
    text = body.decode("utf-8", "replace")
    log("HTTP <- body: %s" % text[:2000])
    try:
        js = json.loads(text)
    except ValueError:
        return None
    data = js.get("data") if isinstance(js, dict) else None
    if isinstance(js, dict):
        log("result=%r (app needs \"0\")" % js.get("result"))
    if isinstance(data, dict):
        ak = data.get("appKey")
        log("appKey=%r -> %s" % (ak, "matches the app" if ak == APP_APPKEY else "app would reject (error 103)"))
        for k in ("deviceId", "authCode", "deviceType", "inModel", "version", "netWork", "uid",
                  "errorCode", "errorMsg"):
            if k in data:
                log("  %s=%r" % (k, data[k]))
    return js


def main(argv: list[str]) -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0],
                                formatter_class=argparse.RawDescriptionHelpFormatter,
                                epilog=__doc__.split("\n\n", 1)[1])
    p.add_argument("--ssid", required=True, help="home Wi-Fi SSID the robot should join")
    p.add_argument("--pwd", required=True, help="home Wi-Fi password")
    p.add_argument("--jdomain", required=True,
                   help="HTTP API host the robot should use, WITHOUT scheme (app: getNotHttpUrl())")
    p.add_argument("--jport", type=int, default=8082, help="HTTP API port (app: BuildConfig.HTTP_PORT=8082)")
    p.add_argument("--sdomain", help="IM/push gateway host (app: bl-im*.robotbona.com); default = --jdomain")
    p.add_argument("--sport", type=int, default=20008, help="IM/push gateway port (app: 20008)")
    p.add_argument("--clean-stime", default="5", help='"cleanSTime" query value (app sends "5")')
    p.add_argument("--target", action="append", dest="targets",
                   help="robot IP to try (repeatable). Default: 192.168.4.1, gateway guess, 192.168.78.1")
    p.add_argument("--http-port", type=int, action="append", dest="http_ports",
                   help="HTTP port(s) to try (repeatable). Default: 80")
    p.add_argument("--udp-port", type=int, default=APP_PORT_UDP, help="robot UDP port (app: 12002)")
    p.add_argument("--udp-local-port", type=int, default=APP_PORT_UDP, help="local UDP port (app binds 12002)")
    p.add_argument("--udp-total", type=float, default=5.0, help="seconds to keep probing UDP (app: ~5)")
    p.add_argument("--udp-per-try", type=float, default=1.0, help="seconds per UDP try (app: 1)")
    p.add_argument("--broadcast", help="also send the UDP probe to this broadcast address, e.g. 192.168.78.255")
    p.add_argument("--http-timeout", type=float, default=10.0, help="HTTP timeout (app: 30 s connect)")
    p.add_argument("--urlencode", action="store_true",
                   help="fully URL-encode the query instead of the app's raw-with-6-escapes form")
    p.add_argument("--skip-udp", action="store_true", help="go straight to the HTTP request")
    p.add_argument("--show-secrets", action="store_true", help="do not redact the Wi-Fi password in logs")
    a = p.parse_args(argv)
    a.sdomain = a.sdomain or a.jdomain

    targets = a.targets or list(DEFAULT_TARGETS)
    if not a.targets:
        gw = gateway_guess("192.168.78.1")
        if gw and gw not in targets:
            targets.insert(1, gw)
    ports = a.http_ports or [80]
    log("targets: %s; HTTP ports: %s" % (", ".join(targets), ports))

    if not a.skip_udp:
        log("step 1/2: UDP version probe (ConnectUtil.sendUdp)")
        found = udp_version_probe(targets, a.udp_port, a.udp_local_port, a.udp_total,
                                  a.udp_per_try, a.broadcast)
        if found:
            log("UDP replies: %s" % found)
            # Prefer whoever answered, then the rest
            targets = list(found) + [t for t in targets if t not in found]
        else:
            log("no UDP reply; the app carries on to the HTTP step anyway (version stays empty)")

    log("step 2/2: HTTP getRobotInfo.do (HttpRobotClient)")
    path = "/robot/getRobotInfo.do?" + build_query(a)
    ok = False
    for t in targets:
        for port in ports:
            js = http_get_robot_info(t, port, path, a.http_timeout, a)
            if isinstance(js, dict) and str(js.get("result")) == "0":
                ok = True
                log("robot at %s:%d accepted the pairing request. The app would now rejoin the home "
                    "Wi-Fi and wait for the robot to come online at jDomain/sDomain." % (t, port))
                break
        if ok:
            break
    if not ok:
        log("no target accepted getRobotInfo.do")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
