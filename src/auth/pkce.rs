use sha2::{Digest, Sha256};
use std::fmt;

/// SHA-256 based PKCE code challenge (Smart Authorization).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PkceCodeChallenge {
    pub(crate) code_challenge: Option<String>,
    pub(crate) method: PkceCodeChallengeMethod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PkceCodeChallengeMethod {
    S256,
}

impl fmt::Display for PkceCodeChallengeMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PkceCodeChallengeMethod::S256 => write!(f, "S256"),
        }
    }
}

/// A cryptographically random PKCE code verifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PkceCodeVerifier(String);

impl PkceCodeVerifier {
    /// Generate a new verifier with a cryptographically random value.
    ///
    /// The value lives in the 43–128 character ASCII range mandated by the
    /// PKCE RFC and must be unpredicatable from the client's perspective.
    pub fn new_random() -> Self {
        use base64::Engine as _;
        use rand::RngCore as _;
        // 97 bytes → 128 chars of base64url… trim to a comfortable 86 bytes → 116 chars.
        let mut bytes = [0u8; 96];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        Self(encoded)
    }

    /// The base64url string used in the authorization request.
    pub fn secret(&self) -> &str {
        &self.0
    }

    /// The PKCE challenge (`S256`, RFC 7636 §4.2).
    pub fn code_challenge(&self) -> PkceCodeChallenge {
        let mut hasher = Sha256::new();
        hasher.update(self.0.as_bytes());
        let digest = hasher.finalize();
        use base64::Engine as _;
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        PkceCodeChallenge {
            code_challenge: Some(challenge),
            method: PkceCodeChallengeMethod::S256,
        }
    }
}

impl PkceCodeChallenge {
    /// The base64url challenge string, when present.
    pub fn secret(&self) -> Option<&str> {
        self.code_challenge.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pkce_verifier_length() {
        let verifier = PkceCodeVerifier::new_random();
        let len = verifier.secret().len();
        assert!(
            (43..=128).contains(&len),
            "verifier length {len} must be within 43..=128"
        );
    }

    #[test]
    fn test_pkce_verifier_is_unique() {
        let a = PkceCodeVerifier::new_random();
        let b = PkceCodeVerifier::new_random();
        assert_ne!(a, b);
    }

    #[test]
    fn test_pkce_challenge_is_s256() {
        let verifier = PkceCodeVerifier::new_random();
        let challenge = verifier.code_challenge();
        assert_eq!(challenge.method, PkceCodeChallengeMethod::S256);

        // Verify against the reference vector from RFC 7636 Appendix B.
        let verifier = PkceCodeVerifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".to_string());
        let challenge = verifier.code_challenge();
        assert_eq!(
            challenge.secret().unwrap(),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}