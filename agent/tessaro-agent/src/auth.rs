//! Who may use the TCP API: the claim model.
//!
//! * A device with no tokens is **unclaimed**. Over TCP it answers only `id`
//!   and `claim`, and its root password is empty.
//! * The first `claim` gets a token, and the root password becomes a random
//!   one that is shown exactly once. The device is claimed from then on.
//! * Further tokens are only ever issued against a valid token (or over the
//!   local socket, which is root-only).
//! * A token is valid until it is revoked, and revoking **removes** it. There
//!   is no expiry and nothing here reads a clock: device clocks drift, and a
//!   token that expired because the RTC battery died would lock everyone out.
//! * `claimed` is not stored. It *is* "`auth.json` holds a token", so it can
//!   never disagree with the tokens.
//!
//! Only a SHA-256 of each token is kept, and comparison is constant-time.

use openssl::memcmp;
use openssl::rand::rand_bytes;
use openssl::sha::sha256;
use serde::{Deserialize, Serialize};

pub const FILE: &str = "auth.json";

const TOKEN_PREFIX: &str = "tsr_";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Auth {
    #[serde(default)]
    pub tokens: Vec<TokenEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenEntry {
    pub id: String,
    pub name: String,
    pub sha256: String,
    /// The id of the token that issued this one, `claim`, or `local`.
    pub issued_by: String,
}

impl Auth {
    pub fn claimed(&self) -> bool {
        !self.tokens.is_empty()
    }

    /// The entry a presented token belongs to. Every entry is compared, match
    /// or not, so the time taken says nothing about which one matched.
    pub fn verify(&self, presented: &str) -> Option<&TokenEntry> {
        let presented = sha256(presented.as_bytes());
        let mut found = None;
        for entry in &self.tokens {
            let Some(stored) = unhex(&entry.sha256) else {
                continue;
            };
            if stored.len() == presented.len() && memcmp::eq(&stored, &presented) {
                found = Some(entry);
            }
        }
        found
    }

    /// A new token for `name`. The secret is returned once and never stored.
    pub fn issue(&mut self, name: &str, issued_by: &str) -> Result<(TokenEntry, String), String> {
        let secret = format!("{TOKEN_PREFIX}{}", hex(&random(32)?));
        let mut id = hex(&random(4)?);
        while self.tokens.iter().any(|entry| entry.id == id) {
            id = hex(&random(4)?);
        }

        let entry = TokenEntry {
            id,
            name: name.to_string(),
            sha256: hex(&sha256(secret.as_bytes())),
            issued_by: issued_by.to_string(),
        };
        self.tokens.push(entry.clone());
        Ok((entry, secret))
    }

    pub fn revoke(&mut self, id: &str) -> Option<TokenEntry> {
        let at = self.tokens.iter().position(|entry| entry.id == id)?;
        Some(self.tokens.remove(at))
    }
}

/// A root password a human can read off a terminal and type: 20 characters,
/// no look-alikes (0/O, 1/l/I). About 116 bits.
pub fn random_password() -> Result<String, String> {
    const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    // The largest multiple of the alphabet that fits in a byte, so every
    // character is equally likely.
    let limit = (256 / ALPHABET.len() * ALPHABET.len()) as u8;

    let mut password = String::with_capacity(20);
    while password.len() < 20 {
        for byte in random(32)? {
            if byte < limit && password.len() < 20 {
                password.push(ALPHABET[byte as usize % ALPHABET.len()] as char);
            }
        }
    }
    Ok(password)
}

pub fn random(len: usize) -> Result<Vec<u8>, String> {
    let mut bytes = vec![0u8; len];
    rand_bytes(&mut bytes).map_err(|err| format!("no randomness: {err}"))?;
    Ok(bytes)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
        .collect()
}

/// A token or client name as it will be shown in `token list`.
pub fn check_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 64 || name.chars().any(char::is_control) {
        return Err("a name must be 1 to 64 printable characters".to_string());
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_issued_token_verifies_and_nothing_else_does() {
        let mut auth = Auth::default();
        assert!(!auth.claimed());

        let (entry, secret) = auth.issue("laptop", "claim").unwrap();
        assert!(auth.claimed());
        assert!(secret.starts_with("tsr_"));
        assert_eq!(auth.verify(&secret).unwrap().id, entry.id);
        assert!(auth.verify("tsr_nope").is_none());
        assert!(auth.verify("").is_none());

        // The secret itself is never stored.
        let file = serde_json::to_string(&auth).unwrap();
        assert!(!file.contains(&secret));
    }

    #[test]
    fn revoking_removes_the_entry() {
        let mut auth = Auth::default();
        let (first, first_secret) = auth.issue("a", "claim").unwrap();
        let (_, second_secret) = auth.issue("b", &first.id).unwrap();

        assert_eq!(auth.revoke(&first.id).unwrap().name, "a");
        assert!(auth.verify(&first_secret).is_none());
        assert!(auth.verify(&second_secret).is_some());
        assert_eq!(auth.tokens.len(), 1);
        assert!(auth.revoke(&first.id).is_none());
    }

    #[test]
    fn passwords_are_twenty_unambiguous_characters() {
        for _ in 0..50 {
            let password = random_password().unwrap();
            assert_eq!(password.len(), 20);
            assert!(!password.contains(['0', 'O', '1', 'l', 'I']));
            protocol::check_password(&password).unwrap();
        }
    }

    #[test]
    fn names() {
        assert_eq!(check_name("  laptop ").unwrap(), "laptop");
        assert!(check_name("").is_err());
        assert!(check_name("a\nb").is_err());
    }
}
