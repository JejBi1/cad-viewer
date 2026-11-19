//! PKCE (RFC 7636, method S256) and the random values of the OAuth flow.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

use crate::error::DriveError;
use crate::secret::Secret;

/// URL-safe base64 text of `n` random bytes from the operating system.
pub(crate) fn random_token(n: usize) -> Result<String, DriveError> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf)
        .map_err(|e| DriveError::SignInFailed(format!("generátor náhodných čísel selhal: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

/// `BASE64URL(SHA256(verifier))`.
pub fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub(crate) struct Pkce {
    pub verifier: Secret,
    pub challenge: String,
}

impl Pkce {
    /// 64 random bytes give an 86 character verifier (allowed: 43 to 128).
    pub fn generate() -> Result<Self, DriveError> {
        let verifier = random_token(64)?;
        let challenge = challenge_for(&verifier);
        Ok(Pkce { verifier: Secret::new(verifier), challenge })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc7636_appendix_b_vector() {
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn generated_values_are_valid_and_unique() {
        let a = Pkce::generate().unwrap();
        let b = Pkce::generate().unwrap();
        let v = a.verifier.expose();
        assert_eq!(v.len(), 86);
        assert!(v.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'));
        assert_eq!(a.challenge, challenge_for(v));
        assert_eq!(a.challenge.len(), 43);
        assert_ne!(v, b.verifier.expose());
        assert_ne!(random_token(32).unwrap(), random_token(32).unwrap());
        assert_eq!(random_token(32).unwrap().len(), 43);
    }
}
