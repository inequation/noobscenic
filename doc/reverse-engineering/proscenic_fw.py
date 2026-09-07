#!/usr/bin/env python3
"""
proscenic_fw.py -- Pack / unpack / verify / sign tool for Proscenic M7 Pro
("CleanPack" application-layer firmware, LDRobot LS_S6 / Rockchip RK3308).

Firmware container format (reverse-engineered from bin/updater + libcpc.so
RsaEncDec::DecFile):

    +----------------------------------------------------------+
    | offset 0x00 .. 0x7F : 128-byte RSA-1024 signature block  |
    |    = RSA_private_encrypt(                                 |
    |          PKCS#1 v1.5 type-1 pad || ascii_hex(md5(body)),  |
    |          manufacturer_private_key)                        |
    | offset 0x80 .. EOF  : body = gzip( GNU tar( rootfs ) )    |
    +----------------------------------------------------------+

Verification (on device):
    m   = RSA_public_decrypt(signature, embedded_public_key)   # -> 32 bytes
    ok  = (m == ascii_hex(md5(body)).encode())                 # 32 ASCII chars

The device's sole trust anchor is the 1024-bit RSA public key embedded in the
`updater` binary (below). The matching PRIVATE key is held only by the vendor's
build server, so a forged image cannot be produced without it. `sign` therefore
needs a private key you supply (e.g. the vendor key if ever recovered, or your
own key if you have first re-flashed the device's `updater` to trust it).
"""
import sys, os, gzip, hashlib, io, argparse, subprocess, tarfile, tempfile

EMBEDDED_PUBKEY_PEM = b"""-----BEGIN PUBLIC KEY-----
MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQCoPHLqLSYudq4J5BIh0vPfIcDb
z2DPCdFwgrMwX9nGBCvjUtG1LjrsloCUcMcxzHJKz9xqZgoH/hfy2Ev3f4aQaiz1
VgdLxboQ2gVfwk1Oy9f104cQYnXfK8mo6utWH+K1VOvj+BnMB7YPpQSFuanpy4s5
XtY6nugvUzqUv/dETwIDAQAB
-----END PUBLIC KEY-----
"""

HEADER_LEN = 128  # RSA-1024 => 128-byte signature

def _load_pub():
    from cryptography.hazmat.primitives.serialization import load_pem_public_key
    return load_pem_public_key(EMBEDDED_PUBKEY_PEM)

def split(path):
    data = open(path, "rb").read()
    if len(data) < HEADER_LEN + 2:
        raise SystemExit("file too small")
    return data[:HEADER_LEN], data[HEADER_LEN:]

def ascii_md5(body: bytes) -> bytes:
    return hashlib.md5(body).hexdigest().encode()  # 32 ascii bytes

def rsa_recover(sig: bytes, pub) -> bytes:
    """Raw RSA^e mod n, then strip PKCS#1 v1.5 type-1 padding (00 01 FF..FF 00)."""
    n = pub.public_numbers().n
    e = pub.public_numbers().e
    m = pow(int.from_bytes(sig, "big"), e, n).to_bytes(HEADER_LEN, "big")
    if m[0:2] != b"\x00\x01":
        raise ValueError("bad PKCS#1 header (not 00 01): %s" % m[:2].hex())
    idx = m.find(b"\x00", 2)
    if idx < 0:
        raise ValueError("no 00 separator after padding")
    if not all(b == 0xFF for b in m[2:idx]):
        raise ValueError("padding is not all 0xFF")
    return m[idx+1:]

def cmd_info(args):
    hdr, body = split(args.file)
    print("file            :", args.file)
    print("total size      :", HEADER_LEN + len(body))
    print("signature (hex) :", hdr.hex())
    print("body size       :", len(body))
    print("body md5        :", hashlib.md5(body).hexdigest())
    print("body is gzip    :", body[:2] == b"\x1f\x8b")
    try:
        raw = gzip.decompress(body)
        print("decompressed    :", len(raw), "bytes; tar:", raw[257:262] == b"ustar")
    except Exception as ex:
        print("decompress      : FAILED (%s)" % ex)
    cmd_verify(args, quiet=True)

def cmd_verify(args, quiet=False):
    hdr, body = split(args.file)
    pub = _load_pub()
    want = ascii_md5(body)
    try:
        got = rsa_recover(hdr, pub)
    except ValueError as ex:
        print("SIGNATURE: INVALID (RSA/padding: %s)" % ex); return 1
    if got == want:
        print("SIGNATURE: VALID  (RSA-1024/MD5 ok; md5=%s)" % want.decode())
        return 0
    print("SIGNATURE: INVALID (recovered %r != md5 %r)" % (got, want.decode()))
    return 1

def cmd_unpack(args):
    hdr, body = split(args.file)
    if not args.no_verify:
        if cmd_verify(args, quiet=True) != 0 and not args.force:
            raise SystemExit("refusing to unpack: signature invalid (use --force)")
    os.makedirs(args.outdir, exist_ok=True)
    raw = gzip.decompress(body)
    with tarfile.open(fileobj=io.BytesIO(raw)) as tf:
        tf.extractall(args.outdir, filter="tar" if sys.version_info>=(3,12) else None)
    print("unpacked %d bytes of tar into %s" % (len(raw), args.outdir))

def build_body_from_dir(indir, mtime=None):
    """Recreate gzip(tar(dir)). Note: exact byte reproduction of a vendor image
    is gzip/tar-implementation dependent; the container is valid regardless."""
    buf = io.BytesIO()
    # GNU tar, sorted, leading ./ like the vendor image
    with tarfile.open(fileobj=buf, mode="w", format=tarfile.GNU_FORMAT) as tf:
        for root, dirs, files in os.walk(indir):
            dirs.sort()
            for name in sorted(files):
                full = os.path.join(root, name)
                arc = "./" + os.path.relpath(full, indir)
                tf.add(full, arcname=arc, recursive=False)
    tar_bytes = buf.getvalue()
    gz = io.BytesIO()
    with gzip.GzipFile(fileobj=gz, mode="wb", mtime=mtime or 0) as g:
        g.write(tar_bytes)
    return gz.getvalue()

def cmd_pack(args):
    if os.path.isdir(args.input):
        body = build_body_from_dir(args.input)
    else:  # already a .tar.gz / body
        body = open(args.input, "rb").read()
    md5hex = ascii_md5(body)  # 32 ascii bytes
    if args.key:
        from cryptography.hazmat.primitives.serialization import load_pem_private_key
        priv = load_pem_private_key(open(args.key, "rb").read(), password=None)
        # Device verifies with RSA_public_decrypt => we must RSA_private_ENCRYPT
        # (a.k.a. raw signature) with PKCS#1 v1.5 type-1 padding over the 32 ascii bytes.
        n = priv.private_numbers().public_numbers.n
        d = priv.private_numbers().d
        klen = (n.bit_length() + 7)//8
        if klen != HEADER_LEN:
            raise SystemExit("private key must be RSA-1024 (got %d bytes)" % klen)
        pad = b"\x00\x01" + b"\xff"*(klen-3-len(md5hex)) + b"\x00" + md5hex
        sig = pow(int.from_bytes(pad,"big"), d, n).to_bytes(klen,"big")
    else:
        sig = b"\x00"*HEADER_LEN
        print("WARNING: no --key given; writing 128 zero bytes as a placeholder "
              "signature. The device WILL REJECT this image (RsaDec fail).", file=sys.stderr)
    with open(args.out, "wb") as f:
        f.write(sig); f.write(body)
    print("wrote %s (%d bytes); body md5=%s; signed=%s"
          % (args.out, HEADER_LEN+len(body), md5hex.decode(), bool(args.key)))

def cmd_genkey(args):
    """Generate an RSA-1024 keypair (for testing / for use with a re-trusted updater)."""
    from cryptography.hazmat.primitives.asymmetric import rsa
    from cryptography.hazmat.primitives import serialization
    k = rsa.generate_private_key(public_exponent=65537, key_size=1024)
    open(args.priv,"wb").write(k.private_bytes(serialization.Encoding.PEM,
        serialization.PrivateFormat.TraditionalOpenSSL, serialization.NoEncryption()))
    open(args.pub,"wb").write(k.public_key().public_bytes(serialization.Encoding.PEM,
        serialization.PublicFormat.SubjectPublicKeyInfo))
    print("wrote", args.priv, "and", args.pub)

def main():
    ap = argparse.ArgumentParser(description="Proscenic M7 Pro CleanPack firmware tool")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("info");    p.add_argument("file");   p.set_defaults(fn=cmd_info)
    p = sub.add_parser("verify");  p.add_argument("file");   p.set_defaults(fn=cmd_verify)
    p = sub.add_parser("unpack");  p.add_argument("file");   p.add_argument("outdir")
    p.add_argument("--no-verify", action="store_true"); p.add_argument("--force", action="store_true")
    p.set_defaults(fn=cmd_unpack)
    p = sub.add_parser("pack");    p.add_argument("input", help="rootfs dir or a prebuilt .tar.gz body")
    p.add_argument("out"); p.add_argument("--key", help="RSA-1024 private key PEM to sign with")
    p.set_defaults(fn=cmd_pack)
    p = sub.add_parser("genkey");  p.add_argument("priv"); p.add_argument("pub"); p.set_defaults(fn=cmd_genkey)
    args = ap.parse_args()
    rc = args.fn(args)
    sys.exit(rc or 0)

if __name__ == "__main__":
    main()
