//! The map-export container (`NBMP`) — doc/PLAN.md §20.6.
//!
//! The restorable artifact the robot produces is its `.bkmap` (a tar.gz of
//! `LastRecord`); this format wraps that blob so a file the user keeps is
//! self-describing *and* self-checking: the header carries the robot serial so
//! importing someone else's map is caught, the blob length so a truncated file is
//! caught, and the md5 of the blob so a mangled copy is caught before it can reach the
//! robot — the firmware answers `code:0` even for a tar that fails to extract, which
//! would leave an empty map. Integers are little-endian.
//!
//! ```text
//! "NBMP" | version u32 | length u32 | md5[16] | sn bytes | 0x00 | blob…
//! ```
//!
//! `length` is the blob length in bytes; `md5` is over those bytes. The server still
//! recomputes the md5 of the exact bytes it serves for the `21025` frame, so what the
//! robot checks can never disagree with what we hand it.

use md5::{Digest as _, Md5};

pub const FOURCC: [u8; 4] = *b"NBMP";
pub const VERSION: u32 = 1;
const MD5_LEN: usize = 16;
const FIXED: usize = 4 + 4 + 4 + MD5_LEN;

#[derive(Debug)]
pub struct Export {
    pub sn: String,
    pub blob: Vec<u8>,
}

/// Wrap a `.bkmap` blob for download.
pub fn encode(sn: &str, blob: &[u8]) -> Vec<u8> {
    let length = u32::try_from(blob.len()).expect("a .bkmap over 4 GiB is not happening");
    let mut out = Vec::with_capacity(FIXED + sn.len() + 1 + blob.len());
    out.extend_from_slice(&FOURCC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(&Md5::digest(blob));
    out.extend_from_slice(sn.as_bytes());
    out.push(0);
    out.extend_from_slice(blob);
    out
}

/// Unwrap an uploaded export, refusing anything that is not exactly one intact
/// `NBMP` container (doc/PLAN.md §20.6). The error is meant for the operator.
pub fn decode(bytes: &[u8]) -> std::result::Result<Export, String> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return Err(
            "this looks like a bare .bkmap; import the .nbmap file the server exported".into(),
        );
    }
    if bytes.len() < FIXED + 1 {
        return Err("not an NBMP export: too short".into());
    }
    if bytes[..4] != FOURCC {
        return Err("not an NBMP export: the file does not start with NBMP".into());
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().expect("4 bytes"));
    if version != VERSION {
        return Err(format!(
            "this export is version {version}; this server speaks version {VERSION}"
        ));
    }
    let length = u32::from_le_bytes(bytes[8..12].try_into().expect("4 bytes")) as usize;
    let stored_md5 = &bytes[12..12 + MD5_LEN];
    let Some(nul) = bytes[FIXED..].iter().position(|byte| *byte == 0) else {
        return Err("the export header has no serial terminator".into());
    };
    let sn = String::from_utf8(bytes[FIXED..FIXED + nul].to_vec())
        .map_err(|_| "the export header serial is not UTF-8".to_string())?;
    if sn.is_empty() {
        return Err("the export header carries an empty serial".into());
    }
    let blob_start = FIXED + nul + 1;
    let blob = &bytes[blob_start..];
    if blob.len() != length {
        return Err(format!(
            "the export is truncated or padded: header says {length} bytes of map, the file carries {}",
            blob.len()
        ));
    }
    if !blob.starts_with(&[0x1f, 0x8b]) {
        return Err("the map payload is not a gzip stream".into());
    }
    if &Md5::digest(blob)[..] != stored_md5 {
        return Err(
            "the file is corrupt: the map's md5 does not match the header (truncated, \
             altered, or not the file the server exported)"
                .into(),
        );
    }
    Ok(Export {
        sn,
        blob: blob.to_vec(),
    })
}

/// Lowercase 32-hex md5 of the exact bytes the robot will download
/// (`BACKUP_MAP.md` §D2: case-sensitive, no trimming).
pub fn md5_hex(bytes: &[u8]) -> String {
    let digest = Md5::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob() -> Vec<u8> {
        let mut blob = vec![0x1f, 0x8b, 0x08, 0x00];
        blob.extend_from_slice(b"pretend tarball");
        blob
    }

    #[test]
    fn a_round_trip_keeps_the_blob_byte_identical() {
        let encoded = encode("LSLDSM7PRO20403551", &blob());
        assert_eq!(&encoded[..4], b"NBMP");
        assert_eq!(u32::from_le_bytes(encoded[4..8].try_into().unwrap()), 1);
        assert_eq!(
            &encoded[12..28],
            &Md5::digest(blob())[..],
            "the header carries the md5"
        );
        let decoded = decode(&encoded).expect("decodes");
        assert_eq!(decoded.sn, "LSLDSM7PRO20403551");
        assert_eq!(decoded.blob, blob());
    }

    #[test]
    fn truncation_and_padding_are_refused() {
        let encoded = encode("SN", &blob());
        let mut truncated = encoded.clone();
        truncated.pop();
        assert!(decode(&truncated).unwrap_err().contains("truncated"));
        let mut padded = encoded.clone();
        padded.push(0);
        assert!(decode(&padded).unwrap_err().contains("truncated or padded"));
    }

    #[test]
    fn a_mangled_blob_or_a_wrong_header_md5_is_refused() {
        let encoded = encode("SN", &blob());

        // One flipped byte inside the blob, length untouched: only the md5 catches it.
        let mut flipped = encoded.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 0xff;
        assert!(decode(&flipped).unwrap_err().contains("md5 does not match"));

        // A header whose md5 does not describe the payload is refused too.
        let mut wrong_header = encoded.clone();
        wrong_header[12] ^= 0xff;
        assert!(
            decode(&wrong_header)
                .unwrap_err()
                .contains("md5 does not match")
        );
    }

    #[test]
    fn foreign_or_bare_files_are_refused_with_a_useful_message() {
        assert!(decode(b"hello").unwrap_err().contains("too short"));
        assert!(
            decode(&[0x1f, 0x8b, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])
                .unwrap_err()
                .contains("bare .bkmap")
        );
        assert!(
            decode(b"XXXX\x01\x00\x00\x00\0\0\0\0")
                .unwrap_err()
                .contains("NBMP")
        );
    }

    #[test]
    fn md5_matches_the_known_vector() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    }
}
