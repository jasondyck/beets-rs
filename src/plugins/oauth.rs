//! OAuth1 (HMAC-SHA1) request signing, as requests-oauthlib's
//! `OAuth1Session` does it for the beatport and discogs plugins: request
//! tokens, the authorization URL, access tokens, and the `Authorization`
//! header for API calls.

use super::requests::quote;
use std::time::{SystemTime, UNIX_EPOCH};

/// `oauth_nonce`: requests-oauthlib's `OAuth1.nonce()`: 16 random bytes
/// as hex. Unpredictable (an OAuth1 requirement), from the OS CSPRNG.
fn nonce() -> String {
    super::osrandom::random_hex(16)
}

/// `oauth_timestamp`: seconds since the epoch.
fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// SHA-1 of `data` (FIPS 180-1).
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

/// HMAC-SHA1 (RFC 2104).
pub fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    let mut k = if key.len() > 64 {
        sha1(key).to_vec()
    } else {
        key.to_vec()
    };
    k.resize(64, 0);
    let mut inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    inner.extend_from_slice(data);
    let mut outer: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    outer.extend_from_slice(&sha1(&inner));
    sha1(&outer)
}

/// base64 (standard alphabet, padded), for the `Authorization` header.
pub fn b64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// An OAuth1 client as requests-oauthlib's `OAuth1Session` (HMAC-SHA1,
/// `oauth_callback=oob`).
#[derive(Debug, Clone)]
pub struct OAuth1 {
    pub consumer_key: String,
    pub consumer_secret: String,
    pub token: Option<String>,
    pub token_secret: Option<String>,
}

/// `urllib.parse.quote(s, safe="~")` as OAuth uses it (RFC 5849 3.6).
pub fn oauth_quote(s: &str) -> String {
    quote(s, "~")
}

impl OAuth1 {
    pub fn new(consumer_key: &str, consumer_secret: &str) -> Self {
        OAuth1 {
            consumer_key: consumer_key.to_string(),
            consumer_secret: consumer_secret.to_string(),
            token: None,
            token_secret: None,
        }
    }

    pub fn with_token(mut self, token: &str, secret: &str) -> Self {
        self.token = Some(token.to_string());
        self.token_secret = Some(secret.to_string());
        self
    }

    /// The `oauth_*` parameters (without the signature) for a request.
    fn oauth_params(&self, extra_callback: Option<&str>) -> Vec<(String, String)> {
        let mut params = vec![
            ("oauth_consumer_key".to_string(), self.consumer_key.clone()),
            ("oauth_nonce".to_string(), nonce()),
            ("oauth_signature_method".to_string(), "HMAC-SHA1".to_string()),
            ("oauth_timestamp".to_string(), timestamp().to_string()),
            ("oauth_version".to_string(), "1.0".to_string()),
        ];
        if let Some(t) = &self.token {
            params.push(("oauth_token".to_string(), t.clone()));
        }
        if let Some(cb) = extra_callback {
            params.push(("oauth_callback".to_string(), cb.to_string()));
        }
        params
    }

    /// The signed `oauth_*` parameters for `method url` with `params`.
    pub fn signed_params(
        &self,
        method: &str,
        url: &str,
        params: &[(String, String)],
        callback: Option<&str>,
    ) -> Vec<(String, String)> {
        let mut all = self.oauth_params(callback);
        all.extend(params.iter().cloned());
        // The signature base string: every parameter, sorted by encoded
        // name then value.
        let mut encoded: Vec<(String, String)> = all
            .iter()
            .map(|(k, v)| (oauth_quote(k), oauth_quote(v)))
            .collect();
        encoded.sort();
        let normalized = encoded
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let base = [
            method.to_uppercase(),
            oauth_quote(url),
            oauth_quote(&normalized),
        ]
        .join("&");
        let key = format!(
            "{}&{}",
            oauth_quote(&self.consumer_secret),
            oauth_quote(self.token_secret.as_deref().unwrap_or(""))
        );
        let signature = b64(&hmac_sha1(key.as_bytes(), base.as_bytes()));
        all.push(("oauth_signature".to_string(), signature));
        all
    }

    /// The `Authorization: OAuth ...` header value for a request.
    pub fn header(
        &self,
        method: &str,
        url: &str,
        params: &[(String, String)],
        callback: Option<&str>,
    ) -> String {
        let signed = self.signed_params(method, url, params, callback);
        let parts: Vec<String> = signed
            .iter()
            .map(|(k, v)| format!("{}=\"{}\"", oauth_quote(k), oauth_quote(v)))
            .collect();
        format!("OAuth {}", parts.join(", "))
    }
}

/// `urllib.parse.parse_qsl`: the `k=v&...` pairs, unquoted.
pub fn parse_qsl(data: &str) -> Vec<(String, String)> {
    data.split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (unquote_plus(k), unquote_plus(v))
        })
        .collect()
}

fn unquote_plus(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}


#[cfg(test)]
mod tests {
    use super::*;

    /// OAuth1 nonces must be unpredictable hex.
    #[test]
    fn nonces_are_random_hex() {
        let a = nonce();
        let b = nonce();
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(a, b);
        // The signed parameters carry that nonce.
        let oauth = OAuth1::new("key", "secret");
        let params = oauth.signed_params("GET", "https://example.com", &[], None);
        let value = |name: &str| {
            params
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(value("oauth_nonce").len(), 32);
        assert!(params.iter().any(|(k, v)| k == "oauth_signature" && !v.is_empty()));
        assert_eq!(value("oauth_signature_method"), "HMAC-SHA1");
    }
}
