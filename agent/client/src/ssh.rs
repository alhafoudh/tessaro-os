//! Shell access by key, as both clients set it up.
//!
//! The public key goes to the device over the conversation that is already
//! pinned and authenticated, the agent adds it to root's `authorized_keys`,
//! and the answer carries the device's SSH host keys. Those are written to a
//! known_hosts file of our own under an alias made from the node id, so ssh
//! checks the host key against what the pinned channel said instead of
//! asking on first use - and an address that moved to another device is a
//! refusal, not a prompt. `tessaro-ctl ssh connect` then becomes `ssh`;
//! tessaro-gui runs one to forward the device's VNC port.

use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use protocol::sshkey::PublicKey;
use protocol::{Command, SshAccess};

use crate::connect::Session;
use crate::nodes;

/// What `ssh-keygen` makes by default, in the order ssh itself tries them.
pub const DEFAULT_KEYS: &[&str] = &[
    "id_ed25519",
    "id_ecdsa",
    "id_ecdsa_sk",
    "id_ed25519_sk",
    "id_rsa",
];

/// The key to send, and the private half to hand ssh when there is one.
#[derive(Debug, PartialEq, Eq)]
pub struct Chosen {
    pub public: PathBuf,
    pub identity: Option<PathBuf>,
}

/// A device that has our key, and how ssh reaches it pinned.
#[derive(Debug)]
pub struct Authorized {
    pub access: SshAccess,
    pub address: SocketAddr,
    pub alias: String,
    pub known_hosts: PathBuf,
    pub identity: Option<PathBuf>,
}

impl Authorized {
    /// `ssh` with the pin, to root on the device, then `args`.
    pub fn argv(&self, port: u16, args: &[String]) -> Vec<String> {
        ssh_argv(
            &self.address.ip().to_string(),
            port,
            &self.alias,
            &self.known_hosts,
            !self.access.host_keys.is_empty(),
            self.identity.as_deref(),
            args,
        )
    }
}

/// Send our public key (`key`, or the first ssh-keygen default) over
/// `session`, and pin the host keys the device answers with.
pub fn authorize(session: &mut Session, key: Option<&Path>) -> Result<Authorized, String> {
    let Some((address, _)) = session.remote.clone() else {
        return Err("this is the device itself; pass --node to reach one over ssh".to_string());
    };
    let chosen = choose_key(key, &home())?;
    let line = fs::read_to_string(&chosen.public)
        .map_err(|err| format!("{}: {err}", chosen.public.display()))?;
    let public =
        PublicKey::parse(&line).map_err(|err| format!("{}: {err}", chosen.public.display()))?;

    let access: SshAccess = session.call(Command::SshAuthorize { key: public.line() })?;

    let alias = format!("tessaro-{}", session.node.id);
    let known_hosts = nodes::dir().join("known_hosts");
    write_known_hosts(&known_hosts, &alias, &access.host_keys)?;
    Ok(Authorized {
        access,
        address,
        alias,
        known_hosts,
        identity: chosen.identity,
    })
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// With `key`: that file if it is a `.pub`, else `PATH.pub` next to a
/// private key, else the file itself. Without: the first default key in
/// `~/.ssh` that has a `.pub`.
pub fn choose_key(key: Option<&Path>, home: &Path) -> Result<Chosen, String> {
    let beside = |public: &Path| {
        let private = public.with_extension("");
        private.is_file().then_some(private)
    };

    if let Some(path) = key {
        if path.extension().is_some_and(|ext| ext == "pub") {
            return Ok(Chosen {
                public: path.to_path_buf(),
                identity: beside(path),
            });
        }
        let public = PathBuf::from(format!("{}.pub", path.display()));
        if public.is_file() {
            return Ok(Chosen {
                public,
                identity: Some(path.to_path_buf()),
            });
        }
        if path.is_file() {
            return Ok(Chosen {
                public: path.to_path_buf(),
                identity: None,
            });
        }
        return Err(format!("{}: no such key", path.display()));
    }

    let dir = home.join(".ssh");
    DEFAULT_KEYS
        .iter()
        .map(|name| dir.join(format!("{name}.pub")))
        .find(|public| public.is_file())
        // The defaults are ssh's own; it finds them without being told.
        .map(|public| Chosen {
            public,
            identity: None,
        })
        .ok_or_else(|| {
            format!(
                "no public key in {} ({}); make one with `ssh-keygen -t ed25519` or pass --key",
                dir.display(),
                DEFAULT_KEYS
                    .iter()
                    .map(|name| format!("{name}.pub"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

/// Replace every line for `alias` with `host_keys`, keeping the rest.
pub fn write_known_hosts(path: &Path, alias: &str, host_keys: &[String]) -> Result<(), String> {
    let existing = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("{}: {err}", path.display())),
    };
    let mut lines: Vec<String> = existing
        .lines()
        .filter(|line| line.split_ascii_whitespace().next() != Some(alias))
        .map(str::to_string)
        .collect();
    lines.extend(host_keys.iter().map(|key| format!("{alias} {key}")));

    let mut body = lines.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    if body == existing {
        return Ok(());
    }
    nodes::write_private(path, body.as_bytes())
}

pub fn ssh_argv(
    ip: &str,
    port: u16,
    alias: &str,
    known_hosts: &Path,
    strict: bool,
    identity: Option<&Path>,
    args: &[String],
) -> Vec<String> {
    let mut argv: Vec<String> = vec!["ssh".into(), "-p".into(), port.to_string()];
    let mut option = |value: String| {
        argv.push("-o".into());
        argv.push(value);
    };
    option(format!("HostKeyAlias={alias}"));
    option(format!("UserKnownHostsFile={}", known_hosts.display()));
    if strict {
        option("StrictHostKeyChecking=yes".into());
    }
    if let Some(identity) = identity {
        option("IdentitiesOnly=yes".into());
        argv.push("-i".into());
        argv.push(identity.display().to_string());
    }
    argv.push(format!("root@{ip}"));
    argv.extend(args.iter().cloned());
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIO7gEEtj0g4zaawVIwrP4wxLZQ2TqgASR86NTHDJ66jj a@laptop";

    #[test]
    fn the_first_default_key_is_chosen() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        assert!(choose_key(None, home.path())
            .unwrap_err()
            .contains("ssh-keygen"));

        fs::write(ssh.join("id_rsa.pub"), KEY).unwrap();
        fs::write(ssh.join("id_ed25519.pub"), KEY).unwrap();
        assert_eq!(
            choose_key(None, home.path()).unwrap(),
            Chosen {
                public: ssh.join("id_ed25519.pub"),
                identity: None,
            }
        );
    }

    #[test]
    fn an_explicit_key_brings_its_private_half() {
        let dir = tempfile::tempdir().unwrap();
        let private = dir.path().join("work");
        let public = dir.path().join("work.pub");
        fs::write(&private, "secret").unwrap();
        fs::write(&public, KEY).unwrap();

        let expected = Chosen {
            public: public.clone(),
            identity: Some(private.clone()),
        };
        assert_eq!(choose_key(Some(&public), dir.path()).unwrap(), expected);
        assert_eq!(choose_key(Some(&private), dir.path()).unwrap(), expected);

        fs::remove_file(&private).unwrap();
        assert_eq!(
            choose_key(Some(&public), dir.path()).unwrap().identity,
            None
        );
        assert!(choose_key(Some(&dir.path().join("nope")), dir.path()).is_err());
    }

    #[test]
    fn known_hosts_keeps_other_nodes_and_replaces_this_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        fs::write(&path, "tessaro-b ssh-rsa BBBB\ntessaro-a ssh-rsa OLD\n").unwrap();

        write_known_hosts(&path, "tessaro-a", &["ssh-rsa NEW".to_string()]).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "tessaro-b ssh-rsa BBBB\ntessaro-a ssh-rsa NEW\n"
        );

        // No host key from the device: no stale entry left to refuse with.
        write_known_hosts(&path, "tessaro-a", &[]).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "tessaro-b ssh-rsa BBBB\n"
        );
    }

    #[test]
    fn the_ssh_command_pins_the_host_key() {
        let argv = ssh_argv(
            "192.0.2.7",
            22,
            "tessaro-abc",
            Path::new("/cfg/known_hosts"),
            true,
            Some(Path::new("/k/work")),
            &["journalctl".to_string(), "-f".to_string()],
        );
        assert_eq!(
            argv.join(" "),
            "ssh -p 22 -o HostKeyAlias=tessaro-abc -o UserKnownHostsFile=/cfg/known_hosts \
             -o StrictHostKeyChecking=yes -o IdentitiesOnly=yes -i /k/work root@192.0.2.7 \
             journalctl -f"
        );

        let loose = ssh_argv("::1", 2222, "a", Path::new("k"), false, None, &[]);
        assert!(!loose
            .iter()
            .any(|arg| arg.starts_with("StrictHostKeyChecking")));
        assert_eq!(loose.last().unwrap(), "root@::1");
    }
}
