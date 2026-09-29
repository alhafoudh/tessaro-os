//! The browser policy files `tessaro-ctl browser policies set` and the
//! GUI's Policies page read. The device checks every document again; this
//! runs the same check (`protocol::policy::check`) first, so a mistake is
//! shown with its line before anything is sent.

use std::fs;
use std::io::Read;
use std::path::Path;

use protocol::policy::{self, POLICY_TEXT_MAX};

/// The text of the policy file at `path`, `-` for standard input, once it
/// passes the check.
pub fn read_file(path: &Path) -> Result<String, String> {
    let text = if path.as_os_str() == "-" {
        let mut text = String::new();
        std::io::stdin()
            .take(POLICY_TEXT_MAX as u64 + 1)
            .read_to_string(&mut text)
            .map_err(|err| format!("standard input: {err}"))?;
        text
    } else {
        let size = fs::metadata(path)
            .map_err(|err| format!("{}: {err}", path.display()))?
            .len();
        if size > POLICY_TEXT_MAX as u64 {
            return Err(format!(
                "{}: {size} bytes; a policy is at most {POLICY_TEXT_MAX}",
                path.display()
            ));
        }
        fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?
    };
    policy::check(&text).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_read_as_typed_once_it_passes() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("lockdown.json");
        fs::write(&good, "// mine\n{\"SpellcheckEnabled\": false,}\n").unwrap();
        assert!(read_file(&good).unwrap().starts_with("// mine"));

        let bad = dir.path().join("bad.json");
        fs::write(&bad, "{\n  \"A\": 1\n  \"B\": 2\n}").unwrap();
        let err = read_file(&bad).unwrap_err();
        assert!(err.contains("bad.json: line 3 column 3"), "{err}");

        let managed = dir.path().join("certs.json");
        fs::write(&managed, "{\"CACertificates\": []}").unwrap();
        assert!(read_file(&managed)
            .unwrap_err()
            .contains("network certs add"));
    }
}
