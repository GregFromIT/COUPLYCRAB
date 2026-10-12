//! Who is the current user. Order:
//! 1. `--dev-user`.
//! 2. `Tailscale-User-Login` (set by `tailscale serve`, which strips client
//!    copies) mapped through `[web.users]`. One user → that user.
//! 3. Several users for that login (shared account) or no/unmapped header:
//!    the `user` cookie set by the picker, if it's one of the allowed users.
//! 4. Otherwise no user: `GET /me` tells the frontend to show the picker.
//!
//! The picker is convenience, not security: both users trust each other.

use crate::{ApiError, ApiResult, AppState, Body};
use axum::{
    extract::{FromRequestParts, State},
    http::{header, request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

pub const LOGIN_HEADER: &str = "tailscale-user-login";

/// Users this request may act as, and whether the choice is already made.
struct Candidates {
    login: Option<String>,
    users: Vec<String>,
    chosen: Option<String>,
}

fn cookie_user(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|c| c.trim().strip_prefix("user=").map(str::to_string))
}

fn candidates(state: &AppState, headers: &HeaderMap) -> Candidates {
    let s = &state.0;
    if let Some(u) = &s.dev_user {
        return Candidates { login: None, users: vec![u.clone()], chosen: Some(u.clone()) };
    }
    let login = headers.get(LOGIN_HEADER).and_then(|v| v.to_str().ok()).map(decode_rfc2047);
    let mapped = login.as_ref().and_then(|l| s.web.users.get(l)).map(|u| u.ids());
    let users = match mapped {
        Some(ids) if ids.len() == 1 => return Candidates { login, chosen: Some(ids[0].clone()), users: ids },
        Some(ids) => ids,
        None => s.known_users.clone(),
    };
    let chosen = cookie_user(headers).filter(|c| users.contains(c));
    Candidates { login, users, chosen }
}

/// Extractor: the current user ID, or 401 `no_user` (frontend shows the picker).
pub struct Me(pub String);

impl FromRequestParts<AppState> for Me {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        candidates(state, &parts.headers)
            .chosen
            .map(Me)
            .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "no_user", "pick a user first (PUT /api/v1/me)"))
    }
}

#[derive(Serialize)]
pub struct MeResponse {
    /// `null` until a user is picked.
    user_id: Option<String>,
    /// Users the picker may offer.
    users: Vec<String>,
    /// Decoded `Tailscale-User-Login`, if present.
    login: Option<String>,
}

pub async fn get_me(State(state): State<AppState>, headers: HeaderMap) -> Json<MeResponse> {
    let c = candidates(&state, &headers);
    Json(MeResponse { user_id: c.chosen, users: c.users, login: c.login })
}

#[derive(Deserialize)]
pub struct PickUser {
    user_id: String,
}

/// The picker: sets `user=<id>` (HttpOnly, SameSite=Strict) if `user_id` is allowed.
pub async fn put_me(State(state): State<AppState>, headers: HeaderMap, Body(p): Body<PickUser>) -> ApiResult<Response> {
    let c = candidates(&state, &headers);
    if !c.users.contains(&p.user_id) {
        return Err(ApiError::forbidden(format!("{:?} is not a user you can pick", p.user_id)));
    }
    let cookie = format!("user={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=31536000", p.user_id);
    let body = MeResponse { user_id: Some(p.user_id), users: c.users, login: c.login };
    Ok(([(header::SET_COOKIE, cookie)], Json(body)).into_response())
}

/// Decode an RFC 2047 encoded-word (`=?UTF-8?B?...?=` / `=?UTF-8?Q?...?=`), as
/// Tailscale uses for non-ASCII header values. Plain values, other charsets and
/// malformed input are returned unchanged, so config can always match the raw value.
/// https://www.rfc-editor.org/rfc/rfc2047#section-4
pub fn decode_rfc2047(raw: &str) -> String {
    try_decode(raw.trim()).unwrap_or_else(|| raw.to_string())
}

fn try_decode(s: &str) -> Option<String> {
    let inner = s.strip_prefix("=?")?.strip_suffix("?=")?;
    let mut parts = inner.splitn(3, '?');
    let (charset, enc, text) = (parts.next()?, parts.next()?, parts.next()?);
    if !charset.eq_ignore_ascii_case("utf-8") && !charset.eq_ignore_ascii_case("us-ascii") {
        return None;
    }
    let bytes = match enc {
        "B" | "b" => base64(text)?,
        "Q" | "q" => {
            let (mut out, b) = (Vec::new(), text.as_bytes());
            let mut i = 0;
            while i < b.len() {
                match b[i] {
                    b'_' => out.push(b' '),
                    b'=' => {
                        out.push(u8::from_str_radix(text.get(i + 1..i + 3)?, 16).ok()?);
                        i += 2;
                    }
                    c => out.push(c),
                }
                i += 1;
            }
            out
        }
        _ => return None,
    };
    String::from_utf8(bytes).ok()
}

fn base64(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0);
    for c in s.bytes().filter(|c| *c != b'=') {
        acc = (acc << 6) | u32::from(val(c)?);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::decode_rfc2047 as d;

    #[test]
    fn rfc2047() {
        assert_eq!(d("alice@example.com"), "alice@example.com");
        // "Zoë" in B and Q encodings.
        assert_eq!(d("=?utf-8?b?Wm/Dqw==?="), "Zoë");
        assert_eq!(d("=?UTF-8?Q?Zo=C3=AB?="), "Zoë");
        assert_eq!(d("=?utf-8?q?a_b?="), "a b");
        // Unsupported charset / malformed: left as-is.
        assert_eq!(d("=?iso-8859-1?q?Zo=EB?="), "=?iso-8859-1?q?Zo=EB?=");
        assert_eq!(d("=?utf-8?q?bad=Z?="), "=?utf-8?q?bad=Z?=");
        assert_eq!(d("=?utf-8?x?abc?="), "=?utf-8?x?abc?=");
    }
}
