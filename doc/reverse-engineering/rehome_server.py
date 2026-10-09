#!/usr/bin/env python3
"""
Channel-B rehome server for the Proscenic M7 Pro (the protocol the ROBOT speaks).

From RE of firmware `network_proxy` (see PROTOCOL.md section B):
  * Transport: raw TCP to the ip:port in /data/bin/Run/Config/ip_port.json
    (point the robot here with:  setUrl {"ip":"<this host>","port":<port>} ).
  * Framing: each message is  <UTF-8 JSON>  followed by the 3-byte delimiter
    b'#\\t#' == b'\\x23\\x09\\x23'.  NO length prefix. The server MUST terminate
    every frame it sends with '#\\t#' too, or the robot's splitter never completes.
  * Message object (the thing the robot dispatches): {"infoType":<int>, "data":{...},
    "dInfo":{"ts":"<str>","userId":"<str>"}}.  dInfo is REQUIRED (string ts/userId) for
    any command whose handler should reply, or the reply is never POSTed.
  * Device -> server (plaintext, never encrypted): 10001 handshake, 21006 ping,
    20002 map upload, 21011 clean-path.  (21020 is NOT a device report: it is the
    server->robot remote-control command, no reply — FUNC_COMMANDS.md §2.1.)
  * Server -> device command — DOUBLE-NESTED envelope (corrected 2026-10-07; the robot
    dispatches the CONTENTS of the outer "data" — CHANNEL_B_INBOUND.md):
        {"encrypt":0,"data":{"infoType":N,"data":{...},"dInfo":{"ts":"..","userId":".."}}}
    (encrypt MUST be an integer or the frame is DROPPED; encrypt:0 => outer data is the
    plaintext message object, the simplest correct path; encrypt:1 => outer data is
    base64(AES-128-ECB(json-of-message-object, session[:16]), padding disabled)).
  * Keepalive: the robot sends {"infoType":21006,"data":{}} periodically and drops +
    reconnects if not ponged. Pong immediately with the enveloped form:
        {"encrypt":0,"data":{"infoType":21006,"data":{}}}
    (add "isExistConnect":true inside the inner data to also set the app-online flag —
    that is what enables status pushes and map uploads).

Usage (order matters — see PAIRING_LOG_ANALYSIS.md §8):
  1) start this server FIRST:  python3 rehome_server.py <port>
  2) setUrl {"ip":"<host running this>","port":<port>}   (writes ip_port.json; no
     reboot/restart needed — the robot's 1 s heartbeat applies it live)
  3) setID {"id":"<userId>","deviceSN":"<robot SN>"}     (arms the connect → robot dials here)
  Do NOT send bindOk: on this firmware it close()s the local UDP socket without stopping
  the listener and spins a ~85 forks/s log storm forever (PAIRING_LOG_ANALYSIS.md §5).
  Pairing ends only after this server pongs 21006 AND the robot's Channel-A preBind POST
  gets code:0 back (then the robot fires internal event EID 0x460).
This is a logging + keepalive skeleton; fill send_command() calls for control.
Command payloads (infoType N / data fields) -> see COMMANDS.md. If you use HTTP
registration instead of the ip_port.json shortcut, your /cleanPack/register response
must include data.session (>=16 bytes; session[:16] is the AES key) and data.cookies.
"""
import socket, struct, json, threading, sys, base64

DELIM = b'#\t#'                    # 0x23 0x09 0x23
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 20009

# Optional AES for encrypt:1 (only needed if you don't use encrypt:0).
# key = session[:16]; AES-128-ECB, padding disabled; plaintext space-padded to x16.
def aes_ecb_encrypt_b64(plaintext: bytes, key16: bytes) -> str:
    from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
    pt = plaintext + b' ' * ((16 - len(plaintext) % 16) % 16)
    enc = Cipher(algorithms.AES(key16), modes.ECB()).encryptor()
    return base64.b64encode(enc.update(pt) + enc.finalize()).decode()

def frame(obj: dict) -> bytes:
    return json.dumps(obj, separators=(',', ':')).encode() + DELIM

def send_command(conn, info_type: int, data: dict, encrypt=0, key16=None, dInfo=None):
    """Send a cloud->device command.

    Wire shape (double nesting): {"encrypt":e,"data":<MESSAGE>} with
    MESSAGE = {"infoType":N,"data":data,"dInfo":{"ts":..,"userId":..}}.
    dInfo needs STRING ts/userId whenever the handler should reply
    (FUN_00459100 refuses otherwise). encrypt=0 keeps the message plaintext
    (recommended); encrypt=1 AESes the MESSAGE object with key16 = session[:16].
    """
    if dInfo is None:
        import time
        dInfo = {"ts": str(int(time.time() * 1000)), "userId": "rehome"}
    message = {"infoType": info_type, "data": data, "dInfo": dInfo}
    if encrypt == 1:
        assert key16, "encrypt=1 needs the 16-byte session key"
        payload = aes_ecb_encrypt_b64(
            json.dumps(message, separators=(',', ':')).encode(), key16)
        conn.sendall(frame({"encrypt": 1, "data": payload}))
    else:
        conn.sendall(frame({"encrypt": 0, "data": message}))

def handle(conn, addr):
    print('[+] robot connected', addr, flush=True)
    buf = b''
    sn = None
    try:
        while True:
            chunk = conn.recv(65536)
            if not chunk:
                break
            buf += chunk
            while DELIM in buf:
                raw, buf = buf.split(DELIM, 1)
                if not raw.strip():
                    continue
                try:
                    msg = json.loads(raw)
                except Exception:
                    print('    RX non-json:', raw[:120], flush=True)
                    continue
                it = msg.get('infoType')
                print(f'    RX infoType={it} {json.dumps(msg)[:200]}', flush=True)
                if it == 10001:                       # handshake
                    sn = (msg.get('data') or {}).get('sn')
                    print('    handshake sn=', sn, flush=True)
                    # no ack needed: inbound 10001 has no handler on the robot
                elif it == 21006:                     # keepalive ping -> MUST pong
                    # enveloped pong; put {"isExistConnect":True} inside the inner
                    # "data" to set the app-online flag (enables pushes/uploads)
                    conn.sendall(frame({"encrypt": 0, "data": {
                        "infoType": 21006, "data": {}}}))
                else:
                    # Device->cloud data (20001/20002/21011 reports etc.). Replies to
                    # those belong on HTTP cleanPack/response, not this socket
                    # (PROTOCOL.md §B); this skeleton only logs them.
                    pass
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
    print(f'Channel-B rehome server listening on :{PORT}', flush=True)
    while True:
        c, a = srv.accept()
        threading.Thread(target=handle, args=(c, a), daemon=True).start()

if __name__ == '__main__':
    main()
