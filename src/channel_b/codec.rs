//! Channel-B framing: `<UTF-8 JSON>` + `#\t#` (`23 09 23`), in both directions,
//! with no length prefix (doc/PLAN.md §10.1, PROTOCOL.md §B).
//!
//! The decoder is incremental because a read can end mid-frame, a delimiter can
//! straddle two reads, and several frames can arrive in one read. Decoding and
//! encoding live here so that no other module ever hand-rolls a frame.

use serde_json::Value;

/// The frame delimiter, verbatim from the firmware (`#` `\t` `#`).
pub const DELIMITER: &[u8; 3] = b"#\t#";

/// A decoded frame exceeded the configured limit and the connection must be dropped.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DecodeError {
    #[error("frame of {len} bytes exceeds the {max}-byte limit")]
    Oversize { len: usize, max: usize },
}

/// Wrap a frame body in the delimiter. `serde_json::to_vec` + these three bytes is
/// the *only* way channel-B frames are written.
pub fn encode(frame: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(frame.len() + DELIMITER.len());
    out.extend_from_slice(frame);
    out.extend_from_slice(DELIMITER);
    out
}

/// A message in the field order the firmware's own writer uses. `serde_json::Value`
/// maps are sorted alphabetically (`data, encrypt, infoType`), which is valid JSON
/// but not the shape the device emits or (possibly) expects; serialising a struct
/// keeps declaration order.
#[derive(serde::Serialize)]
struct Ordered<'a> {
    #[serde(rename = "infoType")]
    info_type: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    encrypt: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
    data: &'a Value,
}

/// Serialise a message and frame it.
///
/// Messages are written as `{"infoType":…,"encrypt":…,"data":…}` — firmware order —
/// and anything that does not look like one (no `infoType`/`data`) falls back to a
/// plain compact serialisation. `Value` is always serialisable, so this cannot fail.
pub fn encode_json(value: &Value) -> Vec<u8> {
    let body = match ordered(value) {
        Some(frame) => serde_json::to_vec(&frame),
        None => serde_json::to_vec(value),
    };
    encode(&body.expect("a JSON value always re-serialises"))
}

fn ordered(value: &Value) -> Option<Ordered<'_>> {
    let map = value.as_object()?;
    Some(Ordered {
        info_type: map.get("infoType")?.as_i64()?,
        encrypt: map.get("encrypt").and_then(Value::as_i64),
        message: map.get("message").and_then(Value::as_str),
        data: map.get("data")?,
    })
}

/// Incremental `#\t#` splitter. Push whatever the socket gave you; get back the
/// frames it completed (delimiter stripped).
pub struct Decoder {
    buf: Vec<u8>,
    max: usize,
}

impl Decoder {
    pub fn new(max_frame_bytes: usize) -> Decoder {
        Decoder {
            buf: Vec::new(),
            max: max_frame_bytes,
        }
    }

    /// Feed one read's worth of bytes; return every frame completed by it.
    ///
    /// A frame (or an unterminated run of bytes) longer than the limit is an error:
    /// the caller logs it and drops the connection rather than buffering forever.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Vec<u8>>, DecodeError> {
        self.buf.extend_from_slice(chunk);

        let mut frames = Vec::new();
        let mut start = 0;
        while let Some(at) = find_delimiter(&self.buf[start..]) {
            let end = start + at;
            if end > self.max {
                return Err(DecodeError::Oversize {
                    len: end,
                    max: self.max,
                });
            }
            frames.push(self.buf[start..end].to_vec());
            start = end + DELIMITER.len();
        }
        if start > 0 {
            self.buf.drain(..start);
        }
        if self.buf.len() > self.max {
            return Err(DecodeError::Oversize {
                len: self.buf.len(),
                max: self.max,
            });
        }
        Ok(frames)
    }
}

fn find_delimiter(buf: &[u8]) -> Option<usize> {
    buf.windows(DELIMITER.len())
        .position(|window| window == &DELIMITER[..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(frame: &[u8]) -> String {
        String::from_utf8(frame.to_vec()).unwrap()
    }

    #[test]
    fn encode_terminates_the_frame() {
        assert_eq!(encode(b"{}"), b"{}#\t#");
        assert_eq!(
            encode_json(&json!({"infoType": 21006})),
            b"{\"infoType\":21006}#\t#"
        );
    }

    #[test]
    fn frames_are_written_in_firmware_field_order() {
        // The device's own writer emits infoType first; serde_json's maps would sort
        // this alphabetically (data, encrypt, infoType).
        let frame = encode_json(&json!({"data": {"a": 1}, "encrypt": 0, "infoType": 21006}));
        assert_eq!(
            frame,
            b"{\"infoType\":21006,\"encrypt\":0,\"data\":{\"a\":1}}#\t#".to_vec()
        );
    }

    #[test]
    fn one_frame_is_decoded_even_when_it_arrives_whole() {
        let mut decoder = Decoder::new(1024);
        let frames = decoder.push(b"{\"a\":1}#\t#").unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(text(&frames[0]), "{\"a\":1}");
    }

    #[test]
    fn back_to_back_frames_are_all_decoded() {
        let mut decoder = Decoder::new(1024);
        let frames = decoder.push(b"{\"a\":1}#\t#{\"b\":2}#\t#").unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(text(&frames[0]), "{\"a\":1}");
        assert_eq!(text(&frames[1]), "{\"b\":2}");
    }

    #[test]
    fn a_frame_split_across_reads_waits_for_the_rest() {
        let mut decoder = Decoder::new(1024);
        assert!(decoder.push(b"{\"infoTy").unwrap().is_empty());
        assert!(decoder.push(b"pe\":21006,\"data\":{}}").unwrap().is_empty());
        let frames = decoder.push(b"#\t#").unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(text(&frames[0]), "{\"infoType\":21006,\"data\":{}}");
    }

    #[test]
    fn a_delimiter_split_across_reads_is_still_found() {
        let mut decoder = Decoder::new(1024);
        assert!(decoder.push(b"{\"a\":1}#").unwrap().is_empty());
        assert!(decoder.push(b"\t").unwrap().is_empty());
        let frames = decoder.push(b"#").unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(text(&frames[0]), "{\"a\":1}");
    }

    #[test]
    fn a_trailing_partial_frame_survives_a_complete_one() {
        let mut decoder = Decoder::new(1024);
        let frames = decoder.push(b"{\"a\":1}#\t#{\"b\":").unwrap();
        assert_eq!(frames.len(), 1);
        let frames = decoder.push(b"2}#\t#").unwrap();
        assert_eq!(text(&frames[0]), "{\"b\":2}");
    }

    #[test]
    fn an_oversize_frame_is_an_error() {
        let mut decoder = Decoder::new(8);
        let err = decoder.push(b"{\"a\":123456}#\t#").unwrap_err();
        assert_eq!(err, DecodeError::Oversize { len: 12, max: 8 });

        // ...and an unterminated run that grows past the limit errors just the same,
        // instead of buffering without bound.
        let mut decoder = Decoder::new(8);
        decoder.push(b"12345678").unwrap();
        let err = decoder.push(b"9").unwrap_err();
        assert_eq!(err, DecodeError::Oversize { len: 9, max: 8 });
    }

    #[test]
    fn an_empty_frame_is_returned_as_empty() {
        let mut decoder = Decoder::new(1024);
        let frames = decoder.push(b"#\t#").unwrap();
        assert_eq!(frames, vec![Vec::<u8>::new()]);
    }
}
