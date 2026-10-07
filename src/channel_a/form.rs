//! The lenient `application/x-www-form-urlencoded` parser — doc/PLAN.md §9.1.
//!
//! Bodies look like `sn=…&ts=0&data={…}`, but **`data=` is plaintext JSON and is
//! not always URL-escaped** [PROTOCOL §A, "proven"], so `axum::Form` /
//! `serde_urlencoded` corrupts or rejects real device bodies: raw JSON contains
//! `&`, `=`, `+` and `%`, and `+` would decode to a space inside string values.
//!
//! Rules, from the plan:
//!  1. split on `&` only until a key named `data` is reached (in every observed
//!     template `data` is last);
//!  2. everything after that `data=` is taken verbatim to end-of-body;
//!  3. scalar fields are percent-decoded; `data` is parsed as JSON from the raw
//!     text first, then from the percent-decoded text;
//!  4. unknown extra fields are kept, never rejected.

use serde_json::Value;

/// A parsed form body.
pub struct Form {
    /// Scalar fields in wire order, percent-decoded. `data` is not in here.
    fields: Vec<(String, String)>,
    /// The verbatim text after `data=`, if the body had one.
    pub data_raw: Option<String>,
    /// `data_raw` parsed as JSON (raw first, percent-decoded second).
    pub data: Option<Value>,
}

impl Form {
    pub fn parse(body: &[u8]) -> Form {
        let text = String::from_utf8_lossy(body).into_owned();
        let mut fields = Vec::new();
        let mut data_raw = None;
        let mut data = None;
        let mut cursor = 0;

        while cursor < text.len() {
            let rest = &text[cursor..];
            let (item, next) = match rest.find('&') {
                Some(at) => (&rest[..at], cursor + at + 1),
                None => (rest, text.len()),
            };
            let (key, value) = item.split_once('=').unwrap_or((item, ""));

            if key == "data" {
                let start = cursor + key.len() + 1;
                let remainder = text.get(start..).unwrap_or_default();
                // `data` is last in the upload templates but **first** in the
                // device's `cleanPack/response` bodies, so taking the rest of the
                // body is not enough: take the longest prefix that parses as JSON
                // and keep parsing the fields that follow it.
                match split_data(remainder) {
                    Some((raw, parsed, end)) => {
                        data_raw = Some(raw);
                        data = Some(parsed);
                        cursor = start + end;
                        if cursor < text.len() {
                            cursor += 1; // step over the '&' that follows the blob
                        }
                        continue;
                    }
                    None => {
                        data_raw = Some(remainder.to_string());
                        break;
                    }
                }
            }
            fields.push((percent_decode(key), percent_decode(value)));
            cursor = next;
        }

        Form {
            fields,
            data_raw,
            data,
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.get(key)?.trim().parse().ok()
    }

    /// Every scalar field, in wire order — for the endpoints that store the lot.
    pub fn entries(&self) -> &[(String, String)] {
        &self.fields
    }

    /// Keys that were neither used nor recognised, for the permanent record.
    pub fn unknown<'a>(&'a self, known: &'a [&'a str]) -> Vec<(&'a str, &'a str)> {
        self.fields
            .iter()
            .filter(|(key, _)| !known.contains(&key.as_str()))
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect()
    }
}

fn parse_data(raw: &str) -> Option<Value> {
    serde_json::from_str(raw)
        .ok()
        .or_else(|| serde_json::from_str(&percent_decode(raw)).ok())
}

/// The `data=` value: the longest prefix that parses as JSON, and how many bytes it
/// consumed. `None` when nothing parses — the caller then keeps it as opaque text.
fn split_data(remainder: &str) -> Option<(String, Value, usize)> {
    let mut ends: Vec<usize> = remainder.match_indices('&').map(|(at, _)| at).collect();
    ends.push(remainder.len());
    for end in ends {
        let candidate = &remainder[..end];
        if let Some(value) = parse_data(candidate) {
            return Some((candidate.to_string(), value, end));
        }
    }
    None
}

/// `%XX` and `+`, byte-wise and total: a malformed escape is kept literally.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(high), Some(low)) => {
                    out.push(high << 4 | low);
                    i += 3;
                }
                _ => {
                    out.push(bytes[i]);
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_the_register_template() {
        let form = Form::parse(b"sn=LSLDSM7PRO20403551&ld_sn=14411442DB220CBE&sig=abc%2Bd%3D&ts=0");
        assert_eq!(form.get("sn"), Some("LSLDSM7PRO20403551"));
        assert_eq!(form.get("ld_sn"), Some("14411442DB220CBE"));
        assert_eq!(
            form.get("sig"),
            Some("abc+d="),
            "sig is genuinely URL-encoded"
        );
        assert_eq!(form.get_i64("ts"), Some(0));
        assert!(form.data.is_none());
    }

    #[test]
    fn takes_data_verbatim_including_ampersands_and_equals() {
        // Unescaped JSON: `&`, `=`, `+` and `%` inside strings must survive.
        let body = b"sn=ABC&infoType=20003&ts=12&userId=u1&data={\"note\":\"a&b=c+d%e\",\"n\":1}";
        let form = Form::parse(body);
        assert_eq!(form.get("sn"), Some("ABC"));
        assert_eq!(form.get("infoType"), Some("20003"));
        assert_eq!(form.get("userId"), Some("u1"));
        assert_eq!(
            form.data,
            Some(json!({"note": "a&b=c+d%e", "n": 1})),
            "the JSON blob is not url-decoded"
        );
        assert_eq!(
            form.data_raw.as_deref(),
            Some("{\"note\":\"a&b=c+d%e\",\"n\":1}")
        );
    }

    #[test]
    fn falls_back_to_percent_decoded_data() {
        let form = Form::parse(b"sn=ABC&data=%7B%22a%22%3A1%7D");
        assert_eq!(form.data, Some(json!({"a": 1})));
    }

    #[test]
    fn keeps_opaque_data_instead_of_failing() {
        let form = Form::parse(b"sn=ABC&data=not json at all");
        assert!(form.data.is_none());
        assert_eq!(form.data_raw.as_deref(), Some("not json at all"));
        assert_eq!(form.get("sn"), Some("ABC"));
    }

    #[test]
    fn data_first_bodies_keep_the_fields_that_follow_it() {
        // The device's `cleanPack/response` template puts `data` first, unlike the
        // upload templates where it is last.
        let body =
            b"data={\"message\":\"ok\",\"infoType\":21011}&infoType=21011&sn=ABC&ts=5&userId=u";
        let form = Form::parse(body);
        assert_eq!(form.get("sn"), Some("ABC"));
        assert_eq!(form.get_i64("infoType"), Some(21011));
        assert_eq!(form.get("ts"), Some("5"));
        assert_eq!(form.get("userId"), Some("u"));
        assert_eq!(form.data, Some(json!({"message": "ok", "infoType": 21011})));
    }

    #[test]
    fn unknown_fields_are_kept() {
        let form = Form::parse(b"devType=3&sn=ABC&taskid=99&event=7&createtime=123&data=%7B%7D");
        let unknown = form.unknown(&["sn"]);
        assert_eq!(
            unknown,
            vec![
                ("devType", "3"),
                ("taskid", "99"),
                ("event", "7"),
                ("createtime", "123"),
            ]
        );
    }

    #[test]
    fn malformed_escapes_and_empty_values_are_tolerated() {
        let form = Form::parse(b"a=100%&b=&c=x+y");
        assert_eq!(form.get("a"), Some("100%"));
        assert_eq!(form.get("b"), Some(""));
        assert_eq!(form.get("c"), Some("x y"));
    }
}
