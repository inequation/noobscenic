# Proscenic M7 Pro firmware tool (`proscenic_fw.py`)

Pack / unpack / verify / sign the **CleanPack** application-layer firmware of the
Proscenic M7 Pro (LDRobot `LS_S6`, Rockchip RK3308). Container format and full
analysis: see `REPORT.md`.

    firmware.bin = [128-byte RSA-1024 PKCS#1v1.5 signature] + [gzip(GNU tar(rootfs))]
    signed value = ascii_hex( md5(body) )   (body = everything after the 128-byte header)

Requires Python 3 + `cryptography` (already in this dev shell).

## Commands

```sh
# Structure + signature status of an image
python3 proscenic_fw.py info   firmware.bin

# Verify the RSA-1024/MD5 signature against the vendor public key (embedded)
python3 proscenic_fw.py verify firmware.bin
#   -> SIGNATURE: VALID  (RSA-1024/MD5 ok; md5=142afe5db6be8d1087c96bd58a87bfe0)

# Verify + extract the rootfs overlay
python3 proscenic_fw.py unpack firmware.bin out/            # refuses if signature invalid
python3 proscenic_fw.py unpack firmware.bin out/ --force    # extract anyway (analysis)

# Build a NEW image from a rootfs directory.
#  - Without --key: writes a 128 zero-byte placeholder signature; the device WILL
#    REJECT it (RsaDec fail). Useful only for format testing.
#  - With --key (RSA-1024 private PEM): produces a properly signed image that a device
#    trusting that public key will accept.
python3 proscenic_fw.py pack   out/ new_firmware.bin
python3 proscenic_fw.py pack   out/ new_firmware.bin --key my_priv.pem

# Generate an RSA-1024 test keypair (to pair with a re-trusted `updater`)
python3 proscenic_fw.py genkey my_priv.pem my_pub.pem
```

## Signing reality check
The device’s only firmware trust anchor is the RSA-1024 **public** key embedded in
`bin/updater` (saved here as `vendor_public_key.pem`). The matching **private** key
lives only on the vendor build server, so a stock device accepts only vendor-signed
images. To run self-signed images you must first give the device your public key —
e.g. get root (UART / `adbd` / `dropbear`, see `REPORT.md` §5) and replace the
embedded key in `updater`, then sign with `pack --key`.

## Keeping the vacuum alive without signing anything
The cleaner route is to re-home the device to your own cloud (`setUrl` + `setSta`,
or DNS override) and re-implement the `cleanPack/*` endpoints — no firmware change,
no crypto break. See `REPORT.md` §6, and `eid_catalog.tsv` for the command map.
