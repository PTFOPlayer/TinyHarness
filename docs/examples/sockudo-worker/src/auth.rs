//! Sockudo HTTP API authentication — Pusher-style HMAC-SHA256 signed requests.
//!
//! Signature = HMAC-SHA256(secret, "METHOD\npath\nsorted_query_string")

use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Parameters needed to sign a Sockudo HTTP API request.
#[derive(Clone)]
pub struct AuthCredentials {
    pub app_id: String,
    pub app_key: String,
    pub app_secret: String,
}

impl AuthCredentials {
    pub fn new(
        app_id: impl Into<String>,
        app_key: impl Into<String>,
        app_secret: impl Into<String>,
    ) -> Self {
        AuthCredentials {
            app_id: app_id.into(),
            app_key: app_key.into(),
            app_secret: app_secret.into(),
        }
    }
}

/// Sign a Sockudo HTTP API request and return the full query string with
/// auth parameters appended.
///
/// `method` is "GET" or "POST".
/// `path` is the path component starting with "/" (e.g. "/apps/test-app/events").
/// `body` is the raw request body (empty string for GET).
pub fn sign_request(creds: &AuthCredentials, method: &str, path: &str, body: &str) -> String {
    let body_md5 = format!("{:x}", md5::compute(body.as_bytes()));
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();

    // Build query params (sorted alphabetically, excluding auth_signature)
    let mut params: Vec<(String, String)> = vec![
        ("auth_key".to_string(), creds.app_key.clone()),
        ("auth_timestamp".to_string(), timestamp),
        ("auth_version".to_string(), "1.0".to_string()),
        ("body_md5".to_string(), body_md5),
    ];
    params.sort_by(|a, b| a.0.cmp(&b.0));

    let qs = params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let string_to_sign = format!("{method}\n{path}\n{qs}");

    let signature = {
        let mut mac = HmacSha256::new_from_slice(creds.app_secret.as_bytes())
            .unwrap_or_else(|_| HmacSha256::new_from_slice(b"").unwrap());
        mac.update(string_to_sign.as_bytes());
        hex_encode(&mac.finalize().into_bytes())
    };

    format!("{qs}&auth_signature={signature}")
}

/// Hex-encode a byte slice.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_request_has_all_params() {
        let creds = AuthCredentials::new("app-id", "my-key", "my-secret");
        let qs = sign_request(&creds, "POST", "/apps/app-id/events", r#"{"test":true}"#);

        assert!(qs.contains("auth_key=my-key"));
        assert!(qs.contains("auth_version=1.0"));
        assert!(qs.contains("auth_signature="));
        assert!(qs.contains("body_md5="));
    }

    #[test]
    fn test_sign_request_deterministic_same_second() {
        let creds = AuthCredentials::new("app-id", "key", "secret");
        let qs1 = sign_request(&creds, "POST", "/apps/app-id/events", "body");
        let qs2 = sign_request(&creds, "POST", "/apps/app-id/events", "body");

        // Extract signatures
        let sig1 = qs1.split("auth_signature=").nth(1).unwrap();
        let sig2 = qs2.split("auth_signature=").nth(1).unwrap();

        // If timestamps are in the same second, signatures match
        let ts1 = qs1
            .split("auth_timestamp=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap();
        let ts2 = qs2
            .split("auth_timestamp=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap();
        if ts1 == ts2 {
            assert_eq!(sig1, sig2);
        }
    }

    #[test]
    fn test_sign_request_different_bodies_different_sigs() {
        let creds = AuthCredentials::new("app-id", "key", "secret");
        let qs1 = sign_request(&creds, "POST", "/apps/app-id/events", "body1");
        let qs2 = sign_request(&creds, "POST", "/apps/app-id/events", "body2");

        let sig1 = qs1.split("auth_signature=").nth(1).unwrap();
        let sig2 = qs2.split("auth_signature=").nth(1).unwrap();

        let ts1 = qs1
            .split("auth_timestamp=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap();
        let ts2 = qs2
            .split("auth_timestamp=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap();
        if ts1 == ts2 {
            assert_ne!(sig1, sig2);
        }
    }
}
