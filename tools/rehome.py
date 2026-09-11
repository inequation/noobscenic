#!/usr/bin/env python3
"""Re-home a Proscenic M7 Pro onto your own server — channel C (UDP JSON) client.

Protocol reference: ../doc/reverse-engineering/PROTOCOL.md section C, and
doc/PLAN.md section 13. Standard library only, on purpose: this usually runs from a
laptop that has just joined the robot's `LDRobot` soft-AP with nothing installed.

The robot's LAN control server binds a UDP port that the firmware sample picks as
rand()%1000 + 9000, so `discover` has to sweep for it. Do not trust that range: the
bench unit answers on **7913**, outside it entirely (see FIELD_NOTES.md), so the sweep
defaults to 7000-9999 and `--discover-ports` widens it.

Typical use, with the robot in pairing mode (soft-AP `LDRobot`, robot at
192.168.78.1) and this machine joined to that AP:

    ./rehome.py discover
    ./rehome.py info
    ./rehome.py rehome --server-host 192.168.1.10 --ssid my-wifi --pwd 'secret'

`rehome` sets the cloud URL and the push-gateway address first and moves the robot to
your Wi-Fi last, because that last step tears down the AP we are talking over.

If `discover` finds nothing, the unit may not speak this protocol at all — see
../doc/reverse-engineering/FIELD_NOTES.md. `portscan` and `listen` are the tools for
working out what it does speak.
"""

from __future__ import annotations

import argparse
import base64
import binascii
import concurrent.futures
import json
import selectors
import socket
import sys
import time

DEFAULT_TARGET = "192.168.78.1"  # the robot in soft-AP pairing mode
# The LS_S6 firmware picks rand()%1000 + 9000, but the bench unit answers on 7913, so
# the default sweep covers both bases. Widen it with --discover-ports if a unit hides
# elsewhere; "1024-65535" works and costs a few seconds more.
DEFAULT_DISCOVER_PORTS = "7000-9999"
PWD_MIN, PWD_MAX = 8, 64  # enforced by the device's setSta handler
SECRET_KEYS = ("staPwd", "pwd", "password")

EXIT_OK, EXIT_FAIL, EXIT_USAGE = 0, 1, 2

# Ports worth a look when the documented 9000-9999 range comes back closed, as it
# does on the Proscenic-6716 unit (see ../doc/reverse-engineering/FIELD_NOTES.md):
# common IoT provisioning and discovery services.
SUSPECT_UDP_PORTS = (
    "53,67,68,123,161,1900,3702,5353,5683,6666,6667,7777,8888,9000-9010,"
    "10000,18888,30303,38899,48899,49152,54321,58866"
)


class ProtocolError(Exception):
    """Something went wrong talking to the robot."""


class NoReply(ProtocolError):
    """Nothing came back within the timeout."""


class CommandFailed(ProtocolError):
    """The robot answered, and the answer was no."""


def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime()) + ".%03dZ" % (
        int(time.time() * 1000) % 1000
    )


def quiet_icmp_resets(sock: socket.socket) -> None:
    """Stop Windows from failing recvfrom() because some *other* port was closed.

    On Windows an unconnected UDP socket that draws an ICMP port-unreachable fails the
    next recvfrom with WSAECONNRESET, so a sweep across a few thousand closed ports
    drowns the one genuine reply. SIO_UDP_CONNRESET disables that. No-op elsewhere.
    """
    if hasattr(socket, "SIO_UDP_CONNRESET"):
        try:
            sock.ioctl(socket.SIO_UDP_CONNRESET, False)
        except OSError:
            pass


def redact(msg: dict, show_secrets: bool) -> dict:
    if show_secrets:
        return msg
    out = dict(msg)
    for key in SECRET_KEYS:
        if key in out:
            out[key] = "***"
    return out


class Channel:
    """One UDP conversation with the robot."""

    def __init__(
        self,
        target: str,
        port: int | None = None,
        timeout: float = 1.0,
        retries: int = 2,
        trace_path: str | None = None,
        dry_run: bool = False,
        show_secrets: bool = False,
        quiet: bool = False,
    ):
        self.target = target
        self.port = port
        self.timeout = timeout
        self.retries = retries
        self.dry_run = dry_run
        self.show_secrets = show_secrets
        self.quiet = quiet
        self.trace = open(trace_path, "a", encoding="utf-8") if trace_path else None
        self.sock: socket.socket | None = None

    # -- plumbing ---------------------------------------------------------

    def _socket(self) -> socket.socket:
        if self.sock is None:
            self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
            quiet_icmp_resets(self.sock)
            self.sock.settimeout(self.timeout)
        return self.sock

    def _record(self, direction: str, peer: str, raw: str, parsed: dict | None) -> None:
        if self.trace is None:
            return
        shown = raw
        if parsed is not None and not self.show_secrets:
            redacted = redact(parsed, False)
            if redacted != parsed:
                shown = json.dumps(redacted, ensure_ascii=False)
        self.trace.write(
            json.dumps(
                {"ts": now_iso(), "dir": direction, "peer": peer, "raw": shown},
                ensure_ascii=False,
            )
            + "\n"
        )
        self.trace.flush()

    def _say(self, text: str) -> None:
        if not self.quiet:
            print(text)

    def close(self) -> None:
        if self.sock is not None:
            self.sock.close()
            self.sock = None
        if self.trace is not None:
            self.trace.close()
            self.trace = None

    # -- exchanges --------------------------------------------------------

    def send_to(self, msg: dict, port: int) -> None:
        """Fire one datagram, expecting nothing back (used by the discovery sweep)."""
        raw = json.dumps(msg, ensure_ascii=False)
        peer = "%s:%d" % (self.target, port)
        self._record("tx", peer, raw, msg)
        if self.dry_run:
            self._say("  [dry-run] -> %s  %s" % (peer, json.dumps(redact(msg, self.show_secrets))))
            return
        self._socket().sendto(raw.encode("utf-8"), (self.target, port))

    def request(self, msg: dict, port: int | None = None) -> dict:
        """Send a command and return the robot's reply, retrying on silence."""
        port = port or self.port
        if port is None:
            raise ProtocolError("no port known — run `discover` first, or pass --port")
        peer = "%s:%d" % (self.target, port)

        if self.dry_run:
            self._say("  [dry-run] -> %s  %s" % (peer, json.dumps(redact(msg, self.show_secrets))))
            self._record("tx", peer, json.dumps(msg, ensure_ascii=False), msg)
            return {"result": "ok", "dry_run": True}

        last_error = "no reply"
        for attempt in range(self.retries + 1):
            self.send_to(msg, port)
            deadline = time.monotonic() + self.timeout
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    break
                sock = self._socket()
                sock.settimeout(remaining)
                try:
                    data, addr = sock.recvfrom(65535)
                except socket.timeout:
                    break
                except OSError as exc:  # ICMP port-unreachable arrives as an error
                    last_error = "socket error: %s" % exc
                    break
                text = data.decode("utf-8", "replace")
                self._record("rx", "%s:%d" % addr, text, None)
                if not data:
                    # A zero-length datagram is the device answering with no payload,
                    # which is not the same as silence and is worth saying plainly.
                    last_error = (
                        "empty datagram from %s:%d — the device answered but sent no "
                        "payload; if this was getWifi, the scan result may simply be "
                        "slower than --timeout" % addr
                    )
                    continue
                try:
                    reply = json.loads(text)
                except json.JSONDecodeError:
                    last_error = "non-JSON reply from %s:%d: %r" % (addr[0], addr[1], text[:200])
                    continue
                if isinstance(reply, dict):
                    return reply
                last_error = "unexpected reply shape: %r" % (reply,)
            if attempt < self.retries:
                self._say("  no reply, retrying (%d/%d)" % (attempt + 1, self.retries))
        raise NoReply(last_error)

    def command(self, cmd: str, **fields) -> dict:
        """Send {"cmd": ...} and insist the robot said ok."""
        msg = {"cmd": cmd}
        msg.update(fields)
        reply = self.request(msg)
        result = reply.get("result")
        if result != "ok":
            raise CommandFailed(
                "%s failed: result=%r code=%r full=%s"
                % (cmd, result, reply.get("code"), json.dumps(reply, ensure_ascii=False))
            )
        return reply

    # -- discovery --------------------------------------------------------

    def discover(self, ports: list[int], settle: float = 2.0) -> list[tuple[str, int]]:
        """Spray {"cmd":"getID"} across `ports` and collect whoever answers."""
        found: list[tuple[str, int]] = []
        seen: set[tuple[str, int]] = set()
        probe = {"cmd": "getID"}

        if self.dry_run:
            self._say(
                "  [dry-run] would send %s to %s ports %d-%d"
                % (json.dumps(probe), self.target, ports[0], ports[-1])
            )
            return []

        sock = self._socket()
        raw = json.dumps(probe).encode("utf-8")
        self._record(
            "tx", "%s:%d-%d" % (self.target, ports[0], ports[-1]), json.dumps(probe), probe
        )
        for index, port in enumerate(ports):
            try:
                sock.sendto(raw, (self.target, port))
            except OSError:
                pass  # a closed port may bounce ICMP; keep sweeping
            if index % 100 == 99:
                # Paces the sweep *and* picks up anything that has already answered,
                # so an early reply cannot be buried behind thousands of later probes.
                self._collect(sock, found, seen, 0.005)

        self._collect(sock, found, seen, settle)
        return found

    def _collect(self, sock, found, seen, budget: float) -> None:
        """Drain whatever has arrived, for at most `budget` seconds."""
        deadline = time.monotonic() + budget
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return
            sock.settimeout(remaining)
            try:
                data, addr = sock.recvfrom(65535)
            except socket.timeout:
                return
            except OSError:
                continue  # ICMP noise from the closed ports; keep draining
            text = data.decode("utf-8", "replace")
            self._record("rx", "%s:%d" % addr, text, None)
            try:
                reply = json.loads(text)
            except json.JSONDecodeError:
                continue
            if not isinstance(reply, dict) or reply.get("result") != "ok":
                continue
            if addr not in seen:
                seen.add(addr)
                found.append(addr)
                self._say("  found %s:%d  %s" % (addr[0], addr[1], json.dumps(reply)))


# -- subcommands ----------------------------------------------------------


def need_port(chan: Channel, args) -> None:
    """Make sure the channel knows which port to talk to."""
    if chan.port is not None:
        return
    ports = parse_ports(args.discover_ports)
    print("discovering (sweeping UDP %d-%d on %s)..." % (ports[0], ports[-1], chan.target))
    found = chan.discover(ports, args.settle)
    if not found:
        raise ProtocolError(
            "no robot answered getID on %s. Is the robot in pairing mode and is this "
            "machine on its network?" % chan.target
        )
    if len(found) > 1:
        print("warning: %d responders; using the first" % len(found), file=sys.stderr)
    chan.port = found[0][1]
    chan.target = found[0][0]
    print("using %s:%d" % (chan.target, chan.port))


def cmd_discover(chan: Channel, args) -> int:
    ports = parse_ports(args.discover_ports)
    print("sweeping UDP %d-%d on %s..." % (ports[0], ports[-1], chan.target))
    found = chan.discover(ports, args.settle)
    if not found:
        print("no responders", file=sys.stderr)
        return EXIT_FAIL
    for ip, port in found:
        print("%s:%d" % (ip, port))
    return EXIT_OK


def cmd_info(chan: Channel, args) -> int:
    need_port(chan, args)
    answered = 0
    for cmd in ("getSn", "getCfg", "checkPwd"):
        try:
            reply = chan.command(cmd)
        except ProtocolError as exc:
            print("%-9s !! %s" % (cmd, exc), file=sys.stderr)
            continue
        answered += 1
        print("%-9s %s" % (cmd, json.dumps(redact(reply, chan.show_secrets), ensure_ascii=False)))
    # One command failing is worth reporting but not fatal; all three failing means
    # we are not talking to the robot at all.
    return EXIT_OK if answered else EXIT_FAIL


def cmd_scan(chan: Channel, args) -> int:
    need_port(chan, args)
    # A scan takes the radio off the channel it is serving the AP on, so the result
    # arrives seconds later and anything we send meanwhile can be lost. Wait, do not
    # retry into the gap.
    chan.timeout = args.scan_timeout
    chan.retries = 0
    reply = chan.command("getWifi")
    networks = reply.get("wifi_list", [])
    if not networks:
        print("(no networks reported)")
    for entry in networks:
        print(json.dumps(entry, ensure_ascii=False))
    return EXIT_OK


def normalise_url(url: str) -> str:
    # Endpoints are appended to this base as `cleanPack/register`, so it must end in "/".
    if not url.endswith("/"):
        print("note: appending a trailing '/' to the base URL", file=sys.stderr)
        url += "/"
    return url


def cmd_set_url(chan: Channel, args) -> int:
    need_port(chan, args)
    reply = chan.command("setUrl", url=normalise_url(args.url))
    print("setUrl ok: %s" % json.dumps(reply, ensure_ascii=False))
    return EXIT_OK


def cmd_set_gateway(chan: Channel, args) -> int:
    need_port(chan, args)
    reply = chan.command("setUrl", ip=args.ip, port=args.port_number)
    print("setUrl(ip/port) ok: %s" % json.dumps(reply, ensure_ascii=False))
    return EXIT_OK


def check_passphrase(pwd: str) -> None:
    if not PWD_MIN <= len(pwd) <= PWD_MAX:
        raise ProtocolError(
            "Wi-Fi passphrase must be %d-%d characters (the device rejects anything "
            "else); got %d" % (PWD_MIN, PWD_MAX, len(pwd))
        )


def cmd_set_sta(chan: Channel, args) -> int:
    need_port(chan, args)
    check_passphrase(args.pwd)
    try:
        reply = chan.command("setSta", ssid=args.ssid, **{args.pwd_key: args.pwd})
        print("setSta ok: %s" % json.dumps(reply, ensure_ascii=False))
    except NoReply as exc:
        print("no confirmation: %s" % exc, file=sys.stderr)
        print("(expected if the AP went away as it switched — verify on your LAN)")
    print("the robot is switching to station mode; the soft-AP is going away now.")
    return EXIT_OK


def cmd_set_ap(chan: Channel, args) -> int:
    need_port(chan, args)
    fields: dict = {}
    if args.ssid:
        fields["ssid"] = args.ssid
    if args.pwd:
        fields["pwd"] = args.pwd
    if args.segment is not None:
        fields["segment"] = args.segment
    reply = chan.command("setAp", **fields)
    print("setAp ok: %s" % json.dumps(reply, ensure_ascii=False))
    return EXIT_OK


def cmd_reset_wifi(chan: Channel, args) -> int:
    need_port(chan, args)
    reply = chan.command("resetWifi")
    print("resetWifi ok: %s" % json.dumps(reply, ensure_ascii=False))
    return EXIT_OK


def cmd_get_log(chan: Channel, args) -> int:
    """Pull the device log package. Chunk field shapes are unverified (PLAN section 16)."""
    need_port(chan, args)
    offset, chunks, total = 0, [], None
    for _ in range(args.max_chunks):
        reply = chan.request({"req": "getLog", "offset": offset})
        if reply.get("result") not in (None, "ok"):
            raise ProtocolError("getLog failed: %s" % json.dumps(reply, ensure_ascii=False))
        package = reply.get("package")
        if not package:
            break
        try:
            blob = base64.b64decode(package)
        except (binascii.Error, ValueError) as exc:
            raise ProtocolError("getLog chunk at offset %d is not base64: %s" % (offset, exc))
        chunks.append(blob)
        total = reply.get("totalLen", total)
        advance = reply.get("pLen") or len(blob)
        if advance <= 0:
            break
        offset += advance
        if total is not None and offset >= total:
            break
    else:
        print("warning: stopped at --max-chunks; log may be truncated", file=sys.stderr)

    data = b"".join(chunks)
    if not data:
        print("no log data returned", file=sys.stderr)
        return EXIT_FAIL
    with open(args.out, "wb") as handle:
        handle.write(data)
    print("wrote %d bytes to %s" % (len(data), args.out))
    if total is not None and len(data) != total:
        print("warning: device reported totalLen=%s, got %d" % (total, len(data)), file=sys.stderr)
    return EXIT_OK


def cmd_rehome(chan: Channel, args) -> int:
    """The whole sequence, in the order that does not cut the branch we sit on."""
    check_passphrase(args.pwd)
    url = normalise_url(args.url or "http://%s:%d/" % (args.server_host, args.http_port))
    gateway_ip = args.gateway_ip or args.server_host

    need_port(chan, args)

    print("1/4 setUrl  %s" % url)
    chan.command("setUrl", url=url)

    print("2/4 setUrl  gateway %s:%d" % (gateway_ip, args.gateway_port))
    chan.command("setUrl", ip=gateway_ip, port=args.gateway_port)

    print("3/4 getCfg  (verify)")
    try:
        cfg = chan.command("getCfg")
        print("    %s" % json.dumps(redact(cfg, chan.show_secrets), ensure_ascii=False))
    except ProtocolError as exc:
        # getCfg is a read; a failure here does not undo the writes above.
        print("    warning: could not verify: %s" % exc, file=sys.stderr)

    print("4/4 setSta  ssid=%s" % args.ssid)
    try:
        chan.command("setSta", ssid=args.ssid, **{args.pwd_key: args.pwd})
    except NoReply as exc:
        # The device drops the soft-AP as it switches to station mode, so the
        # confirmation frequently never makes it back to us. That is not a failure,
        # and reporting it as one would send you chasing a re-home that worked.
        print("    no confirmation: %s" % exc, file=sys.stderr)
        print("    (expected if the AP went away as it switched — verify on your LAN)")

    print()
    print("done. The robot is joining %s and will look for:" % args.ssid)
    print("  channel A (HTTP)    %s" % url)
    print("  channel B (gateway) %s:%d" % (gateway_ip, args.gateway_port))
    print("Start noobscenic on those ports and watch the traces.")
    return EXIT_OK


def parse_ports(spec: str) -> list[int]:
    """"80", "1-1024", "53,9000-9010" -> a sorted list of port numbers."""
    ports: set[int] = set()
    for part in spec.split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part:
            low, _, high = part.partition("-")
            ports.update(range(int(low), int(high) + 1))
        else:
            ports.add(int(part))
    bad = [p for p in ports if not 1 <= p <= 65535]
    if bad:
        raise ProtocolError("port out of range: %s" % bad[0])
    return sorted(ports)


def probe_tcp(host: str, port: int, timeout: float) -> bool:
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(timeout)
    try:
        sock.connect((host, port))
        return True
    except OSError:
        return False
    finally:
        sock.close()


def probe_udp(host: str, port: int, payload: bytes, timeout: float) -> tuple[str, bytes | None]:
    """Classify a UDP port by connecting the socket first.

    A connected UDP socket surfaces the ICMP port-unreachable the device sends for a
    closed port as ECONNREFUSED, which is what lets this tell "closed" apart from
    "nothing came back" without root or a raw socket. Note that the device rate-limits
    ICMP errors (Linux does ~1/second), so on a wide sweep most closed ports still look
    like `no-response`; pace the scan if you need certainty.
    """
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.settimeout(timeout)
    try:
        sock.connect((host, port))
        sock.send(payload)
        try:
            return "open", sock.recv(65535)
        except (ConnectionRefusedError, ConnectionResetError):
            return "closed", None          # Windows reports the ICMP error as a reset
        except socket.timeout:
            return "no-response", None
    except (ConnectionRefusedError, ConnectionResetError):
        return "closed", None
    except OSError as exc:
        return "error: %s" % exc, None
    finally:
        sock.close()


def cmd_portscan(chan: Channel, args) -> int:
    """Find out what the robot is actually listening on."""
    found_any = False

    if args.proto in ("tcp", "both"):
        ports = parse_ports(args.tcp_ports)
        print("TCP: scanning %d ports on %s..." % (len(ports), chan.target))
        opened = []
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as pool:
            futures = {pool.submit(probe_tcp, chan.target, p, args.probe_timeout): p for p in ports}
            for future in concurrent.futures.as_completed(futures):
                if future.result():
                    opened.append(futures[future])
        for port in sorted(opened):
            print("  %5d/tcp  open" % port)
            found_any = True
        if not opened:
            print("  (nothing open)")

    if args.proto in ("udp", "both"):
        ports = parse_ports(args.udp_ports)
        payload = json.dumps({"cmd": "getID"}).encode("utf-8")
        print("UDP: probing %d ports on %s with %s..."
              % (len(ports), chan.target, payload.decode()))
        closed = 0
        for port in ports:
            verdict, data = probe_udp(chan.target, port, payload, args.probe_timeout)
            if verdict == "closed":
                closed += 1
                continue
            if verdict == "open":
                text = data.decode("utf-8", "replace") if data else ""
                print("  %5d/udp  ANSWERED  %s" % (port, text[:200]))
                found_any = True
            elif verdict.startswith("error"):
                print("  %5d/udp  %s" % (port, verdict))
            elif args.show_all:
                print("  %5d/udp  no-response" % port)
            if args.pace_ms:
                time.sleep(args.pace_ms / 1000.0)
        print("  %d ports answered ICMP unreachable (definitely closed)" % closed)

    return EXIT_OK if found_any else EXIT_FAIL


def cmd_listen(chan: Channel, args) -> int:
    """Bind and wait, sending nothing, in case the device announces itself.

    This only sees datagrams aimed at the ports it binds. For complete coverage use a
    real capture (`tcpdump -i <iface> -n udp`); this is the no-tcpdump fallback.
    """
    ports = parse_ports(args.ports)
    selector = selectors.DefaultSelector()
    socks = []
    for port in ports:
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
        try:
            sock.bind(("", port))
        except OSError as exc:
            print("  cannot bind %d: %s" % (port, exc), file=sys.stderr)
            sock.close()
            continue
        selector.register(sock, selectors.EVENT_READ, port)
        socks.append(sock)

    if not socks:
        raise ProtocolError("could not bind any of the requested ports")

    print("listening on %d UDP ports for %.0fs (sending nothing)..."
          % (len(socks), args.duration))
    deadline = time.monotonic() + args.duration
    heard = 0
    try:
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            for key, _ in selector.select(timeout=min(remaining, 1.0)):
                data, addr = key.fileobj.recvfrom(65535)
                heard += 1
                try:
                    traced = data.decode("utf-8")
                except UnicodeDecodeError:
                    traced = "base64:" + base64.b64encode(data).decode("ascii")
                text = data.decode("utf-8", "replace")
                chan._record("rx", "%s:%d" % addr, traced, None)
                print("  %s  %s:%d -> :%d  %d bytes"
                      % (now_iso(), addr[0], addr[1], key.data, len(data)))
                try:
                    print("      json: %s" % json.dumps(json.loads(text), ensure_ascii=False))
                except json.JSONDecodeError:
                    print("      raw:  %r" % data[:200])
    except KeyboardInterrupt:
        print("  interrupted")
    finally:
        for sock in socks:
            sock.close()
        selector.close()

    print("heard %d datagram(s)" % heard)
    return EXIT_OK if heard else EXIT_FAIL


# -- argument parsing -----------------------------------------------------


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="rehome.py",
        description="Point a Proscenic M7 Pro at your own server (channel C, UDP JSON).",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="Run with the robot in pairing mode (soft-AP `LDRobot`) or on the LAN.",
    )
    parser.add_argument("-t", "--target", default=DEFAULT_TARGET, help="robot IP (default %(default)s)")
    parser.add_argument("-p", "--port", type=int, help="robot UDP port; skips discovery")
    parser.add_argument("--timeout", type=float, default=1.0, help="per-reply timeout, seconds")
    parser.add_argument("--retries", type=int, default=2, help="resends before giving up")
    parser.add_argument("--settle", type=float, default=2.0, help="discovery collection window, seconds")
    parser.add_argument("--discover-ports", default=DEFAULT_DISCOVER_PORTS, metavar="RANGE",
                        help="ports the getID sweep covers (default %(default)s)")
    parser.add_argument("--trace", metavar="FILE", help="append a JSONL trace of every datagram")
    parser.add_argument("--dry-run", action="store_true", help="print datagrams, send nothing")
    parser.add_argument("--show-secrets", action="store_true", help="do not redact passphrases")
    parser.add_argument("--quiet", "-q", action="store_true", help="less chatter")
    parser.add_argument(
        "--pwd-key",
        default="staPwd",
        choices=("staPwd", "pwd"),
        help="JSON key for the Wi-Fi passphrase (default %(default)s; some builds use 'pwd')",
    )

    subs = parser.add_subparsers(dest="command", required=True)

    subs.add_parser("discover", help="find the robot's UDP control port").set_defaults(func=cmd_discover)
    subs.add_parser("info", help="getSn + getCfg + checkPwd").set_defaults(func=cmd_info)
    p_scan = subs.add_parser("scan", help="getWifi — list nearby access points")
    p_scan.add_argument("--scan-timeout", type=float, default=20.0,
                        help="how long to wait for the scan result (default %(default)ss)")
    p_scan.set_defaults(func=cmd_scan)

    p_url = subs.add_parser("set-url", help="set the channel-A base URL")
    p_url.add_argument("url", help="e.g. http://192.168.1.10:8080/")
    p_url.set_defaults(func=cmd_set_url)

    p_gw = subs.add_parser("set-gateway", help="set the channel-B push-gateway address")
    p_gw.add_argument("ip")
    p_gw.add_argument("port_number", type=int, metavar="port")
    p_gw.set_defaults(func=cmd_set_gateway)

    p_sta = subs.add_parser("set-sta", help="join a Wi-Fi network (ends the soft-AP)")
    p_sta.add_argument("--ssid", required=True)
    p_sta.add_argument("--pwd", required=True, help="8-64 characters")
    p_sta.set_defaults(func=cmd_set_sta)

    p_ap = subs.add_parser("set-ap", help="switch back to soft-AP mode")
    p_ap.add_argument("--ssid")
    p_ap.add_argument("--pwd")
    p_ap.add_argument("--segment", type=int, help="192.168.<segment>.x, device default 78")
    p_ap.set_defaults(func=cmd_set_ap)

    subs.add_parser("reset-wifi", help="clear the Wi-Fi configuration").set_defaults(func=cmd_reset_wifi)

    p_log = subs.add_parser("get-log", help="pull the device log package")
    p_log.add_argument("-o", "--out", default="device-log.bin")
    p_log.add_argument("--max-chunks", type=int, default=512)
    p_log.set_defaults(func=cmd_get_log)

    p_ps = subs.add_parser("portscan", help="find what the robot actually listens on")
    p_ps.add_argument("--proto", choices=("tcp", "udp", "both"), default="both")
    p_ps.add_argument("--tcp-ports", default="1-65535")
    p_ps.add_argument("--udp-ports", default=SUSPECT_UDP_PORTS)
    p_ps.add_argument("--workers", type=int, default=256, help="TCP concurrency")
    p_ps.add_argument("--probe-timeout", type=float, default=0.5,
                      help="per-port timeout; separate from the global --timeout")
    p_ps.add_argument("--pace-ms", type=float, default=0.0,
                      help="delay between UDP probes; the device rate-limits ICMP errors")
    p_ps.add_argument("--show-all", action="store_true", help="list no-response UDP ports too")
    p_ps.set_defaults(func=cmd_portscan)

    p_li = subs.add_parser("listen", help="wait for the device to announce itself")
    p_li.add_argument("--ports", default=SUSPECT_UDP_PORTS)
    p_li.add_argument("--duration", type=float, default=60.0, help="seconds")
    p_li.set_defaults(func=cmd_listen)

    p_re = subs.add_parser("rehome", help="setUrl + setSta, in the right order")
    p_re.add_argument("--server-host", required=True, help="where noobscenic runs, as the robot sees it")
    p_re.add_argument("--http-port", type=int, default=8080)
    p_re.add_argument("--gateway-ip", help="defaults to --server-host")
    p_re.add_argument("--gateway-port", type=int, default=8081)
    p_re.add_argument("--url", help="override the derived base URL")
    p_re.add_argument("--ssid", required=True, help="the Wi-Fi network the robot should join")
    p_re.add_argument("--pwd", required=True, help="8-64 characters")
    p_re.set_defaults(func=cmd_rehome)

    return parser


def main(argv: list[str]) -> int:
    args = build_parser().parse_args(argv)
    chan = Channel(
        target=args.target,
        port=args.port,
        timeout=args.timeout,
        retries=args.retries,
        trace_path=args.trace,
        dry_run=args.dry_run,
        show_secrets=args.show_secrets,
        quiet=args.quiet,
    )
    try:
        return args.func(chan, args)
    except ProtocolError as exc:
        print("error: %s" % exc, file=sys.stderr)
        return EXIT_FAIL
    except KeyboardInterrupt:
        print("interrupted", file=sys.stderr)
        return EXIT_FAIL
    finally:
        chan.close()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
