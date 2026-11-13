//! Cryptographically random values from the OS, shared by the plugins that
//! need unpredictable tokens: tidal's PKCE verifier and OAuth state, and
//! the OAuth1 nonces of the discogs and beatport clients.

use std::io::Read as _;

/// Random bytes from the OS (`getrandom`, e.g. `getentropy`/`getrandom(2)`),
/// falling back to `/dev/urandom`.
pub fn random_bytes(buf: &mut [u8]) {
    if getrandom::fill(buf).is_ok() {
        return;
    }
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(buf))
        .expect("no OS CSPRNG");
}

/// A random string over `alphabet`, drawn from the OS CSPRNG (tokens and
/// nonces must not be predictable). Bytes that would bias the modulo are
/// rejected.
pub fn random_string(len: usize, alphabet: &[u8]) -> String {
    assert!(!alphabet.is_empty() && alphabet.len() <= 256);
    let limit = 256 - 256 % alphabet.len();
    let mut out = String::new();
    while out.len() < len {
        let mut bytes = [0u8; 64];
        random_bytes(&mut bytes);
        for b in bytes {
            if usize::from(b) < limit {
                out.push(alphabet[usize::from(b) % alphabet.len()] as char);
                if out.len() == len {
                    break;
                }
            }
        }
    }
    out
}

/// A random `len`-byte value as lower-case hex (the OAuth1 `oauth_nonce`).
pub fn random_hex(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    random_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_values_are_unpredictable() {
        let a = random_hex(16);
        let b = random_hex(16);
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(a, b);
        let s = random_string(30, b"abc");
        assert_eq!(s.len(), 30);
        assert!(s.bytes().all(|c| c == b'a' || c == b'b' || c == b'c'));
    }
}
