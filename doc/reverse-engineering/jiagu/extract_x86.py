import struct, zlib, sys
from unicorn import *
from unicorn.x86_const import *

LIB="assets/libjiagu_x86.so"
d=open(LIB,'rb').read()
BASE=0x10000
uc=Uc(UC_ARCH_X86, UC_MODE_32)
uc.mem_map(BASE, 0x200000, UC_PROT_ALL)   # covers code+data (~0x78000)
# map PT_LOAD by vaddr
pho=struct.unpack_from('<I',d,0x1c)[0]; phe=struct.unpack_from('<H',d,0x2a)[0]; phn=struct.unpack_from('<H',d,0x2c)[0]
loads=[]
for i in range(phn):
    o=pho+i*phe
    if struct.unpack_from('<I',d,o)[0]==1:  # PT_LOAD
        fo=struct.unpack_from('<I',d,o+4)[0]; va=struct.unpack_from('<I',d,o+8)[0]; fsz=struct.unpack_from('<I',d,o+0x10)[0]
        uc.mem_write(BASE+va, d[fo:fo+fsz]); loads.append((fo,va,fsz))
print("mapped LOADs:", [(hex(a),hex(b),hex(c)) for a,b,c in loads])
# R_386_RELATIVE (type 8): *(BASE+off) += BASE
e_shoff=struct.unpack_from('<I',d,0x20)[0]; shent=struct.unpack_from('<H',d,0x2e)[0]; shnum=struct.unpack_from('<H',d,0x30)[0]
def sections():
    for i in range(shnum):
        o=e_shoff+i*shent
        yield (struct.unpack_from('<I',d,o+4)[0],   # type
               struct.unpack_from('<I',d,o+0x10)[0],# offset
               struct.unpack_from('<I',d,o+0x14)[0],# size
               struct.unpack_from('<I',d,o+0x18)[0])# entsize... (sh_link at +0x18 actually) - not needed
relcnt=0
for i in range(shnum):
    o=e_shoff+i*shent
    stype=struct.unpack_from('<I',d,o+4)[0]
    if stype==9:  # SHT_REL
        roff=struct.unpack_from('<I',d,o+0x10)[0]; rsz=struct.unpack_from('<I',d,o+0x14)[0]
        for r in range(roff,roff+rsz,8):
            r_off,r_info=struct.unpack_from('<II',d,r)
            if (r_info & 0xff)==8:  # R_386_RELATIVE
                cur=struct.unpack_from('<I', bytes(uc.mem_read(BASE+r_off,4)),0)[0]
                uc.mem_write(BASE+r_off, struct.pack('<I',(cur+BASE)&0xffffffff)); relcnt+=1
print("applied R_386_RELATIVE:", relcnt)

HEAP=0x2000000; uc.mem_map(HEAP,0x4000000,UC_PROT_ALL); hp=[HEAP+0x1000]
STK=0x7000000; uc.mem_map(STK,0x200000,UC_PROT_ALL)
def alloc(n):
    n=n if 0<n<=0x2000000 else 0x1000
    p=(hp[0]+15)&~15; hp[0]=p+((n+15)&~15)+64; uc.mem_write(p,b'\x00'*max(n,16)); return p
PLT={0x10ee0:'new',0x10f90:'new[]',0x10eb0:'calloc',0x10f50:'malloc',
     0x10f70:'del',0x10fa0:'del[]',0x10f60:'free'}
RET=0x900000
def hook(uc,addr,size,ud):
    if addr in PLT:
        nm=PLT[addr]; sp=uc.reg_read(UC_X86_REG_ESP)
        a1=struct.unpack('<I',bytes(uc.mem_read(sp+4,4)))[0]
        a2=struct.unpack('<I',bytes(uc.mem_read(sp+8,4)))[0]
        if nm in('new','new[]','malloc'): uc.reg_write(UC_X86_REG_EAX, alloc(a1))
        elif nm=='calloc': uc.reg_write(UC_X86_REG_EAX, alloc(a1*a2))
        else: uc.reg_write(UC_X86_REG_EAX,0)   # free/delete -> nop
        ret=struct.unpack('<I',bytes(uc.mem_read(sp,4)))[0]
        uc.reg_write(UC_X86_REG_ESP, sp+4); uc.reg_write(UC_X86_REG_EIP, ret)
uc.hook_add(UC_HOOK_CODE, hook)

# .bmp / .mips vaddrs
BMP_VA=0x13470; BMP_SZ=0x426; MIPS_FO=0x12898; MIPS_SZ=0x6412e
p1=alloc(0x40)
uc.mem_write(p1+0x14, struct.pack('<I',BASE+BMP_VA))
uc.mem_write(p1+0x18, struct.pack('<I',BMP_SZ))
sbox=alloc(0x110)
sp=STK+0x100000
uc.mem_write(sp, struct.pack('<III', RET, p1, sbox))
uc.reg_write(UC_X86_REG_ESP, sp)
try:
    uc.emu_start(BASE+0x522c, RET, count=200_000_000)
except UcError as e:
    print("emu stopped:", e, "eip=",hex(uc.reg_read(UC_X86_REG_EIP)))
S=bytes(uc.mem_read(sbox,0x102))
print("sbox[:16]=",S[:16].hex(),"i=",S[0x100],"j=",S[0x101],"is_perm=",sorted(S[:256])==list(range(256)))
# RC4 decrypt .mips
mips=bytearray(d[MIPS_FO:MIPS_FO+MIPS_SZ])
Sb=bytearray(S[:256]); i=S[0x100]; j=S[0x101]
out=bytearray(len(mips))
for n in range(len(mips)):
    i=(i+1)&0xff; a=Sb[i]; j=(j+a)&0xff; Sb[i]=Sb[j]; Sb[j]=a; out[n]=mips[n]^Sb[(a+Sb[i])&0xff]
ln=struct.unpack_from('<I',out,0)[0]
print("decrypted .mips[:8]=",out[:8].hex(),"declared_len=",ln)
try:
    cont=zlib.decompress(bytes(out[4:]))
    print("inflated container:", len(cont), "match=",len(cont)==ln, "ELF@",cont.find(b'\x7fELF'))
    off=cont.find(b'\x7fELF')
    open("libmono_x86_container.bin","wb").write(cont)
    if off>=0:
        e=cont[off:]
        # carve to end of section headers
        e_shoff=struct.unpack_from('<I',e,0x20)[0]; she=struct.unpack_from('<H',e,0x2e)[0]; shn=struct.unpack_from('<H',e,0x30)[0]
        end=e_shoff+she*shn
        open("libmono_x86_clean.so","wb").write(e[:end])
        print("carved libmono_x86_clean.so:", end, "bytes; machine=", struct.unpack_from('<H',e,0x12)[0], "(3=x86)")
    # save keystream (from sbox, run PRGA over zeros)
    Sb2=bytearray(S[:256]); i2=S[0x100]; j2=S[0x101]; ks=bytearray(MIPS_SZ)
    for n in range(MIPS_SZ):
        i2=(i2+1)&0xff; a=Sb2[i2]; j2=(j2+a)&0xff; Sb2[i2]=Sb2[j2]; Sb2[j2]=a; ks[n]=Sb2[(a+Sb2[i2])&0xff]
    open("mips_keystream.bin","wb").write(ks); print("saved keystream", len(ks))
except Exception as ex:
    print("zlib failed:", ex)
    open("mips_decrypted_raw.bin","wb").write(bytes(out))
