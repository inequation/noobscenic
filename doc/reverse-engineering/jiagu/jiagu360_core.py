#!/usr/bin/env python3
"""
jiagu360_core.py — detector/handler for the Qihoo 360 "Jiagu" *runtime-core*
variant (the one used by com.proscenic.psnic), which SafaSafari/jiagu_unpacker
does NOT handle. Reverse-engineering write-up: JIAGU_INTERNALS.md.

What SafaSafari's tool assumes: classes.dex = [shell][AES-CBC blob][4-byte len].
What THIS variant actually does:
  * classes.dex is only a 12-class loader stub (no appended payload).
  * libjiagu_<abi>.so is a stage-1 loader that carries the real engine as an
    ENCRYPTED+ZLIB'd ELF in a camouflaged section (`.mips`), with key material in
    `.bmp`, and custom in-memory-loads it (never on disk).
  * core pipeline (proven): RC4-decrypt -> first 4 bytes = uncompressed length ->
    zlib inflate  ->  the engine ELF (libmono.so), which then decrypts the DEX.

This module:
  1) DETECTS the variant and explains the SafaSafari failure clearly.
  2) Carves the `.bmp` / `.mips` blobs from libjiagu_a64.so.
  3) Implements the exact core cipher (RC4 KSA/PRGA reproduced from the binary +
     zlib) so that GIVEN the RC4 key it emits libmono.so.
  4) Documents the one remaining gap (the `.bmp` key pre-decrypt) and how to close
     it (emulate the native key schedule — see emu_extract_core.py — or dump
     dynamically).
"""
import sys, struct, zlib, argparse, zipfile, io

def read_dex_class_defs(dex: bytes) -> int:
    return struct.unpack_from('<I', dex, 0x60)[0] if dex[:4]==b'dex\n' else -1

def elf_sections(so: bytes):
    """Return {name: (offset,size)} from an ELF64 section header table."""
    if so[:4]!=b'\x7fELF': return {}
    e_shoff=struct.unpack_from('<Q',so,0x28)[0]
    e_shentsize=struct.unpack_from('<H',so,0x3a)[0]
    e_shnum=struct.unpack_from('<H',so,0x3c)[0]
    e_shstrndx=struct.unpack_from('<H',so,0x3e)[0]
    def sh(i):
        o=e_shoff+i*e_shentsize
        return dict(name=struct.unpack_from('<I',so,o)[0],
                    off=struct.unpack_from('<Q',so,o+0x18)[0],
                    size=struct.unpack_from('<Q',so,o+0x20)[0])
    strtab=sh(e_shstrndx)['off']
    out={}
    for i in range(e_shnum):
        s=sh(i); end=so.find(b'\x00',strtab+s['name']); nm=so[strtab+s['name']:end].decode('latin1','replace')
        out[nm]=(s['off'],s['size'])
    return out

def rc4_ksa(key: bytes) -> bytearray:
    """RC4 KSA exactly as FUN_00104a50 (identity init + standard schedule)."""
    S=bytearray(range(256)); j=0
    for i in range(256):
        j=(j+S[i]+key[i%len(key)])&0xff
        S[i],S[j]=S[j],S[i]
    return S

def rc4_prga_decrypt(S: bytearray, data: bytes, i=0, j=0) -> bytes:
    """RC4 PRGA exactly as FUN_00104cb0 (out ^= S[(S[i]+S[j])&0xff])."""
    S=bytearray(S); out=bytearray(data)
    for n in range(len(out)):
        i=(i+1)&0xff; a=S[i]; j=(j+a)&0xff
        S[i]=S[j]; S[j]=a
        out[n]^=S[(a+S[i])&0xff]
    return bytes(out)

def core_decrypt(mips_blob: bytes, rc4_key: bytes) -> bytes:
    """Full core pipeline: RC4 -> u32 length -> zlib inflate. Returns libmono ELF."""
    dec=rc4_prga_decrypt(rc4_ksa(rc4_key), mips_blob)
    outlen=struct.unpack_from('<I',dec,0)[0]
    lib=zlib.decompress(dec[4:])
    if len(lib)!=outlen:
        print(f"[!] length mismatch: header={outlen} inflated={len(lib)}")
    return lib

def detect(apk_or_so: str):
    data=open(apk_or_so,'rb').read()
    so=None; dex=None
    if data[:2]==b'PK':
        z=zipfile.ZipFile(io.BytesIO(data))
        names=z.namelist()
        if 'classes.dex' in names: dex=z.read('classes.dex')
        for n in names:
            if n.startswith('assets/libjiagu') and n.endswith('_a64.so'): so=z.read(n)
        print(f"[*] APK: {'libjiagu*_a64.so' if so else 'NO libjiagu'} present; "
              f"stub classes.dex class_defs={read_dex_class_defs(dex) if dex else '?'}")
    elif data[:4]==b'\x7fELF':
        so=data
    if not so:
        print("[-] No libjiagu_a64.so found."); return
    secs=elf_sections(so)
    print("[*] libjiagu variant markers:",
          {k:secs[k] for k in ('.bmp','.mips') if k in secs} or "(.bmp/.mips not found — different variant)")
    if '.bmp' in secs and '.mips' in secs:
        bo,bl=secs['.bmp']; mo,ml=secs['.mips']
        open('bmp_keyblob.bin','wb').write(so[bo:bo+bl])
        open('mips_core.bin','wb').write(so[mo:mo+ml])
        print(f"[+] carved .bmp (key, {bl}B) and .mips (core, {ml}B) -> bmp_keyblob.bin / mips_core.bin")
        print("[*] Core pipeline is RC4->u32len->zlib (see JIAGU_INTERNALS.md).")
        print("[!] The RC4 key is derived from the ENCRYPTED .bmp; recover it by")
        print("    emulating the native key schedule (emu_extract_core.py), then call")
        print("    core_decrypt(mips, key) here to emit libmono.so. Or dump dynamically.")
    print("\n[=] Why SafaSafari/jiagu_unpacker fails on this APK:")
    print("    it reads classes.dex's last 4 bytes as a shell length (garbage here,")
    print("    ~439MB) and expects an appended AES-CBC blob. This variant keeps NO")
    print("    payload in classes.dex; the engine+DEX live encrypted inside libjiagu.")

if __name__=='__main__':
    ap=argparse.ArgumentParser(description="Detect/handle the Jiagu runtime-core variant")
    ap.add_argument('target', help='APK or libjiagu_a64.so')
    detect(ap.parse_args().target)


def decode_dex_tail_config(classes_dex_path):
    """The appended tail of the stub classes.dex begins with Jiagu's config manifest,
    XOR-0x52 encoded. Returns the decoded key/value strings (APPKEY, pkg, appName, ...)."""
    import re
    d=open(classes_dex_path,'rb').read()
    map_off=struct.unpack_from('<I',d,0x34)[0]
    stub_end=map_off+4+struct.unpack_from('<I',d,map_off)[0]*12
    region=bytes(b^0x52 for b in d[stub_end:stub_end+0x2000])
    return [m.group().decode() for m in re.finditer(rb'[\x20-\x7e]{4,}', region)]

if __name__ != '__main__':
    pass
