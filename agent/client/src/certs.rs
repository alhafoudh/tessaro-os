//! The certificate files `tessaro-ctl network certs add` and the GUI's
//! Network page send, and the words both use for what comes back.
//! The device parses and checks the certificates; this only gets the file
//! into the one form the protocol carries, PEM text.

use std::fs;
use std::path::Path;

/// The certificates in the file at `path`, as PEM text. PEM is sent as it
/// is; a binary DER certificate (a `.cer` or `.crt` as Windows exports
/// them) is wrapped into PEM first. A private key is refused here, before
/// it leaves this machine.
pub fn read_pem(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
    if bytes.len() > protocol::CERT_PEM_MAX {
        return Err(format!(
            "{}: {} bytes; a certificate file is at most {}",
            path.display(),
            bytes.len(),
            protocol::CERT_PEM_MAX
        ));
    }
    let pem = match String::from_utf8(bytes) {
        Ok(text) if text.contains("-----BEGIN") => text,
        // DER: a SEQUENCE, so its first byte is 0x30.
        Ok(text) if text.as_bytes().first() == Some(&0x30) => der_to_pem(text.as_bytes()),
        Err(err) if err.as_bytes().first() == Some(&0x30) => der_to_pem(err.as_bytes()),
        _ => {
            return Err(format!(
                "{}: not a certificate (neither PEM nor DER)",
                path.display()
            ))
        }
    };
    if pem.contains("PRIVATE KEY-----") {
        return Err(format!(
            "{}: holds a private key; send only the certificate",
            path.display()
        ));
    }
    if !pem.contains("-----BEGIN CERTIFICATE-----") {
        return Err(format!("{}: no certificate in it", path.display()));
    }
    Ok(pem)
}

fn der_to_pem(der: &[u8]) -> String {
    let body = data_encoding::BASE64.encode(der);
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for line in body.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    pem
}

/// Seconds since the epoch as a UTC date, `2036-09-25`.
pub fn date(unix: i64) -> String {
    // Howard Hinnant's days-to-civil, for the proleptic Gregorian calendar.
    let days = unix.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Whether a certificate that expires at `not_after` has, by this machine's
/// clock.
pub fn expired(not_after: i64) -> bool {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|now| now.as_secs() as i64 > not_after)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_utc_calendar_days() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(date(2_105_827_199), "2036-09-23");
        assert_eq!(date(-86_400), "1969-12-31");
    }

    #[test]
    fn pem_is_sent_as_is_and_der_is_wrapped() {
        let dir = tempfile::tempdir().unwrap();
        let pem = dir.path().join("ca.pem");
        let text = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n";
        fs::write(&pem, text).unwrap();
        assert_eq!(read_pem(&pem).unwrap(), text);

        let der = dir.path().join("ca.cer");
        fs::write(&der, [0x30u8, 0x82, 0xff, 0x00]).unwrap();
        assert_eq!(
            read_pem(&der).unwrap(),
            "-----BEGIN CERTIFICATE-----\nMIL/AA==\n-----END CERTIFICATE-----\n"
        );
    }

    #[test]
    fn keys_and_other_files_stay_here() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("key.pem");
        fs::write(
            &key,
            "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n\
             -----BEGIN PRIVATE KEY-----\nMIIE\n-----END PRIVATE KEY-----\n",
        )
        .unwrap();
        assert!(read_pem(&key).unwrap_err().contains("private key"));

        let text = dir.path().join("notes.txt");
        fs::write(&text, "hello").unwrap();
        assert!(read_pem(&text).unwrap_err().contains("not a certificate"));
        assert!(read_pem(&dir.path().join("missing.pem")).is_err());
    }
}
