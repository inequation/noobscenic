//! Minimal `multipart/form-data` reader — doc/PLAN.md §20.6.
//!
//! The robot's clean-record upload (`cleanPack/uploadSingle`) is real multipart,
//! binary parts and all (`BACKUP_MAP.md` §A5); the lenient urlencoded parser turns it
//! into nonsense, so binary parts get their own small reader here. Only what the
//! upload uses is supported: no nested multiparts, no quoted-printable, no
//! `base64` content-transfer-encoding — the robot sends `Content-Disposition` and a
//! raw body per part.

/// One part of the body: the form field name, the filename when the part is a file,
/// and the raw content bytes.
#[derive(Debug, PartialEq, Eq)]
pub struct Part {
    pub name: String,
    pub filename: Option<String>,
    pub content: Vec<u8>,
}

impl Part {
    pub fn text(&self) -> Option<&str> {
        std::str::from_utf8(&self.content).ok().map(str::trim)
    }
}

/// The boundary from a `Content-Type` header, if it is multipart at all.
pub fn boundary(content_type: &str) -> Option<String> {
    if !content_type
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("multipart/")
    {
        return None;
    }
    for piece in content_type.split(';') {
        let piece = piece.trim();
        if let Some(value) = piece.strip_prefix("boundary=") {
            return Some(value.trim_matches('"').to_string());
        }
    }
    None
}

/// Split a multipart body into its parts. `None` when the body is malformed enough
/// that the caller should fall back to its lenient path.
pub fn parse(content_type: &str, body: &[u8]) -> Option<Vec<Part>> {
    let boundary = boundary(content_type)?;
    if boundary.is_empty() {
        return None;
    }
    let delimiter = format!("--{boundary}").into_bytes();
    let mut parts = Vec::new();
    let mut cursor = find(body, &delimiter, 0)?;

    loop {
        cursor += delimiter.len();
        if body[cursor..].starts_with(b"--") {
            break; // closing delimiter
        }
        if !body[cursor..].starts_with(b"\r\n") {
            return None;
        }
        cursor += 2;

        // Headers end at the first blank line.
        let header_end = find(body, b"\r\n\r\n", cursor)?;
        let headers = std::str::from_utf8(&body[cursor..header_end]).ok()?;
        let (name, filename) = disposition(headers)?;
        let content_start = header_end + 4;

        // Content runs to the CRLF in front of the next delimiter.
        let next = find(body, &delimiter, content_start)?;
        let content_end = next.checked_sub(2)?;
        if &body[content_end..next] != b"\r\n" {
            return None;
        }

        parts.push(Part {
            name,
            filename,
            content: body[content_start..content_end].to_vec(),
        });
        cursor = next;
    }
    Some(parts)
}

/// `name` and optional `filename` out of a part's headers.
fn disposition(headers: &str) -> Option<(String, Option<String>)> {
    let line = headers.lines().find(|line| {
        line.to_ascii_lowercase()
            .starts_with("content-disposition:")
    })?;
    let mut name = None;
    let mut filename = None;
    for piece in line.split(';').skip(1) {
        let piece = piece.trim();
        if let Some(value) = piece.strip_prefix("name=") {
            name = Some(value.trim_matches('"').to_string());
        } else if let Some(value) = piece.strip_prefix("filename=") {
            filename = Some(value.trim_matches('"').to_string());
        }
    }
    Some((name?, filename))
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from > haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| offset + from)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape the robot sends: text parts around a binary `backupMap`.
    fn robot_body() -> Vec<u8> {
        let mut body = Vec::new();
        let boundary = "----test-boundary";
        for (name, filename, content) in [
            (
                "backupMapMd5",
                None,
                b"53a5b00576b49ee0127f09299ff8c254".to_vec(),
            ),
            ("data", None, b"{}".to_vec()),
            ("sn", None, b"LSLDSM7PRO20403551".to_vec()),
            (
                "cleanFile",
                Some("LSLDSM7PRO20403551_1_2_3_4_1_0.txt"),
                b"infoType 20004".to_vec(),
            ),
            (
                "backupMap",
                Some("LSLDSM7PRO20403551_1_2_3_4_1_0.bkmap"),
                vec![0x1f, 0x8b, 0x00, 0xff, 0x0d, 0x0a, 0x1a],
            ),
        ] {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            body.extend_from_slice(b"Content-Disposition: form-data; name=\"");
            body.extend_from_slice(name.as_bytes());
            body.push(b'"');
            if let Some(filename) = filename {
                body.extend_from_slice(b"; filename=\"");
                body.extend_from_slice(filename.as_bytes());
                body.push(b'"');
            }
            body.extend_from_slice(b"\r\n\r\n");
            body.extend_from_slice(&content);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        body
    }

    #[test]
    fn parses_the_robot_upload_including_binary_parts() {
        let body = robot_body();
        let parts =
            parse("multipart/form-data; boundary=----test-boundary", &body).expect("parses");
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].name, "backupMapMd5");
        assert_eq!(parts[1].text(), Some("{}"));
        assert_eq!(parts[2].text(), Some("LSLDSM7PRO20403551"));
        assert_eq!(
            parts[3].filename.as_deref(),
            Some("LSLDSM7PRO20403551_1_2_3_4_1_0.txt")
        );
        assert_eq!(parts[4].name, "backupMap");
        assert_eq!(
            parts[4].content,
            vec![0x1f, 0x8b, 0x00, 0xff, 0x0d, 0x0a, 0x1a]
        );
    }

    #[test]
    fn a_body_without_a_crlf_before_the_delimiter_is_refused() {
        let body =
            b"--b\r\nContent-Disposition: form-data; name=\"sn\"\r\n\r\nSN--b--\r\n".to_vec();
        assert!(parse("multipart/form-data; boundary=b", &body).is_none());
    }

    #[test]
    fn non_multipart_content_types_are_left_to_the_form_parser() {
        assert!(boundary("application/x-www-form-urlencoded").is_none());
        assert!(parse("application/x-www-form-urlencoded", b"sn=X").is_none());
        assert!(boundary("multipart/form-data; boundary=\"quoted\"").as_deref() == Some("quoted"));
    }
}
