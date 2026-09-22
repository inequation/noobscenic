#!/usr/bin/env python3
"""
extract_libmono.py — statically extract the Jiagu stage-2 engine (libmono.so)
from libjiagu_a64.so, by EMULATING the packer's own key schedule + RC4 (Unicorn)
and then zlib-inflating. Reverse-engineering write-up: JIAGU_INTERNALS.md.

Chain (all proven): .bmp (a real BMP image steganographically holding the RC4 key
material) -> native key schedule -> RC4 S-box -> RC4-decrypt .mips
-> [u32 uncompressed length | zlib stream] -> inflate -> custom core container
-> the embedded libmono.so ELF (carved at its 0x7fELF magic).

Requires: unicorn (`pip install unicorn` / `uv run --with unicorn`).
Usage: python3 extract_libmono.py libjiagu_a64.so [out.so]
NOTE: offsets below are for THIS build's libjiagu_a64.so; the code auto-locates
the `.bmp`/`.mips` sections and the key-schedule function by ELF metadata where
possible, but the FUN_00104ef4 entry (0x4ef4) is build-specific.
"""
import sys, struct, zlib
from unicorn import *
from unicorn.arm64_const import *

KSA_SETUP_VADDR = 0x4ef4   # FUN_00104ef4(this, sbox) — build-specific
def sect(d, name):
    e_shoff=struct.unpack_from('<Q',d,0x28)[0]; sz=struct.unpack_from('<H',d,0x3a)[0]
    n=struct.unpack_from('<H',d,0x3c)[0]; stx=struct.unpack_from('<H',d,0x3e)[0]
    stro=struct.unpack_from('<Q',d,e_shoff+stx*sz+0x18)[0]
    for i in range(n):
        o=e_shoff+i*sz; nmo=struct.unpack_from('<I',d,o)[0]
        nm=d[stro+nmo:d.index(b'\0',stro+nmo)].decode('latin1')
        if nm==name: return (struct.unpack_from('<Q',d,o+0x10)[0],  # vaddr
                              struct.unpack_from('<Q',d,o+0x18)[0],  # file off
                              struct.unpack_from('<Q',d,o+0x20)[0])  # size
    return None

def extract(path, out='libmono_extracted.so'):
    d=open(path,'rb').read(); BASE=0x100000
    bmp=sect(d,'.bmp'); mips=sect(d,'.mips')
    if not bmp or not mips: sys.exit("[-] .bmp/.mips sections not found — not this Jiagu variant")
    uc=Uc(UC_ARCH_ARM64, UC_MODE_ARM); uc.mem_map(BASE,0x200000,UC_PROT_ALL)
    # map PT_LOAD by vaddr (the 0x10000 data-segment skew is the whole trick)
    pho=struct.unpack_from('<Q',d,0x20)[0]; phe=struct.unpack_from('<H',d,0x36)[0]; phn=struct.unpack_from('<H',d,0x38)[0]
    for i in range(phn):
        o=pho+i*phe
        if struct.unpack_from('<I',d,o)[0]==1:
            fo=struct.unpack_from('<Q',d,o+8)[0]; va=struct.unpack_from('<Q',d,o+0x10)[0]; fsz=struct.unpack_from('<Q',d,o+0x20)[0]
            uc.mem_write(BASE+va, d[fo:fo+fsz])
    # R_AARCH64_RELATIVE
    for i in range(struct.unpack_from('<H',d,0x3c)[0]):
        so=struct.unpack_from('<Q',d,0x28)[0]+i*struct.unpack_from('<H',d,0x3a)[0]
        if struct.unpack_from('<I',d,so+4)[0]==4:
            ro=struct.unpack_from('<Q',d,so+0x18)[0]; rs=struct.unpack_from('<Q',d,so+0x20)[0]
            for r in range(ro,ro+rs,24):
                roff,rinfo,radd=struct.unpack_from('<QQq',d,r)
                if (rinfo&0xffffffff)==1027: uc.mem_write(BASE+roff,struct.pack('<Q',BASE+radd))
    HEAP=0x10000000; uc.mem_map(HEAP,0x8000000,UC_PROT_ALL); hp=[HEAP+0x1000]
    STK=0x40000000; uc.mem_map(STK,0x400000,UC_PROT_ALL)
    GU=0x50000000; uc.mem_map(GU,0x10000,UC_PROT_ALL); uc.mem_write(GU,b'\xAA'*8)
    uc.mem_write(BASE+0x19fc0,struct.pack('<Q',GU))
    def alloc(n):
        n=n if 0<n<=0x4000000 else 0x1000; p=(hp[0]+15)&~15; hp[0]=p+((n+15)&~15)+32
        uc.mem_write(p,b'\x00'*max(n,16)); return p
    PLT={BASE+0x1200:'new',BASE+0x13c0:'new[]',BASE+0x12b0:'calloc',BASE+0x1230:'memcpy',
         BASE+0x1380:'memset',BASE+0x1270:'del',BASE+0x1390:'del[]',BASE+0x1420:'free',BASE+0x1290:'stk'}
    def hook(uc,addr,size,ud):
        if addr in PLT:
            nm=PLT[addr]; lr=uc.reg_read(UC_ARM64_REG_LR)
            x0=uc.reg_read(UC_ARM64_REG_X0);x1=uc.reg_read(UC_ARM64_REG_X1);x2=uc.reg_read(UC_ARM64_REG_X2)
            if nm in('new','new[]'): uc.reg_write(UC_ARM64_REG_X0,alloc(x0))
            elif nm=='calloc': uc.reg_write(UC_ARM64_REG_X0,alloc(x0*x1))
            elif nm=='memcpy': uc.mem_write(x0,bytes(uc.mem_read(x1,x2))); uc.reg_write(UC_ARM64_REG_X0,x0)
            elif nm=='memset': uc.mem_write(x0,bytes([x1&0xff])*x2); uc.reg_write(UC_ARM64_REG_X0,x0)
            elif nm=='stk': uc.emu_stop(); return
            uc.reg_write(UC_ARM64_REG_PC,lr)
    uc.hook_add(UC_HOOK_CODE,hook)
    this=alloc(0x40); kb=alloc(0x120); RET=0x9999990
    uc.mem_write(this+0x28,struct.pack('<Q',BASE+bmp[0])); uc.mem_write(this+0x30,struct.pack('<I',bmp[2]-2))
    uc.reg_write(UC_ARM64_REG_SP,STK+0x200000); uc.reg_write(UC_ARM64_REG_X0,this)
    uc.reg_write(UC_ARM64_REG_X1,kb); uc.reg_write(UC_ARM64_REG_LR,RET)
    uc.emu_start(BASE+KSA_SETUP_VADDR, RET, count=50_000_000)
    S=bytearray(uc.mem_read(kb,256)); i=uc.mem_read(kb,0x102)[0x100]; j=uc.mem_read(kb,0x102)[0x101]
    core=bytearray(d[mips[1]:mips[1]+mips[2]-4])   # section size padded; RC4 whole then trim by len
    core=bytearray(d[mips[1]:mips[1]+0x85e6c])
    for n in range(len(core)):
        i=(i+1)&0xff; a=S[i]; j=(j+a)&0xff; S[i]=S[j]; S[j]=a; core[n]^=S[(a+S[i])&0xff]
    ln=struct.unpack_from('<I',core,0)[0]; inflated=zlib.decompress(bytes(core[4:]))
    assert len(inflated)==ln, (len(inflated),ln)
    off=inflated.find(b'\x7fELF'); assert off>=0, "no ELF in core container"
    e=inflated[off:]
    end=struct.unpack_from('<Q',e,0x28)[0]+struct.unpack_from('<H',e,0x3c)[0]*struct.unpack_from('<H',e,0x3a)[0]
    open(out,'wb').write(e[:end])
    print(f"[+] extracted {out}: {end} bytes, ELF machine={e[18]} (183=AArch64)")
    print("[i] Next layer: libmono decrypts the encrypted TAIL of classes.dex (offset ~0xb000,")
    print("    4.66 MB) into the real app DEX via InMemoryDexClassLoader — reverse that in libmono.")

if __name__=='__main__':
    extract(sys.argv[1], sys.argv[2] if len(sys.argv)>2 else 'libmono_extracted.so')
