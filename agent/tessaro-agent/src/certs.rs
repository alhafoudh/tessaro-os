//! The extra certificate authorities the device trusts, on top of the
//! image's `ca-certificates` bundle: one `<fingerprint>.pem` per certificate
//! in `/data/tessaro/ca-certs`, added and revoked with `tessaro-ctl network
//! certs`.
//!
//! They reach the two TLS clients that matter separately. Chromium gets them
//! as the `CACertificates` policy (`render.rs`); the agent's own openssl
//! client adds them as roots when it is built (`http.rs`). `/etc/ssl/certs`
//! is never touched: `update-ca-certificates` would copy the bundle up into
//! the `/etc` overlay, where it would shadow the image's forever.
//!
//! A certificate is kept as it was sent, re-encoded as one PEM block, so the
//! file name and the fingerprint are both the SHA-256 of its DER.

use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

use openssl::asn1::Asn1Time;
use openssl::x509::{X509NameRef, X509Ref, X509VerifyResult, X509};
use protocol::{CertInfo, CERT_PEM_MAX as MAX_PEM};

use crate::store;

/// The most certificates the store holds. Each one is in the Chromium
/// policy, which Chromium reads whole on every change.
pub const MAX_CERTS: usize = 32;

/// What `add` did with the certificates it was sent.
#[derive(Debug, Default)]
pub struct Added {
    pub added: Vec<CertInfo>,
    /// Already in the store; nothing written.
    pub present: Vec<CertInfo>,
}

/// The certificates in `text`, in order. Refuses a private key - that never
/// belongs on the device - and text with no certificate in it.
pub fn parse(text: &str) -> Result<Vec<X509>, String> {
    if text.len() > MAX_PEM {
        return Err(format!(
            "that is {} bytes; a certificate file is at most {MAX_PEM}",
            text.len()
        ));
    }
    if text.contains("PRIVATE KEY-----") {
        return Err("that file holds a private key; send only the certificate".to_string());
    }
    if !text.contains("-----BEGIN CERTIFICATE-----") {
        return Err("no PEM certificate in it (-----BEGIN CERTIFICATE-----)".to_string());
    }
    let certs = X509::stack_from_pem(text.as_bytes())
        .map_err(|err| format!("not a readable PEM certificate: {err}"))?;
    if certs.is_empty() {
        return Err("no PEM certificate in it (-----BEGIN CERTIFICATE-----)".to_string());
    }
    Ok(certs)
}

/// The stored certificates, ordered by subject, then fingerprint. Files that
/// do not parse are skipped: a hand-placed one must not stop the rest.
pub fn load(dir: &Path) -> io::Result<Vec<X509>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut certs: Vec<(CertInfo, X509)> = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "pem") {
            continue;
        }
        let Ok(pem) = fs::read(&path) else { continue };
        let Ok(cert) = X509::from_pem(&pem) else {
            continue;
        };
        if let Ok(info) = info(&cert) {
            if !certs
                .iter()
                .any(|(seen, _)| seen.fingerprint == info.fingerprint)
            {
                certs.push((info, cert));
            }
        }
    }
    certs.sort_by(|(a, _), (b, _)| (&a.subject, &a.fingerprint).cmp(&(&b.subject, &b.fingerprint)));
    Ok(certs.into_iter().map(|(_, cert)| cert).collect())
}

pub fn list(dir: &Path) -> io::Result<Vec<CertInfo>> {
    load(dir)?.iter().map(|cert| info(cert)).collect()
}

/// Store every certificate in `text` that is not there yet. All or nothing
/// for the checks: one unreadable block, or too many for the store, and
/// nothing is written.
pub fn add(dir: &Path, text: &str) -> Result<Added, String> {
    let certs = parse(text)?;
    let stored = list(dir).map_err(|err| format!("{}: {err}", dir.display()))?;

    let mut result = Added::default();
    let mut new: Vec<(CertInfo, X509)> = Vec::new();
    for cert in certs {
        let info = info(&cert).map_err(|err| err.to_string())?;
        if stored
            .iter()
            .any(|seen| seen.fingerprint == info.fingerprint)
            || new
                .iter()
                .any(|(seen, _)| seen.fingerprint == info.fingerprint)
        {
            result.present.push(info);
        } else {
            new.push((info, cert));
        }
    }
    if stored.len() + new.len() > MAX_CERTS {
        return Err(format!(
            "the device holds at most {MAX_CERTS} certificates and has {}; \
             revoke some with `tessaro-ctl network certs revoke`",
            stored.len()
        ));
    }

    if !new.is_empty() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o755)
            .create(dir)
            .map_err(|err| format!("{}: {err}", dir.display()))?;
    }
    for (info, cert) in new {
        let pem = cert.to_pem().map_err(|err| err.to_string())?;
        let path = dir.join(format!("{}.pem", info.fingerprint));
        store::replace(&path, &pem, 0o644, None)
            .map_err(|err| format!("{}: {err}", path.display()))?;
        result.added.push(info);
    }
    Ok(result)
}

/// Remove the one certificate `query` names: its fingerprint, a unique
/// prefix of that, or its exact subject.
pub fn remove(dir: &Path, query: &str) -> Result<CertInfo, String> {
    let certs = list(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let cert = select(&certs, query)?.clone();
    let path = dir.join(format!("{}.pem", cert.fingerprint));
    fs::remove_file(&path).map_err(|err| format!("{}: {err}", path.display()))?;
    store::sync_dir(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    Ok(cert)
}

/// Remove the whole store, if there is one. Returns whether there was.
pub fn clear(dir: &Path) -> io::Result<bool> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

/// Each stored certificate as base64 DER, which is what Chromium's
/// `CACertificates` policy takes.
pub fn policy_entries(dir: &Path) -> io::Result<Vec<String>> {
    load(dir)?
        .iter()
        .map(|cert| {
            cert.to_der()
                .map(|der| openssl::base64::encode_block(&der))
                .map_err(io::Error::other)
        })
        .collect()
}

pub fn info(cert: &X509Ref) -> io::Result<CertInfo> {
    let epoch = Asn1Time::from_unix(0).map_err(io::Error::other)?;
    let not_after = epoch.diff(cert.not_after()).map_err(io::Error::other)?;
    Ok(CertInfo {
        fingerprint: crate::identity::fingerprint(cert)?,
        subject: name(cert.subject_name()),
        issuer: name(cert.issuer_name()),
        not_after: i64::from(not_after.days) * 86_400 + i64::from(not_after.secs),
        self_signed: cert.issued(cert) == X509VerifyResult::OK,
    })
}

/// A name as `CN=Corp Root CA, O=Corp`: most specific first, the way people
/// read them, and its short attribute names.
fn name(name: &X509NameRef) -> String {
    let mut parts: Vec<String> = name
        .entries()
        .map(|entry| {
            let key = entry.object().nid().short_name().unwrap_or("?");
            let value = entry.data().to_string().unwrap_or_else(|_| "?".to_string());
            format!("{key}={value}")
        })
        .collect();
    parts.reverse();
    parts.join(", ")
}

fn select<'a>(certs: &'a [CertInfo], query: &str) -> Result<&'a CertInfo, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err(
            "name a certificate: its fingerprint, a prefix of it, or its subject".to_string(),
        );
    }
    let bare = query.to_ascii_lowercase().replace(':', "");

    let exact: Vec<&CertInfo> = certs
        .iter()
        .filter(|cert| cert.fingerprint == bare)
        .collect();
    let by_subject: Vec<&CertInfo> = certs.iter().filter(|cert| cert.subject == query).collect();
    // A prefix shorter than this is more likely a typo than a choice.
    let by_prefix: Vec<&CertInfo> = if bare.len() >= 4 {
        certs
            .iter()
            .filter(|cert| cert.fingerprint.starts_with(&bare))
            .collect()
    } else {
        Vec::new()
    };

    for candidates in [exact, by_subject, by_prefix] {
        match candidates.as_slice() {
            [] => continue,
            [one] => return Ok(one),
            many => {
                let names: Vec<String> = many
                    .iter()
                    .map(|cert| format!("{} {}", cert.fingerprint, cert.subject))
                    .collect();
                return Err(format!(
                    "{query} matches {} certificates; use the fingerprint:\n  {}",
                    many.len(),
                    names.join("\n  ")
                ));
            }
        }
    }
    Err(format!(
        "no certificate matches {query}; `tessaro-ctl network certs list` shows them"
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use openssl::asn1::Asn1Integer;
    use openssl::bn::BigNum;
    use openssl::ec::{EcGroup, EcKey};
    use openssl::hash::MessageDigest;
    use openssl::nid::Nid;
    use openssl::pkey::{PKey, Private};
    use openssl::x509::extension::BasicConstraints;
    use openssl::x509::X509NameBuilder;

    /// A self-signed CA named `CN=<cn>`, valid for `days`, and its key.
    pub(crate) fn ca(cn: &str, days: u32) -> (X509, PKey<Private>) {
        let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap();
        let key = PKey::from_ec_key(EcKey::generate(&group).unwrap()).unwrap();
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_nid(Nid::ORGANIZATIONNAME, "Test")
            .unwrap();
        name.append_entry_by_nid(Nid::COMMONNAME, cn).unwrap();
        let name = name.build();

        let mut cert = X509::builder().unwrap();
        cert.set_version(2).unwrap();
        let serial = Asn1Integer::from_bn(&BigNum::from_u32(1).unwrap()).unwrap();
        cert.set_serial_number(&serial).unwrap();
        cert.set_subject_name(&name).unwrap();
        cert.set_issuer_name(&name).unwrap();
        cert.set_pubkey(&key).unwrap();
        cert.set_not_before(&Asn1Time::days_from_now(0).unwrap())
            .unwrap();
        cert.set_not_after(&Asn1Time::days_from_now(days).unwrap())
            .unwrap();
        cert.append_extension(BasicConstraints::new().critical().ca().build().unwrap())
            .unwrap();
        cert.sign(&key, MessageDigest::sha256()).unwrap();
        (cert.build(), key)
    }

    fn pem(cert: &X509) -> String {
        String::from_utf8(cert.to_pem().unwrap()).unwrap()
    }

    #[test]
    fn a_chain_is_stored_once_per_certificate() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("ca-certs");
        let (a, _) = ca("Root A", 30);
        let (b, _) = ca("Root B", 30);

        let added = add(&store, &format!("{}\n{}", pem(&a), pem(&b))).unwrap();
        assert_eq!(added.added.len(), 2);
        assert!(added.present.is_empty());

        let again = add(&store, &pem(&a)).unwrap();
        assert!(again.added.is_empty());
        assert_eq!(again.present[0].subject, "CN=Root A, O=Test");

        let listed = list(&store).unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|c| c.subject.as_str())
                .collect::<Vec<_>>(),
            ["CN=Root A, O=Test", "CN=Root B, O=Test"]
        );
        assert!(listed[0].self_signed);
        assert_eq!(listed[0].issuer, listed[0].subject);
        assert_eq!(
            fs::read(store.join(format!("{}.pem", listed[0].fingerprint))).unwrap(),
            a.to_pem().unwrap()
        );
        assert_eq!(policy_entries(&store).unwrap().len(), 2);
    }

    #[test]
    fn not_after_is_seconds_since_the_epoch() {
        let (a, _) = ca("Root", 10);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let left = info(&a).unwrap().not_after - now;
        assert!(
            (10 * 86_400 - 60..=10 * 86_400 + 60).contains(&left),
            "{left}"
        );
    }

    #[test]
    fn junk_keys_and_oversized_text_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("ca-certs");
        assert!(add(&store, "hello")
            .unwrap_err()
            .contains("no PEM certificate"));
        assert!(add(
            &store,
            "-----BEGIN CERTIFICATE-----\nnope\n-----END CERTIFICATE-----\n"
        )
        .unwrap_err()
        .contains("not a readable"));
        let (a, key) = ca("Root", 30);
        let with_key = format!(
            "{}{}",
            pem(&a),
            String::from_utf8(key.private_key_to_pem_pkcs8().unwrap()).unwrap()
        );
        assert!(add(&store, &with_key).unwrap_err().contains("private key"));
        assert!(add(&store, &"x".repeat(MAX_PEM + 1))
            .unwrap_err()
            .contains("at most"));
        assert!(!store.exists(), "nothing refused leaves a store behind");
    }

    #[test]
    fn the_store_has_a_limit() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("ca-certs");
        for n in 0..MAX_CERTS {
            add(&store, &pem(&ca(&format!("Root {n}"), 30).0)).unwrap();
        }
        let err = add(&store, &pem(&ca("One more", 30).0)).unwrap_err();
        assert!(err.contains("at most"), "{err}");
        assert_eq!(list(&store).unwrap().len(), MAX_CERTS);
    }

    #[test]
    fn revoke_by_fingerprint_prefix_or_subject() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("ca-certs");
        for cn in ["One", "Two", "Three"] {
            add(&store, &pem(&ca(cn, 30).0)).unwrap();
        }
        let listed = list(&store).unwrap();
        let one = listed
            .iter()
            .find(|c| c.subject.starts_with("CN=One"))
            .unwrap();
        let three = listed
            .iter()
            .find(|c| c.subject.starts_with("CN=Three"))
            .unwrap();

        // Upper case with colons, the way other tools print one.
        let colons: Vec<String> = one
            .fingerprint
            .to_uppercase()
            .as_bytes()
            .chunks(2)
            .map(|pair| String::from_utf8(pair.to_vec()).unwrap())
            .collect();
        assert_eq!(remove(&store, &colons.join(":")).unwrap(), *one);
        assert_eq!(
            remove(&store, "CN=Two, O=Test").unwrap().subject,
            "CN=Two, O=Test"
        );
        assert_eq!(remove(&store, &three.fingerprint[..8]).unwrap(), *three);
        assert!(list(&store).unwrap().is_empty());
        assert!(remove(&store, "CN=Two, O=Test")
            .unwrap_err()
            .contains("no certificate matches"));
        assert!(remove(&store, "abc").is_err(), "too short to be a prefix");
    }

    #[test]
    fn an_ambiguous_subject_removes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("ca-certs");
        add(&store, &pem(&ca("Same", 30).0)).unwrap();
        add(&store, &pem(&ca("Same", 30).0)).unwrap();

        let err = remove(&store, "CN=Same, O=Test").unwrap_err();
        assert!(err.contains("matches 2 certificates"), "{err}");
        assert_eq!(list(&store).unwrap().len(), 2);
    }

    #[test]
    fn a_stray_file_is_skipped_and_clear_takes_everything() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("ca-certs");
        assert!(!clear(&store).unwrap());
        add(&store, &pem(&ca("Root", 30).0)).unwrap();
        fs::write(store.join("junk.pem"), "junk").unwrap();
        fs::write(store.join("notes.txt"), "junk").unwrap();

        assert_eq!(list(&store).unwrap().len(), 1);
        assert!(clear(&store).unwrap());
        assert!(!store.exists());
    }
}
