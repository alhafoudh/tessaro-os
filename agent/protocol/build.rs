//! `TESSARO_VERSION` for `protocol::VERSION`: the release every binary
//! reports. Taken from the environment when set (the tessaro-kiosk recipe sets
//! it from DISTRO_VERSION, its tree has no VERSION file), otherwise from
//! VERSION at the root of the repo, which tessaro.conf reads too.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=TESSARO_VERSION");
    let version = match std::env::var("TESSARO_VERSION") {
        Ok(version) => version,
        Err(_) => {
            let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../VERSION");
            println!("cargo:rerun-if-changed={}", file.display());
            std::fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("{}: {e}; set TESSARO_VERSION", file.display()))
        }
    };
    let version = version.trim();
    assert!(!version.is_empty(), "TESSARO_VERSION is empty");
    println!("cargo:rustc-env=TESSARO_VERSION={version}");
}
