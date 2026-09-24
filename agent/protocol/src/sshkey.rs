//! SSH public keys, as `tessaro-ctl ssh connect` sends them and root's
//! `authorized_keys` holds them.
//!
//! One line, `TYPE BASE64 [COMMENT]`, in the OpenSSH form every `.pub` file
//! is in. Kept here so the client refuses a key before sending it for the
//! same reasons the agent would.
//!
//! Options (`command=`, `from=`, `no-pty` ...) are refused, not passed
//! through: nothing tessaro-ctl produces has them, and a key line that can
//! carry them is a key line that can carry `command=` from anyone holding a
//! token.

use sha2::{Digest, Sha256};

/// What dropbear 2022.83 verifies. The two `sk-` types are FIDO keys.
pub const TYPES: &[&str] = &[
    "ssh-ed25519",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "ssh-rsa",
    "sk-ssh-ed25519@openssh.com",
    "sk-ecdsa-sha2-nistp256@openssh.com",
];

/// A 16384-bit RSA key is under 3 KiB of base64; this is far above it.
const MAX_LINE: usize = 16 * 1024;

#[derive(Debug, Clone)]
pub struct PublicKey {
    pub kind: String,
    pub blob: Vec<u8>,
    pub comment: String,
}

/// Two lines are the same key when type and blob agree; the comment is only
/// a label.
impl PartialEq for PublicKey {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.blob == other.blob
    }
}

impl Eq for PublicKey {}

impl PublicKey {
    pub fn parse(line: &str) -> Result<Self, String> {
        let line = line.trim();
        if line.is_empty() {
            return Err("the key is empty".to_string());
        }
        if line.len() > MAX_LINE {
            return Err("the key is too long to be a public key".to_string());
        }
        if line.contains(['\n', '\r']) {
            return Err("the key must be a single line".to_string());
        }

        let mut fields = line.split_ascii_whitespace();
        let kind = fields.next().unwrap_or_default();
        if !TYPES.contains(&kind) {
            if kind.contains(['=', ',']) || TYPES.iter().any(|t| line.contains(t)) {
                return Err(
                    "key options (command=, from=, ...) are not accepted; send the bare key"
                        .to_string(),
                );
            }
            if kind.starts_with("-----BEGIN") || kind.starts_with("PuTTY") {
                return Err("that is a private key; send the .pub file".to_string());
            }
            return Err(format!(
                "{kind} is not a key type the device accepts ({})",
                TYPES.join(", ")
            ));
        }

        let encoded = fields
            .next()
            .ok_or_else(|| format!("{kind} key has no key data"))?;
        let blob = data_encoding::BASE64
            .decode(encoded.as_bytes())
            .map_err(|_| "the key data is not base64".to_string())?;
        let inner = inner_type(&blob).ok_or_else(|| "the key data is malformed".to_string())?;
        if inner != kind.as_bytes() {
            return Err(format!(
                "the key data is a {} key, not {kind}",
                String::from_utf8_lossy(inner)
            ));
        }

        let comment = fields.collect::<Vec<_>>().join(" ");
        if comment.chars().any(char::is_control) {
            return Err("the key comment must not contain control characters".to_string());
        }

        Ok(Self {
            kind: kind.to_string(),
            blob,
            comment,
        })
    }

    /// `SHA256:...`, exactly what `ssh-keygen -lf` prints.
    pub fn fingerprint(&self) -> String {
        let digest = Sha256::digest(&self.blob);
        format!("SHA256:{}", data_encoding::BASE64_NOPAD.encode(&digest))
    }

    /// The `authorized_keys` line.
    pub fn line(&self) -> String {
        let encoded = data_encoding::BASE64.encode(&self.blob);
        if self.comment.is_empty() {
            format!("{} {encoded}", self.kind)
        } else {
            format!("{} {encoded} {}", self.kind, self.comment)
        }
    }
}

/// The first field of the wire format: a big-endian u32 length and the name.
fn inner_type(blob: &[u8]) -> Option<&[u8]> {
    let length = u32::from_be_bytes(blob.get(..4)?.try_into().ok()?) as usize;
    blob.get(4..4usize.checked_add(length)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    // `ssh-keygen -t ed25519 -C test@host`, and its `-lf` fingerprint.
    const ED25519: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIO7gEEtj0g4zaawVIwrP4wxLZQ2TqgASR86NTHDJ66jj test@host";
    const ED25519_FINGERPRINT: &str = "SHA256:YY7C2uXwz+G0YAPdSZsL/SANSo5RStBfYENHBRMPE7A";

    fn blob(kind: &str, tail: &[u8]) -> String {
        let mut bytes = (kind.len() as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(kind.as_bytes());
        bytes.extend_from_slice(tail);
        data_encoding::BASE64.encode(&bytes)
    }

    #[test]
    fn a_pub_file_line_round_trips() {
        let key = PublicKey::parse(&format!("{ED25519}\n")).unwrap();
        assert_eq!(key.kind, "ssh-ed25519");
        assert_eq!(key.comment, "test@host");
        assert_eq!(key.line(), ED25519);
        assert_eq!(PublicKey::parse(&key.line()).unwrap(), key);
    }

    #[test]
    fn the_fingerprint_is_ssh_keygens() {
        let key = PublicKey::parse(ED25519).unwrap();
        assert_eq!(key.fingerprint(), ED25519_FINGERPRINT);
    }

    #[test]
    fn the_comment_is_not_part_of_the_identity() {
        let a = PublicKey::parse(ED25519).unwrap();
        let b = PublicKey::parse(ED25519.trim_end_matches(" test@host")).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_eq!(b.comment, "");
    }

    #[test]
    fn a_comment_may_have_spaces() {
        let key = PublicKey::parse(&format!("{ED25519} laptop  of someone")).unwrap();
        assert_eq!(key.comment, "test@host laptop of someone");
    }

    #[test]
    fn options_are_refused() {
        let err = PublicKey::parse(&format!("command=\"/bin/sh\" {ED25519}")).unwrap_err();
        assert!(err.contains("options"), "{err}");
        let err = PublicKey::parse(&format!("no-pty {ED25519}")).unwrap_err();
        assert!(err.contains("options"), "{err}");
    }

    #[test]
    fn a_mismatched_type_is_refused() {
        let line = format!("ssh-rsa {}", blob("ssh-ed25519", &[0; 36]));
        assert!(PublicKey::parse(&line)
            .unwrap_err()
            .contains("ssh-ed25519 key"));
    }

    #[test]
    fn broken_data_is_refused() {
        assert!(PublicKey::parse("ssh-ed25519").is_err());
        assert!(PublicKey::parse("ssh-ed25519 !!!!").is_err());
        assert!(PublicKey::parse("ssh-ed25519 AAAA").is_err());
        assert!(PublicKey::parse("").is_err());
        assert!(PublicKey::parse("ssh-dss AAAA")
            .unwrap_err()
            .contains("not a key type"));
    }

    #[test]
    fn a_private_key_is_named_as_one() {
        let err = PublicKey::parse("-----BEGIN OPENSSH PRIVATE KEY-----").unwrap_err();
        assert!(err.contains("private key"), "{err}");
    }

    #[test]
    fn control_characters_in_the_comment_are_refused() {
        assert!(PublicKey::parse(&format!("{ED25519}\u{7}")).is_err());
    }

    #[test]
    fn every_listed_type_parses() {
        for kind in TYPES {
            let line = format!("{kind} {} c", blob(kind, &[1, 2, 3]));
            assert_eq!(PublicKey::parse(&line).unwrap().kind, *kind);
        }
    }
}
