//! The files the agent renders for systemd from its store (`scripts.rs`,
//! `schedules.rs`): unit files in `/run/systemd/system` and the script
//! bodies they run, kept equal to what the store wants by
//! `control/schedules.rs`.
//!
//! Everything here is pure or blocking file work.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::Path;

use crate::store;

/// Stands in for systemd's `%i` while a command is escaped, which would
/// double its `%`.
pub const INSTANCE: &str = "@@INSTANCE@@";

pub const HEADER: &str = "# Written by tessaro-agent from tessaro.db at every start and change;\n\
                          # edits here do not last. See docs/scripts.md.\n";

/// `text` as one argument of an `ExecStart=` line: in double quotes, with
/// what systemd would otherwise interpret escaped. Backslashes are C
/// escapes inside the quotes, `%` starts a specifier and `$` an environment
/// variable.
pub fn exec_arg(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for ch in text.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '%' => quoted.push_str("%%"),
            '$' => quoted.push_str("$$"),
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

/// `script` as `/bin/sh -c` arguments of an `Exec*=` line, with
/// `INSTANCE` turned into `%i`.
pub fn shell(script: &str) -> String {
    format!("/bin/sh -c {}", exec_arg(script).replace(INSTANCE, "%i"))
}

/// The template an instance was started from:
/// `tessaro-script-ab12-cd34@manual-1700000000-42.service` is
/// `tessaro-script-ab12-cd34@.service`.
pub fn template_of(instance: &str) -> Option<String> {
    let (template, rest) = instance.split_once('@')?;
    (!template.is_empty() && rest.ends_with(".service") && rest != ".service")
        .then(|| format!("{template}@.service"))
}

/// What to do to a directory of rendered files.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub write: Vec<(String, String)>,
    pub remove: Vec<String>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.write.is_empty() && self.remove.is_empty()
    }
}

/// From the files `present` to `wanted`: what differs is written, what is
/// not wanted goes, except what is in `busy` (a run template, or a body, with
/// runs going), which goes once they end.
pub fn plan(
    wanted: &BTreeMap<String, String>,
    present: &BTreeMap<String, Vec<u8>>,
    busy: &BTreeSet<String>,
) -> Plan {
    let write = wanted
        .iter()
        .filter(|(name, body)| present.get(*name).map(Vec::as_slice) != Some(body.as_bytes()))
        .map(|(name, body)| (name.clone(), body.clone()))
        .collect();
    let remove = present
        .keys()
        .filter(|name| !wanted.contains_key(*name) && !busy.contains(*name))
        .cloned()
        .collect();
    Plan { write, remove }
}

/// The files in `dir` whose names start with one of `prefixes`, with their
/// content. A missing directory has none.
pub fn present(dir: &Path, prefixes: &[&str]) -> io::Result<BTreeMap<String, Vec<u8>>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(err) => return Err(err),
    };
    let mut files = BTreeMap::new();
    for entry in entries {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if prefixes.iter().any(|prefix| name.starts_with(prefix)) {
            files.insert(name, fs::read(entry.path())?);
        }
    }
    Ok(files)
}

/// `plan` done in `dir`, each file written with `mode`.
pub fn apply(dir: &Path, plan: &Plan, mode: u32) -> io::Result<()> {
    if !plan.write.is_empty() {
        fs::create_dir_all(dir)?;
    }
    for (name, body) in &plan.write {
        store::replace_if_changed(&dir.join(name), body.as_bytes(), mode)?;
    }
    for name in &plan.remove {
        match fs::remove_file(dir.join(name)) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_arg_escapes_what_systemd_would_interpret() {
        assert_eq!(exec_arg("echo hi"), "\"echo hi\"");
        assert_eq!(
            exec_arg(r#"echo "$HOME" 100% \n"#),
            r#""echo \"$$HOME\" 100%% \\n""#
        );
        assert_eq!(
            shell(&format!("echo {INSTANCE} 5%")),
            "/bin/sh -c \"echo %i 5%%\""
        );
    }

    #[test]
    fn an_instance_names_its_template() {
        assert_eq!(
            template_of("tessaro-script-ab12-cd34@manual-1700000000-42.service").as_deref(),
            Some("tessaro-script-ab12-cd34@.service")
        );
        assert_eq!(template_of("tessaro-schedule-ab12.timer"), None);
        assert_eq!(template_of("tessaro-script-ab12@.service"), None);
    }

    #[test]
    fn a_plan_writes_what_differs_and_keeps_busy_files() {
        let wanted: BTreeMap<String, String> = [
            ("tessaro-schedule-a.timer".to_string(), "t2".to_string()),
            ("tessaro-script-b@.service".to_string(), "f".to_string()),
        ]
        .into();
        let present: BTreeMap<String, Vec<u8>> = [
            ("tessaro-schedule-a.timer".to_string(), b"t1".to_vec()),
            ("tessaro-script-b@.service".to_string(), b"f".to_vec()),
            ("tessaro-script-b-old1@.service".to_string(), b"r".to_vec()),
            ("tessaro-script-b-old2@.service".to_string(), b"r".to_vec()),
        ]
        .into();
        let busy: BTreeSet<String> = ["tessaro-script-b-old1@.service".to_string()].into();
        assert_eq!(
            plan(&wanted, &present, &busy),
            Plan {
                write: vec![("tessaro-schedule-a.timer".into(), "t2".into())],
                remove: vec!["tessaro-script-b-old2@.service".into()],
            }
        );
        let same: BTreeMap<String, Vec<u8>> = wanted
            .iter()
            .map(|(name, body)| (name.clone(), body.clone().into_bytes()))
            .collect();
        assert!(plan(&wanted, &same, &BTreeSet::new()).is_empty());
    }

    #[test]
    fn files_are_written_and_removed_in_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let units = dir.path().join("units");
        fs::create_dir_all(&units).unwrap();
        fs::write(units.join("other.service"), "x").unwrap();
        let wanted: BTreeMap<String, String> = [
            ("tessaro-script-a@.service".to_string(), "f".to_string()),
            ("tessaro-schedule-b.timer".to_string(), "t".to_string()),
        ]
        .into();
        let prefixes = ["tessaro-script-", "tessaro-schedule-"];
        let first = plan(
            &wanted,
            &present(&units, &prefixes).unwrap(),
            &BTreeSet::new(),
        );
        assert_eq!(first.write.len(), 2);
        apply(&units, &first, 0o644).unwrap();
        assert!(plan(
            &wanted,
            &present(&units, &prefixes).unwrap(),
            &BTreeSet::new()
        )
        .is_empty());

        let gone = plan(
            &BTreeMap::new(),
            &present(&units, &prefixes).unwrap(),
            &BTreeSet::new(),
        );
        apply(&units, &gone, 0o644).unwrap();
        assert!(present(&units, &prefixes).unwrap().is_empty());
        assert!(units.join("other.service").exists());
        assert!(present(&dir.path().join("missing"), &prefixes)
            .unwrap()
            .is_empty());

        let bodies = dir.path().join("new").join("scripts");
        let write = Plan {
            write: vec![("a-1.sh".into(), "true\n".into())],
            remove: vec![],
        };
        apply(&bodies, &write, 0o600).unwrap();
        assert_eq!(fs::read_to_string(bodies.join("a-1.sh")).unwrap(), "true\n");
    }
}
