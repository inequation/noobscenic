//! Sessions and cookies — doc/PLAN.md §9.2.
//!
//! `register` mints two things: `data.session`, whose first 16 bytes are the
//! channel-B AES-128-ECB key, and `data.cookies`, the sid the device echoes back on
//! every later request as `Cookie: cookies=<value>`. Lookup rules matter more than
//! minting: a known cookie proceeds, a *missing* `Cookie:` header proceeds with a
//! warning (being strict there is the fastest way into a re-register loop), and a
//! present-but-empty or unknown cookie gets `code:102` — the documented "sid
//! expired" signal that makes the robot re-run `register` [PROTOCOL §A, §B].

use axum::http::HeaderMap;
use axum::http::header::COOKIE;
use sqlx::SqlitePool;

use crate::db::{now_ms, queries};
use crate::error::Result;

/// A freshly minted session, before it is persisted.
#[derive(Debug, Clone)]
pub struct Minted {
    /// 32 ASCII alphanumerics (16 random bytes, hex): ≤63 required, and its first
    /// 16 bytes are the AES-128 key.
    pub session_key: String,
    /// 32 ASCII alphanumerics: the sid value; ≤55 required.
    pub cookie: String,
}

/// What the request's credentials turned out to be.
#[derive(Debug)]
pub enum Credential {
    /// A known, live session; carries the device serial it belongs to.
    Valid(String),
    /// No `Cookie:` header at all — served anyway, with a warning.
    Absent,
    /// A present but empty or unknown cookie: answer `code:102`.
    Stale,
}

pub fn mint() -> Minted {
    Minted {
        session_key: random_hex(16),
        cookie: random_hex(16),
    }
}

/// Hex of `bytes` random bytes, so the result is always twice that long and ASCII.
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::fill(buf.as_mut_slice());
    let mut out = String::with_capacity(bytes * 2);
    for byte in buf {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Look the request's cookie up and, when it is live, touch it.
pub async fn check(
    pool: &SqlitePool,
    headers: &HeaderMap,
    session_ttl_secs: Option<u64>,
) -> Result<Credential> {
    let value = match cookie_value(headers) {
        CookieLookup::Missing => return Ok(Credential::Absent),
        CookieLookup::Empty => return Ok(Credential::Stale),
        CookieLookup::Value(value) => value,
    };

    let Some((sn, created_ms)) = queries::find_session(pool, value).await? else {
        return Ok(Credential::Stale);
    };

    if let Some(ttl) = session_ttl_secs
        && now_ms() - created_ms > ttl as i64 * 1000
    {
        // The configured way to force a re-register on purpose (doc/PLAN.md §6).
        queries::revoke_session(pool, value).await?;
        return Ok(Credential::Stale);
    }

    queries::touch_session(pool, value).await?;
    Ok(Credential::Valid(sn))
}

enum CookieLookup<'a> {
    Missing,
    Empty,
    Value(&'a str),
}

/// `Cookie: cookies=<value>` → `<value>`.
///
/// A header that is present but carries no usable value is *not* the same as no
/// header: it is exactly what a robot that registered against the phase-1
/// catch-all sends, and it must get `code:102` so it re-registers.
fn cookie_value(headers: &HeaderMap) -> CookieLookup<'_> {
    let Some(raw) = headers.get(COOKIE) else {
        return CookieLookup::Missing;
    };
    let Ok(text) = raw.to_str() else {
        return CookieLookup::Missing;
    };
    if text.trim().is_empty() {
        return CookieLookup::Empty;
    }
    for part in text.split(';').map(str::trim) {
        if let Some(value) = part.strip_prefix("cookies=") {
            return if value.is_empty() {
                CookieLookup::Empty
            } else {
                CookieLookup::Value(value)
            };
        }
    }
    CookieLookup::Missing
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(COOKIE, value.parse().unwrap());
        headers
    }

    #[test]
    fn mints_ascii_sessions_within_the_device_buffers() {
        let minted = mint();
        assert_eq!(minted.session_key.len(), 32);
        assert_eq!(minted.cookie.len(), 32);
        assert!(minted.session_key.is_ascii() && minted.cookie.is_ascii());
        assert_ne!(mint().cookie, minted.cookie, "cookies must not repeat");
    }

    #[test]
    fn a_missing_header_is_absent() {
        assert!(matches!(
            cookie_value(&HeaderMap::new()),
            CookieLookup::Missing
        ));
    }

    #[test]
    fn an_empty_header_or_value_is_stale_material() {
        assert!(matches!(cookie_value(&headers("")), CookieLookup::Empty));
        assert!(matches!(
            cookie_value(&headers("cookies=")),
            CookieLookup::Empty
        ));
    }

    #[test]
    fn a_real_value_is_found_among_other_cookies() {
        match cookie_value(&headers("other=1; cookies=abc123")) {
            CookieLookup::Value(value) => assert_eq!(value, "abc123"),
            _ => panic!("expected a value"),
        }
    }
}
