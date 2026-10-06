#!/usr/bin/env python3
"""Re-home a Proscenic M7 Pro onto your own server — channel C (UDP JSON), one run.

Points the robot's cloud at the noobscenic instance on this machine and joins it to
your Wi-Fi. This is the only channel-C operation the project has: the server itself
never speaks channel C (doc/PLAN.md section 13), so this is a one-shot setup tool
rather than a client library. Standard library only, on purpose: it usually runs from
a laptop that has just joined the robot's own open Wi-Fi (the network name begins
with `Proscenic-`) with nothing installed. Protocol reference:
doc/reverse-engineering/PROTOCOL.md section C.

Start noobscenic first — it must be listening on both of its ports before the robot
is armed — then, with the robot in pairing mode (its own `Proscenic-…` AP, robot at
192.168.78.1):

    ./rehome.py --server-host 192.168.1.10 --ssid my-wifi --pwd 'secret' --userid me

The sequence, in the order that does not cut the branch we are sitting on:

  1. discovery: find the robot's UDP control port. The firmware sample picks
     rand()%1000 + 9000, but the bench unit answers on 7913, outside that range, so
     the default sweep covers 7000-9999 (--discover-ports widens it further).
  2. `setUrl url` rewrites the Channel-A base URL (/data/bin/Run/Config/url), which
     is what HTTP registration and the preBind POST are built from.
  3. `setUrl ip/port` writes /data/bin/Run/Config/ip_port.json — the Channel-B push
     gateway. The robot reads that file first and keeps it across reboots, so this
     is the write that actually re-homes the robot; setUrl(url) + setSta alone leave
     it dialing the cached vendor gateway.
  4. `getCfg` verifies the writes landed (a read; failing does not undo them).
  5. `setID` arms the bind. The robot resolves the gateway address and starts
     dialing; pairing ends only once channel B answers its 10001 handshake and every
     21006 ping, and channel A answers the preBind with code:0 (see
     PAIRING_LOG_ANALYSIS.md section 4).
  6. `setSta` + `applyCfg` store the Wi-Fi credentials and commit. This is last
     because the robot drops its soft-AP as it joins your network, so the reply
     often never arrives; that silence is expected, not a failure.

Never send `bindOk` on this firmware: it close()s the local socket without stopping
the listener and spins ~85 shell forks per second until the robot reboots
(PAIRING_LOG_ANALYSIS.md section 5). `--dry-run` prints every datagram instead of
sending it.
"""

from __future__ import annotations

import argparse
import json
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

EXIT_OK, EXIT_FAIL = 0, 1


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
            raise ProtocolError("no port known — discovery did not run, or pass --port")
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

    def discover(
        self, ports: list[int], settle: float = 2.0, probe_cmd: str = "getID"
    ) -> list[tuple[str, int]]:
        """Spray a probe across `ports` and collect whoever answers.

        A refusal counts as a find: not every firmware implements `getID`, and
        `{"result":"invalue cmd"}` identifies the device just as well as an `ok`.
        """
        found: list[tuple[str, int]] = []
        seen: set[tuple[str, int]] = set()
        probe = {"cmd": probe_cmd}

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
            # Any well-formed JSON object proves something is listening. Do NOT
            # require result=="ok": Unit A does not implement getID and answers
            # {"result":"invalue cmd"}, which is still a positive identification.
            if not isinstance(reply, dict):
                continue
            if addr not in seen:
                seen.add(addr)
                found.append(addr)
                self._say("  found %s:%d  %s" % (addr[0], addr[1], json.dumps(reply)))


# -- the re-home sequence -------------------------------------------------


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


def need_port(chan: Channel, args) -> None:
    """Make sure the channel knows which port to talk to."""
    if chan.port is not None:
        return
    ports = parse_ports(args.discover_ports)
    print("discovering (sweeping UDP %d-%d on %s)..." % (ports[0], ports[-1], chan.target))
    found = chan.discover(ports, args.settle, args.probe_cmd)
    if not found:
        raise ProtocolError(
            "no robot answered %s on %s. Is the robot in pairing mode and is this "
            "machine on its network?" % (args.probe_cmd, chan.target)
        )
    if len(found) > 1:
        print("warning: %d responders; using the first" % len(found), file=sys.stderr)
    chan.port = found[0][1]
    chan.target = found[0][0]
    print("using %s:%d" % (chan.target, chan.port))


def normalise_url(url: str) -> str:
    # Endpoints are appended to this base as `cleanPack/register`, so it must end in "/".
    if not url.endswith("/"):
        print("note: appending a trailing '/' to the base URL", file=sys.stderr)
        url += "/"
    return url


def check_passphrase(pwd: str) -> None:
    if not PWD_MIN <= len(pwd) <= PWD_MAX:
        raise ProtocolError(
            "Wi-Fi passphrase must be %d-%d characters (the device rejects anything "
            "else); got %d" % (PWD_MIN, PWD_MAX, len(pwd))
        )


def rehome(chan: Channel, args) -> int:
    """Point the robot at this machine's noobscenic, arm the bind, then join Wi-Fi."""
    check_passphrase(args.pwd)
    url = normalise_url(args.url or "http://%s:%d/" % (args.server_host, args.http_port))
    gateway_ip = args.gateway_ip or args.server_host

    need_port(chan, args)

    print("1/6 setUrl  cloud base URL  %s" % url)
    chan.command("setUrl", url=url)

    print("2/6 setUrl  push gateway    %s:%d  (writes ip_port.json — the write that re-homes)"
          % (gateway_ip, args.gateway_port))
    chan.command("setUrl", ip=gateway_ip, port=args.gateway_port)

    print("3/6 getCfg  (verify)")
    try:
        cfg = chan.command("getCfg")
        print("    %s" % json.dumps(redact(cfg, chan.show_secrets), ensure_ascii=False))
    except ProtocolError as exc:
        # getCfg is a read; a failure here does not undo the writes above.
        print("    warning: could not verify: %s" % exc, file=sys.stderr)

    print("4/6 setID   arm the bind   id=%s" % args.userid)
    sn = chan.command("getSn").get("sn", "")
    reply = chan.command("setID", id=args.userid, deviceSN=sn)
    print("    sn=%s  %s" % (sn, json.dumps(reply, ensure_ascii=False)))

    try:
        print("5/6 setSta  ssid=%s" % args.ssid)
        chan.command("setSta", **{args.ssid_key: args.ssid, args.pwd_key: args.pwd})
        print("6/6 applyCfg  (commit; the soft-AP goes away now)")
        reply = chan.command("applyCfg")
        print("    %s" % json.dumps(reply, ensure_ascii=False))
    except NoReply as exc:
        # The device drops the soft-AP as it switches to station mode, so the
        # confirmation frequently never makes it back to us. That is not a failure,
        # and reporting it as one would send you chasing a re-home that worked.
        print("    no confirmation: %s" % exc, file=sys.stderr)
        print("    (expected if the AP went away as it switched — verify on your LAN)")

    print()
    print("The robot joins %s and will look for:" % args.ssid)
    print("  channel A (HTTP)           %s" % url)
    print("  channel B (push gateway)   %s:%d" % (gateway_ip, args.gateway_port))
    print("noobscenic must be listening on both; watch its traces for the bind.")
    return EXIT_OK


# -- argument parsing -----------------------------------------------------


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="rehome.py",
        description="Point a Proscenic M7 Pro at your own noobscenic server (channel C, UDP JSON).",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="Never sends bindOk (firmware fork storm, PAIRING_LOG_ANALYSIS.md section 5).",
    )

    target = parser.add_argument_group("robot")
    target.add_argument("-t", "--target", default=DEFAULT_TARGET, help="robot IP (default %(default)s)")
    target.add_argument("-p", "--port", type=int, help="robot UDP port; skips discovery")
    target.add_argument("--discover-ports", default=DEFAULT_DISCOVER_PORTS, metavar="RANGE",
                        help="ports the discovery sweep covers (default %(default)s)")
    target.add_argument("--probe-cmd", default="getID", metavar="CMD",
                        help="command the sweep sends (default %(default)s; getSn is a "
                             "good alternative on firmware that lacks getID)")
    target.add_argument("--settle", type=float, default=2.0,
                        help="discovery collection window, seconds (default %(default)s)")
    target.add_argument("--timeout", type=float, default=1.0, help="per-reply timeout, seconds")
    target.add_argument("--retries", type=int, default=2, help="resends before giving up")

    cloud = parser.add_argument_group("new cloud")
    cloud.add_argument("--server-host", required=True,
                       help="where noobscenic runs, as the robot sees it")
    cloud.add_argument("--http-port", type=int, default=8080,
                       help="channel A port (default %(default)s)")
    cloud.add_argument("--gateway-ip", help="channel B address; defaults to --server-host")
    cloud.add_argument("--gateway-port", type=int, default=8081,
                       help="channel B port (default %(default)s)")
    cloud.add_argument("--url", help="override the derived channel-A base URL")
    cloud.add_argument("--userid", required=True,
                       help="the robot binds to it; any non-empty string will do")

    wifi = parser.add_argument_group("wi-fi")
    wifi.add_argument("--ssid", required=True, help="the Wi-Fi network the robot should join")
    wifi.add_argument("--pwd", required=True, help="8-64 characters")
    wifi.add_argument("--ssid-key", default="staName", choices=("staName", "ssid"),
                      help="JSON key for the SSID; %(default)s is what real hardware accepts, "
                           "PROTOCOL.md documents 'ssid'")
    wifi.add_argument("--pwd-key", default="staPwd", choices=("staPwd", "pwd"),
                      help="JSON key for the passphrase (default %(default)s; some builds use 'pwd')")

    debug = parser.add_argument_group("debugging")
    debug.add_argument("--trace", metavar="FILE", help="append a JSONL trace of every datagram")
    debug.add_argument("--dry-run", action="store_true",
                       help="print every datagram, send nothing (give --port to skip discovery)")
    debug.add_argument("--show-secrets", action="store_true", help="do not redact passphrases")
    debug.add_argument("--quiet", "-q", action="store_true", help="less chatter")

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
        return rehome(chan, args)
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
