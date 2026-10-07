//! Map uploads (`infoType` 20002) — doc/PLAN.md §11, MAP.md.
//!
//! The robot sends `{"infoType":20002,"data":{…}}` inside an HTTP
//! `cleanPack/uploadEvents` body. `map` is `base64(LZ4_compress_block(grid))`, one
//! byte per cell, row-major, `width × height` cells. Grids are stored exactly as
//! received (compressed) and decoded here on demand; the LZ4 block decoder is local
//! because the plan's `lz4_flex` dependency is not available offline.

use base64::Engine as _;
use serde::Deserialize;
use serde_json::Value;

use crate::error::{Error, Result};

/// The wire template of PROTOCOL.md §A / MAP.md, parsed leniently.
#[derive(Debug, Clone, Deserialize)]
pub struct MapUpload {
    #[serde(rename = "SN")]
    pub sn: Option<String>,
    #[serde(rename = "mapId")]
    pub map_id: Option<i64>,
    #[serde(rename = "autoAreaId")]
    pub auto_area_id: Option<i64>,
    #[serde(rename = "pathId")]
    pub path_id: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub resolution: Option<f64>,
    pub x_min: Option<f64>,
    pub y_min: Option<f64>,
    pub lz4_len: Option<i64>,
    #[serde(default)]
    pub area: Vec<Value>,
    pub map: Option<String>,
    #[serde(rename = "chargeHandlePos", default)]
    pub charge_handle_pos: Option<Vec<i64>>,
    #[serde(rename = "chargeHandlePhi")]
    pub charge_handle_phi: Option<i64>,
    #[serde(rename = "chargeHandleState")]
    pub charge_handle_state: Option<String>,
}

pub fn parse(data: &Value) -> Result<MapUpload> {
    Ok(serde_json::from_value(data.clone())?)
}

/// A decoded occupancy grid plus the histogram `map.rs` promises to settle the cell
/// semantics from one real upload.
#[derive(Debug)]
pub struct DecodedMap {
    pub cells: Vec<u8>,
    pub histogram: [u32; 256],
}

impl DecodedMap {
    /// `0x00` walls, `0x7F` unknown and `0xFF` free are the documented values;
    /// anything else is a room label written over free floor (FUNC_MAP.md §2.3).
    pub fn summary(&self) -> String {
        let wall = self.histogram[0x00];
        let unknown = self.histogram[0x7F];
        let free = self.histogram[0xFF];
        let labels = self.cells.len() as u32 - wall - unknown - free;
        format!(
            "{} cells: {wall} wall, {unknown} unknown, {free} free, {labels} labelled",
            self.cells.len()
        )
    }
}

/// Decode the grid: base64 → LZ4 block → exactly `width * height` cell bytes.
pub fn decode(upload: &MapUpload) -> Result<DecodedMap> {
    let width = upload.width.unwrap_or(0).max(0) as usize;
    let height = upload.height.unwrap_or(0).max(0) as usize;
    let expected = width
        .checked_mul(height)
        .ok_or_else(|| Error::Lz4("map too large".into()))?;
    if expected == 0 {
        return Err(Error::Lz4("map has no dimensions".into()));
    }
    let encoded = upload.map.as_deref().unwrap_or_default();
    let compressed = base64::engine::general_purpose::STANDARD.decode(encoded)?;
    let cells = lz4_block::decompress(&compressed, expected)?;
    if cells.len() != expected {
        return Err(Error::Lz4(format!(
            "decoded {} bytes, expected {expected} ({width}x{height})",
            cells.len()
        )));
    }
    let mut histogram = [0u32; 256];
    for cell in &cells {
        histogram[*cell as usize] += 1;
    }
    Ok(DecodedMap { cells, histogram })
}

/// Raw LZ4 **block** decompression (`LZ4_decompress_safe` equivalent) for the
/// subset the robot's compressor emits. The block format is a sequence of
/// `token, literals, offset, match` runs; lengths ≥ 15 extend in 255-byte steps.
pub mod lz4_block {
    use crate::error::{Error, Result};

    pub fn decompress(src: &[u8], expected: usize) -> Result<Vec<u8>> {
        let mut out: Vec<u8> = Vec::with_capacity(expected);
        let mut pos = 0usize;

        while pos < src.len() {
            let token = src[pos];
            pos += 1;

            let mut literals = (token >> 4) as usize;
            if literals == 15 {
                loop {
                    let byte = *src
                        .get(pos)
                        .ok_or_else(|| Error::Lz4("truncated literals".into()))?;
                    pos += 1;
                    literals += byte as usize;
                    if byte != 255 {
                        break;
                    }
                }
            }
            let end = pos
                .checked_add(literals)
                .filter(|end| *end <= src.len())
                .ok_or_else(|| Error::Lz4("literal run past the end".into()))?;
            out.extend_from_slice(&src[pos..end]);
            pos = end;
            if out.len() > expected {
                return Err(Error::Lz4("output larger than expected".into()));
            }
            if pos == src.len() {
                break; // the last sequence carries literals only
            }

            let offset = u16::from_le_bytes([
                *src.get(pos).ok_or_else(truncated)?,
                *src.get(pos + 1).ok_or_else(truncated)?,
            ]) as usize;
            pos += 2;
            if offset == 0 || offset > out.len() {
                return Err(Error::Lz4(format!("invalid match offset {offset}")));
            }

            let mut length = (token & 0x0F) as usize;
            if length == 15 {
                loop {
                    let byte = *src
                        .get(pos)
                        .ok_or_else(|| Error::Lz4("truncated match".into()))?;
                    pos += 1;
                    length += byte as usize;
                    if byte != 255 {
                        break;
                    }
                }
            }
            length += 4;
            if out.len() + length > expected {
                return Err(Error::Lz4("match run past the expected length".into()));
            }
            for _ in 0..length {
                // Byte-by-byte: overlapping matches (offset < length) are legal.
                let byte = out[out.len() - offset];
                out.push(byte);
            }
        }
        Ok(out)
    }

    fn truncated() -> Error {
        Error::Lz4("truncated match offset".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_literal_only_block_decodes() {
        // token: 5 literals, no match; the block ends after the literals.
        let mut block = vec![0x50];
        block.extend_from_slice(b"hello");
        assert_eq!(lz4_block::decompress(&block, 5).unwrap(), b"hello");
    }

    #[test]
    fn an_overlapping_match_decodes() {
        // 2 literals ("ab"), then a 4-byte match at offset 2 → "ababab".
        let block = [0x20, b'a', b'b', 0x02, 0x00];
        assert_eq!(lz4_block::decompress(&block, 6).unwrap(), b"ababab");
    }

    #[test]
    fn extended_lengths_decode() {
        // The map compressor's favourite shape: one literal, then a long run of it.
        let total = 18404usize;
        let mut block = vec![0x1F, 0x7F, 0x01, 0x00];
        // The extended match length counts from the 15 + 4 minimum.
        let mut remaining = total - 1 - 19;
        while remaining >= 255 {
            block.push(255);
            remaining -= 255;
        }
        block.push(remaining as u8);
        let cells = lz4_block::decompress(&block, total).unwrap();
        assert_eq!(cells.len(), total);
        assert!(cells.iter().all(|cell| *cell == 0x7F));
    }

    #[test]
    fn malformed_blocks_are_rejected() {
        assert!(
            lz4_block::decompress(&[0x10], 16).is_err(),
            "truncated literals"
        );
        assert!(
            lz4_block::decompress(&[0x00, 0x00, 0x00], 16).is_err(),
            "offset 0"
        );
        assert!(
            lz4_block::decompress(&[0x10, 0xAA, 0x05, 0x00], 4).is_err(),
            "offset too far"
        );
        assert!(
            lz4_block::decompress(&[0x50, b'h', b'e', b'l', b'l', b'o'], 3).is_err(),
            "more output than expected"
        );
    }

    #[test]
    fn a_map_upload_decodes_with_a_histogram() {
        // 4x4 grid of one byte value: one literal, then a 15-byte match at offset 1.
        let block = vec![0x1B, 0x7F, 0x01, 0x00];
        let upload = parse(&json!({
            "SN": "TEST", "mapId": 1, "pathId": 2, "width": 4, "height": 4,
            "resolution": 0.05, "x_min": -1.0, "y_min": -2.0, "lz4_len": block.len(),
            "map": base64::engine::general_purpose::STANDARD.encode(&block),
            "area": [],
        }))
        .unwrap();
        assert_eq!(upload.width, Some(4));
        let decoded = decode(&upload).unwrap();
        assert_eq!(decoded.cells.len(), 16);
        assert_eq!(decoded.histogram[0x7F], 16);
        assert!(decoded.summary().contains("16 cells"));
    }
}
