#!/usr/bin/env python3
"""
Channel-B rehome server for the Proscenic M7 Pro (the protocol the ROBOT speaks).

From RE of firmware `network_proxy` (see PROTOCOL.md section B):
  * Transport: raw TCP to the ip:port in /data/bin/Run/Config/ip_port.json
    (point the robot here with:  setUrl {"ip":"<this host>","port":<port>} ).
  * Framing: each message is  <UTF-8 JSON>  followed by the 3-byte delimiter
    b'#\\t#' == b'\\x23\\x09\\x23'.  NO length prefix. The server MUST terminate
    every frame it sends with '#\\t#' too, or the robot's splitter never completes.
  * Message object: {"infoType":<int>, "data":{...}} (+ "message"/"reason"/"packId").
  * Device -> server (plaintext, never encrypted): 10001 handshake, 21006 ping,
    20002 map upload, 21011 clean-path, 21020 chunked pack.
  * Server -> device command: {"infoType":N,"encrypt":0,"data":{...}}  (encrypt MUST
    be an integer or the frame is DROPPED; encrypt:0 => data is a plaintext object,
    the simplest correct path; encrypt:1 => data is base64(AES-128-ECB(json,
    session[:16]), padding disabled)).
  * Keepalive: the robot sends {"infoType":21006,"data":{}} periodically and drops +
    reconnects if not ponged. Pong immediately.

Usage:
  1) setUrl {"ip":"<host running this>","port":20009}   (writes ip_port.json)
  2) python3 rehome_server.py 20009
  3) reboot robot (or: adb/root  killall network_proxy)
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

def send_command(conn, info_type: int, data: dict, encrypt=0, key16=None):
    """Send a cloud->device command. encrypt=0 keeps data plaintext (recommended)."""
    if encrypt == 1:
        assert key16, "encrypt=1 needs the 16-byte session key"
        payload = aes_ecb_encrypt_b64(json.dumps(data, separators=(',', ':')).encode(), key16)
        conn.sendall(frame({"infoType": info_type, "encrypt": 1, "data": payload}))
    else:
        conn.sendall(frame({"infoType": info_type, "encrypt": 0, "data": data}))

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
                    # ack (harmless); the robot marks online on connect
                    conn.sendall(frame({"infoType": 10001, "data": {"message": "ok"}}))
                elif it == 21006:                     # keepalive ping -> MUST pong
                    conn.sendall(frame({"infoType": 21006, "data": {}}))
                elif it in (20002, 21011, 21020):     # device->cloud data (plaintext)
                    conn.sendall(frame({"infoType": it, "data": {"message": "ok"}}))
                else:
                    conn.sendall(frame({"infoType": it, "data": {"message": "ok"}}))
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
