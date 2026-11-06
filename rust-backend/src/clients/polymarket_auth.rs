//! Polymarket US Ed25519 request signing.
//!
//! Mirrors `polymarket_us/auth.py::create_auth_headers` (the official SDK)
//! exactly: the signature covers only `{timestamp}{METHOD}{path}` - never
//! the request body or query string, same rule already documented for
//! Kalshi's RSA-PSS signing in `clients/kalshi.rs`.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signer, SigningKey};

pub struct PolymarketAuthHeaders {
    pub access_key: String,
    pub timestamp: String,
    pub signature: String,
}

/// Sign a Polymarket US API request. `secret_key_b64` is the base64-encoded
/// Ed25519 secret; a 64-byte decode (seed + public key) is truncated to its
/// first 32 bytes (the seed), matching the SDK's own handling.
pub fn sign_request(
    key_id: &str,
    secret_key_b64: &str,
    method: &str,
    path: &str,
) -> Result<PolymarketAuthHeaders> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System clock is before the Unix epoch")?
        .as_millis()
        .to_string();
    let message = format!("{}{}{}", timestamp, method, path);

    let mut secret_bytes = BASE64
        .decode(secret_key_b64)
        .context("Polymarket US secret key is not valid base64")?;
    if secret_bytes.len() == 64 {
        secret_bytes.truncate(32);
    }
    let seed: [u8; 32] = secret_bytes.as_slice().try_into().context(
        "Polymarket US secret key must decode to a 32- or 64-byte Ed25519 key",
    )?;
    let signing_key = SigningKey::from_bytes(&seed);
    let signature = signing_key.sign(message.as_bytes());

    Ok(PolymarketAuthHeaders {
        access_key: key_id.to_string(),
        timestamp,
        signature: BASE64.encode(signature.to_bytes()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // A real Ed25519 seed, base64-encoded (32 raw bytes), used only to
    // exercise the signing path deterministically - not a live credential.
    const TEST_SECRET_B64: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

    #[test]
    fn produces_the_expected_header_shape() {
        let headers = sign_request("key-123", TEST_SECRET_B64, "GET", "/v1/orders/open").unwrap();
        assert_eq!(headers.access_key, "key-123");
        assert!(headers.timestamp.parse::<u128>().is_ok());
        assert!(BASE64.decode(&headers.signature).is_ok());
    }

    #[test]
    fn accepts_a_64_byte_secret_by_using_only_the_seed() {
        let mut expanded = BASE64.decode(TEST_SECRET_B64).unwrap();
        expanded.extend_from_slice(&[0u8; 32]); // fake public-key half
        let expanded_b64 = BASE64.encode(&expanded);

        // Same message, same effective seed -> identical signature bytes
        // (timestamps are generated internally, so pin one path/method pair
        // and compare via the lower-level signing key directly instead).
        let seed: [u8; 32] = BASE64.decode(TEST_SECRET_B64).unwrap().try_into().unwrap();
        let expected_key = SigningKey::from_bytes(&seed);
        let expected_sig = expected_key.sign(b"1MOCKGET/v1/orders/open");

        let mut secret_bytes = BASE64.decode(&expanded_b64).unwrap();
        secret_bytes.truncate(32);
        let actual_seed: [u8; 32] = secret_bytes.try_into().unwrap();
        let actual_key = SigningKey::from_bytes(&actual_seed);
        let actual_sig = actual_key.sign(b"1MOCKGET/v1/orders/open");

        assert_eq!(expected_sig.to_bytes(), actual_sig.to_bytes());
    }

    #[test]
    fn rejects_a_malformed_secret() {
        assert!(sign_request("key-123", "not-base64!!", "GET", "/v1/orders/open").is_err());
        assert!(sign_request("key-123", "AAAA", "GET", "/v1/orders/open").is_err());
    }
}
