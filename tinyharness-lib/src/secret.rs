use serde::{Deserialize, Serialize, Serializer};

/// Minimal no-new-dependency replacement for `secrecy::SecretString`.
/// Exposes the secret only on explicit `.expose_secret()` calls. The secret
/// cannot be implicitly leaked by the `Debug` implementation, e.g. in logs.
///
/// Retains serialization/deserialization via `serde`.
///
/// Deviations from `secrecy`:
/// 1. Does not zero on `Drop`: the heap bytes remain after deallocation, and,
///    e.g., a core dump could contain leaked secrets.
/// 2. Implements `Clone`: `secrecy` uses its own `SecretBox` smart pointer for
///    space efficiency. Since we can reasonably expect only a handful of
///    secrets in memory for TinyHarness, that optimization is not worth the
///    added complexity.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Expose the underlying secret. Use this as the point the actual value is
    /// required (e.g., in constructing an HTTP request). Do not log the exposed
    /// value.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn masked(&self) -> String {
        if self.0.is_ascii() && self.len() > 8 {
            format!("{}...{}", &self.0[..4], &self.0[self.len() - 4..])
        } else {
            "****".to_string()
        }
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl Serialize for SecretString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer).map(SecretString)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_value() {
        let s = SecretString::new("sk-supersecret");
        assert_eq!(format!("{:?}", s), "[REDACTED]");
        assert!(!format!("{:?}", s).contains("sk-supersecret"));
    }

    #[test]
    fn expose_secret_returns_inner() {
        let s = SecretString::new("sk-supersecret");
        assert_eq!(s.expose_secret(), "sk-supersecret");
    }

    #[test]
    fn is_empty_and_len() {
        assert!(SecretString::new("").is_empty());
        assert_eq!(SecretString::new("").len(), 0);
        assert!(!SecretString::new("abc").is_empty());
        assert_eq!(SecretString::new("abc").len(), 3);
    }

    #[test]
    fn masked_long_ascii() {
        assert_eq!(SecretString::new("abcdef123456").masked(), "abcd...3456");
    }

    #[test]
    fn masked_short_ascii() {
        assert_eq!(SecretString::new("abc").masked(), "****");
        assert_eq!(SecretString::new("abcdefgh").masked(), "****");
    }

    #[test]
    fn masked_exactly_9_chars() {
        // 9 chars is the threshold: still show first/last 4
        assert_eq!(SecretString::new("abcdefghi").masked(), "abcd...fghi");
    }

    #[test]
    fn masked_non_ascii_falls_through_to_stars() {
        // Non-ASCII keys of any length are masked fully, since slicing on a
        // non-ASCII boundary would panic and revealing a slice of UTF-8 bytes
        // could split a code point.
        assert_eq!(SecretString::new("пароль12345").masked(), "****");
    }

    #[test]
    fn clone_yields_equal_value() {
        let s = SecretString::new("sk-supersecret");
        assert_eq!(s.clone(), s);
    }

    #[test]
    fn default_is_empty() {
        let s = SecretString::default();
        assert!(s.is_empty());
        assert_eq!(s.expose_secret(), "");
    }

    #[test]
    fn serde_round_trip_preserves_value() {
        let s = SecretString::new("sk-supersecret");
        let json = serde_json::to_string(&s).unwrap();
        let back: SecretString = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.expose_secret(), "sk-supersecret");
    }

    #[test]
    fn serde_wire_format_is_bare_string() {
        // Pin the on-disk format: a bare JSON string, not a wrapper object.
        // This is what guarantees `settings.json` keeps loading with no
        // migration.
        let s = SecretString::new("sk-supersecret");
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v, serde_json::Value::String("sk-supersecret".to_string()));
    }
}
