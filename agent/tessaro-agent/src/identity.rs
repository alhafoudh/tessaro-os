//! Who this device is: its node id, its name, and its TLS identity.
//!
//! * **Node id** is systemd's app-specific machine id,
//!   `sd_id128_get_machine_app_specific`: HMAC-SHA256 keyed by
//!   `/etc/machine-id` over a fixed Tessaro application id, cut to 128 bits
//!   and stamped as a v4 UUID. The machine id itself is confidential (systemd
//!   says so) and never leaves the device; this is what goes on the wire,
//!   into mDNS and into a technician's known nodes.
//! * **Name** is an adjective-noun pair and four hex digits, all taken from
//!   the node id, so a device always answers to the same name. The digits
//!   make two devices on one segment colliding a 1-in-16-million event rather
//!   than a 1-in-2000 one. `device.name` overrides it.
//! * **TLS identity** is an EC P-256 key and a self-signed certificate made on
//!   first start, under `/data/tessaro/tls/`. Clients pin its SHA-256; there
//!   is no chain to verify, so its validity dates are set to cover all time
//!   and a device whose clock is wrong still works.
//!
//! Wiping `/data` - or the `/etc` overlay, which holds the machine id -
//! re-identifies the device. That is accepted, and documented.

use std::fs;
use std::io;
use std::path::Path;

use openssl::asn1::Asn1Time;
use openssl::bn::{BigNum, MsbOption};
use openssl::ec::{EcGroup, EcKey};
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::PKey;
use openssl::sha::sha256;
use openssl::sign::Signer;
use openssl::x509::{X509Builder, X509NameBuilder, X509Ref, X509};

use protocol::hex;

use crate::store;

/// The Tessaro application id. **Never change it**: it would rename every
/// device in the field.
pub const APP_ID: &str = "8a6c7b172d5443cd9033a24d0df85022";

pub fn node_id(machine_id: &str) -> Result<String, String> {
    let machine = parse_id128(machine_id.trim())
        .ok_or_else(|| "the machine id is not 32 hex digits".to_string())?;
    let app = parse_id128(APP_ID).expect("APP_ID is a valid id");

    let key = PKey::hmac(&machine).map_err(|err| err.to_string())?;
    let mut signer = Signer::new(MessageDigest::sha256(), &key).map_err(|err| err.to_string())?;
    signer.update(&app).map_err(|err| err.to_string())?;
    let mac = signer.sign_to_vec().map_err(|err| err.to_string())?;

    let mut id = [0u8; 16];
    id.copy_from_slice(&mac[..16]);
    // id128_make_v4_uuid(): version 4, RFC 4122 variant.
    id[6] = (id[6] & 0x0f) | 0x40;
    id[8] = (id[8] & 0x3f) | 0x80;
    Ok(hex(&id))
}

pub fn read_node_id(path: &Path) -> Result<String, String> {
    let text = fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    node_id(&text)
}

fn parse_id128(text: &str) -> Option<[u8; 16]> {
    if text.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (at, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(at * 2..at * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

const ADJECTIVES: [&str; 64] = [
    "amber", "ample", "azure", "bold", "brave", "brisk", "calm", "clear", "clever", "cosy",
    "crisp", "dapper", "deft", "eager", "early", "fair", "fancy", "fleet", "fond", "frank",
    "fresh", "gentle", "glad", "golden", "grand", "happy", "hardy", "jolly", "keen", "kind",
    "lively", "lucky", "merry", "mild", "misty", "modest", "neat", "noble", "plucky", "polite",
    "proud", "quick", "quiet", "rapid", "ready", "rosy", "royal", "rustic", "sharp", "shiny",
    "silver", "sleek", "smart", "snowy", "solid", "sturdy", "sunny", "swift", "tidy", "true",
    "vivid", "warm", "witty", "zesty",
];

const NOUNS: [&str; 64] = [
    "badger", "beacon", "birch", "bison", "brook", "canyon", "cedar", "comet", "coral", "crane",
    "delta", "dune", "eagle", "ember", "falcon", "fern", "fjord", "forest", "fox", "garnet",
    "glacier", "harbor", "hawk", "heron", "island", "jaguar", "kestrel", "lagoon", "lark", "lynx",
    "maple", "meadow", "mesa", "moose", "nebula", "oak", "orca", "otter", "owl", "panda", "pebble",
    "pine", "plover", "prairie", "quartz", "raven", "reef", "ridge", "river", "robin", "sable",
    "salmon", "sparrow", "spruce", "summit", "swan", "thistle", "tiger", "tundra", "valley",
    "walrus", "willow", "wren", "yak",
];

/// `brave-otter-3fa2`, from the node id alone.
pub fn friendly_name(node_id: &str) -> String {
    let byte = |at: usize| {
        node_id
            .get(at * 2..at * 2 + 2)
            .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            .unwrap_or(0)
    };
    let suffix = node_id.get(4..8).unwrap_or("0000");
    format!(
        "{}-{}-{suffix}",
        ADJECTIVES[byte(0) as usize % 64],
        NOUNS[byte(1) as usize % 64]
    )
}

#[derive(Debug, Clone)]
pub struct Tls {
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
    /// SHA-256 of the DER certificate, lower-case hex.
    pub fingerprint: String,
}

/// Load the device's TLS identity, making one the first time.
pub fn tls(dir: &Path) -> io::Result<(Tls, bool)> {
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");

    if let (Ok(cert_pem), Ok(key_pem)) = (fs::read(&cert_path), fs::read(&key_path)) {
        if let Ok(cert) = X509::from_pem(&cert_pem) {
            if PKey::private_key_from_pem(&key_pem).is_ok() {
                let fingerprint = fingerprint(&cert)?;
                return Ok((
                    Tls {
                        cert_pem,
                        key_pem,
                        fingerprint,
                    },
                    false,
                ));
            }
        }
    }

    let (cert, key) = generate().map_err(io::Error::other)?;
    let cert_pem = cert.to_pem().map_err(io::Error::other)?;
    let key_pem = key.private_key_to_pem_pkcs8().map_err(io::Error::other)?;

    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    // Key first: a cert without its key is regenerated next time anyway.
    store::replace_if_changed(&key_path, &key_pem, 0o600)?;
    store::replace_if_changed(&cert_path, &cert_pem, 0o644)?;

    let fingerprint = fingerprint(&cert)?;
    Ok((
        Tls {
            cert_pem,
            key_pem,
            fingerprint,
        },
        true,
    ))
}

/// SHA-256 of the certificate's DER, lower-case hex: the device's own, and
/// each extra CA's (`certs.rs`).
pub(crate) fn fingerprint(cert: &X509Ref) -> io::Result<String> {
    let der = cert.to_der().map_err(io::Error::other)?;
    Ok(hex(&sha256(&der)))
}

fn generate() -> Result<(X509, PKey<openssl::pkey::Private>), openssl::error::ErrorStack> {
    let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1)?;
    let key = PKey::from_ec_key(EcKey::generate(&group)?)?;

    let mut name = X509NameBuilder::new()?;
    name.append_entry_by_nid(Nid::COMMONNAME, "tessaro")?;
    let name = name.build();

    let mut serial = BigNum::new()?;
    serial.rand(127, MsbOption::MAYBE_ZERO, false)?;

    let serial = serial.to_asn1_integer()?;
    // All of time, on purpose: nothing verifies dates, and a kiosk whose RTC
    // reset to 1970 must still be reachable.
    let not_before = Asn1Time::from_unix(0)?;
    let not_after = Asn1Time::from_str("99991231235959Z")?;

    let mut builder = X509Builder::new()?;
    builder.set_version(2)?;
    builder.set_serial_number(&serial)?;
    builder.set_subject_name(&name)?;
    builder.set_issuer_name(&name)?;
    builder.set_pubkey(&key)?;
    builder.set_not_before(&not_before)?;
    builder.set_not_after(&not_after)?;
    builder.sign(&key, MessageDigest::sha256())?;

    Ok((builder.build(), key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_id_is_a_v4_uuid_and_not_the_machine_id() {
        let machine = "0123456789abcdef0123456789abcdef";
        let id = node_id(machine).unwrap();

        assert_eq!(id.len(), 32);
        assert_ne!(id, machine);
        assert_eq!(&id[12..13], "4");
        assert!(matches!(&id[16..17], "8" | "9" | "a" | "b"));
        assert_eq!(node_id(&format!("{machine}\n")).unwrap(), id);
        assert!(node_id("nope").is_err());
    }

    /// The whole point of reimplementing it is to agree with systemd, so ask
    /// systemd. Skipped where there is no systemd-id128 or no machine id.
    #[test]
    fn agrees_with_systemd_id128() {
        let Ok(machine) = fs::read_to_string("/etc/machine-id") else {
            return;
        };
        let Ok(output) = std::process::Command::new("systemd-id128")
            .args(["-a", APP_ID, "machine-id"])
            .output()
        else {
            return;
        };
        if !output.status.success() {
            return;
        }

        let theirs = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert_eq!(node_id(&machine).unwrap(), theirs);
    }

    #[test]
    fn the_name_is_stable_and_a_dns_label() {
        let id = node_id("0123456789abcdef0123456789abcdef").unwrap();
        let name = friendly_name(&id);

        assert_eq!(name, friendly_name(&id));
        assert!(protocol::keys::is_label(&name), "{name}");
        assert!(name.ends_with(&id[4..8]));
    }

    #[test]
    fn the_tls_identity_is_made_once_and_then_kept() {
        let dir = tempfile::tempdir().unwrap();
        let tls_dir = dir.path().join("tls");

        let (first, made) = tls(&tls_dir).unwrap();
        assert!(made);
        assert_eq!(first.fingerprint.len(), 64);

        let (second, made) = tls(&tls_dir).unwrap();
        assert!(!made);
        assert_eq!(first.fingerprint, second.fingerprint);

        // native-tls, which the server builds its acceptor with, takes it.
        native_tls::Identity::from_pkcs8(&second.cert_pem, &second.key_pem).unwrap();
    }
}
