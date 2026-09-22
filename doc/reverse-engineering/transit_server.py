#!/usr/bin/env python3
"""
CHANNEL A (app <-> cloud IM) mock server -- NOT the rehome server.

WARNING: the ROBOT does NOT speak this 20-byte imsocket protocol. This is the
phone-app <-> cloud channel (bl-im.robotbona.com:20008). To talk to the robot for a
rehome, use rehome_server.py (Channel B: '#\\t#'-framed {"infoType":..} JSON). See
PROTOCOL.md section B and WIRE_PROTOCOL.md. This file is only useful for experimenting
with the app side.

Wire format (little-endian), from RE of com.baole.blap imsocket:
  20-byte header: u32 total_len | u16 cmd | u16 type(=200) | u16 flag(=0)
                  u16 seq16 | u32 seq32(id) | u32 ext(=0)
  body: UTF-8 JSON, len = total_len - 20

Usage:
  1) point the robot here:  setUrl {"ip":"<this host>","port":20008}  (writes ip_port.json)
  2) run:  python3 transit_server.py            # logs the robot's real LOGIN/heartbeat
  3) reboot robot (or root: killall network_proxy) to force reconnect
Start as a pure logger, read the robot's actual frames, then implement the replies
your rehome needs. See WIRE_PROTOCOL.md for the command/infoType tables.
"""
import socket, struct, json, threading, sys

HDR = 20
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 20008

def parse_frames(buf):
    out = []
    while len(buf) >= 4:
        total = struct.unpack_from('<I', buf, 0)[0]
        if total < HDR or len(buf) < total:
            break
        cmd   = struct.unpack_from('<H', buf, 4)[0]
        seq32 = struct.unpack_from('<I', buf, 12)[0]
        out.append((cmd, seq32, buf[HDR:total]))
        buf = buf[total:]
    return out, buf

def frame(cmd, seq32, body_obj, seq16=0):
    body = json.dumps(body_obj, separators=(',', ':')).encode() if body_obj is not None else b''
    total = HDR + len(body)
    return struct.pack('<IHHHHII', total, cmd, 200, 0, seq16 & 0xffff, seq32, 0) + body

def handle(conn, addr):
    print('[+] conn from', addr, flush=True)
    buf = b''
    try:
        while True:
            data = conn.recv(65536)
            if not data:
                break
            buf += data
            frames, buf = parse_frames(buf)
            for cmd, seq32, body in frames:
                try:
                    obj = json.loads(body or b'{}')
                except Exception:
                    obj = body
                print(f'    RX cmd={cmd} seq={seq32} body={obj!r}', flush=True)
                if cmd == 16:                    # LOGIN -> ack 17
                    conn.sendall(frame(17, seq32, {"code": 0, "message": "ok"}))
                elif cmd == 250:                 # TRANSIT relay -> ack
                    conn.sendall(frame(251, seq32, {"code": 0}))
                else:                            # generic ack, echo seq
                    conn.sendall(frame(cmd + 1, seq32, {"code": 0}))
    except Exception as e:
        print('[!]', addr, e, flush=True)
    finally:
        conn.close()
        print('[-] closed', addr, flush=True)

def main():
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(('0.0.0.0', PORT))
    srv.listen(8)
    print(f'transit server listening on :{PORT}', flush=True)
    while True:
        c, a = srv.accept()
        threading.Thread(target=handle, args=(c, a), daemon=True).start()

if __name__ == '__main__':
    main()
