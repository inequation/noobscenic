//! Map uploads (`infoType` 20002) — doc/PLAN.md §11, MAP.md.
//!
//! The robot sends `{"infoType":20002,"data":{…}}` inside an HTTP
//! `cleanPack/uploadEvents` body. `map` is `base64(LZ4_compress_block(grid))`, one
//! byte per cell, row-major, `width × height` cells. Grids are stored exactly as
//! received (compressed) and decoded here on demand with `lz4_flex`'s block API.

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
    let cells = decompress_block(&compressed, expected)?;
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

/// LZ4 **block** decompression (`LZ4_decompress_safe` equivalent) with the output
/// bounded by the expected grid size — the device's compressor emits raw blocks
/// (`LZ4_compress_default`), no frame header and no size prefix.
fn decompress_block(src: &[u8], expected: usize) -> Result<Vec<u8>> {
    let mut cells = vec![0u8; expected];
    let written = lz4_flex::block::decompress_into(src, &mut cells)
        .map_err(|error| Error::Lz4(error.to_string()))?;
    cells.truncate(written);
    Ok(cells)
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
        assert_eq!(decompress_block(&block, 5).unwrap(), b"hello");
    }

    #[test]
    fn an_overlapping_match_decodes() {
        // 2 literals ("ab"), a 4-byte match at offset 2 → "ababab", then the
        // literal-only sequence every LZ4 block ends with (empty here).
        let block = [0x20, b'a', b'b', 0x02, 0x00, 0x00];
        assert_eq!(decompress_block(&block, 6).unwrap(), b"ababab");
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
        block.push(0x00); // the terminating literal-only sequence
        let cells = decompress_block(&block, total).unwrap();
        assert_eq!(cells.len(), total);
        assert!(cells.iter().all(|cell| *cell == 0x7F));
    }

    #[test]
    fn malformed_blocks_are_rejected() {
        assert!(decompress_block(&[0x10], 16).is_err(), "truncated literals");
        assert!(
            decompress_block(&[0x00, 0x00, 0x00], 16).is_err(),
            "offset 0"
        );
        assert!(
            decompress_block(&[0x10, 0xAA, 0x05, 0x00], 4).is_err(),
            "offset too far"
        );
        assert!(
            decompress_block(&[0x50, b'h', b'e', b'l', b'l', b'o'], 3).is_err(),
            "more output than expected"
        );
    }

    #[test]
    fn a_map_upload_decodes_with_a_histogram() {
        // 4x4 grid of one byte value: one literal, a 15-byte match at offset 1, and
        // the terminating literal-only sequence.
        let block = vec![0x1B, 0x7F, 0x01, 0x00, 0x00];
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
